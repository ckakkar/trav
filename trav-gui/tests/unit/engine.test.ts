import { describe, expect, it } from "vitest";
import { SpeedHistory } from "@/lib/engine";
import type { EngineSnapshot } from "@/lib/types";
import { torrent } from "./fixtures";

const snap = (hashes: string[], rate = 100): EngineSnapshot => ({
  seq: 1,
  torrents: hashes.map((h) => torrent({ infoHash: h, downloadRate: rate })),
  stats: {
    downloadRate: rate * hashes.length,
    uploadRate: 0,
    sessionDownloaded: 0,
    sessionUploaded: 0,
    downloadLimit: 0,
    uploadLimit: 0,
    connectedPeers: 0,
    dhtNodes: 0,
    listenPort: 1,
    connectable: false,
    freeSpace: null,
    upnp: null,
  },
});

describe("SpeedHistory", () => {
  it("keeps a bounded ring and drops removed torrents", () => {
    const h = new SpeedHistory();
    for (let i = 0; i < SpeedHistory.MAX + 50; i++) h.push(snap(["a", "b"], i));
    expect(h.global).toHaveLength(SpeedHistory.MAX);
    expect(h.global.at(-1)!.down).toBe((SpeedHistory.MAX + 49) * 2);
    expect(h.perTorrent.get("a")).toHaveLength(SpeedHistory.MAX);
    h.push(snap(["a"]));
    expect(h.perTorrent.has("b")).toBe(false);
  });
});
