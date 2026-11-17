"use client";

// Each register moves like its material. Ink glides and comes into focus;
// Paper is set down with a little snap, like a sheet off the press; Phosphor
// draws itself line by line and collapses like a CRT switching off. Pinstripe
// lowers sheets from the title bar, then lets small controls glide into place.
// Bevel and Bitmap never glide: windows zoom open in a few hard steps and
// highlights jump. Hologram materialises: a line of light unfolds and flickers.
// Textmode and Teletext redraw a row at a time. Instrument is quick and exact.
// Swarm gathers out of a haze. Departures flips like a flap on a
// station board; Receipt feeds out of the printer a line at a time; Comic pops
// in with a bounce and a tilt.

import { useSyncExternalStore } from "react";
import { useReducedMotion, type TargetAndTransition, type Transition } from "motion/react";

export type Register =
  | "ink"
  | "paper"
  | "phosphor"
  | "pinstripe"
  | "bevel"
  | "bitmap"
  | "hologram"
  | "textmode"
  | "teletext"
  | "instrument"
  | "swarm"
  | "departures"
  | "receipt"
  | "comic";

const REGISTERS: readonly string[] = ["paper", "phosphor", "pinstripe", "bevel", "bitmap", "hologram", "textmode", "teletext", "instrument", "swarm", "departures", "receipt", "comic"];

function current(): Register {
  const t = typeof document === "undefined" ? "ink" : document.documentElement.dataset.theme;
  return t && REGISTERS.includes(t) ? (t as Register) : "ink";
}

function subscribe(cb: () => void) {
  const mo = new MutationObserver(cb);
  mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => mo.disconnect();
}

/** The active register, following theme changes. */
export function useRegister(): Register {
  return useSyncExternalStore(subscribe, current, () => "ink");
}

export const out = [0.17, 0.84, 0.44, 1] as const;

interface Choreo {
  initial: TargetAndTransition;
  animate: TargetAndTransition;
  exit: TargetAndTransition;
  transition: Transition;
}

const inkSpring: Transition = { type: "spring", stiffness: 420, damping: 34, mass: 0.8 };
const paperSnap: Transition = { type: "spring", stiffness: 900, damping: 30, mass: 0.7 };
/** An easing that moves in `n` hard steps, as old window managers drew. */
const stepped = (n: number) => (t: number) => Math.ceil(t * n) / n;
const instant: Transition = { duration: 0 };

/** With Reduce Motion on, every register just fades: no zooms, wipes or blurs. */
const FADE: Choreo = {
  initial: { opacity: 0 },
  animate: { opacity: 1 },
  exit: { opacity: 0, transition: { duration: 0.1 } },
  transition: { duration: 0.12, ease: "linear" },
};

/** The active register's choreography from `table`, or a plain fade under Reduce Motion. */
export function useChoreo(table: Record<Register, Choreo>): Choreo {
  const reg = useRegister();
  return useReducedMotion() ? FADE : table[reg];
}

/** Dialogs and the command palette. */
export const DIALOG: Record<Register, Choreo> = {
  ink: {
    initial: { opacity: 0, y: 18, scale: 0.965, filter: "blur(10px)" },
    animate: { opacity: 1, y: 0, scale: 1, filter: "blur(0px)" },
    exit: { opacity: 0, y: 6, scale: 0.98, filter: "blur(6px)", transition: { duration: 0.18, ease: out } },
    transition: inkSpring,
  },
  paper: {
    initial: { opacity: 0, y: -28, rotate: -1.4 },
    animate: { opacity: 1, y: 0, rotate: 0 },
    exit: { opacity: 0, y: 14, rotate: 0.6, transition: { duration: 0.14, ease: "easeIn" } },
    transition: paperSnap,
  },
  pinstripe: {
    initial: { opacity: 1, y: "-100%" },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, y: "-100%", transition: { duration: 0.24, ease: [0.4, 0, 0.6, 1] } },
    transition: { duration: 0.4, ease: [0.18, 0.78, 0.28, 1] },
  },
  phosphor: {
    initial: { opacity: 0, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, clipPath: "inset(50% 0 50% 0)", transition: { duration: 0.16, ease: "easeIn" } },
    transition: { duration: 0.3, ease: "linear" },
  },
  bevel: {
    initial: { opacity: 1, scale: 0.35 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.16, ease: stepped(4) },
  },
  bitmap: {
    initial: { opacity: 1, scale: 0.12 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.24, ease: stepped(6) },
  },
  hologram: {
    initial: { opacity: 0, scaleX: 0.5, scaleY: 0.01 },
    animate: { opacity: [0, 1, 0.45, 1], scaleX: [0.5, 1, 1, 1], scaleY: [0.01, 0.01, 1, 1] },
    exit: { opacity: 0, scaleY: 0.01, transition: { duration: 0.16, ease: "easeIn" } },
    transition: { duration: 0.44, times: [0, 0.32, 0.72, 1], ease: "easeOut" },
  },
  textmode: {
    initial: { opacity: 1, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.16, ease: stepped(8) },
  },
  teletext: {
    initial: { opacity: 1, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.42, ease: stepped(12) },
  },
  instrument: {
    initial: { opacity: 0, y: 10 },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, transition: { duration: 0.1 } },
    transition: { duration: 0.2, ease: [0.2, 0, 0, 1] },
  },
  swarm: {
    initial: { opacity: 0, scale: 0.94, filter: "blur(12px)" },
    animate: { opacity: 1, scale: 1, filter: "blur(0px)" },
    exit: { opacity: 0, scale: 1.04, filter: "blur(10px)", transition: { duration: 0.2 } },
    transition: { duration: 0.45, ease: out },
  },
  departures: {
    initial: { opacity: 1, rotateX: -88, transformPerspective: 900, transformOrigin: "50% 0%" },
    animate: { opacity: 1, rotateX: 0, transformPerspective: 900, transformOrigin: "50% 0%" },
    exit: { opacity: 0, rotateX: 60, transition: { duration: 0.14, ease: "easeIn" } },
    transition: { duration: 0.32, ease: stepped(7) },
  },
  receipt: {
    initial: { opacity: 1, y: -24, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, y: 0, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, y: 12, transition: { duration: 0.14, ease: "easeIn" } },
    transition: { duration: 0.42, ease: stepped(14) },
  },
  comic: {
    initial: { opacity: 0, scale: 0.55, rotate: -6 },
    animate: { opacity: 1, scale: 1, rotate: -0.6 },
    exit: { opacity: 0, scale: 0.8, rotate: 4, transition: { duration: 0.14, ease: "easeIn" } },
    transition: { type: "spring", stiffness: 520, damping: 13, mass: 0.8 },
  },
};

