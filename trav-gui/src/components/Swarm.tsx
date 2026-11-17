"use client";

// Swarm's particles: one canvas behind the window, driven by the transfers.
//   ambient  a slow drift through a flow field, swirling around the speed
//            readout as fast as the download runs
//   stream   pieces arriving: from the window's edges (the peers) into the
//            leading edge of each downloading row's progress, as many as its
//            speed
//   upload   pieces leaving: out of each uploading row, as many as its speed
//   burst    a finished download, or a dialog appearing out of a ring of light
//   halo     a few orbiting the selected row
// Two canvases: the field behind the panels, and a front one over them for
// what should read on top (halo, bursts, streams on their last stretch).
// Particles shy from the pointer. Nothing runs with Reduce Motion, in a
// background window or a hidden one.

import { useEffect, useRef } from "react";
import { useReducedMotion } from "motion/react";
import type { TorrentSummary } from "@/lib/types";

const MAX = 2400;
const AMBIENT = 0;
const STREAM = 1;
const UPLOAD = 2;
const BURST = 3;
const HALO = 4;

// Colours: teal (download), violet (upload), gold (finished), pale (ambient).
const PALETTE = ["#5ef0d0", "#a78bfa", "#ffd479", "#9fb4ff", "#ffffff"];
const TEAL = 0;
const VIOLET = 1;
const GOLD = 2;
const PALE = 3;
const WHITE = 4;

/** 0 (idle) … 1 (about 16 MB/s), on a log scale like the speed readout's glow. */
function flux(rate: number) {
  return Math.min(1, Math.log10(1 + rate / 65536) / Math.log10(257));
}

/** A soft round glow, drawn once per colour and stamped for every particle. */
function sprite(color: string) {
  const s = 32;
  const c = document.createElement("canvas");
  c.width = c.height = s;
  const g = c.getContext("2d")!;
  const r = g.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
  r.addColorStop(0, "#ffffff");
  r.addColorStop(0.18, color);
  r.addColorStop(0.45, color + "55");
  r.addColorStop(1, color + "00");
  g.fillStyle = r;
  g.fillRect(0, 0, s, s);
  return c;
}

