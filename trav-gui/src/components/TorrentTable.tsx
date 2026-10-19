"use client";

import { AnimatePresence, motion } from "motion/react";
import { memo, useEffect, useRef, useState, type CSSProperties, type MouseEvent } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";
import type { TorrentSummary } from "@/lib/types";
import { statusLabel, type Sort, type SortKey } from "@/lib/torrents";

export { sortTorrents, statusLabel, type Sort, type SortKey } from "@/lib/torrents";
import { ago, bytes, eta, pad2, pct, rate } from "@/lib/format";
import { Tween } from "@/lib/tween";
import { ease } from "./Modal";

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

/** Sheen speed follows download speed (in steps, so it doesn't stutter as rates wobble). */
function flow(rate: number): string {
  if (rate < 64 * 1024) return "3.2s";
  if (rate < 512 * 1024) return "2.2s";
  if (rate < 2 * 1024 * 1024) return "1.5s";
  if (rate < 8 * 1024 * 1024) return "1.05s";
  return "0.8s";
}

const BURST = Array.from({ length: 12 }, (_, i) => {
  const a = (i / 12) * Math.PI * 2 + (i % 2 ? 0.2 : -0.1);
  const d = 16 + (i % 3) * 9;
  return { x: Math.cos(a) * d, y: Math.sin(a) * d * 0.7, s: 2 + (i % 3), hue: i % 4 === 0 ? "var(--accent)" : "var(--ok)" };
});

/** Sparks from the end of the progress bar when a download finishes. */
function Burst() {
  return (
    <span className="burst" aria-hidden>
      {BURST.map((p, i) => (
        <motion.span
          key={i}
          className="spark"
          style={{ width: p.s, height: p.s, background: p.hue }}
          initial={{ x: 0, y: 0, opacity: 1, scale: 1 }}
          animate={{ x: p.x, y: p.y, opacity: 0, scale: 0.3 }}
          transition={{ duration: 0.9 + (i % 3) * 0.12, ease: [0.17, 0.84, 0.44, 1] }}
        />
      ))}
    </span>
  );
}

const DONE = new Set(["seeding", "finished"]);
const BUSY = new Set(["downloading", "metadata"]);

/** Just finished (celebrate) and just added (arrive) moments for a row. */
function useMoments(t: TorrentSummary) {
  const prev = useRef(t.status);
  const [done, setDone] = useState(0);
  const [arrive] = useState(() => Date.now() / 1000 - t.addedAt < 8);
  const [arriving, setArriving] = useState(arrive);
  useEffect(() => {
    const was = prev.current;
    prev.current = t.status;
    if (BUSY.has(was) && DONE.has(t.status)) {
      setDone(Date.now());
      const id = setTimeout(() => setDone(0), 2000);
      return () => clearTimeout(id);
    }
  }, [t.status]);
  useEffect(() => {
    if (!arriving) return;
    const id = setTimeout(() => setArriving(false), 1600);
    return () => clearTimeout(id);
  }, [arriving]);
  return { done, arriving };
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
    const { done, arriving } = useMoments(t);
    const p = Math.max(0, Math.min(1, t.progress));
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
        data-done={done ? "" : undefined}
        data-arrive={arriving || undefined}
        onMouseDown={(e) => e.button === 0 && onPointer(e, t.infoHash)}
        onContextMenu={(e) => onMenu(e, t.infoHash)}
        onDoubleClick={() => onOpen(t.infoHash)}
      >
        <i className="trow-sel" aria-hidden />
        <div className="c-idx mono">{pad2(index + 1)}</div>
        <div className="c-name">
          <div className="tname" title={t.name}>
            {t.name}
          </div>
          <div className="tmeta mono">{meta}</div>
        </div>
        <div className="c-size num mono">{unknownSize ? "—" : bytes(t.size)}</div>
        <div className="c-prog">
          <div className="bar-wrap">
            <div className="bar" style={{ "--p": p, "--flow": flow(t.downloadRate) } as CSSProperties}>
              <div className="bar-fill" />
              <div className="bar-head" />
            </div>
            {done > 0 && <Burst key={done} />}
            {done > 0 && (
              <span key={`stamp-${done}`} className="stamp" aria-hidden>
                Done
              </span>
            )}
          </div>
          <Tween key={t.infoHash} className="bar-pct mono" value={t.progress} format={(v) => pct(v, v >= 0.999 ? 0 : 1)} />
        </div>
        <div className="c-status">
          <span className="sdot" />
          {/* Keyed by label so a change of status replays its entrance. */}
          <span key={st.label} className="slabel mono">
            {st.label}
          </span>
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
              style={{ "--i": Math.min(i, 14) } as CSSProperties}
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
