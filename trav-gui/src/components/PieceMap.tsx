"use client";

import { useEffect, useRef } from "react";

function decode(b64: string): Uint8Array {
  if (!b64) return new Uint8Array();
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const bit = (b: Uint8Array, i: number) => (b[i >> 3] & (0x80 >> (i & 7))) !== 0;

/** How long a column lights up after new pieces in it are verified. */
const FLASH_MS = 1100;

interface Buckets {
  count: number;
  cols: number;
  /** Verified pieces per column, and whether any are in flight. */
  got: number[];
  flying: boolean[];
  span: number[];
}

function bucket(pieces: string, inProgress: string, count: number, cols: number): Buckets {
  const hv = decode(pieces);
  const ip = decode(inProgress);
  const got: number[] = [];
  const flying: boolean[] = [];
  const span: number[] = [];
  for (let c = 0; c < cols; c++) {
    const a = Math.floor((c * count) / cols);
    const b = Math.max(a + 1, Math.floor(((c + 1) * count) / cols));
    let g = 0;
    let f = false;
    for (let i = a; i < b; i++) {
      if (bit(hv, i)) g++;
      else if (bit(ip, i)) f = true;
    }
    got.push(g);
    flying.push(f);
    span.push(b - a);
  }
  return { count, cols, got, flying, span };
}

/**
 * One column per bucket of pieces; shade = fraction verified, a lower band =
 * pieces in flight. Columns that gain pieces flash as they arrive.
 */
export function PieceMap({ pieces, inProgress, count }: { pieces: string; inProgress: string; count: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const prev = useRef<Buckets | null>(null);
  const flashes = useRef(new Map<number, number>());

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    let raf = 0;
    let data: Buckets | null = null;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    const columns = () => Math.max(1, Math.min(count, Math.floor(c.getBoundingClientRect().width)));
    const refresh = () => {
      const cols = columns();
      data = count ? bucket(pieces, inProgress, count, cols) : null;
      const p = prev.current;
      if (data && p && p.count === data.count && p.cols === data.cols && !reduce) {
        const now = performance.now();
        data.got.forEach((g, i) => g > p.got[i] && flashes.current.set(i, now));
      }
      prev.current = data;
    };

    const paint = () => {
      raf = 0;
      const ctx = c.getContext("2d")!;
      const r = c.getBoundingClientRect();
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      if (c.width !== Math.round(r.width * dpr) || c.height !== Math.round(r.height * dpr)) {
        c.width = Math.round(r.width * dpr);
        c.height = Math.round(r.height * dpr);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const st = getComputedStyle(c);
      const color = (v: string) => st.getPropertyValue(v).trim();
      const have = color("--map-have");
      const prog = color("--map-flight");
      const flash = color("--map-flash");
      ctx.clearRect(0, 0, r.width, r.height);
      ctx.fillStyle = color("--map-empty");
      ctx.fillRect(0, 0, r.width, r.height);
      if (!data) return;
      const cw = r.width / data.cols;
      const gap = cw > 3 ? 1 : 0;
      const now = performance.now();
      for (let i = 0; i < data.cols; i++) {
        const x = i * cw;
        if (data.got[i]) {
          ctx.globalAlpha = 0.25 + 0.75 * (data.got[i] / data.span[i]);
          ctx.fillStyle = have;
          ctx.fillRect(x, 0, cw - gap, r.height);
        }
        if (data.flying[i]) {
          ctx.globalAlpha = 0.9;
          ctx.fillStyle = prog;
          ctx.fillRect(x, r.height * 0.55, cw - gap, r.height * 0.45);
        }
        const t = flashes.current.get(i);
        if (t !== undefined) {
          const k = 1 - (now - t) / FLASH_MS;
          if (k <= 0) flashes.current.delete(i);
          else {
            // Bright at once, then easing out; a little wider than the column so it reads.
            ctx.globalAlpha = k * k;
            ctx.fillStyle = flash;
            ctx.fillRect(x - 1, 0, cw - gap + 2, r.height);
          }
        }
      }
      ctx.globalAlpha = 1;
      if (flashes.current.size) raf = requestAnimationFrame(paint);
    };

    refresh();
    paint();
    const ro = new ResizeObserver(() => {
      // A new width regroups the columns: nothing to compare against.
      if (data && columns() !== data.cols) {
        prev.current = null;
        flashes.current.clear();
        refresh();
      }
      if (!raf) paint();
    });
    ro.observe(c);
    const mo = new MutationObserver(() => !raf && paint());
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      mo.disconnect();
    };
  }, [pieces, inProgress, count]);

  return <canvas ref={canvas} className="piecemap" aria-label="Piece map" />;
}
