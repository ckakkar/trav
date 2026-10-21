// Errors in the window go to Trav's log file (`<library>/logs`) through the
// app, with their stacks; the webview's own console is gone once it closes.

import { invoke } from "@tauri-apps/api/core";

export function reportError(kind: string, e: unknown, extra?: string) {
  const err = e instanceof Error ? e : new Error(typeof e === "string" ? e : JSON.stringify(e) ?? String(e));
  const stack = [err.stack, extra].filter(Boolean).join("\n");
  invoke("ui_error", { kind, message: err.message, stack: stack || null }).catch(() => {});
}

let installed = false;

/** Log uncaught exceptions and unhandled promise rejections (once per page). */
export function installErrorReporting() {
  if (installed || typeof window === "undefined") return;
  installed = true;
  window.addEventListener("error", (e) => reportError("uncaught", e.error ?? e.message));
  window.addEventListener("unhandledrejection", (e) => reportError("unhandled rejection", e.reason));
}
