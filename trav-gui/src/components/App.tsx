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
  appVersion,
  call,
  checkUpdate,
  copyText,
  onEngineEvent,
  onMenu as onMenuBar,
  onNativeDrop,
  onOpenRequest,
  pickTorrentFiles,
  reveal,
  setKeepAwake,
  setMenuState,
  setNotify,
  setTraySpeed,
  takePendingOpens,
  type UpdateInfo,
} from "@/lib/rpc";
import { bytes, looksLikeMagnet } from "@/lib/format";
import { FILTER_TESTS, isPausedLike, visibleTorrents, type FilterId } from "@/lib/torrents";
import { load, save } from "@/lib/storage";
import { Sidebar, type FilterDef } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { Launch } from "./Launch";
import { Mark } from "./Mark";
import { TorrentTable, type Sort, type SortKey } from "./TorrentTable";
import { DetailPanel, type DetailTab } from "./DetailPanel";
import { AddDialog, type AddItem } from "./AddDialog";
import { SettingsDialog, type UiPrefs } from "./SettingsDialog";
import { Palette, type Command } from "./Palette";
import { ContextMenu, type MenuEntry } from "./ContextMenu";
import { Modal, ease } from "./Modal";
import { useToast } from "./Toasts";
import { UpdateDialog } from "./UpdateDialog";
import { applyTheme, THEMES, type Theme } from "./theme";

interface FilterSpec {
  id: string;
  label: string;
  icon: React.ReactNode;
  test: (t: TorrentSummary) => boolean;
  hideEmpty?: boolean;
}

const FILTERS: FilterSpec[] = [
  { id: "all", label: "All", icon: <Layers size={15} />, test: FILTER_TESTS.all },
  {
    id: "downloading",
    label: "Downloading",
    icon: <ArrowDownToLine size={15} />,
    test: FILTER_TESTS.downloading,
  },
  { id: "seeding", label: "Seeding", icon: <ArrowUpFromLine size={15} />, test: FILTER_TESTS.seeding },
  { id: "completed", label: "Completed", icon: <CheckCircle2 size={15} />, test: FILTER_TESTS.completed },
  { id: "active", label: "Active", icon: <Activity size={15} />, test: FILTER_TESTS.active },
  { id: "paused", label: "Paused", icon: <Pause size={15} />, test: FILTER_TESTS.paused },
  { id: "error", label: "Errors", icon: <TriangleAlert size={15} />, test: FILTER_TESTS.error, hideEmpty: true },
];

const DEFAULT_PREFS: UiPrefs = { theme: "ink", notify: true, confirmRemove: true, scanlines: true, autoUpdate: true, keepAwake: true, menuBarSpeed: true };

/** ⌘-shortcuts the Mac app's menu bar owns (handled there, not here). */
const MENU_KEYS = new Set(["k", "o", ",", "u", "r", "i", ".", "/", "f", "1", "2", "3", "4", "5", "6", "7"]);

/** The launch sequence plays once per app start, when the window is on screen to see it. */
function launchStage(): "launch" | "done" {
  if (typeof window === "undefined") return "done";
  const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  return reduced || document.visibilityState === "hidden" ? "done" : "launch";
}

