"use client";

// Fluid numbers without React re-renders: every <Tween> registers with one
// shared rAF loop that eases toward the latest value and writes textContent
// directly. Hundreds of live counters cost one animation frame callback.

import { useLayoutEffect, useRef } from "react";

interface Item {
  el: HTMLElement;
  cur: number;
  target: number;
  fmt: (n: number) => string;
  last: string;
}

const active = new Set<Item>();
let raf = 0;
let lastT = 0;
const TAU = 220; // ms time-constant: ~95% of the way in 3τ ≈ 660 ms, matching the 500 ms poll

const reduced = () => typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

function frame(t: number) {
  const dt = lastT ? Math.min(64, t - lastT) : 16;
  lastT = t;
  const k = 1 - Math.exp(-dt / TAU);
  for (const it of active) {
    const diff = it.target - it.cur;
    if (Math.abs(diff) <= Math.max(1e-4, Math.abs(it.target) * 1e-4)) {
      it.cur = it.target;
      active.delete(it);
    } else {
      it.cur += diff * k;
    }
    const s = it.fmt(it.cur);
    if (s !== it.last) {
      it.el.textContent = s;
      it.last = s;
    }
  }
  if (active.size) raf = requestAnimationFrame(frame);
  else {
    raf = 0;
    lastT = 0;
  }
}

export function Tween({
  value,
  format,
  className,
  title,
}: {
  value: number;
  format: (n: number) => string;
  className?: string;
  title?: string;
}) {
  // Callers key <Tween> by identity (e.g. info-hash) so switching subjects snaps rather than sweeps.
  const ref = useRef<HTMLSpanElement>(null);
  const item = useRef<Item | null>(null);
  const fmtRef = useRef(format);
  fmtRef.current = format;

  useLayoutEffect(() => {
    const el = ref.current!;
    const it: Item = { el, cur: value, target: value, fmt: (n) => fmtRef.current(n), last: "" };
    it.last = it.fmt(value);
    el.textContent = it.last;
    item.current = it;
    return () => {
      active.delete(it);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useLayoutEffect(() => {
    const it = item.current;
    if (!it) return;
    it.target = value;
    if (reduced() || !Number.isFinite(value)) {
      it.cur = value;
      it.last = it.fmt(value);
      it.el.textContent = it.last;
      return;
    }
    active.add(it);
    if (!raf) raf = requestAnimationFrame(frame);
  }, [value]);

  // Format changes (unit toggles) must repaint immediately.
  useLayoutEffect(() => {
    const it = item.current;
    if (it) {
      it.last = it.fmt(it.cur);
      it.el.textContent = it.last;
    }
  }, [format]);

  return <span ref={ref} className={className} title={title} />;
}