/** Context menus: out of the pointer. */
export const MENU: Record<Register, Choreo> = {
  ink: {
    initial: { opacity: 0, scale: 0.94, filter: "blur(4px)" },
    animate: { opacity: 1, scale: 1, filter: "blur(0px)" },
    exit: { opacity: 0, scale: 0.97, transition: { duration: 0.1 } },
    transition: { type: "spring", stiffness: 620, damping: 36 },
  },
  paper: {
    initial: { opacity: 1, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: { duration: 0.08 } },
    transition: { duration: 0.16, ease: out },
  },
  pinstripe: {
    initial: { opacity: 0, scale: 0.96 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, scale: 0.98, transition: { duration: 0.12 } },
    transition: { type: "spring", stiffness: 460, damping: 30, mass: 0.7 },
  },
  phosphor: {
    initial: { opacity: 0, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: { duration: 0.08 } },
    transition: { duration: 0.18, ease: "linear" },
  },
  bevel: {
    initial: { opacity: 1, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.12, ease: "linear" },
  },
  bitmap: {
    initial: { opacity: 1 },
    animate: { opacity: 1 },
    exit: { opacity: 0, transition: instant },
    transition: instant,
  },
  hologram: {
    initial: { opacity: 0, clipPath: "inset(0 50% 0 50%)" },
    animate: { opacity: [0, 1, 0.6, 1], clipPath: "inset(0 0% 0 0%)" },
    exit: { opacity: 0, transition: { duration: 0.1 } },
    transition: { duration: 0.24, ease: "easeOut" },
  },
  textmode: {
    initial: { opacity: 1, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.1, ease: stepped(5) },
  },
  teletext: {
    initial: { opacity: 1, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.2, ease: stepped(6) },
  },
  instrument: {
    initial: { opacity: 0, y: -4 },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, transition: { duration: 0.08 } },
    transition: { duration: 0.14, ease: [0.2, 0, 0, 1] },
  },
  swarm: {
    initial: { opacity: 0, scale: 0.92, filter: "blur(6px)" },
    animate: { opacity: 1, scale: 1, filter: "blur(0px)" },
    exit: { opacity: 0, filter: "blur(6px)", transition: { duration: 0.12 } },
    transition: { duration: 0.26, ease: out },
  },
  departures: {
    initial: { opacity: 1, rotateX: -80, transformPerspective: 700, transformOrigin: "50% 0%" },
    animate: { opacity: 1, rotateX: 0, transformPerspective: 700, transformOrigin: "50% 0%" },
    exit: { opacity: 0, transition: { duration: 0.08 } },
    transition: { duration: 0.2, ease: stepped(5) },
  },
  receipt: {
    initial: { opacity: 1, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: { duration: 0.08 } },
    transition: { duration: 0.22, ease: stepped(9) },
  },
  comic: {
    initial: { opacity: 0, scale: 0.7, rotate: 3 },
    animate: { opacity: 1, scale: 1, rotate: 0 },
    exit: { opacity: 0, scale: 0.9, transition: { duration: 0.1 } },
    transition: { type: "spring", stiffness: 700, damping: 15 },
  },
};

