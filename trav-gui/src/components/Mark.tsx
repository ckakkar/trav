"use client";

// Trav's mark (the "T" whose stem ends in a download arrow, over a progress
// line), drawn from the same geometry as the app icon (scripts/app-icon.swift).

import { motion, type Transition } from "motion/react";
import { useId } from "react";

export const MARK_VIEWBOX = "250 240 524 630";

const bar = { x: 282, y: 268, width: 460, height: 104, rx: 52 };
const stem = { x: 460, y: 330, width: 104, height: 270 };
const head = "330,560 694,560 512,752";
const track = { x: 300, y: 812, width: 424, height: 34, rx: 17 };

const box = { transformBox: "fill-box" as const };

/** The still or gently "alive" mark (empty states). Animated by CSS: `.mark[data-live]`. */
export function Mark({ className, live, progress = 0.62 }: { className?: string; live?: boolean; progress?: number }) {
  const clip = `mark-track-${useId().replace(/:/g, "")}`;
  return (
    <svg className={`mark ${className ?? ""}`} viewBox={MARK_VIEWBOX} data-live={live || undefined} aria-hidden>
      <defs>
        <clipPath id={clip}>
          <rect {...track} />
        </clipPath>
      </defs>
      <g className="mark-glyph">
        <rect {...bar} />
        <g className="mark-arrow">
          <rect {...stem} />
          <polygon points={head} strokeWidth={44} strokeLinejoin="round" />
        </g>
      </g>
      <rect className="mark-track" {...track} />
      <rect className="mark-fill" {...track} style={{ ...box, transformOrigin: "left center", transform: `scaleX(${progress})` }} />
      <g clipPath={`url(#${clip})`}>
        <rect className="mark-glint" {...track} width={60} />
      </g>
    </svg>
  );
}

const out = [0.17, 0.84, 0.44, 1] as const;

/** The mark assembling itself: bar, stem, the arrow dropping in, the line filling. */
export function MarkIntro({ className, onFilled }: { className?: string; onFilled?: () => void }) {
  const t = (delay: number, duration: number): Transition => ({ delay, duration, ease: out });
  return (
    <svg className={`mark ${className ?? ""}`} viewBox={MARK_VIEWBOX} aria-hidden>
      <motion.rect {...bar} style={{ ...box, transformOrigin: "center" }} initial={{ scaleX: 0, opacity: 0 }} animate={{ scaleX: 1, opacity: 1 }} transition={t(0.05, 0.5)} />
      <motion.rect {...stem} style={{ ...box, transformOrigin: "top center" }} initial={{ scaleY: 0 }} animate={{ scaleY: 1 }} transition={t(0.3, 0.32)} />
      <motion.polygon
        points={head}
        strokeWidth={44}
        strokeLinejoin="round"
        initial={{ y: -150, opacity: 0 }}
        animate={{ y: 0, opacity: 1 }}
        transition={{ delay: 0.42, type: "spring", stiffness: 420, damping: 17, mass: 0.9 }}
      />
      <motion.rect className="mark-track" {...track} initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={t(0.35, 0.4)} />
      <motion.rect
        className="mark-fill"
        {...track}
        style={{ ...box, transformOrigin: "left center" }}
        initial={{ scaleX: 0 }}
        animate={{ scaleX: 1 }}
        transition={t(0.68, 0.62)}
        onAnimationComplete={onFilled}
      />
    </svg>
  );
}
