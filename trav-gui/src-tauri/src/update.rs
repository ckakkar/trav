//! In-app updates from GitHub releases.
//!
//! The native side checks (15 s after launch, then hourly) and tells the
//! window with `trav://update-available`; the window shows the release notes
//! and calls `install_update`, which downloads (reporting
//! `trav://update-progress`), installs over the running app, stops the engine
//! cleanly and relaunches. The window acknowledges each announcement with
//! `update_ack`: a window that has crashed can't, and then a native dialog
//! offers the update instead, so a broken release can always be replaced.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Update, UpdaterExt};

/// First check after launch, then how often to look again.
const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// How long the window has to acknowledge an announcement before the native
/// dialog takes over.
const ACK_WITHIN: Duration = Duration::from_secs(30);

/// The update found by the last check, waiting to be installed.
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

/// Update checking done on the native side, so it works without the window.
pub struct Updates {
    /// Check periodically (the window's "Check for updates automatically").
    auto: AtomicBool,
    announced: Mutex<Option<Announcement>>,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            auto: AtomicBool::new(true),
            announced: Mutex::new(None),
        }
    }
}

/// The latest update the window was told about.
struct Announcement {
    version: String,
    at: Instant,
    /// The window answered: it is showing the update, or the user put it off.
    acked: bool,
    /// The native dialog was offered for it already.
    fell_back: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Available {
    info: UpdateInfo,
    /// Asked for from the menu: show it even if the user put this version off.
    manual: bool,
}

#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// Ask GitHub for a newer release; a found update is kept for `install_update`.
async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.date().to_string()),
    });
    *app.state::<Pending>().0.lock().unwrap() = update;
    Ok(info)
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    check(&app).await
}

/// Check now and announce the result to the window (the menu's "Check for
/// Updates…"). Being up to date is announced too, so the window can say so.
pub fn check_now(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match check(&app).await {
            Ok(Some(info)) => announce(&app, info, true),
            Ok(None) => {
                let _ = app.emit("trav://update-none", ());
            }
            Err(e) => {
                tracing::warn!("update check failed: {e}");
                let _ = app.emit("trav://update-failed", e);
            }
        }
    });
}

/// Record an announcement; false when it's a re-check of a version already
/// announced, which stays quiet (a menu check always announces).
fn record(announced: &mut Option<Announcement>, version: &str, manual: bool, now: Instant) -> bool {
    if !manual && announced.as_ref().is_some_and(|a| a.version == version) {
        return false;
    }
    *announced = Some(Announcement {
        version: version.to_string(),
        at: now,
        acked: false,
        fell_back: false,
    });
    true
}

/// The version to offer natively: announced, unanswered for `ACK_WITHIN`
/// while the window was on screen, and not offered already.
fn due_fallback(
    announced: &mut Option<Announcement>,
    visible: bool,
    now: Instant,
) -> Option<String> {
    let a = announced.as_mut()?;
    if a.acked || a.fell_back || !visible || now.duration_since(a.at) < ACK_WITHIN {
        return None;
    }
    a.fell_back = true;
    Some(a.version.clone())
}

fn announce(app: &AppHandle, info: UpdateInfo, manual: bool) {
    let fresh = {
        let state = app.state::<Updates>();
        let mut announced = state.announced.lock().unwrap();
        record(&mut announced, &info.version, manual, Instant::now())
    };
    if fresh {
        let _ = app.emit("trav://update-available", Available { info, manual });
    }
}

/// The window received an announcement (and is showing it or was told to wait).
#[tauri::command]
pub fn update_ack(version: String, app: AppHandle) {
    let state = app.state::<Updates>();
    if let Some(a) = state.announced.lock().unwrap().as_mut()
        && a.version == version
    {
        a.acked = true;
    }
}

#[tauri::command]
pub fn set_auto_update(enabled: bool, app: AppHandle) {
    app.state::<Updates>()
        .auto
        .store(enabled, Ordering::Relaxed);
}

