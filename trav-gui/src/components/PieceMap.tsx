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

/** One pixel column per bucket of pieces; shade = fraction verified, amber = in flight. */
export function PieceMap({ pieces, inProgress, count }: { pieces: string; inProgress: string; count: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const paint = () => {
      const ctx = c.getContext("2d")!;
      const r = c.getBoundingClientRect();
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      c.width = Math.round(r.width * dpr);
      c.height = Math.round(r.height * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const st = getComputedStyle(c);
      const have = st.getPropertyValue("--accent").trim();
      const prog = st.getPropertyValue("--warn").trim();
      const empty = st.getPropertyValue("--line-soft").trim();
      ctx.fillStyle = empty;
      ctx.fillRect(0, 0, r.width, r.height);
      if (!count) return;
      const hv = decode(pieces);
      const ip = decode(inProgress);
      const cols = Math.max(1, Math.min(count, Math.floor(r.width)));
      const cw = r.width / cols;
      for (let c0 = 0; c0 < cols; c0++) {
        const a = Math.floor((c0 * count) / cols);
        const b = Math.max(a + 1, Math.floor(((c0 + 1) * count) / cols));
        let got = 0;
        let flying = false;
        for (let i = a; i < b; i++) {
          if (bit(hv, i)) got++;
          else if (bit(ip, i)) flying = true;
        }
        const x = c0 * cw;
        const gap = cw > 3 ? 1 : 0;
        if (got) {
          ctx.globalAlpha = 0.25 + 0.75 * (got / (b - a));
          ctx.fillStyle = have;
          ctx.fillRect(x, 0, cw - gap, r.height);
        }
        if (flying) {
          ctx.globalAlpha = 0.9;
          ctx.fillStyle = prog;
          ctx.fillRect(x, r.height * 0.55, cw - gap, r.height * 0.45);
        }
        ctx.globalAlpha = 1;
      }
    };
    paint();
    const ro = new ResizeObserver(paint);
    ro.observe(c);
    return () => ro.disconnect();
  }, [pieces, inProgress, count]);

  return <canvas ref={canvas} className="piecemap" aria-label="Piece map" />;
}
