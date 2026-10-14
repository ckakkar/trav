"use client";

import type { GlobalStats } from "@/lib/types";
import type { Link } from "@/lib/engine";
import { bytes, rate } from "@/lib/format";
import { Tween } from "@/lib/tween";

export function StatusBar({ stats, link, onLimits }: { stats: GlobalStats | null; link: Link; onLimits: () => void }) {
  const linkLabel = { connecting: "connecting", online: "engine", offline: "offline" }[link];
  return (
    <footer className="statusbar mono">
      <span className="sb-item" data-link={link}>
        <span className="sdot" />
        {linkLabel}
      </span>
      {stats && (
        <>
          <span className="sb-item" title={stats.connectable ? "Peers can reach you" : "No inbound connections seen yet — forward the port or enable UPnP"}>
            port {stats.listenPort}
            <span className={stats.connectable ? "ok-text" : "faint"}>{stats.connectable ? " ● open" : " ○"}</span>
          </span>
          <span className="sb-item">dht {stats.dhtNodes}</span>
          <span className="sb-item">peers {stats.connectedPeers}</span>
          {stats.freeSpace != null && <span className="sb-item">free {bytes(stats.freeSpace)}</span>}
          <span className="sb-spacer" />
          <button className="sb-item sb-btn" onClick={onLimits} title="Bandwidth limits">
            <span className="accent-text">↓</span> <Tween value={stats.downloadRate} format={rate} />
            {stats.downloadLimit > 0 && <span className="faint"> / {rate(stats.downloadLimit)}</span>}
          </button>
          <button className="sb-item sb-btn" onClick={onLimits} title="Bandwidth limits">
            <span className="up-text">↑</span> <Tween value={stats.uploadRate} format={rate} />
            {stats.uploadLimit > 0 && (
              <span className="faint" title={stats.uploadAuto ? "Automatic limit: keeps uploads from slowing downloads" : undefined}>
                {" "}/ {stats.uploadAuto ? "auto " : ""}
                {rate(stats.uploadLimit)}
              </span>
            )}
          </button>
        </>
      )}
    </footer>
  );
}
