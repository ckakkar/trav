// Transport: Tauri IPC inside the desktop app, HTTP against `trav --daemon`
// everywhere else. Both land in trav-core's `rpc::dispatch`, so the method
// names and payloads are identical.

import type { EngineEvent } from "./types";

export class AuthError extends Error {
  constructor() {
    super("unauthorized");
  }
}

export const isDesktop = (): boolean => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const isMac = (): boolean =>
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

const TOKEN_KEY = "trav.token";

function apiBase(): string {
  return (process.env.NEXT_PUBLIC_TRAV_API ?? "").replace(/\/$/, "");
}

export function getToken(): string | null {
  try {
    const fromUrl = new URLSearchParams(window.location.search).get("token");
    if (fromUrl) {
      localStorage.setItem(TOKEN_KEY, fromUrl);
      return fromUrl;
    }
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setToken(t: string) {
  try {
    localStorage.setItem(TOKEN_KEY, t);
  } catch {
    /* storage unavailable */
  }
}

export async function call<T = unknown>(method: string, params?: unknown): Promise<T> {
  if (isDesktop()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("rpc", { method, params: params ?? null });
  }
  const token = getToken();
  const res = await fetch(`${apiBase()}/api/rpc`, {
    method: "POST",
    headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
    body: JSON.stringify({ method, params: params ?? null }),
  });
  if (res.status === 401) throw new AuthError();
  const body = (await res.json().catch(() => null)) as { ok: boolean; result?: T; error?: string } | null;
  if (!body) throw new Error(`HTTP ${res.status}`);
  if (!body.ok) throw new Error(body.error ?? "request failed");
  return body.result as T;
}

/** Subscribe to engine events (completion, errors…). Returns an unsubscribe fn. */
export function onEngineEvent(cb: (e: EngineEvent) => void): () => void {
  if (isDesktop()) {
    let off: (() => void) | undefined;
    let dead = false;
    import("@tauri-apps/api/event").then(({ listen }) =>
      listen<EngineEvent>("trav://event", (e) => cb(e.payload)).then((u) => (dead ? u() : (off = u))),
    );
    return () => {
      dead = true;
      off?.();
    };
  }
  const token = getToken();
  const es = new EventSource(`${apiBase()}/api/events${token ? `?token=${encodeURIComponent(token)}` : ""}`);
  es.onmessage = (m) => {
    try {
      cb(JSON.parse(m.data));
    } catch {
      /* ignore malformed */
    }
  };
  return () => es.close();
}

// ── Desktop-only integrations (no-ops / fallbacks on the web) ─────────────────

export async function pickTorrentFiles(): Promise<string[]> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ multiple: true, directory: false, filters: [{ name: "Torrent", extensions: ["torrent"] }] });
  if (!r) return [];
  return Array.isArray(r) ? r : [r];
}

export async function pickFolder(defaultPath?: string): Promise<string | null> {
  if (!isDesktop()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: true, multiple: false, defaultPath });
  return typeof r === "string" ? r : null;
}

export async function reveal(path: string) {
  if (!isDesktop()) return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

export async function openPath(path: string) {
  if (!isDesktop()) return;
  const { openPath } = await import("@tauri-apps/plugin-opener");
  await openPath(path);
}

/** Paths/magnets handed to the app by the OS (file association, magnet: links, CLI args). */
export async function takePendingOpens(): Promise<string[]> {
  if (!isDesktop()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string[]>("take_pending_opens");
}

export function onOpenRequest(cb: () => void): () => void {
  if (!isDesktop()) return () => {};
  let off: (() => void) | undefined;
  let dead = false;
  import("@tauri-apps/api/event").then(({ listen }) =>
    listen("trav://open", () => cb()).then((u) => (dead ? u() : (off = u))),
  );
  return () => {
    dead = true;
    off?.();
  };
}

/** Native file drag & drop (desktop). Web drag & drop is handled with DOM events. */
export function onNativeDrop(onHover: (over: boolean) => void, onDrop: (paths: string[]) => void): () => void {
  if (!isDesktop()) return () => {};
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

export function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => {
      const s = String(r.result);
      resolve(s.slice(s.indexOf(",") + 1));
    };
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
}
