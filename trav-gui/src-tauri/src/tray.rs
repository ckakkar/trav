//! The menu bar extra. While Trav downloads, its glyph comes alive: the arrow
//! bobs toward the line beneath it, the line fills with overall progress and
//! a glint runs along what's left, and the speed sits beside it. A finished
//! download makes the line pulse. Its menu shows live totals and the active
//! transfers (pick one to jump to it), with the everyday actions below.
//!
//! The same loop drives the Dock's progress bar and the keep-awake assertion.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Emitter, Manager, Wry};
use trav_core::{EngineSnapshot, Event, TorrentStatus, TorrentSummary};

use crate::{Shell, hand_over, openable, quit, show};

const SIZE: usize = 64;
const FRAME: Duration = Duration::from_millis(66);
/// Status (title, menu, Dock) refreshes every this many frames (~1 s).
const STATUS_EVERY: u32 = 15;
const MAX_ROWS: usize = 6;

// Rows and columns of the template glyph (icons/tray.png).
const STEM_TOP: usize = 16;
const ARROW_END: usize = 52;
const TRACK_Y: (usize, usize) = (54, 58);
const TRACK_X: (usize, usize) = (13, 51);
/// How far the arrow dips toward the line, in glyph pixels.
const BOB: f64 = 2.0;

/// What the icon shows.
#[derive(Clone, Copy, PartialEq)]
enum Look {
    Still,
    /// Downloading; overall progress when known (fetching metadata has none).
    Active(Option<f64>),
    /// A download just finished.
    Done(Instant),
}

/// Renders frames from the glyph's alpha channel.
struct Glyph {
    alpha: Vec<u8>,
}

impl Glyph {
    fn load() -> Self {
        let img = tauri::include_image!("icons/tray.png");
        let alpha = img.rgba().as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        Self { alpha }
    }

    fn at(&self, x: usize, y: usize) -> f64 {
        self.alpha[y * SIZE + x] as f64
    }

    /// The glyph with the stem stretched and the arrow lowered by `drop` rows.
    fn dropped(&self, x: usize, y: usize, drop: usize) -> f64 {
        if drop == 0 || y < STEM_TOP {
            self.at(x, y)
        } else if y < STEM_TOP + drop {
            self.at(x, STEM_TOP)
        } else if y < ARROW_END + drop {
            self.at(x, y - drop)
        } else if y < TRACK_Y.0 {
            0.0
        } else {
            self.at(x, y)
        }
    }

    /// One frame: `t` is seconds since the animation began.
    fn frame(&self, look: Look, t: f64) -> Image<'static> {
        let (bob, progress, glint, pulse) = match look {
            Look::Still => (0.0, None, None, 1.0),
            Look::Active(p) => {
                let bob = BOB * (1.0 - (t * TAU / 1.25).cos()) / 2.0;
                (bob, Some(p.unwrap_or(0.0)), Some((t / 1.6).fract()), 1.0)
            }
            Look::Done(at) => {
                let k = at.elapsed().as_secs_f64();
                (
                    0.0,
                    Some(1.0),
                    None,
                    0.55 + 0.45 * (k * TAU * 1.5).cos().abs(),
                )
            }
        };
        let (lo, hi) = (bob.floor() as usize, bob.ceil() as usize);
        let mix = bob - bob.floor();
        let (x0, x1) = TRACK_X;
        let fill_to = progress.map(|p| x0 as f64 + p.clamp(0.0, 1.0) * (x1 - x0) as f64);
        let mut rgba = vec![0u8; SIZE * SIZE * 4];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let mut a = self.dropped(x, y, lo) * (1.0 - mix) + self.dropped(x, y, hi) * mix;
                if (TRACK_Y.0..TRACK_Y.1).contains(&y)
                    && let Some(fill) = fill_to
                {
                    // Filled part solid, the rest faint, with a glint running along it.
                    let covered = (fill - x as f64).clamp(0.0, 1.0);
                    let shine = glint.map_or(0.0, |g| {
                        let at = fill + g * (x1 as f64 - fill);
                        (-((x as f64 - at) / 2.5).powi(2)).exp()
                    });
                    let level = 0.3 + 0.7 * covered + 0.55 * shine * (1.0 - covered);
                    a *= level.min(1.0) * pulse;
                }
                rgba[(y * SIZE + x) * 4 + 3] = a.round().clamp(0.0, 255.0) as u8;
            }
        }
        Image::new_owned(rgba, SIZE as u32, SIZE as u32)
    }
}

