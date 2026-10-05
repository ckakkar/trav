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
    let colors = { down: "", up: "", line: "", faint: "" };
    let frames = 0;
    let visible = true;

    const readColors = () => {
      colors = {
        down: cssVar(c, "--accent"),
        up: cssVar(c, "--up"),
        line: cssVar(c, "--line-soft"),
        faint: cssVar(c, "--text-faint"),
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

    function path(points: [number, number][]) {
      ctx.beginPath();
      ctx.moveTo(points[0][0], points[0][1]);
      for (let i = 1; i < points.length - 1; i++) {
        const mx = (points[i][0] + points[i + 1][0]) / 2;
        const my = (points[i][1] + points[i + 1][1]) / 2;
        ctx.quadraticCurveTo(points[i][0], points[i][1], mx, my);
      }
      const last = points[points.length - 1];
      ctx.lineTo(last[0], last[1]);
    }

    function draw() {
      raf = 0;
      if (!visible || document.hidden) return;
      if (++frames % 60 === 0) readColors();
      const samples = src.current() ?? [];
      const now = performance.now() - SAMPLE_MS;
      // Until a full window of history exists, stretch what we have (min 20 s)
      // so the trace fills the canvas instead of hugging the right edge.
      const first = samples.length ? samples[0].t : now;
      const span = Math.max(20_000, Math.min(windowMs, now - first));
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
        ctx.font = `10px ${cssVar(c, "--font-mono") || "ui-monospace, monospace"}`;
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

      if (vis.length >= 2) {
        for (const series of ["up", "down"] as const) {
          const color = series === "down" ? colors.down : colors.up;
          const pts: [number, number][] = vis.map((s) => [x(s.t), y(s[series])]);
          path(pts);
          ctx.lineTo(pts[pts.length - 1][0], bottom);
          ctx.lineTo(pts[0][0], bottom);
          ctx.closePath();
          const g = ctx.createLinearGradient(0, top, 0, bottom);
          g.addColorStop(0, withAlpha(color, series === "down" ? 0.28 : 0.12));
          g.addColorStop(1, withAlpha(color, 0));
          ctx.fillStyle = g;
          ctx.fill();
          path(pts);
          ctx.strokeStyle = color;
          ctx.lineWidth = series === "down" ? 1.6 : 1.3;
          ctx.setLineDash(series === "up" ? [3, 3] : []);
          ctx.stroke();
          ctx.setLineDash([]);
        }
        // Leading-edge dot on the download trace.
        const last = vis[vis.length - 1];
        ctx.fillStyle = colors.down;
        ctx.beginPath();
        ctx.arc(Math.min(w - 2, x(last.t)), y(last.down), 2.2, 0, Math.PI * 2);
        ctx.fill();
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
