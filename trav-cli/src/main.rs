use anyhow::Result;
use tokio::runtime::Runtime;
use tracing::{info, Level};
use tracing_subscriber;

use trav_core::Engine;
use trav_tui::TuiApp;

fn main() -> Result<()> {
    // We do NOT want to initialize a global tracing subscriber that prints to stdout, 
    // because that would conflict with the TUI. We should either log to a file or 
    // disable logging for now. Let's log to a file.
    let file_appender = tracing_appender::rolling::never(".", "trav.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_max_level(Level::DEBUG)
        .init();

    info!("Starting trav CLI...");

    // Create a multi-threaded Tokio runtime
    let rt = Runtime::new()?;
    // We enter the tokio runtime context so that `mpsc::channel` inside Engine::new works gracefully
    // although `Engine::new()` does NOT require it strictly.
    let (engine, command_tx, event_rx, snapshot) = rt.block_on(async { Engine::new() });

    // Spawn the core Engine on a background Tokio task
    rt.spawn(async move {
        if let Err(e) = engine.run().await {
            tracing::error!("Engine runtime error: {:?}", e);
        }
    });

    // Parse CLI arguments to optionally inject an AddTorrent command on startup
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let file_path = std::path::PathBuf::from(&args[1]);
        let download_dir = if args.len() > 2 {
            std::path::PathBuf::from(&args[2])
        } else {
            std::path::PathBuf::from("./downloads")
        };
        let _ = command_tx.try_send(trav_core::message::Command::AddTorrent { file_path, download_dir });
    }

    // Run the TUI on the main thread (which acts as a blocking GUI event loop)
    let mut tui_app = TuiApp::new(command_tx, event_rx, snapshot);
    rt.block_on(async move {
        if let Err(e) = tui_app.run().await {
            tracing::error!("TUI runtime error: {:?}", e);
        }
    });

    info!("trav CLI shutdown gracefully.");
    Ok(())
}