fn fmt_rate(b: u64) -> String {
    const U: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B/s")
    } else if v >= 100.0 {
        format!("{v:.0} {}", U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn fmt_eta(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s left"),
        60..3600 => format!("{}m left", secs / 60),
        _ => format!("{}h {}m left", secs / 3600, secs % 3600 / 60),
    }
}

fn downloading(t: &TorrentSummary) -> bool {
    matches!(
        t.status,
        TorrentStatus::Downloading | TorrentStatus::Metadata
    )
}

/// One transfer in the menu, e.g. "debian.iso — 62% · 2.1 MB/s · 3m left".
fn row_text(t: &TorrentSummary) -> String {
    let mut name: String = t.name.chars().take(34).collect();
    if t.name.chars().count() > 34 {
        name.push('…');
    }
    let detail = if t.status == TorrentStatus::Metadata {
        "fetching metadata".to_string()
    } else if downloading(t) {
        let mut d = format!("{:.0}% · {}", t.progress * 100.0, fmt_rate(t.download_rate));
        if let Some(eta) = t.eta.filter(|_| t.download_rate > 0) {
            d.push_str(" · ");
            d.push_str(&fmt_eta(eta));
        }
        d
    } else {
        format!("seeding · ↑ {}", fmt_rate(t.upload_rate))
    };
    format!("{name} — {detail}")
}

struct TrayMenu {
    speed: MenuItem<Wry>,
    summary: MenuItem<Wry>,
    rows: Vec<(String, MenuItem<Wry>)>,
}

fn item(app: &AppHandle, id: &str, text: &str, enabled: bool) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, id, text, enabled, None::<&str>)
}

/// Build the menu around the given transfers (hash, text).
fn build_menu(
    app: &AppHandle,
    transfers: &[(String, String)],
) -> tauri::Result<(Menu<Wry>, TrayMenu)> {
    let speed = item(app, "tray.speed", "↓ 0 B/s   ↑ 0 B/s", false)?;
    let summary = item(app, "tray.summary", "Nothing transferring", false)?;
    let menu = Menu::with_items(app, &[&speed, &summary])?;
    let mut rows = Vec::new();
    if !transfers.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        for (hash, text) in transfers {
            let row = item(app, &format!("tray.select:{hash}"), text, true)?;
            menu.append(&row)?;
            rows.push((hash.clone(), row));
        }
    }
    menu.append_items(&[
        &PredefinedMenuItem::separator(app)?,
        &item(app, "tray.show", "Open Trav", true)?,
        &item(app, "tray.open-torrent", "Add Torrent File…", true)?,
        &item(app, "tray.paste", "Add Magnet from Clipboard", true)?,
        &PredefinedMenuItem::separator(app)?,
        &item(app, "tray.pause", "Pause All", true)?,
        &item(app, "tray.resume", "Resume All", true)?,
        &PredefinedMenuItem::separator(app)?,
        &item(app, "tray.settings", "Settings…", true)?,
        &item(app, "tray.check-updates", "Check for Updates…", true)?,
        &item(app, "tray.quit", "Quit Trav", true)?,
    ])?;
    Ok((
        menu,
        TrayMenu {
            speed,
            summary,
            rows,
        },
    ))
}

/// Magnet links (and .torrent paths) on the clipboard.
fn clipboard_items() -> Vec<String> {
    std::process::Command::new("/usr/bin/pbpaste")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|s| openable(s, None))
        .collect()
}

