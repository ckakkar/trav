// Fourteen registers. Ink, Paper and Phosphor; then the retro ones (Pinstripe,
// Bevel, Meadow, Bitmap, Textmode, Teletext, Hi-Fi) and the futuristic ones
// (Hologram, Glass, Instrument, Quantum).

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
  | "hifi"
  | "glass"
  | "instrument"
  | "quantum"
  | "meadow";

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
  { id: "hifi", label: "Hi-Fi", bg: "#0d0a08", line: "#b9b4ab", accent: "#ffa53a", text: "#f6dcb0", dark: true },
  { id: "glass", label: "Glass", bg: "#e9e6f4", line: "#ffffff", accent: "#4f46e5", text: "#14131c", dark: false },
  { id: "instrument", label: "Instrument", bg: "#e8e6e0", line: "#c6c2b8", accent: "#ff4f12", text: "#151514", dark: false },
  { id: "quantum", label: "Quantum", bg: "#020206", line: "#1c1c2c", accent: "#b9a8ff", text: "#ecebf5", dark: true },
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
