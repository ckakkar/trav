//! The macOS menu bar. Items the UI acts on are forwarded to the webview as
//! `trav://menu` events carrying the item id; the UI reports its selection
//! back through `set_menu_state` so items enable and relabel themselves.

use std::sync::Mutex;

use serde::Deserialize;
use tauri::menu::{AboutMetadata, CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Wry};

pub const HELP_URL: &str = "https://github.com/ckakkar/trav/blob/main/USAGE.md";
pub const ISSUES_URL: &str = "https://github.com/ckakkar/trav/issues/new";

/// Items whose state follows the selection.
pub struct Handles {
    toggle_run: MenuItem<Wry>,
    sequential: CheckMenuItem<Wry>,
    for_selection: Vec<MenuItem<Wry>>,
    for_one: Vec<MenuItem<Wry>>,
}

#[derive(Default)]
pub struct MenuState(pub Mutex<Option<Handles>>);

/// What the UI has selected.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Selection {
    count: usize,
    all_paused: bool,
    sequential: bool,
}

#[tauri::command]
pub fn set_menu_state(selection: Selection, state: tauri::State<'_, MenuState>) {
    let Some(h) = &*state.0.lock().unwrap() else {
        return;
    };
    let any = selection.count > 0;
    let one = selection.count == 1;
    let _ = h.toggle_run.set_text(if any && selection.all_paused {
        "Resume"
    } else {
        "Pause"
    });
    let _ = h.toggle_run.set_enabled(any);
    for i in &h.for_selection {
        let _ = i.set_enabled(any);
    }
    for i in &h.for_one {
        let _ = i.set_enabled(one);
    }
    let _ = h.sequential.set_enabled(one);
    let _ = h.sequential.set_checked(one && selection.sequential);
}

/// App menu ids carry an `app.` prefix: menu events also reach the tray's handler.
fn item(
    app: &AppHandle,
    id: &str,
    text: &str,
    accel: Option<&str>,
) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, format!("app.{id}"), text, true, accel)
}

pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let sep = || PredefinedMenuItem::separator(app);
    let about = AboutMetadata {
        name: Some("Trav".into()),
        version: Some(app.package_info().version.to_string()),
        copyright: Some("© 2026 Cyrus Kakkar".into()),
        credits: Some("App by Cyrus · kkrwhofrags.xyz".into()),
        website: Some("https://github.com/ckakkar/trav".into()),
        website_label: Some("github.com/ckakkar/trav".into()),
        ..Default::default()
    };
    let app_menu = Submenu::with_items(
        app,
        "Trav",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Trav"), Some(about))?,
            &item(app, "check-updates", "Check for Updates…", None)?,
            &sep()?,
            &item(app, "settings", "Settings…", Some("CmdOrCtrl+,"))?,
            &sep()?,
            &PredefinedMenuItem::services(app, None)?,
            &sep()?,
            &PredefinedMenuItem::hide(app, Some("Hide Trav"))?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &sep()?,
            // Ours, not the predefined item: that terminates on the spot,
            // before the engine can save progress and tell trackers.
            &item(app, "quit", "Quit Trav", Some("CmdOrCtrl+Q"))?,
        ],
    )?;

    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item(
                app,
                "open-torrent",
                "Open Torrent File…",
                Some("CmdOrCtrl+O"),
            )?,
            &item(app, "add-magnet", "Add Magnet Link…", Some("CmdOrCtrl+U"))?,
            &sep()?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &sep()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
            &sep()?,
            &item(app, "find", "Find…", Some("CmdOrCtrl+F"))?,
        ],
    )?;

    let filters = [
        ("all", "All"),
        ("downloading", "Downloading"),
        ("seeding", "Seeding"),
        ("completed", "Completed"),
        ("active", "Active"),
        ("paused", "Paused"),
        ("error", "Errors"),
    ];
    let filter_items = filters
        .iter()
        .enumerate()
        .map(|(i, (id, label))| {
            item(
                app,
                &format!("filter:{id}"),
                label,
                Some(&format!("CmdOrCtrl+{}", i + 1)),
            )
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let view = Submenu::new(app, "View", true)?;
    view.append(&item(
        app,
        "palette",
        "Command Palette…",
        Some("CmdOrCtrl+K"),
    )?)?;
    view.append(&sep()?)?;
    for f in &filter_items {
        view.append(f)?;
    }
    view.append(&sep()?)?;
    view.append(&item(
        app,
        "toggle-details",
        "Show/Hide Details",
        Some("CmdOrCtrl+I"),
    )?)?;
    view.append(&item(
        app,
        "theme",
        "Next Theme",
        Some("Shift+CmdOrCtrl+L"),
    )?)?;
    view.append(&sep()?)?;
    view.append(&PredefinedMenuItem::fullscreen(app, None)?)?;

    let toggle_run = item(app, "toggle-run", "Pause", Some("CmdOrCtrl+."))?;
    let reveal = item(app, "reveal", "Show in Finder", Some("CmdOrCtrl+R"))?;
    let copy_magnet = item(
        app,
        "copy-magnet",
        "Copy Magnet Link",
        Some("Shift+CmdOrCtrl+C"),
    )?;
    let recheck = item(app, "recheck", "Verify Local Data", None)?;
    let reannounce = item(app, "reannounce", "Ask Trackers for More Peers", None)?;
    let sequential = CheckMenuItem::with_id(
        app,
        "app.sequential",
        "Download in Order",
        true,
        false,
        None::<&str>,
    )?;
    let top = item(app, "queue-top", "Move to Top of Queue", None)?;
    let bottom = item(app, "queue-bottom", "Move to Bottom of Queue", None)?;
    let remove = item(app, "remove", "Remove…", Some("CmdOrCtrl+Backspace"))?;
    let remove_data = item(
        app,
        "remove-data",
        "Remove and Move Data to Trash…",
        Some("Alt+CmdOrCtrl+Backspace"),
    )?;
    let transfers = Submenu::with_items(
        app,
        "Transfers",
        true,
        &[
            &toggle_run,
            &item(app, "pause-all", "Pause All", Some("Alt+CmdOrCtrl+."))?,
            &item(app, "resume-all", "Resume All", Some("Alt+CmdOrCtrl+/"))?,
            &sep()?,
            &reveal,
            &copy_magnet,
            &sep()?,
            &sequential,
            &top,
            &bottom,
            &sep()?,
            &recheck,
            &reannounce,
            &sep()?,
            &remove,
            &remove_data,
        ],
    )?;

    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, Some("Zoom"))?,
            &sep()?,
            &item(app, "show-main", "Trav", Some("CmdOrCtrl+0"))?,
            &sep()?,
            &PredefinedMenuItem::bring_all_to_front(app, None)?,
        ],
    )?;
    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[
            &item(app, "help", "Trav Help", None)?,
            &item(app, "releases", "Release Notes", None)?,
            &sep()?,
            &item(app, "issue", "Report an Issue…", None)?,
        ],
    )?;
    let _ = window.set_as_windows_menu_for_nsapp();
    let _ = help.set_as_help_menu_for_nsapp();

    let menu = Menu::with_items(
        app,
        &[&app_menu, &file, &edit, &view, &transfers, &window, &help],
    )?;

    let handles = Handles {
        toggle_run: toggle_run.clone(),
        sequential,
        for_selection: vec![reveal, recheck, reannounce, remove, remove_data],
        for_one: vec![copy_magnet, top, bottom],
    };
    *app.state::<MenuState>().0.lock().unwrap() = Some(handles);
    set_menu_state(Selection::default(), app.state::<MenuState>());
    Ok(menu)
}

/// Handle a menu click: open links here, everything else in the UI.
pub fn on_event(app: &AppHandle, id: &str) {
    use tauri_plugin_opener::OpenerExt;
    let Some(id) = id.strip_prefix("app.") else {
        return;
    };
    if id == "quit" {
        crate::quit(app, false);
        return;
    }
    let url = match id {
        "help" => Some(HELP_URL.to_string()),
        "issue" => Some(ISSUES_URL.to_string()),
        "releases" => Some(format!("{}/releases", crate::REPO_URL)),
        _ => None,
    };
    if let Some(url) = url {
        let _ = app.opener().open_url(url, None::<&str>);
        return;
    }
    crate::show(app);
    if id != "show-main" {
        let _ = app.emit("trav://menu", id);
    }
}
