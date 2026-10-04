"use client";

import { AnimatePresence, motion } from "motion/react";
import { memo, type MouseEvent } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";
import type { TorrentStatus, TorrentSummary } from "@/lib/types";
import { ago, bytes, eta, pad2, pct, rate } from "@/lib/format";
import { Tween } from "@/lib/tween";
import { ease } from "./Modal";

export type SortKey = "queue" | "name" | "size" | "progress" | "status" | "down" | "up" | "eta" | "peers" | "ratio" | "added";
export interface Sort {
  key: SortKey;
  dir: 1 | -1;
}

const COLS: { key: SortKey; label: string; cls: string }[] = [
  { key: "queue", label: "#", cls: "c-idx" },
  { key: "name", label: "Name", cls: "c-name" },
  { key: "size", label: "Size", cls: "c-size num" },
  { key: "progress", label: "Progress", cls: "c-prog" },
  { key: "status", label: "Status", cls: "c-status" },
  { key: "down", label: "Down", cls: "c-down num" },
  { key: "up", label: "Up", cls: "c-up num" },
  { key: "eta", label: "ETA", cls: "c-eta num" },
  { key: "peers", label: "Peers", cls: "c-peers num" },
  { key: "ratio", label: "Ratio", cls: "c-ratio num" },
  { key: "added", label: "Added", cls: "c-added num" },
];

export function statusLabel(t: TorrentSummary): { label: string; tone: string } {
  const map: Record<TorrentStatus, [string, string]> = {
    downloading: ["Downloading", "down"],
    seeding: ["Seeding", "seed"],
    paused: ["Paused", "idle"],
    queued: ["Queued", "idle"],
    checking: ["Checking", "warn"],
    metadata: ["Fetching info", "warn"],
    finished: ["Finished", "seed-dim"],
    error: ["Error", "err"],
  };
  const [label, tone] = map[t.status];
  if (t.status === "downloading" && t.downloadRate === 0 && t.peers + t.seeds === 0) return { label: "Stalled", tone: "warn" };
  return { label, tone };
}

const statusRank: Record<TorrentStatus, number> = {
  downloading: 0,
  metadata: 1,
  checking: 2,
  seeding: 3,
  queued: 4,
  finished: 5,
  paused: 6,
  error: 7,
};

export function sortTorrents(list: TorrentSummary[], s: Sort): TorrentSummary[] {
  const key = (t: TorrentSummary): number | string => {
    switch (s.key) {
      case "queue":
        return t.queuePosition;
      case "name":
        return t.name.toLowerCase();
      case "size":
        return t.size;
      case "progress":
        return t.progress;
      case "status":
        return statusRank[t.status];
      case "down":
        return t.downloadRate;
      case "up":
        return t.uploadRate;
      case "eta":
        return t.eta ?? Number.MAX_SAFE_INTEGER;
      case "peers":
        return t.peers + t.seeds;
      case "ratio":
        return t.ratio;
      case "added":
        return t.addedAt;
    }
  };
  return [...list].sort((a, b) => {
    const ka = key(a);
    const kb = key(b);
    const c = ka < kb ? -1 : ka > kb ? 1 : a.queuePosition - b.queuePosition;
    return c * s.dir;
  });
}

interface RowProps {
  t: TorrentSummary;
  index: number;
  selected: boolean;
  focused: boolean;
  onPointer: (e: MouseEvent, hash: string) => void;
  onMenu: (e: MouseEvent, hash: string) => void;
  onOpen: (hash: string) => void;
}

