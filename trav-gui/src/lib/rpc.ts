// The bridge to the app's Rust side (src-tauri): every engine call is an IPC
// `rpc` command routed to trav-core's `rpc::dispatch`, plus the native
// integrations (dialogs, Finder, menu bar, updates, login item, keep-awake).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { EngineEvent } from "./types";

export async function call<T = unknown>(method: string, params?: unknown): Promise<T> {
  return invoke<T>("rpc", { method, params: params ?? null });
}

/** Subscribe to an app event; returns an unsubscribe function. */
function subscribe<T>(event: string, cb: (payload: T) => void): () => void {
  let off: (() => void) | undefined;
  let dead = false;
  listen<T>(event, (e) => cb(e.payload)).then((u) => (dead ? u() : (off = u)));
  return () => {
    dead = true;
    off?.();
  };
}

/** Engine events (completion, errors, metadata…). */
export const onEngineEvent = (cb: (e: EngineEvent) => void) => subscribe<EngineEvent>("trav://event", cb);

export async function pickTorrentFiles(): Promise<string[]> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ multiple: true, directory: false, filters: [{ name: "Torrent", extensions: ["torrent"] }] });
  if (!r) return [];
  return Array.isArray(r) ? r : [r];
}

export async function pickFolder(defaultPath?: string): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: true, multiple: false, defaultPath });
  return typeof r === "string" ? r : null;
}

export async function reveal(path: string) {
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

/** The maker's site, in the default browser. */
export const AUTHOR_URL = "https://kkrwhofrags.xyz";
export async function openAuthorSite() {
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(AUTHOR_URL);
}

export async function openPath(path: string) {
  const { openPath } = await import("@tauri-apps/plugin-opener");
  await openPath(path);
}

/** Torrent files and magnet links handed to the app by Finder and browsers. */
export const takePendingOpens = () => invoke<string[]>("take_pending_opens");

export const onOpenRequest = (cb: () => void) => subscribe<null>("trav://open", () => cb());

/** Files dragged onto the window. */
export function onNativeDrop(onHover: (over: boolean) => void, onDrop: (paths: string[]) => void): () => void {
  let off: (() => void) | undefined;
  let dead = false;
  import("@tauri-apps/api/webview").then(({ getCurrentWebview }) =>
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "enter" || p.type === "over") onHover(true);
        else if (p.type === "leave") onHover(false);
        else if (p.type === "drop") {
          onHover(false);
          onDrop(p.paths);
        }
      })
      .then((u) => (dead ? u() : (off = u))),
  );
  return () => {
    dead = true;
    off?.();
  };
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Menu bar commands (ids like "open-torrent", "filter:seeding", "remove"). */
export const onMenu = (cb: (id: string) => void) => subscribe<string>("trav://menu", cb);

/** Lets the menu bar enable items and say "Pause" or "Resume". */
export function setMenuState(selection: { count: number; allPaused: boolean; sequential: boolean }) {
  invoke("set_menu_state", { selection }).catch(() => {});
}

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
  date: string | null;
}

/** Ask GitHub for a newer release; null when up to date. */
export const checkUpdate = () => invoke<UpdateInfo | null>("check_update");

/** Download and install the update found by `checkUpdate`, then relaunch. */
export async function installUpdate(onProgress: (downloaded: number, total: number | null) => void): Promise<void> {
  const off = subscribe<{ downloaded: number; total: number | null }>("trav://update-progress", (p) =>
    onProgress(p.downloaded, p.total),
  );
  try {
    await invoke("install_update");
  } finally {
    off();
  }
}

export const getAutostart = () => invoke<boolean>("get_autostart");
export const setAutostart = (enabled: boolean) => invoke<boolean>("set_autostart", { enabled });

export function setKeepAwake(enabled: boolean) {
  invoke("set_keep_awake", { enabled }).catch(() => {});
}

export function setNotify(enabled: boolean) {
  invoke("set_notify", { enabled }).catch(() => {});
}

/** Show the download speed beside the menu bar icon. */
export function setTraySpeed(enabled: boolean) {
  invoke("set_tray_speed", { enabled }).catch(() => {});
}

export async function appVersion(): Promise<string> {
  const { getVersion } = await import("@tauri-apps/api/app");
  return getVersion();
}
