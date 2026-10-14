//! Download one torrent for a fixed time and report throughput: a harness for
//! measuring the engine against real swarms.
//!
//!   cargo run --release -p trav-core --example fetch -- <file.torrent|magnet> [seconds] [out-dir]
//!
//! Set TRAV_ENCRYPTION (prefer, require or off) or TRAV_PUBLIC_TRACKERS=0 to compare.
//!
//! Prints a line every 5 s and the average at the end. Uses a throwaway state
//! directory under the output folder, so it never touches the app's library.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use trav_core::{AddTorrent, Engine, Settings, TorrentSource};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Vec<String> = std::env::args().collect();
    let input = args
        .get(1)
        .ok_or("usage: fetch <file.torrent|magnet> [seconds] [out-dir]")?;
    let secs: u64 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(60);
    let out = args
        .get(3)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("trav-fetch"));
    std::fs::create_dir_all(&out)?;

    // TRAV_ENCRYPTION=prefer|require|off and TRAV_PUBLIC_TRACKERS=0 for A/B runs.
    let mut settings = Settings {
        download_dir: out.clone(),
        ..Settings::default()
    };
    if let Ok(e) = std::env::var("TRAV_ENCRYPTION") {
        settings.encryption = serde_json::from_value(serde_json::Value::String(e))?;
    }
    if std::env::var("TRAV_PUBLIC_TRACKERS").as_deref() == Ok("0") {
        settings.public_trackers = false;
    }
    let engine = Engine::start_with(out.join(".trav-state"), Some(settings)).await?;
    engine
        .add(AddTorrent {
            source: TorrentSource::from_input(input),
            save_path: None,
            paused: false,
            sequential: false,
            file_priorities: None,
        })
        .await?;

    let start = Instant::now();
    let mut first_byte: Option<Duration> = None;
    // When the last 5% began: how long the tail (end-game) takes.
    let mut tail_from: Option<Duration> = None;
    let mut last_report = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let snap = engine.snapshot();
        let Some(t) = snap.torrents.first() else {
            continue;
        };
        if first_byte.is_none() && t.done_bytes > 0 {
            first_byte = Some(start.elapsed());
        }
        if tail_from.is_none() && t.progress >= 0.95 {
            tail_from = Some(start.elapsed());
        }
        let elapsed = start.elapsed();
        if last_report.elapsed() >= Duration::from_secs(5) {
            last_report = Instant::now();
            let encrypted = engine.details(&t.info_hash).map_or(0, |d| {
                d.peers.iter().filter(|p| p.flags.contains('E')).count()
            });
            println!(
                "{:>4}s  {:>7.1} MiB  {:>6.2} MiB/s  peers {:>3} (seeds {:>3}, encrypted {:>3})  {:?}",
                elapsed.as_secs(),
                t.downloaded as f64 / 1048576.0,
                t.download_rate as f64 / 1048576.0,
                t.peers + t.seeds,
                t.seeds,
                encrypted,
                t.status,
            );
        }
        if elapsed >= Duration::from_secs(secs) || t.progress >= 1.0 {
            println!(
                "done: {:.1} MiB in {:.0} s = {:.2} MiB/s average; first data after {:?}",
                t.downloaded as f64 / 1048576.0,
                elapsed.as_secs_f64(),
                t.downloaded as f64 / 1048576.0 / elapsed.as_secs_f64(),
                first_byte,
            );
            if let Some(from) = tail_from.filter(|_| t.progress >= 1.0) {
                println!(
                    "last 5% took {:.1} s",
                    (start.elapsed() - from).as_secs_f64()
                );
            }
            break;
        }
    }
    engine.shutdown().await;
    Ok(())
}
