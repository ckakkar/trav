// Three registers: Ink (night instrument), Paper (risograph print), Phosphor (terminal).

export type Theme = "ink" | "paper" | "phosphor";

export const THEMES: { id: Theme; label: string; bg: string; line: string; accent: string; text: string }[] = [
  { id: "ink", label: "Ink", bg: "#04070d", line: "#1c2537", accent: "#56c7ff", text: "#e3eaf4" },
  { id: "paper", label: "Paper", bg: "#f9f8f4", line: "#1d2873", accent: "#ff3d8a", text: "#1d2873" },
  { id: "phosphor", label: "Phosphor", bg: "#000000", line: "#123a20", accent: "#6af59a", text: "#45cf6c" },
];

/**
 * How each register arrives: Ink blooms out from where you clicked, Paper is
 * rolled on left to right like a print pass, Phosphor scans down the screen.
 */
function reveal(theme: Theme, origin: { x: number; y: number }): { clipPath: string[]; duration: number; easing: string } {
  if (theme === "paper") {
    return { clipPath: ["inset(0 100% 0 0)", "inset(0 0% 0 0)"], duration: 620, easing: "cubic-bezier(.6,.05,.2,1)" };
  }
  if (theme === "phosphor") {
    return { clipPath: ["inset(0 0 100% 0)", "inset(0 0 0% 0)"], duration: 460, easing: "linear" };
  }
  const r = Math.hypot(Math.max(origin.x, innerWidth - origin.x), Math.max(origin.y, innerHeight - origin.y));
  return {
    clipPath: [`circle(0px at ${origin.x}px ${origin.y}px)`, `circle(${r}px at ${origin.x}px ${origin.y}px)`],
    duration: 640,
    easing: "cubic-bezier(.17,.84,.44,1)",
  };
}

/** Apply a theme with its own transition where supported (from the window's center without a pointer). */
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
  const { clipPath, duration, easing } = reveal(theme, origin ?? { x: innerWidth / 2, y: innerHeight / 2 });
  const t = doc.startViewTransition(swap);
  t.ready
    .then(() => root.animate({ clipPath }, { duration, easing, pseudoElement: "::view-transition-new(root)" }).finished)
    .catch(() => {});
  t.finished.finally(settle);
}
