mod web;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;
use trav_core::{AddTorrent, Engine, Settings, TorrentSource};

/// Trav — a fast, headless BitTorrent engine with a terminal UI and a web UI.
#[derive(Parser, Debug)]
#[command(name = "trav", version, about)]
struct Cli {
    /// .torrent files, magnet links or info-hashes to add on startup.
    items: Vec<String>,

    /// Run headless and serve the web UI instead of the terminal UI.
    #[arg(short, long)]
    daemon: bool,

    /// Address for the web UI / JSON API (daemon mode, or alongside the TUI with --web).
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:9696")]
    web_bind: SocketAddr,

    /// Also serve the web UI while running the terminal UI.
    #[arg(long)]
    web: bool,

    /// Require this token for API access (mandatory when binding a non-loopback address).
    #[arg(long, env = "TRAV_TOKEN")]
    token: Option<String>,

    /// Where downloads go (persisted to settings).
    #[arg(short = 's', long, value_name = "DIR")]
    save_path: Option<PathBuf>,

    /// Peer listen port (persisted to settings).
    #[arg(short, long)]
    port: Option<u16>,

    /// Engine state directory (resume data, settings).
    #[arg(long, value_name = "DIR", env = "TRAV_STATE_DIR")]
    state_dir: Option<PathBuf>,
}

fn default_state_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("trav")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let state_dir = cli.state_dir.clone().unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_dir).with_context(|| format!("creating {}", state_dir.display()))?;

    // The TUI owns the terminal, so log to a file there; the daemon logs to stderr.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,hyper=warn,reqwest=warn".into());
    let _guard = if cli.daemon {
        tracing_subscriber::fmt().with_env_filter(filter).init();
        None
    } else {
        let appender = tracing_appender::rolling::never(&state_dir, "trav.log");
        let (writer, guard) = tracing_appender::non_blocking(appender);
        tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(writer).init();
        Some(guard)
    };

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(run(cli, state_dir))
}

async fn run(cli: Cli, state_dir: PathBuf) -> Result<()> {
    let engine = Engine::start(&state_dir).await.context("starting engine")?;

    if cli.save_path.is_some() || cli.port.is_some() {
        let mut s: Settings = engine.settings();
        if let Some(p) = cli.save_path.clone() {
            s.download_dir = std::path::absolute(p)?;
        }
        if let Some(p) = cli.port {
            s.listen_port = p;
        }
        engine.update_settings(s).await?;
    }

    for item in &cli.items {
        let src = match TorrentSource::from_input(item) {
            TorrentSource::File(p) => TorrentSource::Bytes(std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?),
            other => other,
        };
        match engine.add(AddTorrent::new(src)).await {
            Ok(h) => info!("added {h}"),
            Err(e) => eprintln!("trav: {item}: {e}"),
        }
    }

    if cli.daemon {
        println!("trav {} · web UI on http://{}", env!("CARGO_PKG_VERSION"), cli.web_bind);
        tokio::select! {
            r = web::serve(engine.clone(), cli.web_bind, cli.token.clone()) => r?,
            _ = shutdown_signal() => {}
        }
    } else {
        if cli.web {
            let (e, bind, token) = (engine.clone(), cli.web_bind, cli.token.clone());
            tokio::spawn(async move {
                if let Err(err) = web::serve(e, bind, token).await {
                    tracing::error!("web UI: {err}");
                }
            });
        }
        let mut tui = trav_tui::TuiApp::new(engine.clone());
        tui.run().await?;
    }

    engine.shutdown().await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