export default function App({ skipLaunch = false }: { skipLaunch?: boolean }) {
  const toast = useToast();
  // launch → (overlay dissolving, app animating in) reveal → done
  const [boot, setBoot] = useState<"launch" | "reveal" | "done">(() => (skipLaunch ? "done" : launchStage()));
  const [inactive, setInactive] = useState(false);
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
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  /** Versions the user said "Later" to: automatic checks don't ask again this session. */
  const deferred = useRef(new Set<string>());
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
    setNotify(prefs.notify);
  }, [prefs.notify]);
  useEffect(() => setKeepAwake(prefs.keepAwake), [prefs.keepAwake]);
  useEffect(() => setTraySpeed(prefs.menuBarSpeed), [prefs.menuBarSpeed]);

  // Decorative motion rests while the window is in the background or hidden (saves energy).
  useEffect(() => {
    const update = () => setInactive(document.visibilityState === "hidden" || !document.hasFocus());
    update();
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    document.addEventListener("visibilitychange", update);
    return () => {
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);
  useEffect(() => {
    appVersion().then(setVersion).catch(() => {});
  }, []);

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
  const visible = useMemo(() => visibleTorrents(torrents, activeFilter.id as FilterId, search, sort), [torrents, activeFilter, search, sort]);

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
    toast(title, { body: String((e as Error)?.message ?? e), tone: "error" });
  }, [toast]);

  const runUpdateCheck = useCallback(
    async (manual: boolean) => {
      try {
        const u = await checkUpdate();
        if (u && (manual || !deferred.current.has(u.version))) setUpdate(u);
        else if (!u && manual) toast("You’re up to date", { body: version ? `Trav ${version} is the latest version.` : undefined, tone: "ok" });
      } catch (e) {
        if (manual) toast("Couldn’t check for updates", { body: String((e as Error)?.message ?? e), tone: "error" });
      }
    },
    [toast, version],
  );
  useEffect(() => {
    // Development builds aren't released; only a manual check makes sense there.
    if (!prefs.autoUpdate || process.env.NODE_ENV === "development") return;
    const first = setTimeout(() => runUpdateCheck(false), 15_000);
    const every = setInterval(() => runUpdateCheck(false), 3600_000);
    return () => {
      clearTimeout(first);
      clearInterval(every);
    };
  }, [prefs.autoUpdate, runUpdateCheck]);

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
    try {
      const paths = await pickTorrentFiles();
      enqueue(paths.map((path) => ({ kind: "path", path })));
    } catch (e) {
      fail(e);
    }
  }, [enqueue, fail]);

  const openMagnet = useCallback(() => {
    setAddItems([]);
    setAddOpen(true);
  }, []);

  const revealHash = useCallback(
    (hash: string) => {
      const t = byHash.get(hash);
      if (!t) return;
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

  const onOpen = useCallback((hash: string) => revealHash(hash), [revealHash]);

  const onSort = useCallback((k: SortKey) => setSort((s) => (s.key === k ? { key: k, dir: (s.dir * -1) as 1 | -1 } : { key: k, dir: k === "name" || k === "queue" ? 1 : -1 })), []);

  // ── Engine events → toasts ────────────────────────────────────────────────
  useEffect(
    () =>
      onEngineEvent((e: EngineEvent) => {
        if (e.type === "torrentCompleted") {
          toast("Download complete", { body: e.name, tone: "ok" });
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
    const onPaste = (e: ClipboardEvent) => {
      const el = e.target instanceof Element ? e.target : null;
      if (el?.closest("input, textarea, [contenteditable]")) return;
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
      const typing = e.target instanceof Element && e.target.closest("input, textarea, select");
      if (mod && (MENU_KEYS.has(e.key.toLowerCase()) || e.key === "Backspace" || (e.shiftKey && e.key.toLowerCase() === "l"))) return;
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

  // ── Mac menu bar ──────────────────────────────────────────────────────────
  // Keep the Transfers menu in step with the selection.
  const menuSel = useMemo(() => {
    const ts = selectedList.map((h) => byHash.get(h)).filter(Boolean) as TorrentSummary[];
    return { count: ts.length, allPaused: ts.length > 0 && ts.every(isPausedLike), sequential: ts.length === 1 && ts[0].sequential };
  }, [selectedList, byHash]);
  const { count: menuCount, allPaused: menuPaused, sequential: menuSequential } = menuSel;
  useEffect(() => setMenuState({ count: menuCount, allPaused: menuPaused, sequential: menuSequential }), [menuCount, menuPaused, menuSequential]);

  const onMenuItem = useRef<(id: string) => void>(() => {});
  onMenuItem.current = (id: string) => {
    const editing = !!(document.activeElement as Element | null)?.closest?.("input, textarea, select, [contenteditable]");
    const one = selectedList.length === 1 ? selectedList[0] : null;
    switch (id) {
      case "open-torrent":
        return void openTorrentPicker();
      case "add-magnet":
        return openMagnet();
      case "settings":
        return setSettingsOpen(true);
      case "palette":
        return setPaletteOpen((o) => !o);
      case "find":
        return searchInput.current?.focus();
      case "toggle-details":
        return setCollapsed((c) => !c);
      case "theme":
        return cycleTheme();
      case "check-updates":
        return void runUpdateCheck(true);
      case "pause-all":
        return void call("pauseAll").then(refresh).catch(fail);
      case "resume-all":
        return void call("resumeAll").then(refresh).catch(fail);
    }
    if (id.startsWith("filter:")) return setFilter(id.slice("filter:".length));
    if (id.startsWith("select:")) return jumpTo(id.slice("select:".length));
    if (anyModal || !selectedList.length) return;
    switch (id) {
      case "toggle-run":
        return toggleRun(selectedList);
      case "reveal":
        return selectedList.slice(0, 10).forEach(revealHash);
      case "recheck":
        return void act("recheck", selectedList);
      case "reannounce":
        return void act("reannounce", selectedList);
      case "remove":
      case "remove-data":
        // ⌘⌫ while typing edits the text field, not the transfer list.
        if (!editing) requestRemove(selectedList, id === "remove-data");
        return;
    }
    if (!one) return;
    switch (id) {
      case "copy-magnet":
        return void call<{ magnet: string } | null>("details", { hash: one }).then(async (d) => {
          if (d && (await copyText(d.magnet))) toast("Magnet link copied");
        });
      case "sequential":
        return void call("setSequential", { hash: one, enabled: !byHash.get(one)?.sequential }).then(refresh).catch(fail);
      case "queue-top":
      case "queue-bottom":
        return void call("setQueuePosition", { hash: one, position: id === "queue-top" ? 0 : 1e6 }).then(refresh).catch(fail);
    }
  };
  useEffect(() => onMenuBar((id) => onMenuItem.current(id)), []);

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
      { label: "Show in Finder", icon: <FolderOpen size={14} />, hint: "↵", disabled: many, onSelect: () => revealHash(hash) },
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
      {
        label: "Remove and move data to Trash",
        icon: <Trash2 size={14} />,
        hint: "⇧Del",
        danger: true,
        onSelect: () => requestRemove(hashes, true),
      },
    ];
  };

  const commands: Command[] = useMemo(
    () => [
      { id: "add", label: "Add torrent file…", hint: "⌘O", icon: <FileDown size={14} />, run: openTorrentPicker },
      { id: "magnet", label: "Add magnet link…", hint: "⌘V", icon: <Magnet size={14} />, run: openMagnet },
      { id: "resume-all", label: "Resume all", icon: <Play size={14} />, run: () => call("resumeAll").then(refresh).catch(fail) },
      { id: "pause-all", label: "Pause all", icon: <Pause size={14} />, run: () => call("pauseAll").then(refresh).catch(fail) },
      { id: "settings", label: "Settings", hint: "⌘,", icon: <Settings2 size={14} />, run: () => setSettingsOpen(true) },
      { id: "update", label: "Check for updates…", icon: <RefreshCw size={14} />, run: () => runUpdateCheck(true) },
      { id: "theme", label: "Cycle theme", hint: "⌘⇧L", icon: <SunMoon size={14} />, run: () => cycleTheme() },
      ...THEMES.map((t) => ({ id: `theme-${t.id}`, label: `Theme: ${t.label}`, icon: <SunMoon size={14} />, run: () => setPrefs((p) => ({ ...p, theme: t.id })) })),
      ...FILTERS.map((f) => ({ id: `filter-${f.id}`, label: `Show ${f.label.toLowerCase()}`, icon: f.icon, run: () => setFilter(f.id) })),
    ],
    [openTorrentPicker, openMagnet, refresh, fail, cycleTheme, runUpdateCheck],
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
            <Mark className="empty-mark" live />
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
    <div className="app" data-mac data-desktop data-boot={boot !== "done" ? boot : undefined} data-inactive={inactive || undefined}>
      <Sidebar
        filters={filterDefs}
        active={filter}
        onFilter={setFilter}
        stats={stats}
        history={history}
        onSettings={() => setSettingsOpen(true)}
        onTheme={(e) => cycleTheme(e)}
        onPalette={() => setPaletteOpen(true)}
        mac
        version={version}
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
              <Magnet size={14} /> <span className="btn-label">Magnet</span>
            </button>
            <button className="btn primary" onClick={openTorrentPicker} title="Add torrent file (⌘O)">
              <Plus size={14} /> <span className="btn-label">Add</span>
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
        <div className="panel-wrap" data-collapsed={collapsed || (snapshot && torrents.length === 0) || undefined}>
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
        version={version}
        onCheckUpdates={() => runUpdateCheck(true)}
      />

      <UpdateDialog
        update={update}
        onClose={() => {
          if (update) deferred.current.add(update.version);
          setUpdate(null);
        }}
        onError={(m) => toast("Update failed", { body: m, tone: "error" })}
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
                <span className="toggle-label">Also move downloaded files to the Trash</span>
              </label>
            </div>
            <footer className="modal-foot">
              <span className="mono faint">{removeReq.deleteFiles ? "files go to the Trash" : "files stay on disk"}</span>
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
                  {removeReq.deleteFiles ? "Move to Trash" : "Remove"}
                </button>
              </div>
            </footer>
          </>
        )}
      </Modal>

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.hash)} onClose={() => setMenu(null)} />}

      {boot === "launch" || boot === "reveal" ? (
        <Launch onReveal={() => setBoot("reveal")} onGone={() => setTimeout(() => setBoot("done"), 900)} />
      ) : null}

    </div>
  );
}
