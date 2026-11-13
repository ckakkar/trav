// @vitest-environment happy-dom
//
// Renders the whole window against a mocked engine, in every theme, and
// fails on anything that throws while rendering. Typecheck and the unit tests
// passed while 0.3.48 shipped a window that crashed at launch; this would not.

import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { EngineSnapshot, TorrentDetails, TorrentPreview, TorrentSummary } from "@/lib/types";
import { torrent } from "../unit/fixtures";

// React reads this to decide whether act() is in charge of scheduling.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const THEMES = ["ink", "paper", "phosphor"] as const;

const LIBRARY: TorrentSummary[] = [
  torrent({ infoHash: "1".repeat(40), name: "ubuntu-26.04-desktop-amd64.iso", status: "downloading", progress: 0.41, downloadRate: 3_000_000, uploadRate: 90_000, eta: 900, peers: 61, seeds: 48 }),
  torrent({ infoHash: "2".repeat(40), name: "Prelinger Archives", status: "metadata", hasMetadata: false, size: 0, progress: 0 }),
  torrent({ infoHash: "3".repeat(40), name: "Blender demo files", status: "seeding", progress: 1, uploadRate: 400_000, ratio: 2.3 }),
  torrent({ infoHash: "4".repeat(40), name: "debian netinst", status: "checking", progress: 0.47 }),
  torrent({ infoHash: "5".repeat(40), name: "Sintel 1080p", status: "paused", progress: 0.62 }),
  torrent({ infoHash: "6".repeat(40), name: "enwiki dump", status: "queued", progress: 0 }),
  torrent({ infoHash: "7".repeat(40), name: "Fedora Live", status: "error", progress: 0.18, error: "disk write failed: no space left on device" }),
];

function snapshot(torrents: TorrentSummary[]): EngineSnapshot {
  return {
    seq: 1,
    torrents,
    stats: {
      downloadRate: 3_000_000, uploadRate: 490_000, sessionDownloaded: 4e9, sessionUploaded: 8e8, downloadLimit: 0,
      uploadLimit: 1_200_000, uploadAuto: true, connectedPeers: 97, dhtNodes: 318, listenPort: 51413, connectable: true,
      freeSpace: 184e9, upnp: "mapped by UPnP",
    },
  };
}

function details(t: TorrentSummary): TorrentDetails {
  return {
    summary: t, comment: "test", createdBy: "mktorrent", creationDate: 1_700_000_000, pieceLength: 4 << 20,
    numPieces: t.hasMetadata ? 16 : 0, pieces: t.hasMetadata ? "/wA=" : "", piecesInProgress: t.status === "downloading" ? "AAE=" : "",
    files: t.hasMetadata
      ? [0, 1, 2].map((i) => ({ index: i, path: `dir/file-${i}.bin`, diskPath: `/dl/dir/file-${i}.bin`, size: 100, done: 50, progress: 0.5, priority: i === 2 ? 0 : 1 }))
      : [],
    peers: [{ addr: "203.0.113.5:51413", client: "qBittorrent 5.1", flags: "DE", progress: 0.5, downloadRate: 1000, uploadRate: 10, downloaded: 1, uploaded: 1, source: "dht" }],
    trackers: [{ url: "udp://tracker.example:1337/announce", tier: 0, status: "working", message: null, peers: 50, seeds: 400, leechers: 80, nextAnnounce: 1460 }],
    magnet: `magnet:?xt=urn:btih:${t.infoHash}`, contentPath: `/dl/${t.name}`, wasted: 0, hashFails: 0, dhtEnabled: true,
  };
}

const PREVIEW: TorrentPreview = {
  infoHash: "9".repeat(40), name: "Night of the Living Dead (1968)", totalSize: 1_100_000_000, pieceLength: 2 << 20, numPieces: 525,
  files: [{ index: 0, path: "film.mkv", size: 1_000_000_000 }, { index: 1, path: "Subs/en.srt", size: 90_000 }],
  trackers: [], comment: null, createdBy: null, private: false, alreadyAdded: false,
};

/** Every error the window reports (render crashes go through `ui_error`). */
let reported: string[] = [];

function mockEngine(torrents: TorrentSummary[], opens: string[] = []) {
  let pending = [...opens];
  mockWindows("main");
  mockIPC((cmd, args) => {
    const a = (args ?? {}) as { method?: string; params?: { hash?: string }; kind?: string; message?: string };
    switch (cmd) {
      case "rpc":
        switch (a.method) {
          case "snapshot":
            return snapshot(torrents);
          case "details":
            return details(torrents.find((t) => t.infoHash === a.params?.hash) ?? torrents[0]);
          case "inspect":
            return PREVIEW;
          case "getSettings":
            return {
              downloadDir: "/dl", listenPort: 51413, maxPeersPerTorrent: 150, maxPeersGlobal: 600, maxActiveDownloads: 5, downloadLimit: 0,
              uploadLimit: 0, uploadSlots: 8, enableDht: true, enablePex: true, enableLsd: true, enableUpnp: true, enableUtp: true,
              seedRatioLimit: 0, autoUploadLimit: true, encryption: "prefer", publicTrackers: true, version: 3,
            };
          default:
            return null;
        }
      case "ui_error":
        reported.push(`${a.kind}: ${a.message}`);
        return null;
      case "take_pending_opens": {
        const out = pending;
        pending = [];
        return out;
      }
      case "plugin:event|listen":
        return 1;
      case "plugin:app|version":
        return "0.0.0";
      default:
        return null;
    }
  });
}

