// The three registers of kkrwhofrags.xyz: dark ink, editorial paper, terminal phosphor.

export type Theme = "ink" | "paper" | "phosphor";

export const THEMES: { id: Theme; label: string; bg: string; line: string; accent: string; text: string }[] = [
  { id: "ink", label: "Ink", bg: "#0c0d10", line: "#23262d", accent: "#ff8a3d", text: "#e7e8ea" },
  { id: "paper", label: "Paper", bg: "#f5f2ea", line: "#ddd7c9", accent: "#b93213", text: "#17150f" },
  { id: "phosphor", label: "Phosphor", bg: "#000000", line: "#123a20", accent: "#6af59a", text: "#45cf6c" },
];

/** Apply a theme, revealing it with a circular wipe from (x, y) where supported. */
export function applyTheme(theme: Theme, origin?: { x: number; y: number }) {
  const root = document.documentElement;
  if (root.dataset.theme === theme) return;
  const swap = () => {
    root.dataset.theme = theme;
    const meta = document.querySelector('meta[name="theme-color"]');
    meta?.setAttribute("content", THEMES.find((t) => t.id === theme)!.bg);
  };
  const doc = document as Document & { startViewTransition?: (cb: () => void) => { ready: Promise<void> } };
  const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!doc.startViewTransition || reduce || !origin) {
    swap();
    return;
  }
  const r = Math.hypot(Math.max(origin.x, innerWidth - origin.x), Math.max(origin.y, innerHeight - origin.y));
  const t = doc.startViewTransition(swap);
  t.ready
    .then(() =>
      root.animate(
        { clipPath: [`circle(0px at ${origin.x}px ${origin.y}px)`, `circle(${r}px at ${origin.x}px ${origin.y}px)`] },
        { duration: 560, easing: "cubic-bezier(.17,.84,.44,1)", pseudoElement: "::view-transition-new(root)" },
      ),
    )
    .catch(() => {});
}