const Row = memo(
  function Row({ t, index, selected, focused, onPointer, onMenu, onOpen }: RowProps) {
    const st = statusLabel(t);
    const unknownSize = !t.hasMetadata;
    const meta = [
      unknownSize ? "size unknown" : `${bytes(t.doneBytes)} of ${bytes(t.size)}`,
      t.status === "downloading" || t.status === "seeding" ? `${t.seeds} seeds · ${t.peers} peers` : null,
      t.sequential ? "sequential" : null,
      t.private ? "private" : null,
      t.error,
    ]
      .filter(Boolean)
      .join("  ·  ");
    return (
      <div
        role="row"
        aria-selected={selected}
        className="trow"
        data-status={t.status}
        data-tone={st.tone}
        data-selected={selected || undefined}
        data-focused={focused || undefined}
        onMouseDown={(e) => e.button === 0 && onPointer(e, t.infoHash)}
        onContextMenu={(e) => onMenu(e, t.infoHash)}
        onDoubleClick={() => onOpen(t.infoHash)}
      >
        <div className="c-idx mono">{pad2(index + 1)}</div>
        <div className="c-name">
          <div className="tname" title={t.name}>
            {t.name}
          </div>
          <div className="tmeta mono">{meta}</div>
        </div>
        <div className="c-size num mono">{unknownSize ? "—" : bytes(t.size)}</div>
        <div className="c-prog">
          <div className="bar">
            <div className="bar-fill" style={{ transform: `scaleX(${Math.max(0, Math.min(1, t.progress))})` }} />
          </div>
          <Tween key={t.infoHash} className="bar-pct mono" value={t.progress} format={(v) => pct(v, v >= 0.999 ? 0 : 1)} />
        </div>
        <div className="c-status">
          <span className="sdot" />
          <span className="slabel mono">{st.label}</span>
        </div>
        <div className="c-down num mono">
          <Tween key={t.infoHash} value={t.downloadRate} format={rate} />
        </div>
        <div className="c-up num mono">
          <Tween key={t.infoHash} value={t.uploadRate} format={rate} />
        </div>
        <div className="c-eta num mono">{t.status === "downloading" ? eta(t.eta) : ""}</div>
        <div className="c-peers num mono">
          {t.seeds}
          <span className="faint"> · </span>
          {t.peers}
        </div>
        <div className="c-ratio num mono">{t.ratio.toFixed(2)}</div>
        <div className="c-added num mono">{ago(t.addedAt)}</div>
      </div>
    );
  },
  (a, b) =>
    a.index === b.index &&
    a.selected === b.selected &&
    a.focused === b.focused &&
    a.onPointer === b.onPointer &&
    a.onMenu === b.onMenu &&
    a.onOpen === b.onOpen &&
    a.t.name === b.t.name &&
    a.t.status === b.t.status &&
    a.t.progress === b.t.progress &&
    a.t.size === b.t.size &&
    a.t.doneBytes === b.t.doneBytes &&
    a.t.downloadRate === b.t.downloadRate &&
    a.t.uploadRate === b.t.uploadRate &&
    a.t.eta === b.t.eta &&
    a.t.peers === b.t.peers &&
    a.t.seeds === b.t.seeds &&
    a.t.ratio === b.t.ratio &&
    a.t.error === b.t.error &&
    a.t.sequential === b.t.sequential &&
    a.t.addedAt === b.t.addedAt,
);

export function TorrentTable({
  torrents,
  sort,
  onSort,
  selected,
  focus,
  onPointer,
  onMenu,
  onOpen,
  empty,
}: {
  torrents: TorrentSummary[];
  sort: Sort;
  onSort: (k: SortKey) => void;
  selected: Set<string>;
  focus: string | null;
  onPointer: (e: MouseEvent, hash: string) => void;
  onMenu: (e: MouseEvent, hash: string) => void;
  onOpen: (hash: string) => void;
  empty: React.ReactNode;
}) {
  const animate = torrents.length <= 150;
  return (
    <div className="table" role="grid" aria-rowcount={torrents.length}>
      <div className="thead" role="row">
        {COLS.map((c) => (
          <button
            key={c.key}
            role="columnheader"
            className={`th ${c.cls}${sort.key === c.key ? " sorted" : ""}`}
            onClick={() => onSort(c.key)}
            aria-sort={sort.key === c.key ? (sort.dir === 1 ? "ascending" : "descending") : "none"}
          >
            <span>{c.label}</span>
            {sort.key === c.key && (sort.dir === 1 ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
          </button>
        ))}
      </div>
      <div className="tbody">
        {torrents.length === 0 && empty}
        <AnimatePresence initial={false}>
          {torrents.map((t, i) => (
            <motion.div
              key={t.infoHash}
              layout={animate ? "position" : false}
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, height: 0, transition: { duration: 0.22, ease } }}
              transition={{ duration: 0.38, ease, layout: { duration: 0.45, ease } }}
              className="trow-wrap"
            >
              <Row
                t={t}
                index={i}
                selected={selected.has(t.infoHash)}
                focused={focus === t.infoHash}
                onPointer={onPointer}
                onMenu={onMenu}
                onOpen={onOpen}
              />
            </motion.div>
          ))}
        </AnimatePresence>
      </div>
    </div>
  );
}
