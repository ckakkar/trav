// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, RwLock};
use trav_core::snapshot::{EngineSnapshot, TorrentSnapshot};
use trav_core::Engine;

struct AppState(Arc<RwLock<EngineSnapshot>>);

#[tauri::command]
fn get_snapshot(state: tauri::State<AppState>) -> Result<EngineSnapshot, String> {
    match state.0.read() {
        Ok(snap) => Ok(snap.clone()),
        Err(e) => Err(format!("Lock poisoned: {}", e))
    }
}

fn main() {
    // We launch Tokio and the core engine natively alongside Tauri!
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let (engine, _command_tx, _event_rx, snapshot) = rt.block_on(async { Engine::new() });

    rt.spawn(async move {
        if let Err(e) = engine.run().await {
            eprintln!("Trav core engine crashed: {}", e);
        }
    });

    tauri::Builder::default()
        .manage(AppState(snapshot))
        .invoke_handler(tauri::generate_handler![get_snapshot])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