fn on_menu(app: &AppHandle, id: &str) {
    let Some(id) = id.strip_prefix("tray.") else {
        return;
    };
    let engine = app.state::<Arc<Shell>>().engine.get().cloned();
    match id {
        "show" => show(app),
        "pause" => {
            if let Some(e) = engine {
                e.pause_all();
            }
        }
        "resume" => {
            if let Some(e) = engine {
                e.resume_all();
            }
        }
        "paste" => {
            let items = clipboard_items();
            if items.is_empty() {
                // Nothing usable copied: open the magnet box instead.
                show(app);
                let _ = app.emit("trav://menu", "add-magnet");
            } else {
                hand_over(app, items);
            }
        }
        "quit" => quit(app, false),
        "open-torrent" | "settings" | "check-updates" => {
            show(app);
            let _ = app.emit("trav://menu", id);
        }
        _ => {
            if let Some(hash) = id.strip_prefix("select:") {
                show(app);
                let _ = app.emit("trav://menu", format!("select:{hash}"));
            }
        }
    }
}

/// The transfers worth listing: downloads first, then active seeds.
fn listed(s: &EngineSnapshot) -> Vec<&TorrentSummary> {
    let mut v: Vec<&TorrentSummary> = s
        .torrents
        .iter()
        .filter(|t| downloading(t) || (t.status == TorrentStatus::Seeding && t.upload_rate > 0))
        .collect();
    v.sort_by_key(|t| {
        (
            !downloading(t),
            std::cmp::Reverse(t.download_rate + t.upload_rate),
        )
    });
    v.truncate(MAX_ROWS);
    v
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let glyph = Glyph::load();
    let (menu, mut handles) = build_menu(app, &[])?;
    let tray = TrayIconBuilder::with_id("main")
        .icon(glyph.frame(Look::Still, 0.0))
        .icon_as_template(true)
        .tooltip("Trav")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, e| on_menu(app, e.id.as_ref()))
        .build(app)?;

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(FRAME);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let start = Instant::now();
        let mut n: u32 = 0;
        let mut look = Look::Still;
        let mut drawn_still = true;
        let mut listed_hashes: Vec<String> = Vec::new();
        let mut dock: Option<Option<u64>> = None;
        let mut awake: Option<keepawake::KeepAwake> = None;
        let mut events: Option<tokio::sync::broadcast::Receiver<Event>> = None;
        loop {
            tick.tick().await;
            n = n.wrapping_add(1);
            let shell = app.state::<Arc<Shell>>().inner().clone();
            let Some(engine) = shell.engine.get().cloned() else {
                continue;
            };
            let events = events.get_or_insert_with(|| engine.events());
            let mut finished = false;
            while let Ok(e) = events.try_recv() {
                finished |= matches!(e, Event::TorrentCompleted { .. });
            }
            if finished {
                look = Look::Done(Instant::now());
            }

            if n % STATUS_EVERY == 1 || finished {
                let s = engine.snapshot();
                let active: Vec<&TorrentSummary> =
                    s.torrents.iter().filter(|t| downloading(t)).collect();
                let (size, done) = active
                    .iter()
                    .filter(|t| t.status != TorrentStatus::Metadata)
                    .fold((0u64, 0u64), |(a, b), t| (a + t.size, b + t.done_bytes));
                let progress = (size > 0).then(|| done as f64 / size as f64);
                if !matches!(look, Look::Done(at) if at.elapsed() < Duration::from_millis(1600)) {
                    look = if active.is_empty() {
                        Look::Still
                    } else {
                        Look::Active(progress)
                    };
                }

                // Speed beside the icon while downloading.
                let title = (!active.is_empty() && shell.tray_speed.load(Ordering::Relaxed))
                    .then(|| fmt_rate(s.stats.download_rate));
                let _ = tray.set_title(title);
                let line = format!(
                    "↓ {}   ↑ {}",
                    fmt_rate(s.stats.download_rate),
                    fmt_rate(s.stats.upload_rate)
                );
                let _ = tray.set_tooltip(Some(format!("Trav — {line}")));

                // Menu: rebuild when the listed transfers change, else retitle.
                let rows = listed(&s);
                let hashes: Vec<String> = rows.iter().map(|t| t.info_hash.clone()).collect();
                if hashes != listed_hashes {
                    let texts: Vec<(String, String)> = rows
                        .iter()
                        .map(|t| (t.info_hash.clone(), row_text(t)))
                        .collect();
                    if let Ok((menu, h)) = build_menu(&app, &texts) {
                        let _ = tray.set_menu(Some(menu));
                        handles = h;
                        listed_hashes = hashes;
                    }
                } else {
                    for ((_, item), t) in handles.rows.iter().zip(&rows) {
                        let _ = item.set_text(row_text(t));
                    }
                }
                let _ = handles.speed.set_text(&line);
                let seeding = s
                    .torrents
                    .iter()
                    .filter(|t| t.status == TorrentStatus::Seeding)
                    .count();
                let summary = match (active.len(), seeding) {
                    (0, 0) => "Nothing transferring".to_string(),
                    (d, 0) => format!("{d} downloading"),
                    (0, u) => format!("{u} seeding"),
                    (d, u) => format!("{d} downloading · {u} seeding"),
                };
                let _ = handles.summary.set_text(summary);

                // Overall download progress on the Dock icon.
                let percent = progress
                    .map(|p| (p * 100.0) as u64)
                    .or_else(|| (!active.is_empty()).then_some(0));
                if dock != Some(percent)
                    && let Some(w) = app.get_webview_window("main")
                {
                    let _ = w.set_progress_bar(ProgressBarState {
                        status: Some(if percent.is_some() {
                            ProgressBarStatus::Normal
                        } else {
                            ProgressBarStatus::None
                        }),
                        progress: percent,
                    });
                    dock = Some(percent);
                }

                // Keep the Mac from idle-sleeping mid-transfer (the display may still sleep).
                let busy = !active.is_empty() || s.stats.upload_rate > 0;
                if busy && shell.keep_awake.load(Ordering::Relaxed) {
                    if awake.is_none() {
                        awake = keepawake::Builder::default()
                            .idle(true)
                            .reason("Trav is transferring files")
                            .create()
                            .ok();
                    }
                } else {
                    awake = None;
                }
            }

            // The icon: animate while there's something to show, else draw it once.
            match look {
                Look::Still if drawn_still => {}
                Look::Still => {
                    // The template flag goes with every image, or macOS stops tinting it.
                    let _ =
                        tray.set_icon_with_as_template(Some(glyph.frame(Look::Still, 0.0)), true);
                    drawn_still = true;
                }
                _ => {
                    let t = start.elapsed().as_secs_f64();
                    let _ = tray.set_icon_with_as_template(Some(glyph.frame(look, t)), true);
                    drawn_still = false;
                }
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(img: &Image<'_>, x: usize, y: usize) -> u8 {
        img.rgba()[(y * SIZE + x) * 4 + 3]
    }

    #[test]
    fn still_frame_is_the_glyph() {
        let g = Glyph::load();
        let f = g.frame(Look::Still, 0.0);
        assert!(
            g.alpha
                .iter()
                .zip(f.rgba().as_chunks::<4>().0)
                .all(|(&a, p)| a == p[3])
        );
    }

    #[test]
    fn progress_fills_the_line_and_the_arrow_dips() {
        let g = Glyph::load();
        let half = g.frame(Look::Active(Some(0.5)), 0.0);
        let y = 55;
        // Filled start, faint end.
        assert!(alpha(&half, TRACK_X.0 + 2, y) > 200);
        assert!(alpha(&half, TRACK_X.1 - 2, y) < 150);
        // Half a bob period in, the arrow tip has moved down two rows.
        let low = g.frame(Look::Active(Some(0.5)), 1.25 / 2.0);
        assert_eq!(alpha(&g.frame(Look::Still, 0.0), 32, 52), 0);
        assert!(alpha(&low, 32, 52) > 100, "tip lowered into the gap");
        // The stem stays joined to the bar.
        assert!(alpha(&low, 32, 17) > 200);
    }

    #[test]
    fn labels() {
        assert_eq!(fmt_rate(512), "512 B/s");
        assert_eq!(fmt_rate(2_202_009), "2.1 MB/s");
        assert_eq!(fmt_rate(150 * 1024 * 1024), "150 MB/s");
        assert_eq!(fmt_eta(42), "42s left");
        assert_eq!(fmt_eta(185), "3m left");
        assert_eq!(fmt_eta(7300), "2h 1m left");
    }
}