/** Just enough browser for the window: canvas, observers, media queries. */
beforeAll(() => {
  const ctx = new Proxy({}, { get: (_, k) => (k === "createLinearGradient" ? () => ({ addColorStop() {} }) : () => {}), set: () => true });
  HTMLCanvasElement.prototype.getContext = (() => ctx) as unknown as HTMLCanvasElement["getContext"];
  class Observer {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer });
  // happy-dom's Web Animations reject `finished` on cancel with nobody
  // listening; without them motion animates in JavaScript, as it can anywhere.
  for (const proto of [Element.prototype, HTMLElement.prototype]) delete (proto as { animate?: unknown }).animate;
  window.matchMedia = ((q: string) => ({ matches: false, media: q, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} })) as unknown as typeof window.matchMedia;
});

let root: Root | null = null;
let host: HTMLElement;

beforeEach(() => {
  reported = [];
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  host.remove();
  clearMocks();
  localStorage.clear();
});

const settle = (ms = 60) => act(() => new Promise((r) => setTimeout(r, ms)));

async function mount(theme: (typeof THEMES)[number]) {
  localStorage.setItem("trav.ui.prefs", JSON.stringify({ theme }));
  document.documentElement.dataset.theme = theme;
  const [{ default: App }, { ToastProvider }, { Recover }] = await Promise.all([
    import("@/components/App"),
    import("@/components/Toasts"),
    import("@/components/Recover"),
  ]);
  await act(async () => {
    root = createRoot(host);
    root.render(
      <ToastProvider>
        <Recover>{() => <App skipLaunch />}</Recover>
      </ToastProvider>,
    );
  });
  await settle(120);
}

function click(el: Element | null | undefined) {
  expect(el, "element to click").toBeTruthy();
  return act(async () => {
    el!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
    el!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

const byText = (selector: string, text: string) => [...host.querySelectorAll(selector)].find((e) => e.textContent?.includes(text));

function expectHealthy() {
  expect(reported, "errors reported by the window").toEqual([]);
  expect(host.querySelector(".recover"), "the crash-recovery screen").toBeNull();
  expect(host.querySelector(".app"), "the app shell").not.toBeNull();
}

describe.each(THEMES)("the window in %s", (theme) => {
  it("renders a full library and every detail tab", async () => {
    mockEngine(LIBRARY);
    await mount(theme);
    expect(host.querySelectorAll(".trow")).toHaveLength(LIBRARY.length);
    for (const row of host.querySelectorAll(".trow")) {
      await click(row);
      await settle();
      for (const tab of ["Overview", "Files", "Peers", "Trackers", "Speed"]) {
        await click(byText(".tab", tab));
        await settle(20);
      }
    }
    expectHealthy();
  });

  it("renders an empty library", async () => {
    mockEngine([]);
    await mount(theme);
    expect(host.querySelector(".empty")).not.toBeNull();
    expectHealthy();
  });

  it("opens settings, the palette and the magnet dialog", async () => {
    mockEngine(LIBRARY);
    await mount(theme);
    await click(host.querySelector('[aria-label="Settings"]'));
    await settle();
    for (const tab of ["Downloads", "Bandwidth", "Network", "Interface", "App"]) {
      await click(byText(".set-nav-item", tab));
      await settle(20);
    }
    await click(host.querySelector('[aria-label="Close"]'));
    await settle();
    await click(host.querySelector('[aria-label="Command palette"]'));
    await settle();
    await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
    await click(byText(".btn", "Magnet"));
    await settle();
    expectHealthy();
  });

  it("is navigable by VoiceOver and the keyboard", async () => {
    mockEngine(LIBRARY);
    await mount(theme);
    const grid = host.querySelector('[role="grid"]')!;
    expect(grid.getAttribute("aria-rowcount")).toBe(String(LIBRARY.length + 1));
    const rows = host.querySelectorAll('[role="row"][id^="row-"]');
    expect(rows).toHaveLength(LIBRARY.length);
    expect(rows[0].querySelectorAll('[role="gridcell"]').length).toBeGreaterThan(5);
    expect(rows[0].querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow")).toBe("41");
    await click(rows[0]);
    await settle();
    expect(grid.getAttribute("aria-activedescendant")).toBe(rows[0].id);
    // Tabs point at their panel, and the arrow keys move between them.
    const tab = host.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')!;
    expect(document.getElementById(tab.getAttribute("aria-controls")!)?.getAttribute("role")).toBe("tabpanel");
    tab.focus();
    await act(async () => tab.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true })));
    await settle();
    expect(host.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toContain("Files");
    // Tab stays inside an open dialog.
    await click(host.querySelector('[aria-label="Settings"]'));
    await settle();
    const dialog = document.querySelector('[role="dialog"]')!;
    for (let i = 0; i < 40; i++) {
      await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", shiftKey: i % 3 === 0, bubbles: true })));
      expect(dialog.contains(document.activeElement), `focus after ${i + 1} Tab presses`).toBe(true);
    }
    expectHealthy();
  });

  it("shows a .torrent waiting to be added, with its files", async () => {
    mockEngine(LIBRARY, ["/Users/you/Downloads/night.torrent"]);
    await mount(theme);
    await settle(200);
    expect(document.body.textContent).toContain("Night of the Living Dead (1968)");
    expect(document.body.textContent).toContain("Subs/");
    expectHealthy();
  });
});
