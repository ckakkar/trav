import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

// Tauri's IPC mocks hang off `window`.
beforeEach(() => {
  (globalThis as { window?: unknown }).window = globalThis;
});
afterEach(() => clearMocks());

describe("IPC bridge", () => {
  it("routes engine calls through the rpc command", async () => {
    const seen: [string, unknown][] = [];
    mockIPC((cmd, args) => {
      seen.push([cmd, args]);
      return { seq: 3 };
    });
    const { call } = await import("@/lib/rpc");
    await expect(call("snapshot")).resolves.toEqual({ seq: 3 });
    await call("pause", { hashes: ["ab"] });
    expect(seen).toEqual([
      ["rpc", { method: "snapshot", params: null }],
      ["rpc", { method: "pause", params: { hashes: ["ab"] } }],
    ]);
  });

  it("surfaces engine errors", async () => {
    mockIPC(() => {
      throw "torrent is already in the list";
    });
    const { call } = await import("@/lib/rpc");
    await expect(call("add", { magnet: "x" })).rejects.toBe("torrent is already in the list");
  });

  it("tells the menu bar about the selection", async () => {
    const seen: [string, unknown][] = [];
    mockIPC((cmd, args) => void seen.push([cmd, args]));
    const { setMenuState } = await import("@/lib/rpc");
    setMenuState({ count: 2, allPaused: true, sequential: false });
    await new Promise((r) => setTimeout(r, 0));
    expect(seen).toEqual([["set_menu_state", { selection: { count: 2, allPaused: true, sequential: false } }]]);
  });

  it("returns update info, or null when up to date", async () => {
    mockIPC((cmd) => (cmd === "check_update" ? { version: "0.3.9", currentVersion: "0.3.8", notes: null, date: null } : null));
    const { checkUpdate } = await import("@/lib/rpc");
    await expect(checkUpdate()).resolves.toMatchObject({ version: "0.3.9" });
    mockIPC(() => null);
    await expect(checkUpdate()).resolves.toBeNull();
  });
});
