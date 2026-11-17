import { useSyncExternalStore } from "react";

// Fourteen registers. Ink, Paper and Phosphor; the retro ones (Pinstripe,
// Bevel, Meadow, Bitmap, Textmode, Teletext); the futuristic ones (Hologram,
// Instrument); and found objects, where the window becomes something else
// entirely (a split-flap departure board, a till receipt, a comic page).

export type Theme =
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
  | "meadow"
  | "departures"
  | "receipt"
  | "comic";

/** `dark`: which half of the Mac's appearance a theme suits, for "Match the Mac". */
export const THEMES: { id: Theme; label: string; bg: string; line: string; accent: string; text: string; dark: boolean }[] = [
  { id: "ink", label: "Ink", bg: "#04070d", line: "#1c2537", accent: "#56c7ff", text: "#e3eaf4", dark: true },
  { id: "paper", label: "Paper", bg: "#f9f8f4", line: "#1d2873", accent: "#ff3d8a", text: "#1d2873", dark: false },
  { id: "phosphor", label: "Phosphor", bg: "#000000", line: "#123a20", accent: "#6af59a", text: "#45cf6c", dark: true },
  { id: "pinstripe", label: "Pinstripe", bg: "#f1f2f4", line: "#9ba3ae", accent: "#1466bb", text: "#26313e", dark: false },
  { id: "bevel", label: "Bevel", bg: "#c0c0c0", line: "#808080", accent: "#000080", text: "#000000", dark: false },
  { id: "meadow", label: "Meadow", bg: "#ece9d8", line: "#0a246a", accent: "#316ac5", text: "#000000", dark: false },
  { id: "bitmap", label: "Bitmap", bg: "#ffffff", line: "#000000", accent: "#000000", text: "#000000", dark: false },
  { id: "hologram", label: "Hologram", bg: "#05030c", line: "#2e2660", accent: "#ff5fd2", text: "#eeebff", dark: true },
  { id: "textmode", label: "Textmode", bg: "#0000aa", line: "#55ffff", accent: "#ffff55", text: "#aaaaaa", dark: true },
  { id: "teletext", label: "Teletext", bg: "#000000", line: "#0000ff", accent: "#ffff00", text: "#ffffff", dark: true },
  { id: "instrument", label: "Instrument", bg: "#e8e6e0", line: "#c6c2b8", accent: "#ff4f12", text: "#151514", dark: false },
  { id: "departures", label: "Departures", bg: "#111214", line: "#2a2c30", accent: "#ffcc1a", text: "#f2efe6", dark: true },
  { id: "receipt", label: "Receipt", bg: "#fbf8f1", line: "#2b2b2b", accent: "#d6332a", text: "#232323", dark: false },
  { id: "comic", label: "Comic", bg: "#fff6d6", line: "#111111", accent: "#e8178a", text: "#111111", dark: false },
];

/** Every theme arrives as a bloom from where you clicked (the window's center without a pointer). */
function reveal(origin: { x: number; y: number }): { clipPath: string[]; duration: number; easing: string } {
  const r = Math.hypot(Math.max(origin.x, innerWidth - origin.x), Math.max(origin.y, innerHeight - origin.y));
  return {
    clipPath: [`circle(0px at ${origin.x}px ${origin.y}px)`, `circle(${r}px at ${origin.x}px ${origin.y}px)`],
    duration: 640,
    easing: "cubic-bezier(.17,.84,.44,1)",
  };
}

/** Apply a theme, blooming it in where supported. */
export function applyTheme(theme: Theme, origin?: { x: number; y: number }) {
  const root = document.documentElement;
  if (root.dataset.theme === theme) return;
  // Registers rest in different places (Paper's highlighter is scaled to zero,
  // Ink's capsule is transparent): with transitions on, every row would animate
  // between them. Swap with transitions off, then turn them back on.
  root.dataset.switching = "";
  const settle = () => requestAnimationFrame(() => requestAnimationFrame(() => delete root.dataset.switching));
  const swap = () => {
    root.dataset.theme = theme;
    const meta = document.querySelector('meta[name="theme-color"]');
    meta?.setAttribute("content", THEMES.find((t) => t.id === theme)!.bg);
  };
  const doc = document as Document & { startViewTransition?: (cb: () => void) => { ready: Promise<void>; finished: Promise<void> } };
  const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!doc.startViewTransition || reduce) {
    swap();
    settle();
    return;
  }
  const { clipPath, duration, easing } = reveal(origin ?? { x: innerWidth / 2, y: innerHeight / 2 });
  const t = doc.startViewTransition(swap);
  t.ready
    .then(() => root.animate({ clipPath }, { duration, easing, pseudoElement: "::view-transition-new(root)" }).finished)
    .catch(() => {});
  t.finished.finally(settle);
}

export const isTheme = (t: unknown): t is Theme => THEMES.some((x) => x.id === t);

/** The theme list in Settings, in three families. */
export const THEME_GROUPS: { label: string; ids: Theme[] }[] = [
  { label: "Originals", ids: ["ink", "paper", "phosphor"] },
  { label: "Retro", ids: ["pinstripe", "bevel", "meadow", "bitmap", "textmode", "teletext"] },
  { label: "Future", ids: ["hologram", "instrument"] },
  { label: "Found objects", ids: ["departures", "receipt", "comic"] },
];

/** The theme choice: one theme, or (Match the Mac) one for Light mode and one for Dark. */
export interface ThemeChoice {
  theme: Theme;
  autoTheme?: boolean;
  lightTheme?: Theme;
  darkTheme?: Theme;
}

/** The theme to show for `p` when the Mac is in Dark mode (`dark`) or not. */
export function effectiveTheme(p: ThemeChoice, dark: boolean): Theme {
  const t = p.autoTheme ? (dark ? (p.darkTheme ?? "ink") : (p.lightTheme ?? "paper")) : p.theme;
  return isTheme(t) ? t : "ink";
}

/** Pick `t`. With Match the Mac on, it becomes the theme for the current appearance. */
export function chooseTheme<P extends ThemeChoice>(p: P, t: Theme, dark: boolean): P {
  if (!p.autoTheme) return { ...p, theme: t };
  return dark ? { ...p, darkTheme: t } : { ...p, lightTheme: t };
}

/** Turn Match the Mac on or off, filling in both themes from the one in use. */
export function matchTheMac<P extends ThemeChoice>(p: P, on: boolean, dark: boolean): P {
  const now = effectiveTheme(p, dark);
  if (!on) return { ...p, autoTheme: false, theme: now };
  const nowDark = THEMES.find((t) => t.id === now)?.dark ?? true;
  return {
    ...p,
    autoTheme: true,
    lightTheme: p.lightTheme ?? (nowDark ? "paper" : now),
    darkTheme: p.darkTheme ?? (nowDark ? now : "ink"),
  };
}

const DARK_QUERY = "(prefers-color-scheme: dark)";
/** Whether the Mac is in Dark mode right now. */
export const darkNow = () => typeof window !== "undefined" && !!window.matchMedia?.(DARK_QUERY).matches;

/** Whether the Mac is in Dark mode, following changes. */
export function useDarkAppearance(): boolean {
  return useSyncExternalStore(
    (cb) => {
      const mq = window.matchMedia?.(DARK_QUERY);
      mq?.addEventListener("change", cb);
      return () => mq?.removeEventListener("change", cb);
    },
    darkNow,
    () => true,
  );
}
