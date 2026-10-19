//! Trav for macOS: hosts the engine in-process and exposes it to the webview
//! through a single `rpc` command (trav-core's `rpc::dispatch`), plus the
//! native pieces: menu bar, Dock, menu bar extra, updates, Trash, login item.
#![cfg(target_os = "macos")]

mod menu;
mod tray;
mod update;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;
use trav_core::{Engine, EngineHandle, Event};

pub(crate) const REPO_URL: &str = "https://github.com/ckakkar/trav";
/// Passed by the login item, so Trav starts in the background.
const AUTOSTART_ARG: &str = "--autostart";

#[derive(Default)]
struct Shell {
    engine: OnceLock<EngineHandle>,
    opens: Mutex<Opens>,
    notify: AtomicBool,
    keep_awake: AtomicBool,
    /// Show the download speed beside the menu bar icon.
    tray_speed: AtomicBool,
    quitting: AtomicBool,
}

/// Torrent paths / magnet links handed over by the OS, drained by the UI.
#[derive(Default)]
struct Opens {
    pending: Vec<String>,
    /// What was queued lately: macOS reports one open through several
    /// channels (the Opened event, the deep-link plugin, launch arguments),
    /// and each must show the Add dialog only once.
    recent: Vec<(String, Instant)>,
}

impl Opens {
    const DEDUPE: Duration = Duration::from_secs(5);

    /// Queue the items not already pending or seen lately; true if any were.
    fn add(&mut self, items: impl IntoIterator<Item = String>, now: Instant) -> bool {
        self.recent
            .retain(|(_, t)| now.saturating_duration_since(*t) < Self::DEDUPE);
        let before = self.pending.len();
        for item in items {
            if self.pending.contains(&item) || self.recent.iter().any(|(r, _)| *r == item) {
                continue;
            }
            self.recent.push((item.clone(), now));
            self.pending.push(item);
        }
        self.pending.len() > before
    }

    fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending)
    }
}

type State<'a> = tauri::State<'a, Arc<Shell>>;

#[tauri::command]
async fn rpc(method: String, params: Value, state: State<'_>) -> Result<Value, String> {
    let engine = state.engine.get().ok_or("engine is not running")?;
    if method == "remove" && params.get("deleteFiles").and_then(Value::as_bool) == Some(true) {
        return remove_to_trash(engine, params).await;
    }
    trav_core::rpc::dispatch(engine, &method, params).await
}

/// In the app, "delete data" moves it to the Trash rather than erasing it, so
/// a mistake can be undone.
async fn remove_to_trash(engine: &EngineHandle, mut params: Value) -> Result<Value, String> {
    let mut hashes: Vec<String> = params
        .get("hashes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|h| h.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if let Some(h) = params.get("hash").and_then(Value::as_str) {
        hashes.push(h.to_owned());
    }
    let owned: Vec<(PathBuf, HashSet<PathBuf>)> = hashes
        .iter()
        .filter_map(|h| engine.details(h))
        .filter_map(|d| {
            let save = PathBuf::from(&d.summary.save_path);
            let content = PathBuf::from(d.content_path?);
            // Only the torrent's own file or folder inside its save folder.
            let files = d.files.iter().filter_map(|f| f.disk_path.as_ref());
            (content != save && content.starts_with(&save) && content.exists())
                .then(|| (content, files.map(PathBuf::from).collect()))
        })
        .collect();
    params["deleteFiles"] = Value::Bool(false);
    let result = trav_core::rpc::dispatch(engine, "remove", params).await?;
    if !owned.is_empty() {
        tauri::async_runtime::spawn_blocking(move || {
            let paths: Vec<PathBuf> = owned
                .iter()
                .flat_map(|(content, files)| trash_targets(content, files))
                .collect();
            let res = trash_paths(&paths);
            for (content, _) in &owned {
                prune_empty(content);
            }
            res
        })
        .await
        .map_err(|e| e.to_string())??;
    }
    Ok(result)
}

