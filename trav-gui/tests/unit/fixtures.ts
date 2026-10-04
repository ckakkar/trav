import type { TorrentSummary } from "@/lib/types";

export function torrent(over: Partial<TorrentSummary> = {}): TorrentSummary {
  return {
    infoHash: "a".repeat(40),
    name: "ubuntu.iso",
    status: "downloading",
    progress: 0.5,
    size: 1000,
    totalSize: 1000,
    doneBytes: 500,
    downloaded: 500,
    uploaded: 0,
    downloadRate: 0,
    uploadRate: 0,
    eta: null,
    ratio: 0,
    peers: 0,
    seeds: 0,
    swarmPeers: null,
    swarmSeeds: null,
    addedAt: 1_700_000_000,
    completedAt: null,
    savePath: "/dl",
    error: null,
    queuePosition: 0,
    hasMetadata: true,
    sequential: false,
    private: false,
    availability: 1,
    ...over,
  };
}
