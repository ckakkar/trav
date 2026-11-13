// @vitest-environment happy-dom
//
// How the window answers update announcements from update.rs (adopted from
// QA's findings F1 and F3): it acknowledges only once the update dialog has
// actually rendered, and it picks up an announcement sent before it was
// listening. Either failing would leave the native fallback to misfire.

import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { EngineSnapshot } from "@/lib/types";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const INFO = { version: "9.9.9", currentVersion: "0.3.0", notes: "### Fixed\n- Things", date: null };

const EMPTY: EngineSnapshot = {
  seq: 1,
  torrents: [],
  stats: {
    altSpeed: false, downloadRate: 0, uploadRate: 0, sessionDownloaded: 0, sessionUploaded: 0, downloadLimit: 0, uploadLimit: 0,
    uploadAuto: false, connectedPeers: 0, dhtNodes: 0, listenPort: 51413, connectable: false, freeSpace: null, upnp: null,
  },
};

/** Each update_ack, with whether the dialog was on screen when it came. */
let acks: { version: string; dialogShown: boolean }[] = [];
let pending: { info: typeof INFO; manual: boolean } | null = null;

const dialogShown = () => document.body.textContent?.includes(`Trav ${INFO.version} is here.`) ?? false;

function mockEngine() {
  mockWindows("main");
  mockIPC(
    (cmd, args) => {
      const a = (args ?? {}) as { method?: string; version?: string };
      switch (cmd) {
        case "rpc":
          return a.method === "snapshot" ? EMPTY : null;
        case "update_pending":
          return pending;
        case "update_ack":
          acks.push({ version: a.version!, dialogShown: dialogShown() });
          if (pending?.info.version === a.version) pending = null;
          return null;
        case "plugin:app|version":
          return "0.3.0";
        case "take_pending_opens":
          return [];
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );
}

beforeAll(() => {
  const ctx = new Proxy({}, { get: (_, k) => (k === "createLinearGradient" ? () => ({ addColorStop() {} }) : () => {}), set: () => true });
  HTMLCanvasElement.prototype.getContext = (() => ctx) as unknown as HTMLCanvasElement["getContext"];
  class Observer {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer });
  for (const proto of [Element.prototype, HTMLElement.prototype]) delete (proto as { animate?: unknown }).animate;
  window.matchMedia = ((q: string) => ({ matches: false, media: q, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} })) as unknown as typeof window.matchMedia;
});

let root: Root | null = null;
let host: HTMLElement;

beforeEach(() => {
  acks = [];
  pending = null;
  host = document.createElement("div");
  document.body.append(host);
  mockEngine();
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  host.remove();
  clearMocks();
  localStorage.clear();
});

const settle = (ms = 80) => act(() => new Promise((r) => setTimeout(r, ms)));

async function mount() {
  const [{ default: App }, { ToastProvider }] = await Promise.all([import("@/components/App"), import("@/components/Toasts")]);
  await act(async () => {
    root = createRoot(host);
    root.render(
      <ToastProvider>
        <App skipLaunch />
      </ToastProvider>,
    );
  });
  await settle(150);
}

describe("update announcements", () => {
  it("replays one sent before the window was listening, and acknowledges it only once shown", async () => {
    pending = { info: INFO, manual: false };
    await mount();
    expect(dialogShown(), "the missed update is shown").toBe(true);
    expect(acks).toEqual([{ version: "9.9.9", dialogShown: true }]);
  });

  it("acknowledges an announced update after its dialog has rendered", async () => {
    await mount();
    await act(async () => emit("trav://update-available", { info: INFO, manual: false }));
    await settle();
    expect(dialogShown()).toBe(true);
    expect(acks.length).toBeGreaterThan(0);
    expect(acks.every((a) => a.dialogShown), "never acknowledged before the dialog rendered").toBe(true);
  });

  it("answers for a version the user put off without showing it again", async () => {
    await mount();
    await act(async () => emit("trav://update-available", { info: INFO, manual: false }));
    await settle();
    const later = [...document.querySelectorAll("button")].find((b) => b.textContent === "Later");
    await act(async () => later!.click());
    await settle(400);
    acks = [];
    await act(async () => emit("trav://update-available", { info: INFO, manual: false }));
    await settle();
    expect(dialogShown()).toBe(false);
    expect(acks).toEqual([{ version: "9.9.9", dialogShown: false }]);
  });
});
