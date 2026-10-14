//! In-app updates from GitHub releases. The UI calls `check_update`, shows the
//! release notes, then `install_update`, which downloads (reporting
//! `trav://update-progress`), installs over the running app, stops the engine
//! cleanly and relaunches.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The update found by the last check, waiting to be installed.
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
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
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = emitter.emit("trav://update-progress", Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("update {} installed; restarting", update.version);
    crate::quit(&app, true);
    Ok(())
}
