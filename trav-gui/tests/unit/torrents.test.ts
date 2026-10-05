import { describe, expect, it } from "vitest";
import { FILTER_TESTS, isPausedLike, sortTorrents, statusLabel, visibleTorrents } from "@/lib/torrents";
import { torrent } from "./fixtures";

const list = [
  torrent({ infoHash: "1".repeat(40), name: "Bravo", queuePosition: 1, downloadRate: 10, size: 300 }),
  torrent({ infoHash: "2".repeat(40), name: "alpha", queuePosition: 0, status: "seeding", progress: 1, uploadRate: 5, size: 100 }),
  torrent({ infoHash: "3".repeat(40), name: "Charlie", queuePosition: 2, status: "paused", size: 200 }),
  torrent({ infoHash: "4".repeat(40), name: "delta", queuePosition: 3, status: "error", error: "disk full", size: 50 }),
  torrent({ infoHash: "5".repeat(40), name: "echo", queuePosition: 4, status: "metadata", hasMetadata: false, progress: 0 }),
];
const names = (xs: { name: string }[]) => xs.map((t) => t.name);

describe("statusLabel", () => {
  it("maps every status and detects stalls", () => {
    expect(statusLabel(torrent()).label).toBe("Stalled");
    expect(statusLabel(torrent({ peers: 1 }))).toEqual({ label: "Downloading", tone: "down" });
    expect(statusLabel(torrent({ status: "metadata" })).label).toBe("Fetching info");
    expect(statusLabel(torrent({ status: "error" })).tone).toBe("err");
    expect(statusLabel(torrent({ status: "finished" })).tone).toBe("seed-dim");
  });
});

describe("sortTorrents", () => {
  it("sorts by name case-insensitively in both directions", () => {
    expect(names(sortTorrents(list, { key: "name", dir: 1 }))).toEqual(["alpha", "Bravo", "Charlie", "delta", "echo"]);
    expect(names(sortTorrents(list, { key: "name", dir: -1 }))[0]).toBe("echo");
  });

  it("defaults to queue order and ranks active statuses first", () => {
    expect(names(sortTorrents(list, { key: "queue", dir: 1 }))).toEqual(["alpha", "Bravo", "Charlie", "delta", "echo"]);
    expect(names(sortTorrents(list, { key: "status", dir: 1 }))[0]).toBe("Bravo");
  });

  it("puts unknown ETAs last and does not mutate input", () => {
    const xs = [torrent({ name: "slow", eta: 500 }), torrent({ name: "never", eta: null }), torrent({ name: "fast", eta: 5 })];
    const before = names(xs);
    expect(names(sortTorrents(xs, { key: "eta", dir: 1 }))).toEqual(["fast", "slow", "never"]);
    expect(names(xs)).toEqual(before);
  });
});

describe("filters", () => {
  it("partition the library as the sidebar shows it", () => {
    expect(list.filter(FILTER_TESTS.downloading).map((t) => t.name)).toEqual(["Bravo", "echo"]);
    expect(list.filter(FILTER_TESTS.seeding).map((t) => t.name)).toEqual(["alpha"]);
    expect(list.filter(FILTER_TESTS.completed).map((t) => t.name)).toEqual(["alpha"]);
    expect(list.filter(FILTER_TESTS.active).map((t) => t.name)).toEqual(["Bravo", "alpha"]);
    expect(list.filter(FILTER_TESTS.paused).map((t) => t.name)).toEqual(["Charlie"]);
    expect(list.filter(FILTER_TESTS.error).map((t) => t.name)).toEqual(["delta"]);
  });

  it("visibleTorrents combines filter, search by name or hash prefix, and sort", () => {
    expect(names(visibleTorrents(list, "all", "  ALP ", { key: "queue", dir: 1 }))).toEqual(["alpha"]);
    expect(names(visibleTorrents(list, "all", "333", { key: "queue", dir: 1 }))).toEqual(["Charlie"]);
    expect(names(visibleTorrents(list, "downloading", "", { key: "name", dir: -1 }))).toEqual(["echo", "Bravo"]);
  });

  it("isPausedLike covers resumable states", () => {
    expect(["paused", "finished", "error"].every((s) => isPausedLike(torrent({ status: s as never })))).toBe(true);
    expect(isPausedLike(torrent({ status: "seeding" }))).toBe(false);
  });
});
