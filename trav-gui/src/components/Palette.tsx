"use client";

import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Magnet, Search } from "lucide-react";
import type { TorrentSummary } from "@/lib/types";
import { looksLikeMagnet, pct } from "@/lib/format";
import { ease } from "./Modal";

export interface Command {
  id: string;
  label: string;
  hint?: string;
  icon?: ReactNode;
  run: () => void;
}

function score(q: string, s: string): number {
  if (!q) return 1;
  const a = s.toLowerCase();
  const b = q.toLowerCase();
  const i = a.indexOf(b);
  if (i >= 0) return 100 - i;
  // Subsequence fallback.
  let j = 0;
  for (const ch of a) if (ch === b[j]) j++;
  return j === b.length ? 10 : 0;
}

export function Palette({
  open,
  onClose,
  commands,
  torrents,
  onJump,
  onMagnet,
}: {
  open: boolean;
  onClose: () => void;
  commands: Command[];
  torrents: TorrentSummary[];
  onJump: (hash: string) => void;
  onMagnet: (uri: string) => void;
}) {
  const [q, setQ] = useState("");
  const [idx, setIdx] = useState(0);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQ("");
      setIdx(0);
      requestAnimationFrame(() => input.current?.focus());
    }
  }, [open]);

  const results = useMemo(() => {
    const out: Command[] = [];
    if (looksLikeMagnet(q)) out.push({ id: "magnet", label: "Add this magnet link", icon: <Magnet size={14} />, run: () => onMagnet(q.trim()) });
    const cmds = commands
      .map((c) => ({ c, s: score(q, c.label) }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s)
      .map((x) => x.c);
    const ts = q
      ? torrents
          .map((t) => ({ t, s: score(q, t.name) }))
          .filter((x) => x.s > 0)
          .sort((a, b) => b.s - a.s)
          .slice(0, 8)
          .map(({ t }): Command => ({ id: `t:${t.infoHash}`, label: t.name, hint: `${t.status} · ${pct(t.progress, 0)}`, run: () => onJump(t.infoHash) }))
      : [];
    return [...out, ...cmds, ...ts];
  }, [q, commands, torrents, onJump, onMagnet]);

  useEffect(() => setIdx(0), [q]);

  const run = (c?: Command) => {
    if (!c) return;
    onClose();
    c.run();
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div className="modal-scrim palette-scrim" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.15 }} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
          <motion.div
            className="palette"
            role="dialog"
            aria-label="Command palette"
            initial={{ opacity: 0, y: -10, scale: 0.985 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.26, ease }}
          >
            <div className="palette-input">
              <Search size={15} />
              <input
                ref={input}
                value={q}
                placeholder="Type a command, a torrent name, or paste a magnet…"
                onChange={(e) => setQ(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "ArrowDown") {
                    e.preventDefault();
                    setIdx((i) => Math.min(results.length - 1, i + 1));
                  } else if (e.key === "ArrowUp") {
                    e.preventDefault();
                    setIdx((i) => Math.max(0, i - 1));
                  } else if (e.key === "Enter") {
                    e.preventDefault();
                    run(results[idx]);
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    onClose();
                  }
                }}
              />
              <kbd>esc</kbd>
            </div>
            <div className="palette-list" role="listbox">
              {results.length === 0 && <div className="palette-empty mono faint">no matches</div>}
              {results.map((c, i) => (
                <button key={c.id} role="option" aria-selected={i === idx} className="palette-item" onMouseEnter={() => setIdx(i)} onClick={() => run(c)}>
                  <span className="palette-icon">{c.icon}</span>
                  <span className="palette-label">{c.label}</span>
                  {c.hint && <span className="palette-hint mono">{c.hint}</span>}
                </button>
              ))}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