/// Check periodically, and offer an update natively when the window doesn't
/// acknowledge it (it has crashed, or never loaded).
pub fn spawn_checker(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        let mut last_check: Option<Instant> = None;
        loop {
            let state = app.state::<Updates>();
            if state.auto.load(Ordering::Relaxed)
                && last_check.is_none_or(|t| t.elapsed() >= CHECK_EVERY)
            {
                last_check = Some(Instant::now());
                match check(&app).await {
                    Ok(Some(info)) => announce(&app, info, false),
                    Ok(None) => {}
                    Err(e) => tracing::warn!("update check failed: {e}"),
                }
            }
            let unanswered = {
                let visible = window_visible(&app);
                due_fallback(
                    &mut state.announced.lock().unwrap(),
                    visible,
                    Instant::now(),
                )
            };
            if let Some(version) = unanswered {
                tracing::warn!(
                    "the window didn't answer the update to {version}; offering it natively"
                );
                offer_natively(&app, version);
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

fn window_visible(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

fn offer_natively(app: &AppHandle, version: String) {
    let app = app.clone();
    app.dialog()
        .message(format!(
            "Trav {version} is available. The window isn't responding, so it can't show the update; \
             install it now? Transfers resume after Trav relaunches."
        ))
        .title("Software update")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install & Relaunch".into(),
            "Later".into(),
        ))
        .show(move |install| {
            if !install {
                return;
            }
            tauri::async_runtime::spawn(async move {
                if let Err(e) = install_update(app.clone()).await {
                    tracing::error!("installing the update failed: {e}");
                    app.dialog()
                        .message(format!("The update couldn't be installed: {e}"))
                        .title("Software update")
                        .kind(MessageDialogKind::Error)
                        .show(|_| {});
                }
            });
        });
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let update = app
        .state::<Pending>()
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or("No update is waiting to be installed.")?;
    let emitter = app.clone();
    let mut downloaded = 0u64;
    let installed = update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = emitter.emit("trav://update-progress", Progress { downloaded, total });
            },
            || {},
        )
        .await;
    if let Err(e) = installed {
        // Keep it for another try (unless a newer check has replaced it).
        let state = app.state::<Pending>();
        let mut pending = state.0.lock().unwrap();
        if pending.is_none() {
            *pending = Some(update);
        }
        return Err(e.to_string());
    }
    tracing::info!("update {} installed; restarting", update.version);
    crate::quit(&app, true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rechecks_stay_quiet_and_menu_checks_announce_again() {
        let t = Instant::now();
        let mut a = None;
        assert!(record(&mut a, "0.4.1", false, t));
        assert!(!record(&mut a, "0.4.1", false, t + CHECK_EVERY));
        assert!(record(&mut a, "0.4.1", true, t + CHECK_EVERY));
        assert!(record(&mut a, "0.4.2", false, t + CHECK_EVERY));
        assert_eq!(a.unwrap().version, "0.4.2");
    }

    #[test]
    fn falls_back_once_when_a_visible_window_never_answers() {
        let t = Instant::now();
        let mut a = None;
        record(&mut a, "0.4.1", false, t);
        // Not yet: give the window its time.
        assert_eq!(due_fallback(&mut a, true, t + ACK_WITHIN / 2), None);
        // A hidden window may simply be asleep: wait for it to be shown.
        assert_eq!(due_fallback(&mut a, false, t + ACK_WITHIN * 2), None);
        assert_eq!(
            due_fallback(&mut a, true, t + ACK_WITHIN * 2).as_deref(),
            Some("0.4.1")
        );
        // Offered once per announcement.
        assert_eq!(due_fallback(&mut a, true, t + ACK_WITHIN * 3), None);
    }

    #[test]
    fn an_answered_announcement_never_falls_back() {
        let t = Instant::now();
        let mut a = None;
        record(&mut a, "0.4.1", false, t);
        a.as_mut().unwrap().acked = true;
        assert_eq!(due_fallback(&mut a, true, t + ACK_WITHIN * 10), None);
    }
}