/// What to move to the Trash for a torrent with data at `content`: all of it
/// in one piece when everything there is the torrent's, else only `files`,
/// so things the user (or another torrent) put in its folder stay put.
fn trash_targets(content: &Path, files: &HashSet<PathBuf>) -> Vec<PathBuf> {
    if !content.is_dir() || only_these(content, files) {
        return vec![content.to_path_buf()];
    }
    files
        .iter()
        .filter(|f| f.starts_with(content) && f.symlink_metadata().is_ok())
        .cloned()
        .collect()
}

/// Whether every file under `dir` is one of `files` (Finder's `.DS_Store` aside).
fn only_these(dir: &Path, files: &HashSet<PathBuf>) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().all(|e| {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => only_these(&p, files),
            _ => files.contains(&p) || e.file_name() == ".DS_Store",
        }
    })
}

/// Remove folders under (and including) `dir` that are left empty but for
/// `.DS_Store`; true if `dir` is gone.
fn prune_empty(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut empty = true;
    for e in entries.flatten() {
        let p = e.path();
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if !(is_dir && prune_empty(&p) || e.file_name() == ".DS_Store") {
            empty = false;
        }
    }
    if empty {
        let _ = std::fs::remove_file(dir.join(".DS_Store"));
    }
    empty && std::fs::remove_dir(dir).is_ok()
}

fn trash_paths(paths: &[PathBuf]) -> Result<(), String> {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    let mut ctx = trash::TrashContext::default();
    // The Finder method would ask for permission to control Finder.
    ctx.set_delete_method(DeleteMethod::NsFileManager);
    ctx.delete_all(paths).map_err(|e| {
        format!("The transfer was removed, but its files couldn’t be moved to the Trash: {e}")
    })
}

#[tauri::command]
fn take_pending_opens(state: State<'_>) -> Vec<String> {
    state.opens.lock().unwrap().take()
}

#[tauri::command]
fn set_notify(enabled: bool, state: State<'_>) {
    state.notify.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
fn set_keep_awake(enabled: bool, state: State<'_>) {
    state.keep_awake.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
fn set_tray_speed(enabled: bool, state: State<'_>) {
    state.tray_speed.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(enabled: bool, app: AppHandle) -> Result<bool, String> {
    let l = app.autolaunch();
    if enabled { l.enable() } else { l.disable() }.map_err(|e| e.to_string())?;
    Ok(l.is_enabled().unwrap_or(false))
}

/// Keep only things we can add: magnet links and existing `.torrent` files.
pub(crate) fn openable(arg: &str, cwd: Option<&Path>) -> Option<String> {
    let a = arg.trim();
    if a.starts_with("magnet:") {
        return Some(a.to_string());
    }
    let path = if let Some(rest) = a.strip_prefix("file://") {
        url::Url::parse(&format!("file://{rest}"))
            .ok()?
            .to_file_path()
            .ok()?
    } else {
        let p = PathBuf::from(a);
        match cwd {
            Some(c) if p.is_relative() => c.join(p),
            _ => p,
        }
    };
    let is_torrent = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("torrent"));
    (is_torrent && path.is_file()).then(|| path.to_string_lossy().into_owned())
}

/// Queue OS-provided items and nudge the UI to pull them.
pub(crate) fn hand_over(app: &AppHandle, items: impl IntoIterator<Item = String>) {
    let shell = app.state::<Arc<Shell>>();
    let added = shell.opens.lock().unwrap().add(items, Instant::now());
    if added {
        show(app);
        let _ = app.emit("trav://open", ());
    }
}

pub(crate) fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Flush resume data and announce `stopped`, then exit (or relaunch after an update).
pub(crate) fn quit(app: &AppHandle, restart: bool) {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    if shell.quitting.swap(true, Ordering::SeqCst) && !restart {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(e) = shell.engine.get() {
            let _ = tokio::time::timeout(Duration::from_secs(6), e.shutdown()).await;
        }
        // Exiting skips destructors: write out buffered log lines first.
        drop(LOG_GUARD.lock().unwrap().take());
        if restart {
            app.restart();
        }
        app.exit(0);
    });
}

fn forward_events(app: AppHandle, engine: EngineHandle) {
    tauri::async_runtime::spawn(async move {
        let mut rx = engine.events();
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let _ = app.emit("trav://event", &ev);
                    if let Event::TorrentCompleted { name, .. } = &ev
                        && app.state::<Arc<Shell>>().notify.load(Ordering::Relaxed)
                    {
                        let _ = app
                            .notification()
                            .builder()
                            .title("Download complete")
                            .body(name)
                            .show();
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

fn state_dir() -> PathBuf {
    // TRAV_STATE_DIR points it at another library (a scratch one for testing).
    std::env::var_os("TRAV_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("trav")
        })
}

/// Daily-rotated log files under `<state>/logs` (7 kept), mirrored to stderr in
/// debug builds. Panics are logged with a backtrace before the default hook runs.
/// Flushes the log file when dropped (see `quit`).
static LOG_GUARD: Mutex<Option<tracing_appender::non_blocking::WorkerGuard>> = Mutex::new(None);

fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,hyper=warn,reqwest=warn".into());
    let logs = state_dir().join("logs");
    let _ = std::fs::create_dir_all(&logs);
    let file = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("trav-desktop")
        .filename_suffix("log")
        .max_log_files(7)
        .build(logs)
        .ok();
    let (file_layer, guard) = match file {
        Some(appender) => {
            let (w, g) = tracing_appender::non_blocking(appender);
            (
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(w),
                ),
                Some(g),
            )
        }
        None => (None, None),
    };
    let stderr = cfg!(debug_assertions).then(tracing_subscriber::fmt::layer);
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr)
        .try_init();

    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(target: "panic", "{info}\n{}", std::backtrace::Backtrace::force_capture());
        default(info);
    }));
    guard
}

