//! `trav get`: download torrents into a folder with a progress display, then exit.
//!
//! Runs a private engine in a throwaway state directory, so it works next to the
//! desktop app or a daemon (which hold the shared library's lock) and leaves the
//! library untouched. An interrupted download resumes on the next run: the engine
//! verifies whatever is already on disk instead of fetching it again.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{QueueableCommand, cursor, terminal};
use trav_core::{AddTorrent, Engine, Settings, TorrentSource, TorrentStatus, TorrentSummary};
use trav_tui::fmt;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(clap::Args, Debug)]
pub struct GetArgs {
    /// .torrent files, magnet links or info-hashes.
    #[arg(required = true, value_name = "TORRENT")]
    pub items: Vec<String>,

    /// Folder to download into (default: the current directory).
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// Keep seeding after the download completes, until Ctrl-C.
    #[arg(long)]
    pub seed: bool,

    /// Fetch pieces in order, so media can be previewed while it downloads.
    #[arg(long)]
    pub sequential: bool,
}

/// Downloads everything, returning the process exit code: 0 when every torrent
/// completed, 1 if any failed, 130 when interrupted first.
pub async fn run(args: GetArgs, library: &Path) -> Result<i32> {
    let dir = match &args.output {
        Some(d) => std::path::absolute(d)?,
        None => std::env::current_dir()?,
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    // The user's saved preferences (rate limits, port, DHT, ...) apply here too.
    // Reading the file needs no lock, unlike opening the library itself.
    let mut settings: Settings = std::fs::read(library.join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    settings.download_dir = dir.clone();
    settings.max_active_downloads = 0;

    let state = tempfile::Builder::new().prefix("trav-get-").tempdir()?;
    // Known DHT nodes let peer discovery start warm instead of from the bootstrap routers.
    let _ = std::fs::copy(library.join("dht.json"), state.path().join("dht.json"));
    let engine = Engine::start_with(state.path(), Some(settings))
        .await
        .context("starting engine")?;

    let mut hashes = Vec::new();
    for item in &args.items {
        let mut req = AddTorrent::new(TorrentSource::from_input(item));
        req.sequential = args.sequential;
        match engine.add(req).await {
            Ok(h) if !hashes.contains(&h) => hashes.push(h),
            Ok(_) => {}
            Err(e) => eprintln!("trav: {item}: {e}"),
        }
    }
    if hashes.is_empty() {
        engine.shutdown().await;
        return Ok(1);
    }
    eprintln!("Downloading to {}", dir.display());

    let mut view = Progress::new(std::io::stderr().is_terminal());
    let mut snapshots = engine.subscribe();
    let stop = crate::shutdown_signal();
    tokio::pin!(stop);
    let interrupted = loop {
        let snap = engine.snapshot();
        let ours: Vec<&TorrentSummary> = hashes
            .iter()
            .filter_map(|h| snap.torrents.iter().find(|t| &t.info_hash == h))
            .collect();
        view.draw(&ours);
        if ours.iter().all(|t| settled(t.status, args.seed)) {
            break false;
        }
        tokio::select! {
            changed = snapshots.changed() => if changed.is_err() { break false },
            _ = &mut stop => break true,
        }
    };

    let snap = engine.snapshot();
    let (mut done, mut failed) = (0, 0);
    for h in &hashes {
        let Some(t) = snap.torrents.iter().find(|t| &t.info_hash == h) else {
            continue;
        };
        match t.status {
            TorrentStatus::Seeding | TorrentStatus::Finished => {
                done += 1;
                let path = engine
                    .details(h)
                    .and_then(|d| d.content_path)
                    .unwrap_or_else(|| t.save_path.clone());
                println!("✓ {}  {}  →  {path}", t.name, fmt::bytes(t.size));
            }
            TorrentStatus::Error => {
                failed += 1;
                let why = t.error.as_deref().unwrap_or("unknown error");
                eprintln!("✗ {}: {why}", t.name);
            }
            _ => {}
        }
    }
    if interrupted && done + failed < hashes.len() {
        eprintln!("Stopped. Run the same command again to pick up where it left off.");
    }
    engine.shutdown().await;
    Ok(if failed > 0 {
        1
    } else if done < hashes.len() {
        130
    } else {
        0
    })
}

/// Nothing more to wait for: done (or, when seeding, stopped by the ratio limit) or failed.
fn settled(status: TorrentStatus, seed: bool) -> bool {
    match status {
        TorrentStatus::Error | TorrentStatus::Finished => true,
        TorrentStatus::Seeding => !seed,
        _ => false,
    }
}

/// What a torrent is doing, after its name.
fn describe(t: &TorrentSummary) -> String {
    let n = t.peers + t.seeds;
    let peers = format!("{n} peer{}", if n == 1 { "" } else { "s" });
    match t.status {
        TorrentStatus::Metadata => format!("fetching metadata · {peers}"),
        TorrentStatus::Checking => format!("verifying data on disk {:.0}%", t.progress * 100.0),
        TorrentStatus::Queued => "queued".into(),
        TorrentStatus::Paused => "paused".into(),
        TorrentStatus::Downloading => {
            let tail = match t.eta {
                _ if n == 0 => "looking for peers…".to_string(),
                Some(eta) => format!("{peers} · {} left", fmt::eta(Some(eta))),
                None => peers,
            };
            format!(
                "{} {:>5.1}%  {} / {}  ↓ {}  {tail}",
                fmt::bar(t.progress, 20),
                t.progress * 100.0,
                fmt::bytes(t.done_bytes),
                fmt::bytes(t.size),
                fmt::rate(t.download_rate),
            )
        }
        TorrentStatus::Seeding => format!(
            "✓ done · seeding ↑ {} · ratio {:.2}",
            fmt::rate(t.upload_rate),
            t.ratio
        ),
        TorrentStatus::Finished => "✓ done".into(),
        TorrentStatus::Error => format!("✗ {}", t.error.as_deref().unwrap_or("error")),
    }
}

/// Shortens `s` to at most `width` terminal columns (marking the cut with `…`),
/// then pads it with spaces to exactly `width` when `pad` is set.
fn fit(s: &str, width: usize, pad: bool) -> String {
    let mut out = String::new();
    let mut used = 0;
    if s.width() <= width {
        out.push_str(s);
        used = s.width();
    } else if width > 0 {
        for c in s.chars() {
            let w = c.width().unwrap_or(0);
            if used + w + 1 > width {
                break;
            }
            out.push(c);
            used += w;
        }
        out.push('…');
        used += 1;
    }
    if pad {
        out.extend(std::iter::repeat_n(' ', width.saturating_sub(used)));
    }
    out
}

/// Live block of one line per torrent on a terminal; periodic plain lines otherwise.
struct Progress {
    tty: bool,
    drawn: u16,
    last_plain: Option<Instant>,
    last_status: Vec<TorrentStatus>,
}

impl Progress {
    fn new(tty: bool) -> Self {
        Self {
            tty,
            drawn: 0,
            last_plain: None,
            last_status: Vec::new(),
        }
    }

    fn draw(&mut self, ts: &[&TorrentSummary]) {
        if self.tty {
            self.draw_live(ts);
        } else {
            self.draw_plain(ts);
        }
    }

    fn draw_live(&mut self, ts: &[&TorrentSummary]) {
        // One column short of the edge: a full-width line wraps on some terminals,
        // which would throw off the cursor-up redraw.
        let width = terminal::size().map_or(80, |(w, _)| w as usize).max(20) - 1;
        let name_w = ts
            .iter()
            .map(|t| t.name.width())
            .max()
            .unwrap_or(0)
            .min(width / 3)
            .max(8);
        let mut err = std::io::stderr().lock();
        if self.drawn > 0 {
            let _ = err.queue(cursor::MoveToPreviousLine(self.drawn));
        }
        for t in ts {
            let line = format!("{}  {}", fit(&t.name, name_w, true), describe(t));
            let _ = err.queue(terminal::Clear(terminal::ClearType::CurrentLine));
            let _ = writeln!(err, "{}", fit(&line, width, false));
        }
        let _ = err.flush();
        self.drawn = ts.len() as u16;
    }

    fn draw_plain(&mut self, ts: &[&TorrentSummary]) {
        let status: Vec<TorrentStatus> = ts.iter().map(|t| t.status).collect();
        let due = self
            .last_plain
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(10));
        if !due && status == self.last_status {
            return;
        }
        for t in ts {
            eprintln!("{}  {}", t.name, describe(t));
        }
        self.last_plain = Some(Instant::now());
        self.last_status = status;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_truncates_by_display_width() {
        assert_eq!(fit("ubuntu.iso", 20, false), "ubuntu.iso");
        assert_eq!(fit("ubuntu.iso", 12, true), "ubuntu.iso  ");
        assert_eq!(fit("ubuntu-26.04-desktop.iso", 10, false), "ubuntu-26…");
        // Wide (CJK) characters take two columns each.
        let cut = fit("日本語のファイル名", 7, false);
        assert_eq!(cut, "日本語…");
        assert!(cut.width() <= 7);
        assert_eq!(fit("abc", 0, false), "");
    }

    #[test]
    fn settles_when_done_or_failed() {
        assert!(!settled(TorrentStatus::Downloading, false));
        assert!(!settled(TorrentStatus::Metadata, false));
        assert!(settled(TorrentStatus::Seeding, false));
        assert!(
            !settled(TorrentStatus::Seeding, true),
            "--seed keeps going after completion"
        );
        assert!(settled(TorrentStatus::Finished, true));
        assert!(settled(TorrentStatus::Error, true));
    }
}
