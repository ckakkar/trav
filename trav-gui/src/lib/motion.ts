"use client";

// Each register moves like its material. Ink glides and comes into focus;
// Paper is set down with a little snap, like a sheet off the press; Phosphor
// draws itself line by line and collapses like a CRT switching off.

import { useSyncExternalStore } from "react";
import type { TargetAndTransition, Transition } from "motion/react";

export type Register = "ink" | "paper" | "phosphor";

function current(): Register {
  const t = typeof document === "undefined" ? "ink" : document.documentElement.dataset.theme;
  return t === "paper" || t === "phosphor" ? t : "ink";
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
  phosphor: {
    initial: { opacity: 0, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, clipPath: "inset(50% 0 50% 0)", transition: { duration: 0.16, ease: "easeIn" } },
    transition: { duration: 0.3, ease: "linear" },
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
  phosphor: {
    initial: { opacity: 0, clipPath: "inset(0 0 100% 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0 0% 0)" },
    exit: { opacity: 0, transition: { duration: 0.08 } },
    transition: { duration: 0.18, ease: "linear" },
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
  phosphor: {
    initial: { opacity: 0, clipPath: "inset(0 100% 0 0)" },
    animate: { opacity: 1, clipPath: "inset(0 0% 0 0)" },
    exit: { opacity: 0, transition: { duration: 0.12 } },
    transition: { duration: 0.32, ease: "linear" },
  },
};

/** The shared-layout highlights (active filter, selected tab). */
export const SLIDE: Record<Register, Transition> = {
  ink: { type: "spring", stiffness: 520, damping: 40, mass: 0.9 },
  paper: { duration: 0.26, ease: [0.6, 0.05, 0.2, 1] },
  phosphor: { duration: 0.12, ease: "linear" },
};