pub fn run() {
    *LOG_GUARD.lock().unwrap() = init_logging();

    let shell = Arc::new(Shell {
        notify: AtomicBool::new(true),
        keep_awake: AtomicBool::new(true),
        tray_speed: AtomicBool::new(true),
        ..Default::default()
    });

    // Initial argv (`open -a Trav file.torrent --args …`, CLI use).
    let cwd = std::env::current_dir().ok();
    let autostarted = std::env::args().any(|a| a == AUTOSTART_ARG);
    shell.opens.lock().unwrap().add(
        std::env::args()
            .skip(1)
            .filter_map(|a| openable(&a, cwd.as_deref())),
        Instant::now(),
    );

    let app = tauri::Builder::default()
        // Must be first: a second launch forwards its args here and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let cwd = PathBuf::from(cwd);
            let items: Vec<String> = argv
                .iter()
                .skip(1)
                .filter_map(|a| openable(a, Some(&cwd)))
                .collect();
            if items.is_empty() {
                show(app);
            } else {
                hand_over(app, items);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args([AUTOSTART_ARG])
                .app_name("Trav")
                .build(),
        )
        .plugin(
            // Size and position only: a window closed to the tray still opens next time.
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .manage(shell.clone())
        .manage(menu::MenuState::default())
        .manage(update::Pending::default())
        .invoke_handler(tauri::generate_handler![
            rpc,
            take_pending_opens,
            set_notify,
            set_keep_awake,
            set_tray_speed,
            get_autostart,
            set_autostart,
            menu::set_menu_state,
            update::check_update,
            update::install_update,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let dir = state_dir();
            match tauri::async_runtime::block_on(Engine::start(&dir)) {
                Ok(engine) => {
                    forward_events(handle.clone(), engine.clone());
                    let _ = shell.engine.set(engine);
                }
                Err(e) => {
                    let msg = format!(
                        "Trav's engine could not start:\n\n{e}\n\nState directory: {}",
                        dir.display()
                    );
                    handle
                        .dialog()
                        .message(msg)
                        .kind(MessageDialogKind::Error)
                        .title("Trav")
                        .blocking_show();
                    std::process::exit(1);
                }
            }

            app.set_menu(menu::build(&handle)?)?;
            app.on_menu_event(|app, e| menu::on_event(app, e.id().as_ref()));

            // magnet: links — at launch and while running.
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                let items: Vec<String> = urls
                    .iter()
                    .filter_map(|u| openable(u.as_str(), None))
                    .collect();
                shell.opens.lock().unwrap().add(items, Instant::now());
            }
            let h = handle.clone();
            app.deep_link().on_open_url(move |e| {
                let items: Vec<String> = e
                    .urls()
                    .iter()
                    .filter_map(|u| openable(u.as_str(), None))
                    .collect();
                hand_over(&h, items);
            });

            // Started by the login item: stay in the background (Dock / tray).
            if autostarted
                && shell.opens.lock().unwrap().pending.is_empty()
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.hide();
            }

            tray::setup(&handle)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps transfers running, like other Mac apps;
            // the Dock icon or the menu bar extra brings it back.
            if let WindowEvent::CloseRequested { api, .. } = event
                && !window
                    .app_handle()
                    .state::<Arc<Shell>>()
                    .quitting
                    .load(Ordering::SeqCst)
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Trav");

    app.run(|app, event| match event {
        RunEvent::ExitRequested { api, code, .. }
            // Cmd+Q / OS shutdown: flush state first, then exit for real.
            if code.is_none() && !app.state::<Arc<Shell>>().quitting.load(Ordering::SeqCst) => {
                api.prevent_exit();
                quit(app, false);
            }
        RunEvent::Opened { urls } => {
            let items: Vec<String> = urls.iter().filter_map(|u| openable(u.as_str(), None)).collect();
            hand_over(app, items);
        }
        RunEvent::Reopen { .. } => show(app),
        // macOS is ending the app (Dock › Quit, logout, restart) and won't
        // wait for async work: save progress synchronously before it goes.
        RunEvent::Exit => {
            if let Some(engine) = app.state::<Arc<Shell>>().engine.get() {
                engine.save_now();
            }
            drop(LOG_GUARD.lock().unwrap().take());
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_open_reported_twice_is_queued_once() {
        let t = Instant::now();
        let mut o = Opens::default();
        assert!(o.add(["/a.torrent".to_string()], t));
        // The same open arriving through a second channel.
        assert!(!o.add(["/a.torrent".to_string()], t));
        assert!(o.add(["/a.torrent".into(), "magnet:?xt=b".into()], t));
        assert_eq!(o.take(), ["/a.torrent", "magnet:?xt=b"]);
        // Already taken by the UI, but still recent: ignored.
        assert!(!o.add(["/a.torrent".to_string()], t + Duration::from_secs(1)));
        // Opened again later: a real second request.
        assert!(o.add(["/a.torrent".to_string()], t + Opens::DEDUPE));
        assert_eq!(o.take(), ["/a.torrent"]);
    }

    #[test]
    fn trash_spares_files_that_arent_the_torrents() {
        let root = std::env::temp_dir().join(format!("trav-trash-{}", std::process::id()));
        let content = root.join("Show");
        std::fs::create_dir_all(content.join("extras")).unwrap();
        for f in ["ep1.mkv", "extras/a.srt", "notes.txt"] {
            std::fs::write(content.join(f), b"x").unwrap();
        }
        let mut files: HashSet<PathBuf> = ["ep1.mkv", "extras/a.srt"]
            .iter()
            .map(|f| content.join(f))
            .collect();
        let mut got = trash_targets(&content, &files);
        got.sort();
        assert_eq!(got, [content.join("ep1.mkv"), content.join("extras/a.srt")]);
        // With the torrent's files gone, its emptied folders go; the user's file stays.
        for f in &got {
            std::fs::remove_file(f).unwrap();
        }
        assert!(!prune_empty(&content));
        assert!(!content.join("extras").exists());
        assert!(content.join("notes.txt").exists());
        // When everything there is the torrent's, the folder goes in one piece.
        files.insert(content.join("notes.txt"));
        assert_eq!(
            trash_targets(&content, &files),
            std::slice::from_ref(&content)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
