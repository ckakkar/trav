// @vitest-environment happy-dom
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { CreateDialog } from "@/components/CreateDialog";
import { Recover } from "@/components/Recover";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const CREATED = { infoHash: "a".repeat(40), out: "/Users/you/Desktop/share.torrent", name: "film.bin", totalSize: 100_000_000, pieceLength: 131072, numPieces: 763 };
let root: Root | null = null;
let host: HTMLElement;
let commands: { cmd: string; args: Record<string, unknown> }[];
let reported: string[];
let chosenPath: string;
let savePath: string | null;
let failure: string | null;
let delayed: boolean;
let pending: { resolve(value: typeof CREATED): void; reject(error: Error): void } | null;
const onCreated = vi.fn();
const onClose = vi.fn();

beforeAll(() => {
  for (const proto of [Element.prototype, HTMLElement.prototype]) delete (proto as { animate?: unknown }).animate;
  window.matchMedia = ((media: string) => ({ matches: false, media, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} })) as unknown as typeof window.matchMedia;
});

beforeEach(() => {
  commands = []; reported = []; chosenPath = "/Users/you/Movies/film.bin";
  savePath = CREATED.out; failure = null; delayed = false; pending = null;
  onCreated.mockClear(); onClose.mockClear();
  host = document.createElement("div"); document.body.append(host);
  mockWindows("main");
  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    commands.push({ cmd, args });
    if (cmd === "plugin:dialog|open") return chosenPath;
    if (cmd === "plugin:dialog|save") return savePath;
    if (cmd === "ui_error") { reported.push(String(args.message)); return null; }
    if (cmd === "rpc") {
      if (args.method === "createTorrent") {
        if (failure) throw new Error(failure);
        if (delayed) return new Promise<typeof CREATED>((resolve, reject) => { pending = { resolve, reject }; });
        return CREATED;
      }
      if (args.method === "cancelCreate") { pending?.reject(new Error("Torrent creation cancelled")); pending = null; return true; }
    }
    return null;
  }, { shouldMockEvents: true });
});

afterEach(async () => {
  await act(async () => { pending?.reject(new Error("Torrent creation cancelled")); pending = null; root?.unmount(); });
  root = null; host.remove(); clearMocks();
});

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 30)));
const button = (text: string) => [...host.querySelectorAll<HTMLButtonElement>("button")].find((element) => element.textContent?.includes(text));
async function click(element: HTMLElement | undefined | null) {
  expect(element).toBeTruthy();
  await act(async () => element!.click());
  await settle();
}
async function change(selector: string, value: string) {
  const element = host.querySelector<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>(selector)!;
  await act(async () => {
    const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : element instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
}
async function mount(theme = "ink") {
  document.documentElement.dataset.theme = theme;
  await act(async () => {
    root = createRoot(host);
    root.render(<Recover>{() => <CreateDialog open onClose={onClose} onCreated={onCreated} />}</Recover>);
  });
  await settle();
}
function healthy() {
  expect(reported).toEqual([]);
  expect(host.querySelector(".recover")).toBeNull();
  expect(host.querySelector('[role="dialog"]')?.getAttribute("aria-label")).toBe("Create torrent");
}

for (const theme of ["ink", "paper", "phosphor", "pinstripe", "bevel", "bitmap", "hologram"]) {
  describe(`Create torrent in ${theme}`, () => {
    it("renders the fields and completes a native-picker/mockIPC create flow", async () => {
      await mount(theme);
      expect(host.textContent).toContain("Trackers");
      expect(host.textContent).toContain("Web seeds");
      expect(button("Create torrent")?.disabled).toBe(true);
      expect(host.querySelectorAll('[role="switch"]')).toHaveLength(2);
      expect(host.querySelector<HTMLInputElement>('[role="switch"]:checked')).not.toBeNull();
      await click(button("Choose file"));
      await change("#create-trackers", "https://tracker.example/announce\nudp://backup.example:1337/announce\n\nhttps://last.example/announce");
      await change("#create-web-seeds", "https://mirror.example/files/");
      await change("#create-source", "private-tracker");
      await click(button("Create torrent"));
      expect(host.textContent).toContain("Torrent created");
      expect(host.textContent).toContain(CREATED.out);
      expect(onCreated).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ infoHash: CREATED.infoHash, out: CREATED.out, name: CREATED.name }), true, chosenPath);
      const creation = commands.find(({ cmd, args }) => cmd === "rpc" && args.method === "createTorrent")!;
      expect(creation.args.params).toMatchObject({ path: chosenPath, out: CREATED.out, trackers: [["https://tracker.example/announce", "udp://backup.example:1337/announce"], ["https://last.example/announce"]], webSeeds: ["https://mirror.example/files/"], source: "private-tracker", pieceLength: null });
      const save = commands.find(({ cmd }) => cmd === "plugin:dialog|save")!;
      expect(save.args.options).toMatchObject({ defaultPath: "film.bin.torrent", filters: [{ name: "Torrent", extensions: ["torrent"] }] });
      await click(button("Show in Finder"));
      expect(commands.some(({ cmd }) => cmd.startsWith("plugin:opener|"))).toBe(true);
      await click(button("Done")); expect(onClose).toHaveBeenCalledOnce();
      healthy();
    });
  });
}

it("picks a folder and passes the optional seeding preference", async () => {
  chosenPath = "/Users/you/Shared";
  await mount(); await click(button("Choose folder"));
  const open = commands.find(({ cmd }) => cmd === "plugin:dialog|open")!;
  expect(open.args.options).toMatchObject({ directory: true, multiple: false });
  await click(host.querySelectorAll<HTMLInputElement>('[role="switch"]')[1]);
  await click(button("Create torrent"));
  expect(onCreated).toHaveBeenCalledWith(expect.anything(), false, chosenPath);
  healthy();
});

it("uses keyed progress, displays MB/s, and cancels the active job", async () => {
  delayed = true;
  await mount(); await click(button("Choose file")); await click(button("Create torrent"));
  const creation = commands.find(({ cmd, args }) => cmd === "rpc" && args.method === "createTorrent")!;
  const id = (creation.args.params as { id: string }).id;
  await act(async () => { await emit("trav://event", { type: "createProgress", id: "another-job", hashed: 99, total: 100 }); });
  expect(host.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow")).toBe("0");
  await act(async () => { await emit("trav://event", { type: "createProgress", id, hashed: 50_000_000, total: 100_000_000 }); });
  expect(host.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow")).toBe("50");
  expect(host.textContent).toContain("MB/s");
  await click(button("Cancel"));
  expect(commands.find(({ cmd, args }) => cmd === "rpc" && args.method === "cancelCreate")?.args.params).toEqual({ id });
  expect(host.textContent).toContain("Creation cancelled");
  expect(onCreated).not.toHaveBeenCalled();
  healthy();
});

it("shows backend errors in the existing error callout", async () => {
  failure = "Choose a file or folder with something in it";
  await mount(); await click(button("Choose file")); await click(button("Create torrent"));
  expect(host.querySelector(".callout.err")?.textContent).toBe(failure);
  expect(onCreated).not.toHaveBeenCalled();
  healthy();
});

it("returns to editing when the native save dialog is cancelled", async () => {
  savePath = null;
  await mount(); await click(button("Choose file")); await click(button("Create torrent"));
  expect(commands.some(({ args }) => args.method === "createTorrent")).toBe(false);
  expect(button("Create torrent")?.disabled).toBe(false);
  expect(onCreated).not.toHaveBeenCalled();
  healthy();
});