/** Toasts: arrive at the corner, leave to the side. */
export const TOAST: Record<Register, Choreo> = {
  ink: {
    initial: { opacity: 0, x: 40, scale: 0.96, filter: "blur(6px)" },
    animate: { opacity: 1, x: 0, scale: 1, filter: "blur(0px)" },
    exit: { opacity: 0, x: 40, transition: { duration: 0.2, ease: out } },
    transition: inkSpring,
  },
  paper: {
    initial: { opacity: 0, y: -18, rotate: 2.2 },
    animate: { opacity: 1, y: 0, rotate: -0.6 },
    exit: { opacity: 0, y: 10, transition: { duration: 0.14 } },
    transition: paperSnap,
  },
  pinstripe: {
    initial: { opacity: 0, x: 32 },
    animate: { opacity: 1, x: 0 },
    exit: { opacity: 0, x: 20, transition: { duration: 0.18 } },
    transition: { type: "spring", stiffness: 270, damping: 28, mass: 1 },
  },
  phosphor: {
    initial: { opacity: 0, clipPath: "inset(0 100% 0 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0% 0 0)" },
    exit: { opacity: 0, transition: { duration: 0.12 } },
    transition: { duration: 0.32, ease: "linear" },
  },
  bevel: {
    initial: { opacity: 1 },
    animate: { opacity: 1 },
    exit: { opacity: 0, transition: instant },
    transition: instant,
  },
  bitmap: {
    initial: { opacity: 1, scale: 0.2 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.16, ease: stepped(4) },
  },
  hologram: {
    initial: { opacity: 0, x: 28, skewX: -12 },
    animate: { opacity: [0, 1, 0.5, 1], x: 0, skewX: 0 },
    exit: { opacity: 0, x: 16, transition: { duration: 0.16 } },
    transition: { duration: 0.36, ease: "easeOut" },
  },
  textmode: {
    initial: { opacity: 1 },
    animate: { opacity: 1 },
    exit: { opacity: 0, transition: instant },
    transition: instant,
  },
  teletext: {
    initial: { opacity: 1, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: instant },
    transition: { duration: 0.2, ease: stepped(4) },
  },
  instrument: {
    initial: { opacity: 0, x: 18 },
    animate: { opacity: 1, x: 0 },
    exit: { opacity: 0, x: 10, transition: { duration: 0.12 } },
    transition: { duration: 0.18, ease: [0.2, 0, 0, 1] },
  },
  swarm: {
    initial: { opacity: 0, x: 30, filter: "blur(8px)" },
    animate: { opacity: 1, x: 0, filter: "blur(0px)" },
    exit: { opacity: 0, filter: "blur(8px)", transition: { duration: 0.2 } },
    transition: { duration: 0.4, ease: out },
  },
  departures: {
    initial: { opacity: 1, rotateX: -90, transformPerspective: 600, transformOrigin: "50% 0%" },
    animate: { opacity: 1, rotateX: 0, transformPerspective: 600, transformOrigin: "50% 0%" },
    exit: { opacity: 0, rotateX: 90, transition: { duration: 0.16, ease: stepped(4) } },
    transition: { duration: 0.28, ease: stepped(6) },
  },
  receipt: {
    initial: { opacity: 1, y: "-110%" },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, y: -16, transition: { duration: 0.16 } },
    transition: { duration: 0.5, ease: stepped(10) },
  },
  comic: {
    initial: { opacity: 0, scale: 0.2, rotate: -12, transformOrigin: "0% 100%" },
    animate: { opacity: 1, scale: 1, rotate: -1.5, transformOrigin: "0% 100%" },
    exit: { opacity: 0, scale: 0.6, transition: { duration: 0.14 } },
    transition: { type: "spring", stiffness: 600, damping: 12, mass: 0.7 },
  },
};

/** The shared-layout highlights (active filter, selected tab). */
export const SLIDE: Record<Register, Transition> = {
  ink: { type: "spring", stiffness: 520, damping: 40, mass: 0.9 },
  paper: { duration: 0.26, ease: [0.6, 0.05, 0.2, 1] },
  pinstripe: { type: "spring", stiffness: 230, damping: 28, mass: 1.1 },
  phosphor: { duration: 0.12, ease: "linear" },
  bevel: instant,
  bitmap: instant,
  hologram: { type: "spring", stiffness: 700, damping: 38, mass: 0.6 },
  textmode: instant,
  teletext: instant,
  instrument: { duration: 0.16, ease: [0.2, 0, 0, 1] },
  swarm: { type: "spring", stiffness: 300, damping: 30, mass: 0.9 },
  departures: { duration: 0.2, ease: stepped(4) },
  receipt: { duration: 0.18, ease: "linear" },
  comic: { type: "spring", stiffness: 520, damping: 14, mass: 0.7 },
};
