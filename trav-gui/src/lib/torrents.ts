// Pure, framework-free torrent list logic (sorting, labels, filters). Unit-tested.

import type { TorrentStatus, TorrentSummary } from "./types";

export type SortKey = "queue" | "name" | "size" | "progress" | "status" | "down" | "up" | "eta" | "peers" | "ratio" | "added";
export interface Sort {
  key: SortKey;
  dir: 1 | -1;
}

export function statusLabel(t: TorrentSummary): { label: string; tone: string } {
  const map: Record<TorrentStatus, [string, string]> = {
    downloading: ["Downloading", "down"],
    seeding: ["Seeding", "seed"],
    paused: ["Paused", "idle"],
    queued: ["Queued", "idle"],
    checking: ["Checking", "warn"],
    metadata: ["Fetching info", "warn"],
    finished: ["Finished", "seed-dim"],
    error: ["Error", "err"],
  };
  const [label, tone] = map[t.status];
  if (t.status === "downloading" && t.downloadRate === 0 && t.peers + t.seeds === 0) return { label: "Stalled", tone: "warn" };
  return { label, tone };
}

const statusRank: Record<TorrentStatus, number> = {
  downloading: 0,
  metadata: 1,
  checking: 2,
  seeding: 3,
  queued: 4,
  finished: 5,
  paused: 6,
  error: 7,
};

export function sortTorrents(list: TorrentSummary[], s: Sort): TorrentSummary[] {
  const key = (t: TorrentSummary): number | string => {
    switch (s.key) {
      case "queue":
        return t.queuePosition;
      case "name":
        return t.name.toLowerCase();
      case "size":
        return t.size;
      case "progress":
        return t.progress;
      case "status":
        return statusRank[t.status];
      case "down":
        return t.downloadRate;
      case "up":
        return t.uploadRate;
      case "eta":
        return t.eta ?? Number.MAX_SAFE_INTEGER;
      case "peers":
        return t.peers + t.seeds;
      case "ratio":
        return t.ratio;
      case "added":
        return t.addedAt;
    }
  };
  return [...list].sort((a, b) => {
    const ka = key(a);
    const kb = key(b);
    const c = ka < kb ? -1 : ka > kb ? 1 : a.queuePosition - b.queuePosition;
    return c * s.dir;
  });
}


export const isPausedLike = (t: TorrentSummary) => t.status === "paused" || t.status === "finished" || t.status === "error";

export type FilterId = "all" | "downloading" | "seeding" | "completed" | "active" | "paused" | "error";

export const FILTER_TESTS: Record<FilterId, (t: TorrentSummary) => boolean> = {
  all: () => true,
  downloading: (t) => !(t.hasMetadata && t.progress >= 1) && !["paused", "error", "finished"].includes(t.status),
  seeding: (t) => t.status === "seeding",
  completed: (t) => t.hasMetadata && t.progress >= 1,
  active: (t) => t.downloadRate + t.uploadRate > 0,
  paused: (t) => t.status === "paused" || t.status === "finished",
  error: (t) => t.status === "error",
};

/** Filter + search (name substring or info-hash prefix) + sort, as the main table shows it. */
export function visibleTorrents(list: TorrentSummary[], filter: FilterId, search: string, sort: Sort): TorrentSummary[] {
  const q = search.trim().toLowerCase();
  const test = FILTER_TESTS[filter] ?? FILTER_TESTS.all;
  return sortTorrents(
    list.filter((t) => test(t) && (!q || t.name.toLowerCase().includes(q) || t.infoHash.startsWith(q))),
    sort,
  );
}
