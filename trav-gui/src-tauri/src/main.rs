// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, RwLock};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tauri::{CustomMenuItem, Manager, SystemTray, SystemTrayEvent, SystemTrayMenu};
use trav_core::message::Command;
use trav_core::snapshot::EngineSnapshot;
use trav_core::Engine;

struct AppState {
    snapshot: Arc<RwLock<EngineSnapshot>>,
    command_tx: mpsc::Sender<Command>,
}

#[tauri::command]
fn get_snapshot(state: tauri::State<AppState>) -> Result<EngineSnapshot, String> {
    match state.snapshot.read() {
        Ok(snap) => Ok(snap.clone()),
        Err(e) => Err(format!("Lock poisoned: {}", e))
    }
}

#[tauri::command]
async fn add_torrent(file_path: String, download_dir: Option<String>, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let torrent_path = PathBuf::from(&file_path);
    if !torrent_path.exists() || torrent_path.extension().and_then(|e| e.to_str()) != Some("torrent") {
        return Err("Only existing .torrent files are accepted".to_string());
    }

    let target_download_dir = download_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./downloads"));

    state
        .command_tx
        .send(Command::AddTorrent {
            file_path: torrent_path,
            download_dir: target_download_dir,
        })
        .await
        .map_err(|e| format!("Failed to send AddTorrent command: {}", e))
}

fn main() {
    // We launch Tokio and the core engine natively alongside Tauri!
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let (engine, command_tx, _event_rx, snapshot) = rt.block_on(async { Engine::new() });

    rt.spawn(async move {
        if let Err(e) = engine.run().await {
            eprintln!("Trav core engine crashed: {}", e);
        }
    });

    let tray_menu = SystemTrayMenu::new()
        .add_item(CustomMenuItem::new("toggle".to_string(), "Show/Hide"))
        .add_item(CustomMenuItem::new("quit".to_string(), "Quit"));
    let system_tray = SystemTray::new().with_menu(tray_menu);

    tauri::Builder::default()
        .manage(AppState { snapshot, command_tx })
        .invoke_handler(tauri::generate_handler![get_snapshot, add_torrent])
        .system_tray(system_tray)
        .on_system_tray_event(|app, event| {
            match event {
                SystemTrayEvent::MenuItemClick { id, .. } if id.as_str() == "quit" => {
                    std::process::exit(0);
                }
                SystemTrayEvent::MenuItemClick { id, .. } if id.as_str() == "toggle" => {
                    if let Some(window) = app.get_window("main") {
                        if window.is_visible().unwrap_or(true) {
                            let _ = window.hide();
                        } else {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
