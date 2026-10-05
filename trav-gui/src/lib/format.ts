const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

/** IEC math, short SI-style labels (what every torrent client shows). */
export function bytes(n: number, digits = 1): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const i = Math.min(UNITS.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  const v = n / 1024 ** i;
  if (i === 0) return `${Math.round(v)} B`;
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(digits)} ${UNITS[i]}`;
}

export function rate(n: number): string {
  return n < 1 ? "—" : `${bytes(n)}/s`;
}

/** Split a byte value so the unit can be styled separately. */
export function bytesParts(n: number): [string, string] {
  const [v, u] = bytes(n).split(" ");
  return [v, u ?? ""];
}

export function eta(s: number | null | undefined): string {
  if (s == null || !Number.isFinite(s) || s > 86400 * 365) return "∞";
  if (s >= 86400) return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
  if (s >= 3600) return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}m`;
  if (s >= 60) return `${Math.floor(s / 60)}m ${String(Math.floor(s % 60)).padStart(2, "0")}s`;
  return `${Math.max(0, Math.floor(s))}s`;
}

export function pct(p: number, digits = 1): string {
  const v = Math.max(0, Math.min(1, p)) * 100;
  return v >= 100 ? "100%" : `${v.toFixed(digits)}%`;
}

export function ago(unix: number | null | undefined): string {
  if (!unix) return "—";
  const d = Math.max(0, Date.now() / 1000 - unix);
  if (d < 45) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  if (d < 86400 * 30) return `${Math.round(d / 86400)} d ago`;
  return new Date(unix * 1000).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

export function date(unix: number | null | undefined): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function pad2(i: number): string {
  return String(i).padStart(2, "0");
}

/** Parse "1.5 MB", "800k", "2m" → bytes/sec (0 = unlimited). */
export function parseRate(input: string): number {
  const m = input.trim().toLowerCase().match(/^([\d.]+)\s*([kmg]?)i?b?(\/s)?$/);
  if (!m) return 0;
  const mult = { "": 1024, k: 1024, m: 1024 ** 2, g: 1024 ** 3 }[m[2] as "" | "k" | "m" | "g"];
  return Math.round(parseFloat(m[1]) * mult);
}

export function looksLikeMagnet(s: string): boolean {
  const t = s.trim();
  return /^magnet:\?/i.test(t) || /^[a-f0-9]{40}$/i.test(t);
}
