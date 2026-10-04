"use client";

import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as RMouseEvent } from "react";
import {
  Activity,
  ArrowDownToLine,
  ArrowUpFromLine,
  CheckCircle2,
  Copy,
  FileDown,
  FolderOpen,
  Layers,
  ListOrdered,
  Magnet,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Search,
  Settings2,
  ShieldCheck,
  SunMoon,
  Trash2,
  TriangleAlert,
  ChevronsUp,
  ChevronsDown,
  X,
} from "lucide-react";

import type { EngineEvent, TorrentSummary } from "@/lib/types";
import { useEngine } from "@/lib/engine";
import {
  AuthError,
  call,
  copyText,
  fileToBase64,
  isDesktop,
  isMac,
  onEngineEvent,
  onNativeDrop,
  onOpenRequest,
  pickTorrentFiles,
  reveal,
  setToken,
  takePendingOpens,
} from "@/lib/rpc";
import { bytes, looksLikeMagnet } from "@/lib/format";
import { load, save } from "@/lib/storage";
import { Sidebar, type FilterDef } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { TorrentTable, sortTorrents, type Sort, type SortKey } from "./TorrentTable";
import { DetailPanel, type DetailTab } from "./DetailPanel";
import { AddDialog, type AddItem } from "./AddDialog";
import { SettingsDialog, type UiPrefs } from "./SettingsDialog";
import { Palette, type Command } from "./Palette";
import { ContextMenu, type MenuEntry } from "./ContextMenu";
import { Modal, ease } from "./Modal";
import { useToast } from "./Toasts";
import { applyTheme, THEMES, type Theme } from "./theme";

interface FilterSpec {
  id: string;
  label: string;
  icon: React.ReactNode;
  test: (t: TorrentSummary) => boolean;
  hideEmpty?: boolean;
}

const FILTERS: FilterSpec[] = [
  { id: "all", label: "All", icon: <Layers size={15} />, test: () => true },
  {
    id: "downloading",
    label: "Downloading",
    icon: <ArrowDownToLine size={15} />,
    test: (t) => !(t.hasMetadata && t.progress >= 1) && !["paused", "error", "finished"].includes(t.status),
  },
  { id: "seeding", label: "Seeding", icon: <ArrowUpFromLine size={15} />, test: (t) => t.status === "seeding" },
  { id: "completed", label: "Completed", icon: <CheckCircle2 size={15} />, test: (t) => t.hasMetadata && t.progress >= 1 },
  { id: "active", label: "Active", icon: <Activity size={15} />, test: (t) => t.downloadRate + t.uploadRate > 0 },
  { id: "paused", label: "Paused", icon: <Pause size={15} />, test: (t) => t.status === "paused" || t.status === "finished" },
  { id: "error", label: "Errors", icon: <TriangleAlert size={15} />, test: (t) => t.status === "error", hideEmpty: true },
];

const DEFAULT_PREFS: UiPrefs = { theme: "ink", notify: true, confirmRemove: true, scanlines: true };
const isPausedLike = (t: TorrentSummary) => t.status === "paused" || t.status === "finished" || t.status === "error";

