"use client";

// Time-based canvas graph: x is derived from sample timestamps relative to
// "now", so the trace glides continuously between 2 Hz samples instead of
// stepping. The y-scale eases toward its target so rescales never jump.

import { useEffect, useRef } from "react";
import type { Sample } from "@/lib/engine";
import { rate } from "@/lib/format";

const SAMPLE_MS = 500;

function cssVar(el: Element, name: string) {
  return getComputedStyle(el).getPropertyValue(name).trim() || "#888";
}

function withAlpha(color: string, a: number) {
  if (color.startsWith("#") && (color.length === 7 || color.length === 4)) {
    const hex = color.length === 4 ? color.replace(/#(.)(.)(.)/, "#$1$1$2$2$3$3") : color;
    const n = parseInt(hex.slice(1), 16);
    return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
  }
  return color;
}

export function Graph({
  source,
  windowMs = 120_000,
  axis = false,
  className,
}: {
  source: () => Sample[] | undefined;
  windowMs?: number;
  axis?: boolean;
  className?: string;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const src = useRef(source);
  src.current = source;

  useEffect(() => {
    const c = canvas.current!;
    const ctx = c.getContext("2d")!;
    let raf = 0;
    let w = 0;
    let h = 0;
    let dpr = 1;
    let shownMax = 64 * 1024;
    let colors = { down: "", up: "", line: "", faint: "", style: "plain", cell: 5, off: "" };
    let frames = 0;
    let visible = true;

    const readColors = () => {
      colors = {
        down: cssVar(c, "--accent"),
        up: cssVar(c, "--up"),
        line: cssVar(c, "--line-soft"),
        faint: cssVar(c, "--text-faint"),
        // How the register draws: "glow" (Ink), "print" (Paper, overprinting inks),
        // "pixel" (Bevel, Bitmap: straight 1px steps over a dithered fill),
        // "blocks" (Textmode, Teletext, Hi-Fi: columns of lit cells, like an LED
        // meter), "dots" (Instrument: a dot-matrix display), "plain".
        style: cssVar(c, "--graph"),
        // Cell size for "blocks" and "dots", and the colour of unlit cells.
        cell: Math.max(3, parseFloat(getComputedStyle(c).getPropertyValue("--graph-cell")) || 5),
        off: getComputedStyle(c).getPropertyValue("--graph-off").trim(),
      };
    };
    readColors();

    const resize = () => {
      const r = c.getBoundingClientRect();
      dpr = Math.min(2, window.devicePixelRatio || 1);
      w = Math.max(1, r.width);
      h = Math.max(1, r.height);
      c.width = Math.round(w * dpr);
      c.height = Math.round(h * dpr);
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(c);
    const io = new IntersectionObserver(([e]) => {
      visible = e.isIntersecting;
      if (visible && !raf) raf = requestAnimationFrame(draw);
    });
    io.observe(c);
    const mo = new MutationObserver(readColors);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    /** A 50% checkerboard of `color`, one CSS pixel per square: a fill that dithers. */
    const dithers = new Map<string, CanvasPattern | null>();
    function dither(color: string) {
      if (!dithers.has(color)) {
        const tile = document.createElement("canvas");
        tile.width = tile.height = 2;
        const t = tile.getContext("2d");
        if (t) {
          t.fillStyle = color;
          t.fillRect(0, 0, 1, 1);
          t.fillRect(1, 1, 1, 1);
        }
        dithers.set(color, t ? ctx.createPattern(tile, "repeat") : null);
      }
      return dithers.get(color) ?? color;
    }

    function path(points: [number, number][], straight = false) {
      ctx.beginPath();
      ctx.moveTo(points[0][0], points[0][1]);
      if (straight) {
        for (const [px, py] of points.slice(1)) ctx.lineTo(Math.round(px) + 0.5, Math.round(py) + 0.5);
        return;
      }
      for (let i = 1; i < points.length - 1; i++) {
        const mx = (points[i][0] + points[i + 1][0]) / 2;
        const my = (points[i][1] + points[i + 1][1]) / 2;
        ctx.quadraticCurveTo(points[i][0], points[i][1], mx, my);
      }
      const last = points[points.length - 1];
      ctx.lineTo(last[0], last[1]);
    }

    /** The history as columns of cells, lit up to each moment's download speed;
     *  the upload is a single cell at its own level. */
    function cells(vis: Sample[], x: (t: number) => number, y: (v: number) => number, top: number, bottom: number) {
      const size = colors.cell;
      const gap = Math.max(1, Math.round(size / 5));
      const pitch = size + gap;
      const cols = Math.max(1, Math.floor(w / pitch));
      const rows = Math.max(1, Math.floor((bottom - top + gap) / pitch));
      const left = w - cols * pitch + gap / 2;
      const base = bottom;
      const dots = colors.style === "dots";
      const tStart = vis[0].t;
      const tEnd = vis[vis.length - 1].t;
      const xStart = x(tStart);
      const xEnd = x(tEnd);
      // A column's value: the samples interpolated at its center.
      let j = 0;
      const at = (t: number, key: "down" | "up") => {
        while (j < vis.length - 2 && vis[j + 1].t < t) j++;
        const a = vis[j];
        const b = vis[Math.min(j + 1, vis.length - 1)];
        const f = b.t === a.t ? 0 : Math.min(1, Math.max(0, (t - a.t) / (b.t - a.t)));
        return a[key] + (b[key] - a[key]) * f;
      };
      const level = (v: number) => Math.round(((base - y(v)) / (bottom - top)) * rows);
      const cell = (col: number, row: number) => {
        const cx = left + col * pitch;
        const cy = base - (row + 1) * pitch + gap;
        if (dots) {
          ctx.beginPath();
          ctx.arc(cx + size / 2, cy + size / 2, size / 2, 0, Math.PI * 2);
          ctx.fill();
        } else ctx.fillRect(Math.round(cx), Math.round(cy), size, size);
      };
      const off = colors.off || withAlpha(colors.line, 0.55);
      j = 0; // columns run forward in time, so the cursor only moves on
      for (let col = 0; col < cols; col++) {
        const cx = left + col * pitch + size / 2;
        const has = cx >= xStart - pitch && cx <= xEnd + pitch;
        let down = -1;
        let up = -1;
        if (has) {
          const t = tStart + ((cx - xStart) / Math.max(1, xEnd - xStart)) * (tEnd - tStart);
          down = Math.min(rows, level(at(t, "down")));
          up = Math.min(rows, level(at(t, "up")));
        }
        for (let row = 0; row < rows; row++) {
          ctx.fillStyle = row < down ? colors.down : off;
          cell(col, row);
        }
        if (up > 0) {
          ctx.fillStyle = colors.up;
          cell(col, up - 1);
        }
      }
    }

    function draw() {
      raf = 0;
      if (!visible || document.hidden) return;
      if (++frames % 60 === 0) readColors();
      const samples = src.current() ?? [];
      const now = performance.now() - SAMPLE_MS;
      // Until a full window of history exists, stretch what we have (a few
      // seconds at least) so the trace fills the canvas from launch and zooms
      // out as history builds up, instead of hugging the right edge.
      const first = samples.length ? samples[0].t : now;
      const span = Math.max(4_000, Math.min(windowMs, now - first));
      const t0 = now - span;
      const vis = samples.filter((s) => s.t >= t0 - SAMPLE_MS * 2);

      let target = 64 * 1024;
      for (const s of vis) target = Math.max(target, s.down, s.up);
      target *= 1.18;
      shownMax += (target - shownMax) * 0.08;

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const top = axis ? 14 : 4;
      const bottom = h - (axis ? 2 : 1);
      const y = (v: number) => bottom - (v / shownMax) * (bottom - top);
      const x = (t: number) => ((t - t0) / span) * w;

      if (axis) {
        ctx.strokeStyle = colors.line;
        ctx.lineWidth = 1;
        ctx.setLineDash([2, 4]);
        ctx.font = `10px ${cssVar(c, "--font-data") || "ui-monospace, monospace"}`;
        ctx.fillStyle = colors.faint;
        ctx.textAlign = "right";
        for (const f of [0.25, 0.5, 0.75]) {
          const yy = Math.round(y(shownMax * f)) + 0.5;
          ctx.beginPath();
          ctx.moveTo(0, yy);
          ctx.lineTo(w, yy);
          ctx.stroke();
          ctx.fillText(rate(shownMax * f), w - 4, yy - 4);
        }
        ctx.setLineDash([]);
      }

      if (vis.length >= 2 && (colors.style === "blocks" || colors.style === "dots")) {
        cells(vis, x, y, top, bottom);
      } else if (vis.length >= 2) {
        const print = colors.style === "print";
        const glow = colors.style === "glow";
        const pixel = colors.style === "pixel";
        ctx.imageSmoothingEnabled = !pixel;
        for (const series of ["up", "down"] as const) {
          const color = series === "down" ? colors.down : colors.up;
          const pts: [number, number][] = vis.map((s) => [x(s.t), y(s[series])]);
          path(pts, pixel);
          ctx.lineTo(pts[pts.length - 1][0], bottom);
          ctx.lineTo(pts[0][0], bottom);
          ctx.closePath();
          if (pixel) {
            // Only the download is filled, in a dither of its own color.
            ctx.fillStyle = series === "down" ? dither(color) : "transparent";
          } else if (print) {
            // Flat ink that overprints: pink over blue turns violet where they meet.
            ctx.globalCompositeOperation = "multiply";
            ctx.fillStyle = withAlpha(color, series === "down" ? 0.32 : 0.22);
          } else {
            const g = ctx.createLinearGradient(0, top, 0, bottom);
            g.addColorStop(0, withAlpha(color, series === "down" ? (glow ? 0.34 : 0.28) : glow ? 0.16 : 0.12));
            g.addColorStop(1, withAlpha(color, 0));
            ctx.fillStyle = g;
          }
          ctx.fill();
          path(pts, pixel);
          ctx.strokeStyle = color;
          ctx.lineWidth = pixel ? 1 : print ? (series === "down" ? 2 : 1.5) : series === "down" ? 1.6 : 1.3;
          ctx.setLineDash(series === "up" && !print ? [3, 3] : []);
          if (glow) {
            ctx.shadowColor = color;
            ctx.shadowBlur = series === "down" ? 10 : 6;
          }
          ctx.stroke();
          ctx.shadowBlur = 0;
          ctx.setLineDash([]);
          ctx.globalCompositeOperation = "source-over";
        }
        // Leading edge of the download trace.
        const last = vis[vis.length - 1];
        const lx = Math.min(w - 3, x(last.t));
        const ly = y(last.down);
        ctx.fillStyle = colors.down;
        if (pixel) ctx.fillRect(Math.round(lx) - 2, Math.round(ly) - 2, 4, 4);
        else if (print) ctx.fillRect(lx - 2.5, ly - 2.5, 5, 5);
        else {
          if (glow) {
            ctx.fillStyle = withAlpha(colors.down, 0.25);
            ctx.beginPath();
            ctx.arc(lx, ly, 5 + Math.sin(performance.now() / 260) * 1.2, 0, Math.PI * 2);
            ctx.fill();
            ctx.fillStyle = colors.down;
          }
          ctx.beginPath();
          ctx.arc(lx, ly, 2.2, 0, Math.PI * 2);
          ctx.fill();
        }
      }
      raf = requestAnimationFrame(draw);
    }
    raf = requestAnimationFrame(draw);
    const onVis = () => !document.hidden && !raf && (raf = requestAnimationFrame(draw));
    document.addEventListener("visibilitychange", onVis);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      io.disconnect();
      mo.disconnect();
      document.removeEventListener("visibilitychange", onVis);
    };
  }, [windowMs, axis]);

  return <canvas ref={canvas} className={className} aria-hidden="true" />;
}
