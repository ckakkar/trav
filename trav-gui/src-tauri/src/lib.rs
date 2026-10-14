//! Trav for macOS: hosts the engine in-process and exposes it to the webview
//! through a single `rpc` command (same surface as `trav --daemon`), plus the
//! native pieces: menu bar, Dock, menu bar extra, updates, Trash, login item.
#![cfg(target_os = "macos")]

mod menu;
mod update;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;
use trav_core::{Engine, EngineHandle, Event, TorrentStatus};

pub(crate) const REPO_URL: &str = "https://github.com/ckakkar/trav";
/// Passed by the login item, so Trav starts in the background.
const AUTOSTART_ARG: &str = "--autostart";

#[derive(Default)]
struct Shell {
    engine: OnceLock<EngineHandle>,
    /// Torrent paths / magnet links handed over by the OS, drained by the UI.
    pending: Mutex<Vec<String>>,
    notify: AtomicBool,
    keep_awake: AtomicBool,
    quitting: AtomicBool,
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
    let paths: Vec<PathBuf> = hashes
        .iter()
        .filter_map(|h| engine.details(h))
        .filter_map(|d| {
            let save = PathBuf::from(&d.summary.save_path);
            let content = PathBuf::from(d.content_path?);
            // Only the torrent's own file or folder inside its save folder.
            (content != save && content.starts_with(&save) && content.exists()).then_some(content)
        })
        .collect();
    params["deleteFiles"] = Value::Bool(false);
    let result = trav_core::rpc::dispatch(engine, "remove", params).await?;
    if !paths.is_empty() {
        tauri::async_runtime::spawn_blocking(move || trash_paths(&paths))
            .await
            .map_err(|e| e.to_string())??;
    }
    Ok(result)
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
    std::mem::take(&mut *state.pending.lock().unwrap())
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
fn openable(arg: &str, cwd: Option<&Path>) -> Option<String> {
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
fn hand_over(app: &AppHandle, items: impl IntoIterator<Item = String>) {
    let shell = app.state::<Arc<Shell>>();
    let mut q = shell.pending.lock().unwrap();
    let before = q.len();
    q.extend(items);
    if q.len() > before {
        drop(q);
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

fn toggle(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false) {
            let _ = w.hide();
        } else {
            show(app);
        }
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
        if restart {
            app.restart();
        }
        app.exit(0);
    });
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
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show_i = MenuItem::with_id(app, "tray.show", "Show Trav", true, None::<&str>)?;
    let resume_i = MenuItem::with_id(app, "tray.resume", "Resume all", true, None::<&str>)?;
    let pause_i = MenuItem::with_id(app, "tray.pause", "Pause all", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "tray.quit", "Quit Trav", true, None::<&str>)?;
    let speed_i = MenuItem::with_id(app, "tray.speed", "↓ — · ↑ —", false, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &speed_i,
            &PredefinedMenuItem::separator(app)?,
            &show_i,
            &resume_i,
            &pause_i,
            &PredefinedMenuItem::separator(app)?,
            &quit_i,
        ],
    )?;
    // A monochrome template glyph, tinted by the menu bar.
    let tray = TrayIconBuilder::with_id("main")
        .icon(tauri::include_image!("icons/tray.png"))
        .icon_as_template(true)
        .tooltip("Trav")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| {
            let engine = app.state::<Arc<Shell>>().engine.get().cloned();
            match e.id.as_ref() {
                "tray.show" => show(app),
                "tray.resume" => {
                    if let Some(e) = engine {
                        e.resume_all();
                    }
                }
                "tray.pause" => {
                    if let Some(e) = engine {
                        e.pause_all();
                    }
                }
                "tray.quit" => quit(app, false),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                toggle(tray.app_handle());
            }
        })
        .build(app)?;

    // Every 2 s: speeds in the menu bar extra, overall download progress on
    // the Dock icon, and the keep-awake assertion. (No count badge: macOS hides
    // Dock badges of ad-hoc signed apps that post notifications.)
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last: Option<Option<u64>> = None;
        let mut awake: Option<keepawake::KeepAwake> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let shell = app.state::<Arc<Shell>>().inner().clone();
            let Some(engine) = shell.engine.get().cloned() else {
                continue;
            };
            let s = engine.snapshot();
            let line = format!(
                "↓ {} · ↑ {}",
                fmt_rate(s.stats.download_rate),
                fmt_rate(s.stats.upload_rate)
            );
            let _ = tray.set_tooltip(Some(format!("Trav — {line}")));
            let _ = speed_i.set_text(&line);

            let active: Vec<_> = s
                .torrents
                .iter()
                .filter(|t| {
                    matches!(
                        t.status,
                        TorrentStatus::Downloading | TorrentStatus::Metadata
                    )
                })
                .collect();
            let (size, done) = active
                .iter()
                .fold((0u64, 0u64), |(a, b), t| (a + t.size, b + t.done_bytes));
            let percent = (!active.is_empty()).then(|| (done * 100).checked_div(size).unwrap_or(0));
            if last != Some(percent)
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
                last = Some(percent);
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
    });
    Ok(())
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
    // Shared with the `trav` CLI so both front-ends see the same library.
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
    let _log_guard = init_logging();

    let shell = Arc::new(Shell {
        notify: AtomicBool::new(true),
        keep_awake: AtomicBool::new(true),
        ..Default::default()
    });

    // Initial argv (`open -a Trav file.torrent --args …`, CLI use).
    let cwd = std::env::current_dir().ok();
    let autostarted = std::env::args().any(|a| a == AUTOSTART_ARG);
    shell.pending.lock().unwrap().extend(
        std::env::args()
            .skip(1)
            .filter_map(|a| openable(&a, cwd.as_deref())),
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
                shell.pending.lock().unwrap().extend(items);
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
                && shell.pending.lock().unwrap().is_empty()
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.hide();
            }

            setup_tray(&handle)?;
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
        _ => {}
    });
}
