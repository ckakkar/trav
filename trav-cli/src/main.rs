mod get;
mod web;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;
use trav_core::{AddTorrent, Engine, Settings, TorrentSource};

const EXAMPLES: &str = "\
Examples:
  trav get ubuntu.iso.torrent           download into this folder, then exit
  trav get 'magnet:?xt=urn:btih:…' -o ~/Downloads
  trav                                  terminal UI for your library
  trav ubuntu.iso.torrent               ...with this torrent added
  trav --daemon                         headless; web UI on http://127.0.0.1:9696
  trav --create ./folder -o out.torrent make a torrent";

/// Trav — a fast, quiet BitTorrent client: terminal UI, web UI, or one-shot downloads.
#[derive(Parser, Debug)]
#[command(name = "trav", version, about, after_help = EXAMPLES)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// .torrent files, magnet links or info-hashes to add to the library on startup.
    items: Vec<String>,

    /// Run headless and serve the web UI instead of the terminal UI.
    #[arg(short, long)]
    daemon: bool,

    /// Probe a running daemon's /api/health on --web-bind and exit 0/1 (for container health checks).
    #[arg(long)]
    health_check: bool,

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

    /// Create a .torrent from a file or folder, then exit.
    #[arg(long, value_name = "PATH")]
    create: Option<PathBuf>,

    /// Output path for --create (default: <name>.torrent).
    #[arg(short, long, value_name = "FILE", requires = "create")]
    output: Option<PathBuf>,

    /// Tracker URL(s) to embed with --create.
    #[arg(long = "tracker", value_name = "URL", requires = "create")]
    trackers: Vec<String>,

    /// Mark the created torrent private (no DHT/PEX).
    #[arg(long, requires = "create")]
    private: bool,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Download torrents into a folder with a progress bar, then exit.
    ///
    /// Runs on its own, so it works while the desktop app or a daemon is open,
    /// and leaves your library untouched. Re-run an interrupted download to resume it.
    #[command(visible_alias = "download")]
    Get(get::GetArgs),
}

/// Piece size targeting ~1500 pieces, clamped to 16 KiB–16 MiB.
fn auto_piece_size(total: u64) -> u32 {
    let target = (total / 1500).clamp(16 << 10, 16 << 20);
    target.next_power_of_two().min(16 << 20) as u32
}

fn create(
    path: &std::path::Path,
    output: Option<PathBuf>,
    trackers: &[String],
    private: bool,
) -> Result<()> {
    let total = if path.is_dir() {
        fn walk(p: &std::path::Path) -> u64 {
            std::fs::read_dir(p)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| {
                            if e.path().is_dir() {
                                walk(&e.path())
                            } else {
                                e.metadata().map(|m| m.len()).unwrap_or(0)
                            }
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(path)
    } else {
        std::fs::metadata(path)?.len()
    };
    let piece = auto_piece_size(total);
    let bytes = trav_core::metainfo::create_torrent(path, piece, trackers, private)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "torrent".into());
    let out = output.unwrap_or_else(|| PathBuf::from(format!("{name}.torrent")));
    std::fs::write(&out, &bytes)?;
    let meta = trav_core::metainfo::Metainfo::from_bytes(&bytes)?;
    println!(
        "{}  {}  ({} pieces × {} KiB)",
        hex_hash(&meta.info_hash),
        out.display(),
        meta.info.num_pieces(),
        piece / 1024
    );
    Ok(())
}

fn hex_hash(h: &[u8; 20]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn default_state_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("trav")
}

/// Minimal HTTP probe so container images need no curl.
fn health_check(bind: SocketAddr) -> bool {
    use std::io::{Read, Write};
    let ip = if bind.ip().is_unspecified() {
        [127, 0, 0, 1].into()
    } else {
        bind.ip()
    };
    let addr = SocketAddr::new(ip, bind.port());
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2))
    else {
        return false;
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    if s.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut head = [0u8; 12];
    s.read_exact(&mut head).is_ok() && head.ends_with(b" 200")
}

/// Route panics through tracing so they land in the log file, not a lost stderr.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(target: "panic", "{info}\n{}", std::backtrace::Backtrace::force_capture());
        default(info);
    }));
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.health_check {
        std::process::exit(if health_check(cli.web_bind) { 0 } else { 1 });
    }
    if let Some(path) = &cli.create {
        return create(path, cli.output.clone(), &cli.trackers, cli.private);
    }
    let state_dir = cli.state_dir.clone().unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("creating {}", state_dir.display()))?;

    // The TUI and `get` own the terminal, so they log to a file; the daemon logs to stderr.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,hyper=warn,reqwest=warn".into());
    let log_prefix = match cli.command {
        Some(Command::Get(_)) => "trav-get",
        None => "trav",
    };
    let guard = if cli.daemon && cli.command.is_none() {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            // Plain text when piped (docker logs, journald).
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
            .with_writer(std::io::stderr)
            .init();
        None
    } else {
        // Daily files, last 7 kept: logs/trav.YYYY-MM-DD.log. The directory must
        // exist up front, or the appender complains on stderr while pruning.
        std::fs::create_dir_all(state_dir.join("logs"))?;
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix(log_prefix)
            .filename_suffix("log")
            .max_log_files(7)
            .build(state_dir.join("logs"))
            .context("opening log directory")?;
        let (writer, guard) = tracing_appender::non_blocking(appender);
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(writer)
            .init();
        Some(guard)
    };

    install_panic_hook();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    if let Some(Command::Get(args)) = cli.command {
        let code = rt.block_on(get::run(args, &state_dir))?;
        // process::exit skips destructors: stop the runtime and flush logs first.
        drop(rt);
        drop(guard);
        std::process::exit(code);
    }
    let r = rt.block_on(run(cli, state_dir));
    drop(guard);
    r
}

async fn run(cli: Cli, state_dir: PathBuf) -> Result<()> {
    let engine = match Engine::start(&state_dir).await {
        Ok(e) => e,
        Err(trav_core::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
            anyhow::bail!(
                "{e}.\nTrav is already running (the desktop app or `trav --daemon`): add the \
                 torrent there, or download it on its own with `trav get <torrent>`."
            )
        }
        Err(e) => return Err(e).context("starting engine"),
    };

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
            TorrentSource::File(p) => TorrentSource::Bytes(
                std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?,
            ),
            other => other,
        };
        match engine.add(AddTorrent::new(src)).await {
            Ok(h) => info!("added {h}"),
            Err(e) => eprintln!("trav: {item}: {e}"),
        }
    }

    if cli.daemon {
        println!(
            "trav {} · web UI on http://{}",
            env!("CARGO_PKG_VERSION"),
            cli.web_bind
        );
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

pub(crate) async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler");
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