export function Swarm({ torrents, paused }: { torrents: TorrentSummary[]; paused: boolean }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const front = useRef<HTMLCanvasElement>(null);
  const data = useRef(torrents);
  data.current = torrents;
  const halt = useRef(paused);
  halt.current = paused;
  const reduced = useReducedMotion();

  useEffect(() => {
    if (reduced) return;
    const cv = canvas.current!;
    const fv = front.current!;
    const back = cv.getContext("2d", { alpha: true })!;
    const fore = fv.getContext("2d", { alpha: true })!;
    const sprites = PALETTE.map(sprite);

    // Particle state, in flat arrays.
    const x = new Float32Array(MAX);
    const y = new Float32Array(MAX);
    const vx = new Float32Array(MAX);
    const vy = new Float32Array(MAX);
    const life = new Float32Array(MAX);
    const span = new Float32Array(MAX);
    const size = new Float32Array(MAX);
    const kind = new Uint8Array(MAX);
    const color = new Uint8Array(MAX);
    const tx = new Float32Array(MAX);
    const ty = new Float32Array(MAX);
    const seed = new Float32Array(MAX);
    const owner = new Array<string | null>(MAX).fill(null);
    const near = new Uint8Array(MAX); // drawn on the front canvas
    let count = 0;

    let w = 0;
    let h = 0;
    let dpr = 1;
    const resize = () => {
      dpr = Math.min(1.5, window.devicePixelRatio || 1);
      w = window.innerWidth;
      h = window.innerHeight;
      for (const c of [cv, fv]) {
        c.width = Math.round(w * dpr);
        c.height = Math.round(h * dpr);
        c.style.width = `${w}px`;
        c.style.height = `${h}px`;
      }
    };
    resize();
    window.addEventListener("resize", resize);

    const pointer = { x: -1e4, y: -1e4 };
    const onMove = (e: PointerEvent) => {
      pointer.x = e.clientX;
      pointer.y = e.clientY;
    };
    const onLeave = () => {
      pointer.x = pointer.y = -1e4;
    };
    window.addEventListener("pointermove", onMove, { passive: true });
    document.addEventListener("pointerleave", onLeave);

    function spawn(k: number, px: number, py: number, pvx: number, pvy: number, l: number, sz: number, col: number, who: string | null = null) {
      if (count >= MAX) return -1;
      const i = count++;
      x[i] = px;
      y[i] = py;
      vx[i] = pvx;
      vy[i] = pvy;
      life[i] = l;
      span[i] = l;
      size[i] = sz;
      kind[i] = k;
      color[i] = col;
      seed[i] = Math.random() * 1000;
      owner[i] = who;
      near[i] = k === BURST || k === HALO ? 1 : 0;
      tx[i] = px;
      ty[i] = py;
      return i;
    }
    function kill(i: number) {
      const j = --count;
      if (i === j) return;
      x[i] = x[j];
      y[i] = y[j];
      vx[i] = vx[j];
      vy[i] = vy[j];
      life[i] = life[j];
      span[i] = span[j];
      size[i] = size[j];
      kind[i] = kind[j];
      color[i] = color[j];
      tx[i] = tx[j];
      ty[i] = ty[j];
      seed[i] = seed[j];
      owner[i] = owner[j];
      near[i] = near[j];
    }
    /** A point on the window's edge: where pieces come from. */
    function edge(): [number, number] {
      const r = Math.random() * (2 * w + 2 * h);
      if (r < w) return [r, -8];
      if (r < 2 * w) return [r - w, h + 8];
      if (r < 2 * w + h) return [-8, r - 2 * w];
      return [w + 8, r - 2 * w - h];
    }
    function burst(cx: number, cy: number, n: number, col: number, power = 1) {
      for (let k = 0; k < n; k++) {
        const a = Math.random() * Math.PI * 2;
        const s = (1.5 + Math.random() * 5.5) * power;
        spawn(BURST, cx, cy, Math.cos(a) * s, Math.sin(a) * s, 50 + Math.random() * 60, 0.7 + Math.random() * 1.4, Math.random() < 0.25 ? WHITE : col);
      }
    }
    function ring(r: DOMRect, n: number) {
      for (let k = 0; k < n; k++) {
        const t = Math.random() * 2 * (r.width + r.height);
        let px: number;
        let py: number;
        let nx = 0;
        let ny = 0;
        if (t < r.width) [px, py, ny] = [r.left + t, r.top, -1];
        else if (t < 2 * r.width) [px, py, ny] = [r.left + t - r.width, r.bottom, 1];
        else if (t < 2 * r.width + r.height) [px, py, nx] = [r.left, r.top + t - 2 * r.width, -1];
        else [px, py, nx] = [r.right, r.top + t - 2 * r.width - r.height, 1];
        const s = 0.6 + Math.random() * 2.6;
        spawn(BURST, px, py, nx * s + (Math.random() - 0.5), ny * s + (Math.random() - 0.5), 40 + Math.random() * 40, 0.6 + Math.random(), Math.random() < 0.5 ? TEAL : PALE);
      }
    }

    // The window's furniture, measured every few frames rather than every one.
    type Row = { hash: string; head: DOMRect | null; bar: DOMRect; row: DOMRect; selected: boolean };
    let rows: Row[] = [];
    let speedcard: DOMRect | null = null;
    let dialogOpen = false;
    let frame = 0;
    const measure = () => {
      const list = document.querySelector(".list-region")?.getBoundingClientRect();
      rows = [];
      document.querySelectorAll<HTMLElement>(".trow").forEach((el) => {
        const row = el.getBoundingClientRect();
        if (list && (row.bottom < list.top || row.top > list.bottom)) return;
        const bar = el.querySelector(".bar")?.getBoundingClientRect();
        if (!bar) return;
        const fill = el.querySelector(".bar-fill")?.getBoundingClientRect() ?? null;
        rows.push({ hash: el.id.replace(/^row-/, ""), head: fill, bar, row, selected: el.hasAttribute("data-selected") });
      });
      speedcard = document.querySelector(".speedcard")?.getBoundingClientRect() ?? null;
      const dialog = document.querySelector(".modal-scrim .modal, .modal-scrim .palette");
      if (dialog && !dialogOpen) {
        // Wait a beat for it to settle, then let it appear out of a ring of light.
        setTimeout(() => {
          const d = document.querySelector(".modal-scrim .modal, .modal-scrim .palette");
          if (d) ring(d.getBoundingClientRect(), 180);
        }, 140);
      }
      dialogOpen = !!dialog;
    };

    // Finished downloads burst: remember where each torrent's progress stood.
    const progress = new Map<string, number>();
    const owed = new Map<string, number>();

    let ambientTarget = 0;
    let last = performance.now();
    let raf = 0;
    const tick = (now: number) => {
      raf = requestAnimationFrame(tick);
      if (halt.current || document.hidden) {
        last = now;
        return;
      }
      const dt = Math.min(2.5, (now - last) / 16.67);
      last = now;
      if (frame++ % 6 === 0) measure();

      const byHash = new Map(data.current.map((t) => [t.infoHash, t]));
      const total = data.current.reduce((a, t) => a + t.downloadRate, 0);
      const f = flux(total);

      // Bursts for downloads that just finished.
      for (const t of data.current) {
        const before = progress.get(t.infoHash);
        if (before !== undefined && before < 0.9995 && t.progress >= 0.9995) {
          const r = rows.find((q) => q.hash === t.infoHash);
          if (r) burst(r.bar.right, r.bar.top + r.bar.height / 2, 220, GOLD, 1.2);
        }
        progress.set(t.infoHash, t.progress);
      }

      // Ambient drift: a population that grows a little with the speed.
      ambientTarget = Math.round(Math.min(520, (w * h) / 5200) * (0.7 + 0.5 * f));
      let ambient = 0;
      for (let i = 0; i < count; i++) if (kind[i] === AMBIENT) ambient++;
      for (let k = 0; ambient < ambientTarget && k < 6; k++, ambient++) {
        // Two depths: sharp motes, and a few large soft ones out of focus.
        const near = Math.random() < 0.26;
        const i = spawn(AMBIENT, Math.random() * w, Math.random() * h, 0, 0, 400 + Math.random() * 700, near ? 2.6 + Math.random() * 3.2 : 0.4 + Math.random() * 1.1, Math.random() < 0.65 ? PALE : Math.random() < 0.55 ? TEAL : VIOLET);
        if (i >= 0 && near) seed[i] = -seed[i] - 1; // marks it as out of focus
      }

      // Streams in, and uploads out, per row and per its own speed.
      for (const r of rows) {
        const t = byHash.get(r.hash);
        if (!t) continue;
        if (t.downloadRate > 0 && r.head) {
          const want = (2 + 70 * flux(t.downloadRate)) * (dt / 60);
          let due = (owed.get(r.hash) ?? 0) + want;
          for (; due >= 1; due--) {
            const [ex, ey] = edge();
            const i = spawn(STREAM, ex, ey, (Math.random() - 0.5) * 2, (Math.random() - 0.5) * 2, 600, 1 + Math.random() * 0.9, Math.random() < 0.15 ? WHITE : TEAL, r.hash);
            if (i < 0) break;
          }
          owed.set(r.hash, due);
        }
        if (t.uploadRate > 0) {
          const want = (1 + 40 * flux(t.uploadRate)) * (dt / 60);
          let due = (owed.get(`up:${r.hash}`) ?? 0) + want;
          const ox = r.head ? r.head.right : r.bar.left;
          for (; due >= 1; due--) {
            const a = -Math.PI / 2 + (Math.random() - 0.5) * 2.4;
            spawn(UPLOAD, ox, r.bar.top + r.bar.height / 2, Math.cos(a) * 2.2 + 1.2, Math.sin(a) * 2.2, 90 + Math.random() * 70, 0.7 + Math.random() * 0.9, VIOLET);
          }
          owed.set(`up:${r.hash}`, due);
        }
      }

      // The selected row's halo.
      const sel = rows.find((r) => r.selected);
      let halo = 0;
      for (let i = 0; i < count; i++) if (kind[i] === HALO) halo++;
      if (sel) for (; halo < 44; halo++) spawn(HALO, sel.row.left + Math.random() * sel.row.width, sel.row.top, 0, 0, 1e9, 0.9 + Math.random() * 0.8, Math.random() < 0.55 ? TEAL : WHITE);

      // Move.
      const t0 = now * 0.001;
      const sc = speedcard;
      const scx = sc ? sc.left + sc.width / 2 : 0;
      const scy = sc ? sc.top + sc.height * 0.55 : 0;
      for (let i = count - 1; i >= 0; i--) {
        const k = kind[i];
        life[i] -= dt;
        if (life[i] <= 0) {
          kill(i);
          continue;
        }
        let ax = 0;
        let ay = 0;
        if (k === AMBIENT) {
          // A flow field from a few slow waves.
          const a = Math.sin(x[i] * 0.0042 + t0 * 0.21) * 1.7 + Math.cos(y[i] * 0.0051 - t0 * 0.17) * 1.7 + Math.sin((x[i] + y[i]) * 0.0021 + seed[i]) * 0.6;
          ax = Math.cos(a) * 0.022;
          ay = Math.sin(a) * 0.022;
          // The speed readout pulls a vortex, as strong as the download.
          if (sc && f > 0.02) {
            const dx = x[i] - scx;
            const dy = y[i] - scy;
            const d2 = dx * dx + dy * dy;
            const reach = 190 * 190;
            if (d2 < reach) {
              const d = Math.sqrt(d2) + 1;
              const s = (1 - d2 / reach) * f * 0.16;
              ax += (-dy / d) * s - (dx / d) * s * 0.18;
              ay += (dx / d) * s - (dy / d) * s * 0.18;
            }
          }
          vx[i] = vx[i] * 0.965 + ax * dt;
          vy[i] = vy[i] * 0.965 + ay * dt;
        } else if (k === STREAM) {
          const r = rows.find((q) => q.hash === owner[i]);
          if (!r || !r.head) {
            kind[i] = AMBIENT;
            life[i] = Math.min(life[i], 120);
            continue;
          }
          const gx = r.head.right;
          const gy = r.head.top + r.head.height / 2;
          const dx = gx - x[i];
          const dy = gy - y[i];
          const d = Math.hypot(dx, dy) + 0.001;
          if (d < 7) {
            // Absorbed into the bar: a brief flash where it lands.
            spawn(BURST, gx, gy, (Math.random() - 0.5) * 1.6, (Math.random() - 0.5) * 1.6, 10 + Math.random() * 8, 0.8, WHITE);
            kill(i);
            continue;
          }
          near[i] = d < 70 ? 1 : 0;
          const sp = 5 + 5 * Math.min(1, d / 500);
          // Steer toward the head, with a sideways curl that tightens as it nears.
          const curl = Math.min(1, 120 / d) * 0.9;
          const dxn = dx / d;
          const dyn = dy / d;
          vx[i] = vx[i] * 0.9 + (dxn * sp - dyn * curl * sp * 0.4) * 0.1 * dt;
          vy[i] = vy[i] * 0.9 + (dyn * sp + dxn * curl * sp * 0.4) * 0.1 * dt;
        } else if (k === UPLOAD) {
          vx[i] *= 0.985;
          vy[i] = vy[i] * 0.985 - 0.015 * dt;
        } else if (k === BURST) {
          vx[i] *= 0.94;
          vy[i] = vy[i] * 0.94 + 0.02 * dt;
        } else if (k === HALO) {
          if (!sel) {
            kill(i);
            continue;
          }
          // Run around the selected row's outline.
          const per = 2 * (sel.row.width + sel.row.height);
          const pos = (Math.abs(seed[i]) * 37 + t0 * (60 + (Math.abs(seed[i]) % 40))) % per;
          let gx: number;
          let gy: number;
          if (pos < sel.row.width) [gx, gy] = [sel.row.left + pos, sel.row.top + 1];
          else if (pos < sel.row.width + sel.row.height) [gx, gy] = [sel.row.right - 1, sel.row.top + pos - sel.row.width];
          else if (pos < 2 * sel.row.width + sel.row.height) [gx, gy] = [sel.row.right - (pos - sel.row.width - sel.row.height), sel.row.bottom - 1];
          else [gx, gy] = [sel.row.left + 1, sel.row.bottom - (pos - 2 * sel.row.width - sel.row.height)];
          vx[i] = vx[i] * 0.8 + (gx - x[i]) * 0.05 * dt;
          vy[i] = vy[i] * 0.8 + (gy - y[i]) * 0.05 * dt;
        }
        // Shy of the pointer.
        if (k !== HALO) {
          const px = x[i] - pointer.x;
          const py = y[i] - pointer.y;
          const p2 = px * px + py * py;
          if (p2 < 8100) {
            const p = Math.sqrt(p2) + 1;
            const s = (1 - p / 90) * 0.9;
            vx[i] += (px / p) * s * dt;
            vy[i] += (py / p) * s * dt;
          }
        }
        x[i] += vx[i] * dt;
        y[i] += vy[i] * dt;
        if (k === AMBIENT) {
          if (x[i] < -20) x[i] = w + 20;
          else if (x[i] > w + 20) x[i] = -20;
          if (y[i] < -20) y[i] = h + 20;
          else if (y[i] > h + 20) y[i] = -20;
        }
      }

      // Draw: additive light, behind or in front of the panels.
      for (const c of [back, fore]) {
        c.setTransform(dpr, 0, 0, dpr, 0, 0);
        c.clearRect(0, 0, w, h);
        c.globalCompositeOperation = "lighter";
      }
      for (let i = 0; i < count; i++) {
        const ctx = near[i] ? fore : back;
        const k = kind[i];
        const age = 1 - life[i] / span[i];
        let a: number;
        if (k === AMBIENT) a = Math.min(1, age * 8, (life[i] / span[i]) * 8) * (seed[i] < 0 ? 0.13 + 0.06 * Math.sin(t0 * 0.9 + seed[i]) : 0.4 + 0.3 * Math.sin(t0 * 1.7 + seed[i]));
        else if (k === STREAM) a = Math.min(1, age * 30) * 0.95;
        else if (k === HALO) a = 0.55 + 0.45 * Math.sin(t0 * 4 + seed[i]);
        else a = Math.min(1, (life[i] / span[i]) * 1.6);
        if (a <= 0.01) continue;
        ctx.globalAlpha = a;
        const s = size[i] * (k === STREAM ? 9 : k === AMBIENT ? 7 : 8);
        ctx.drawImage(sprites[color[i]], x[i] - s / 2, y[i] - s / 2, s, s);
        // Streams and bursts leave a comet's tail.
        if (k === STREAM || k === BURST || k === UPLOAD) {
          for (let q = 1; q <= 3; q++) {
            ctx.globalAlpha = a * (0.42 - q * 0.11);
            const ts = s * (1 - q * 0.2);
            ctx.drawImage(sprites[color[i]], x[i] - vx[i] * 1.6 * q - ts / 2, y[i] - vy[i] * 1.6 * q - ts / 2, ts, ts);
          }
        }
      }
      for (const c of [back, fore]) {
        c.globalAlpha = 1;
        c.globalCompositeOperation = "source-over";
      }
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", resize);
      window.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerleave", onLeave);
      back.clearRect(0, 0, cv.width, cv.height);
      fore.clearRect(0, 0, fv.width, fv.height);
    };
  }, [reduced]);

  if (reduced) return null;
  return (
    <>
      <canvas ref={canvas} className="swarm-canvas" aria-hidden="true" />
      <canvas ref={front} className="swarm-canvas swarm-front" aria-hidden="true" />
    </>
  );
}