export default function App() {
  const toast = useToast();
  const [prefs, setPrefs] = useState<UiPrefs>(() => ({ ...DEFAULT_PREFS, ...load<Partial<UiPrefs>>("prefs", {}) }));
  const [filter, setFilter] = useState<string>(() => load("filter", "all"));
  const [sort, setSort] = useState<Sort>(() => load("sort", { key: "queue", dir: 1 } as Sort));
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [focus, setFocus] = useState<string | null>(null);
  const anchor = useRef<string | null>(null);
  // Hashes we just added/jumped to: not in the snapshot yet, so don't prune them.
  const fresh = useRef(new Map<string, number>());
  const [tab, setTab] = useState<DetailTab>(() => load("tab", "overview"));
  const [panelH, setPanelH] = useState<number>(() => load("panelH", 320));
  const [collapsed, setCollapsed] = useState<boolean>(() => load("collapsed", false));
  const [addItems, setAddItems] = useState<AddItem[]>([]);
  const [addOpen, setAddOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [removeReq, setRemoveReq] = useState<{ hashes: string[]; deleteFiles: boolean } | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; hash: string } | null>(null);
  const [dropHover, setDropHover] = useState(false);
  const [mac, setMac] = useState(false);
  const [desktop, setDesktop] = useState(false);
  const [tokenInput, setTokenInput] = useState("");
  const fileInput = useRef<HTMLInputElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);

  const { snapshot, details, link, history, refresh } = useEngine(focus);
  const [downloadDir, setDownloadDir] = useState("");
  const loadSettings = useCallback(() => {
    call<{ downloadDir: string }>("getSettings")
      .then((s) => setDownloadDir(s.downloadDir))
      .catch(() => {});
  }, []);
  useEffect(() => {
    if (link === "online") loadSettings();
  }, [link, loadSettings]);
  const torrents = useMemo(() => snapshot?.torrents ?? [], [snapshot]);
  const stats = snapshot?.stats ?? null;

  // ── Persistence & theme ───────────────────────────────────────────────────
  useEffect(() => {
    setMac(isMac());
    setDesktop(isDesktop());
  }, []);
  useEffect(() => save("prefs", prefs), [prefs]);
  useEffect(() => save("filter", filter), [filter]);
  useEffect(() => save("sort", sort), [sort]);
  useEffect(() => save("tab", tab), [tab]);
  useEffect(() => save("panelH", panelH), [panelH]);
  useEffect(() => save("collapsed", collapsed), [collapsed]);
  useEffect(() => {
    document.documentElement.dataset.scanlines = prefs.scanlines ? "on" : "off";
    if (document.documentElement.dataset.theme !== prefs.theme) applyTheme(prefs.theme);
  }, [prefs.theme, prefs.scanlines]);
  useEffect(() => {
    if (isDesktop()) import("@tauri-apps/api/core").then(({ invoke }) => invoke("set_notify", { enabled: prefs.notify }).catch(() => {}));
  }, [prefs.notify]);

  const cycleTheme = useCallback((e?: { clientX: number; clientY: number }) => {
    setPrefs((p) => {
      const i = THEMES.findIndex((t) => t.id === p.theme);
      const next = THEMES[(i + 1) % THEMES.length].id as Theme;
      applyTheme(next, e ? { x: e.clientX, y: e.clientY } : undefined);
      return { ...p, theme: next };
    });
  }, []);

  // ── Derived lists ─────────────────────────────────────────────────────────
  const filterDefs: FilterDef[] = useMemo(
    () =>
      FILTERS.map((f) => ({ id: f.id, label: f.label, icon: f.icon, count: torrents.filter(f.test).length, hide: f.hideEmpty })).filter(
        (f) => !(f.hide && f.count === 0),
      ),
    [torrents],
  );
  const activeFilter = FILTERS.find((f) => f.id === filter) ?? FILTERS[0];
  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    const list = torrents.filter((t) => activeFilter.test(t) && (!q || t.name.toLowerCase().includes(q) || t.infoHash.startsWith(q)));
    return sortTorrents(list, sort);
  }, [torrents, activeFilter, search, sort]);

  // Drop selection entries for torrents that disappeared.
  useEffect(() => {
    const now = Date.now();
    for (const [h, t] of fresh.current) if (now - t > 5000) fresh.current.delete(h);
    const live = new Set([...torrents.map((t) => t.infoHash), ...fresh.current.keys()]);
    setSelected((s) => {
      const next = new Set([...s].filter((h) => live.has(h)));
      return next.size === s.size ? s : next;
    });
    if (focus && !live.has(focus)) setFocus(null);
  }, [torrents, focus]);

  const selectedList = useMemo(() => (selected.size ? [...selected] : focus ? [focus] : []), [selected, focus]);
  const byHash = useMemo(() => new Map(torrents.map((t) => [t.infoHash, t])), [torrents]);

  // ── Actions ───────────────────────────────────────────────────────────────
  const fail = useCallback((e: unknown, title = "Something went wrong") => {
    if (e instanceof AuthError) return;
    toast(title, { body: String((e as Error)?.message ?? e), tone: "error" });
  }, [toast]);

  const act = useCallback(
    async (method: string, hashes: string[], extra?: Record<string, unknown>) => {
      if (!hashes.length) return;
      try {
        await call(method, { hashes, ...extra });
        refresh();
      } catch (e) {
        fail(e);
      }
    },
    [refresh, fail],
  );

  const toggleRun = useCallback(
    (hashes: string[]) => {
      const ts = hashes.map((h) => byHash.get(h)).filter(Boolean) as TorrentSummary[];
      if (!ts.length) return;
      const resume = ts.every(isPausedLike);
      act(resume ? "resume" : "pause", hashes);
    },
    [byHash, act],
  );

  const requestRemove = useCallback(
    (hashes: string[], deleteFiles = false) => {
      if (!hashes.length) return;
      if (prefs.confirmRemove || deleteFiles) setRemoveReq({ hashes, deleteFiles });
      else act("remove", hashes, { deleteFiles });
    },
    [prefs.confirmRemove, act],
  );

  const enqueue = useCallback((items: AddItem[]) => {
    if (!items.length) return;
    setAddItems((xs) => [...xs, ...items]);
    setAddOpen(true);
  }, []);

  const openTorrentPicker = useCallback(async () => {
    if (isDesktop()) {
      try {
        const paths = await pickTorrentFiles();
        enqueue(paths.map((path) => ({ kind: "path", path })));
      } catch (e) {
        fail(e);
      }
    } else fileInput.current?.click();
  }, [enqueue, fail]);

  const openMagnet = useCallback(() => {
    setAddItems([]);
    setAddOpen(true);
  }, []);

  const revealHash = useCallback(
    (hash: string) => {
      const t = byHash.get(hash);
      if (!t || !isDesktop()) return;
      const path = details?.summary.infoHash === hash && details.contentPath ? details.contentPath : t.savePath;
      reveal(path).catch((e) => fail(e, "Can't open folder"));
    },
    [byHash, details, fail],
  );

  const jumpTo = useCallback((hash: string) => {
    fresh.current.set(hash, Date.now());
    setFilter("all");
    setSelected(new Set([hash]));
    setFocus(hash);
    anchor.current = hash;
    requestAnimationFrame(() => document.querySelector(`[data-focused]`)?.scrollIntoView({ block: "nearest" }));
  }, []);

  // ── Selection ─────────────────────────────────────────────────────────────
  const onPointer = useCallback(
    (e: RMouseEvent, hash: string) => {
      const order = visible.map((t) => t.infoHash);
      if (e.shiftKey && anchor.current) {
        const a = order.indexOf(anchor.current);
        const b = order.indexOf(hash);
        if (a >= 0 && b >= 0) {
          const [lo, hi] = a < b ? [a, b] : [b, a];
          setSelected(new Set(order.slice(lo, hi + 1)));
          setFocus(hash);
          return;
        }
      }
      if (e.metaKey || e.ctrlKey) {
        setSelected((s) => {
          const n = new Set(s);
          if (n.has(hash)) n.delete(hash);
          else n.add(hash);
          return n;
        });
      } else {
        setSelected(new Set([hash]));
      }
      setFocus(hash);
      anchor.current = hash;
    },
    [visible],
  );

  const onMenu = useCallback((e: RMouseEvent, hash: string) => {
    e.preventDefault();
    setSelected((s) => (s.has(hash) ? s : new Set([hash])));
    setFocus(hash);
    setMenu({ x: e.clientX, y: e.clientY, hash });
  }, []);

  const onOpen = useCallback((hash: string) => (isDesktop() ? revealHash(hash) : setCollapsed(false)), [revealHash]);

  const onSort = useCallback((k: SortKey) => setSort((s) => (s.key === k ? { key: k, dir: (s.dir * -1) as 1 | -1 } : { key: k, dir: k === "name" || k === "queue" ? 1 : -1 })), []);

  // ── Engine events → toasts ────────────────────────────────────────────────
  useEffect(
    () =>
      onEngineEvent((e: EngineEvent) => {
        if (e.type === "torrentCompleted") {
          toast("Download complete", { body: e.name, tone: "ok" });
          if (!isDesktop() && prefs.notify && document.hidden && "Notification" in window && Notification.permission === "granted") {
            new Notification("Download complete", { body: e.name });
          }
        } else if (e.type === "torrentError") toast("Torrent error", { body: `${e.name}: ${e.message}`, tone: "error" });
        else if (e.type === "metadataReceived") toast("Metadata received", { body: e.name });
        else if (e.type === "torrentAdded") toast("Added", { body: e.name });
        refresh();
      }),
    [toast, refresh, prefs.notify],
  );

  // ── OS integrations: open requests, drag & drop, paste ───────────────────
  useEffect(() => {
    const pull = () =>
      takePendingOpens()
        .then((items) =>
          enqueue(
            items.map((s): AddItem => (looksLikeMagnet(s) ? { kind: "magnet", uri: s } : { kind: "path", path: s })),
          ),
        )
        .catch(() => {});
    pull();
    return onOpenRequest(pull);
  }, [enqueue]);

  useEffect(
    () =>
      onNativeDrop(setDropHover, (paths) => {
        const ts = paths.filter((p) => p.toLowerCase().endsWith(".torrent"));
        if (ts.length) enqueue(ts.map((path) => ({ kind: "path", path })));
        else toast("Only .torrent files can be dropped", { tone: "error" });
      }),
    [enqueue, toast],
  );

  useEffect(() => {
    if (isDesktop()) return;
    let depth = 0;
    const isFiles = (e: DragEvent) => !!e.dataTransfer && [...e.dataTransfer.types].some((t) => t === "Files" || t === "text/uri-list" || t === "text/plain");
    const enter = (e: DragEvent) => {
      if (!isFiles(e)) return;
      e.preventDefault();
      depth++;
      setDropHover(true);
    };
    const over = (e: DragEvent) => isFiles(e) && e.preventDefault();
    const leave = () => {
      depth = Math.max(0, depth - 1);
      if (!depth) setDropHover(false);
    };
    const drop = async (e: DragEvent) => {
      e.preventDefault();
      depth = 0;
      setDropHover(false);
      const dt = e.dataTransfer;
      if (!dt) return;
      const files = [...dt.files].filter((f) => f.name.toLowerCase().endsWith(".torrent"));
      const items: AddItem[] = await Promise.all(files.map(async (f) => ({ kind: "bytes" as const, name: f.name, b64: await fileToBase64(f) })));
      const text = dt.getData("text/uri-list") || dt.getData("text/plain");
      for (const line of text.split(/\s+/)) if (looksLikeMagnet(line)) items.push({ kind: "magnet", uri: line });
      if (items.length) enqueue(items);
    };
    window.addEventListener("dragenter", enter);
    window.addEventListener("dragover", over);
    window.addEventListener("dragleave", leave);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragenter", enter);
      window.removeEventListener("dragover", over);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("drop", drop);
    };
  }, [enqueue]);

  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const el = e.target as HTMLElement;
      if (el.closest("input, textarea, [contenteditable]")) return;
      const text = e.clipboardData?.getData("text") ?? "";
      const links = text.split(/\s+/).filter(looksLikeMagnet);
      if (links.length) {
        e.preventDefault();
        enqueue(links.map((uri) => ({ kind: "magnet", uri })));
      }
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [enqueue]);

  // ── Keyboard ──────────────────────────────────────────────────────────────
  const anyModal = addOpen || settingsOpen || paletteOpen || !!removeReq || !!menu;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      const typing = (e.target as HTMLElement).closest("input, textarea, select");
      if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen((o) => !o);
        return;
      }
      if (anyModal) return;
      if (mod && e.key.toLowerCase() === "o") {
        e.preventDefault();
        openTorrentPicker();
      } else if (mod && e.key === ",") {
        e.preventDefault();
        setSettingsOpen(true);
      } else if (mod && e.shiftKey && e.key.toLowerCase() === "l") {
        e.preventDefault();
        cycleTheme();
      } else if (typing) {
        if (e.key === "Escape") (e.target as HTMLElement).blur();
      } else if (mod && e.key.toLowerCase() === "a") {
        e.preventDefault();
        setSelected(new Set(visible.map((t) => t.infoHash)));
      } else if (e.key === "/") {
        e.preventDefault();
        searchInput.current?.focus();
      } else if (e.key === "Escape") {
        setSelected(new Set());
        setSearch("");
      } else if (["ArrowDown", "ArrowUp", "j", "k"].includes(e.key)) {
        e.preventDefault();
        if (!visible.length) return;
        const order = visible.map((t) => t.infoHash);
        const i = focus ? order.indexOf(focus) : -1;
        const dir = e.key === "ArrowDown" || e.key === "j" ? 1 : -1;
        const next = order[Math.max(0, Math.min(order.length - 1, i < 0 ? 0 : i + dir))];
        setFocus(next);
        if (e.shiftKey) setSelected((s) => new Set([...s, next]));
        else {
          setSelected(new Set([next]));
          anchor.current = next;
        }
        requestAnimationFrame(() => document.querySelector("[data-focused]")?.scrollIntoView({ block: "nearest" }));
      } else if (e.key === " ") {
        e.preventDefault();
        toggleRun(selectedList);
      } else if (e.key === "Delete" || (e.key === "Backspace" && mod)) {
        e.preventDefault();
        requestRemove(selectedList, e.shiftKey);
      } else if (e.key === "Enter" && focus) {
        onOpen(focus);
      } else if (/^[1-5]$/.test(e.key)) {
        setTab((["overview", "files", "peers", "trackers", "speed"] as DetailTab[])[Number(e.key) - 1]);
        setCollapsed(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [anyModal, openTorrentPicker, cycleTheme, visible, focus, toggleRun, selectedList, requestRemove, onOpen]);

  // ── Panel resizing ────────────────────────────────────────────────────────
  const startResize = (e: React.PointerEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = collapsed ? 0 : panelH;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    document.body.dataset.resizing = "row";
    const move = (ev: PointerEvent) => {
      const h = Math.max(0, Math.min(window.innerHeight - 220, startH + (startY - ev.clientY)));
      if (h < 120) setCollapsed(true);
      else {
        setCollapsed(false);
        setPanelH(h);
      }
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      delete document.body.dataset.resizing;
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };

  // ── Context menu & palette entries ────────────────────────────────────────
  const menuItems = (hash: string): MenuEntry[] => {
    const hashes = selected.has(hash) ? [...selected] : [hash];
    const t = byHash.get(hash);
    const many = hashes.length > 1;
    const paused = hashes.every((h) => byHash.get(h) && isPausedLike(byHash.get(h)!));
    return [
      paused
        ? { label: many ? `Resume ${hashes.length}` : "Resume", icon: <Play size={14} />, hint: "Space", onSelect: () => act("resume", hashes) }
        : { label: many ? `Pause ${hashes.length}` : "Pause", icon: <Pause size={14} />, hint: "Space", onSelect: () => act("pause", hashes) },
      { label: "Force recheck", icon: <ShieldCheck size={14} />, onSelect: () => act("recheck", hashes) },
      { label: "Reannounce", icon: <RefreshCw size={14} />, onSelect: () => act("reannounce", hashes) },
      "sep",
      {
        label: t?.sequential ? "Disable sequential" : "Sequential download",
        icon: <ListOrdered size={14} />,
        disabled: many,
        onSelect: () => call("setSequential", { hash, enabled: !t?.sequential }).then(refresh).catch(fail),
      },
      { label: "Move to top of queue", icon: <ChevronsUp size={14} />, disabled: many, onSelect: () => call("setQueuePosition", { hash, position: 0 }).then(refresh).catch(fail) },
      { label: "Move to bottom", icon: <ChevronsDown size={14} />, disabled: many, onSelect: () => call("setQueuePosition", { hash, position: 1e6 }).then(refresh).catch(fail) },
      "sep",
      ...(isDesktop() ? [{ label: "Show in folder", icon: <FolderOpen size={14} />, hint: "↵", disabled: many, onSelect: () => revealHash(hash) }] : []),
      {
        label: "Copy magnet link",
        icon: <Copy size={14} />,
        disabled: many,
        onSelect: async () => {
          const d = await call<{ magnet: string } | null>("details", { hash });
          if (d && (await copyText(d.magnet))) toast("Magnet link copied");
        },
      },
      { label: "Copy info-hash", icon: <Copy size={14} />, disabled: many, onSelect: () => copyText(hash).then(() => toast("Info-hash copied")) },
      "sep",
      { label: many ? `Remove ${hashes.length}` : "Remove", icon: <X size={14} />, hint: "Del", onSelect: () => requestRemove(hashes, false) },
      { label: "Remove and delete data", icon: <Trash2 size={14} />, hint: "⇧Del", danger: true, onSelect: () => requestRemove(hashes, true) },
    ];
  };

  const commands: Command[] = useMemo(
    () => [
      { id: "add", label: "Add torrent file…", hint: "⌘O", icon: <FileDown size={14} />, run: openTorrentPicker },
      { id: "magnet", label: "Add magnet link…", hint: "⌘V", icon: <Magnet size={14} />, run: openMagnet },
      { id: "resume-all", label: "Resume all", icon: <Play size={14} />, run: () => call("resumeAll").then(refresh).catch(fail) },
      { id: "pause-all", label: "Pause all", icon: <Pause size={14} />, run: () => call("pauseAll").then(refresh).catch(fail) },
      { id: "settings", label: "Settings", hint: "⌘,", icon: <Settings2 size={14} />, run: () => setSettingsOpen(true) },
      { id: "theme", label: "Cycle theme", hint: "⌘⇧L", icon: <SunMoon size={14} />, run: () => cycleTheme() },
      ...THEMES.map((t) => ({ id: `theme-${t.id}`, label: `Theme: ${t.label}`, icon: <SunMoon size={14} />, run: () => setPrefs((p) => ({ ...p, theme: t.id })) })),
      ...FILTERS.map((f) => ({ id: `filter-${f.id}`, label: `Show ${f.label.toLowerCase()}`, icon: f.icon, run: () => setFilter(f.id) })),
    ],
    [openTorrentPicker, openMagnet, refresh, fail, cycleTheme],
  );

  // ── Render ────────────────────────────────────────────────────────────────
  const totalSize = visible.reduce((a, t) => a + t.size, 0);
  const selTs = selectedList.map((h) => byHash.get(h)).filter(Boolean) as TorrentSummary[];
  const selPaused = selTs.length > 0 && selTs.every(isPausedLike);

  const empty = (
    <div className="empty">
      <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.6, ease }}>
        {torrents.length === 0 ? (
          <>
            <div className="eyebrow">[00] · the swarm is quiet</div>
            <h2 className="display empty-title">Nothing downloading.</h2>
            <p className="empty-sub">Drop a .torrent anywhere, paste a magnet link, or pick a file.</p>
            <div className="empty-actions">
              <button className="btn primary" onClick={openTorrentPicker}>
                <FileDown size={14} /> Open .torrent <kbd>⌘O</kbd>
              </button>
              <button className="btn" onClick={openMagnet}>
                <Magnet size={14} /> Paste magnet <kbd>⌘V</kbd>
              </button>
            </div>
          </>
        ) : (
          <>
            <h2 className="display empty-title sm">No matches.</h2>
            <p className="empty-sub">
              Nothing in <em>{activeFilter.label.toLowerCase()}</em>
              {search && (
                <>
                  {" "}
                  for “{search}”
                </>
              )}
              .
            </p>
          </>
        )}
      </motion.div>
    </div>
  );

  return (
    <div className="app" data-mac={mac || undefined} data-desktop={desktop || undefined}>
      <Sidebar
        filters={filterDefs}
        active={filter}
        onFilter={setFilter}
        stats={stats}
        history={history}
        onSettings={() => setSettingsOpen(true)}
        onTheme={(e) => cycleTheme(e)}
        onPalette={() => setPaletteOpen(true)}
        mac={mac && desktop}
      />

      <main className="main" style={{ ["--panel-h" as string]: collapsed || (snapshot && torrents.length === 0) ? "0px" : `${panelH}px` }}>
        <header className="toolbar" data-tauri-drag-region>
          <div className="tb-title" data-tauri-drag-region>
            <h1 className="display" data-tauri-drag-region>
              {activeFilter.label}
            </h1>
            <span className="mono faint" data-tauri-drag-region>
              {visible.length} · {bytes(totalSize)}
            </span>
          </div>
          <label className="tb-search">
            <Search size={14} />
            <input ref={searchInput} placeholder="Filter" value={search} onChange={(e) => setSearch(e.target.value)} spellCheck={false} />
            {search ? (
              <button className="icon-btn xs" onClick={() => setSearch("")} aria-label="Clear filter">
                <X size={12} />
              </button>
            ) : (
              <kbd>/</kbd>
            )}
          </label>
          <div className="tb-actions">
            <AnimatePresence>
              {selTs.length > 0 && (
                <motion.div className="tb-sel" initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={{ duration: 0.2 }}>
                  <button className="icon-btn" title={selPaused ? "Resume (Space)" : "Pause (Space)"} aria-label={selPaused ? "Resume" : "Pause"} onClick={() => toggleRun(selectedList)}>
                    {selPaused ? <Play size={15} /> : <Pause size={15} />}
                  </button>
                  <button className="icon-btn" title="Remove (Del)" aria-label="Remove" onClick={() => requestRemove(selectedList)}>
                    <Trash2 size={15} />
                  </button>
                  <span className="tb-divider" />
                </motion.div>
              )}
            </AnimatePresence>
            <button className="btn" onClick={openMagnet} title="Add magnet link (⌘V)">
              <Magnet size={14} /> Magnet
            </button>
            <button className="btn primary" onClick={openTorrentPicker} title="Add torrent file (⌘O)">
              <Plus size={14} /> Add
            </button>
          </div>
        </header>

        <AnimatePresence>
          {link === "offline" && (
            <motion.div className="banner" initial={{ height: 0, opacity: 0 }} animate={{ height: "auto", opacity: 1 }} exit={{ height: 0, opacity: 0 }}>
              <span className="sdot err" /> Can’t reach the Trav engine — retrying…
            </motion.div>
          )}
        </AnimatePresence>

        <div className="list-region" onMouseDown={(e) => e.target === e.currentTarget && setSelected(new Set())}>
          <TorrentTable
            torrents={visible}
            sort={sort}
            onSort={onSort}
            selected={selected}
            focus={focus}
            onPointer={onPointer}
            onMenu={onMenu}
            onOpen={onOpen}
            empty={snapshot ? empty : <div className="empty mono faint">connecting to engine…</div>}
          />
        </div>

        <div className="resizer" onPointerDown={startResize} onDoubleClick={() => setCollapsed((c) => !c)} role="separator" aria-orientation="horizontal" title="Drag to resize · double-click to collapse">
          <span />
        </div>
        <div className="panel-wrap" data-collapsed={collapsed || undefined}>
          <DetailPanel
            details={details}
            tab={tab}
            onTab={(t) => {
              setTab(t);
              setCollapsed(false);
            }}
            history={history}
            selectionCount={selected.size}
            actions={{
              setPriorities: (hash, priorities) => call("setFilePriorities", { hash, priorities }).then(refresh).catch(fail),
              addPeers: (hash, peers) => call("addPeers", { hash, peers }).then(() => toast("Peers queued")).catch(fail),
              reannounce: (hash) => act("reannounce", [hash]).then(() => toast("Reannouncing")),
              toast,
            }}
          />
        </div>
      </main>

      <StatusBar stats={stats} link={link} onLimits={() => setSettingsOpen(true)} />

      <input
        ref={fileInput}
        type="file"
        accept=".torrent,application/x-bittorrent"
        multiple
        hidden
        onChange={async (e) => {
          const files = [...(e.target.files ?? [])];
          e.target.value = "";
          enqueue(await Promise.all(files.map(async (f) => ({ kind: "bytes" as const, name: f.name, b64: await fileToBase64(f) }))));
        }}
      />

      <AnimatePresence>
        {dropHover && (
          <motion.div className="dropzone" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.18 }}>
            <motion.div className="dropzone-inner" initial={{ scale: 0.97 }} animate={{ scale: 1 }} transition={{ duration: 0.4, ease }}>
              <span className="eyebrow">release to add</span>
              <span className="display">Drop it in the swarm.</span>
              <span className="mono faint">.torrent files and magnet links</span>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>

      <AddDialog
        open={addOpen}
        items={addItems}
        onClose={() => {
          setAddOpen(false);
          setAddItems([]);
        }}
        onConsume={(n) =>
          setAddItems((xs) => {
            const rest = xs.slice(n);
            if (!rest.length) setAddOpen(false);
            return rest;
          })
        }
        onEnqueue={enqueue}
        defaultSavePath={downloadDir}
        onAdded={(hash) => {
          refresh();
          jumpTo(hash);
        }}
        onError={(m) => toast("Couldn’t add torrent", { body: m, tone: "error" })}
      />

      <SettingsDialog
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        prefs={prefs}
        onPrefs={setPrefs}
        stats={stats}
        onSaved={() => {
          toast("Settings saved", { tone: "ok" });
          loadSettings();
          refresh();
        }}
        onError={(m) => toast("Settings", { body: m, tone: "error" })}
      />

      <Palette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={commands}
        torrents={torrents}
        onJump={jumpTo}
        onMagnet={(uri) => enqueue([{ kind: "magnet", uri }])}
      />

      <Modal
        open={!!removeReq}
        onClose={() => setRemoveReq(null)}
        label="Remove torrents"
        width={480}
        eyebrow={removeReq?.deleteFiles ? "Remove · delete data" : "Remove"}
        title={removeReq && removeReq.hashes.length > 1 ? `Remove ${removeReq.hashes.length} torrents?` : "Remove this torrent?"}
      >
        {removeReq && (
          <>
            <div className="modal-body">
              <ul className="remove-list">
                {removeReq.hashes.slice(0, 6).map((h) => (
                  <li key={h} className="ellipsis">
                    {byHash.get(h)?.name ?? h}
                  </li>
                ))}
                {removeReq.hashes.length > 6 && <li className="faint">and {removeReq.hashes.length - 6} more</li>}
              </ul>
              <label className="toggle danger-toggle">
                <input type="checkbox" role="switch" checked={removeReq.deleteFiles} onChange={(e) => setRemoveReq({ ...removeReq, deleteFiles: e.target.checked })} />
                <span className="toggle-track">
                  <span className="toggle-thumb" />
                </span>
                <span className="toggle-label">Also delete downloaded files</span>
              </label>
            </div>
            <footer className="modal-foot">
              <span className="mono faint">{removeReq.deleteFiles ? "files are deleted permanently" : "files stay on disk"}</span>
              <div className="foot-actions">
                <button className="btn ghost" onClick={() => setRemoveReq(null)}>
                  Cancel
                </button>
                <button
                  className={`btn ${removeReq.deleteFiles ? "danger" : "primary"}`}
                  data-autofocus
                  onClick={() => {
                    act("remove", removeReq.hashes, { deleteFiles: removeReq.deleteFiles });
                    setRemoveReq(null);
                  }}
                >
                  {removeReq.deleteFiles ? "Delete" : "Remove"}
                </button>
              </div>
            </footer>
          </>
        )}
      </Modal>

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.hash)} onClose={() => setMenu(null)} />}

      <AnimatePresence>
        {link === "auth" && (
          <motion.div className="gate" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
            <form
              className="gate-card"
              onSubmit={(e) => {
                e.preventDefault();
                setToken(tokenInput.trim());
                refresh();
              }}
            >
              <div className="eyebrow">trav daemon · locked</div>
              <h2 className="display">Enter access token</h2>
              <p className="mono faint">Started with --token. It is stored in this browser only.</p>
              <input className="input mono" type="password" autoFocus value={tokenInput} onChange={(e) => setTokenInput(e.target.value)} />
              <button className="btn primary" type="submit">
                Unlock
              </button>
            </form>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
