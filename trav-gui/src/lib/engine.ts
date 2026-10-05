"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { AuthError, call } from "./rpc";
import type { EngineSnapshot, TorrentDetails } from "./types";

export interface Sample {
  t: number;
  down: number;
  up: number;
}

/** Mutable ring buffers read directly by canvas graphs every animation frame. */
export class SpeedHistory {
  static readonly MAX = 360; // 3 minutes at 2 Hz
  global: Sample[] = [];
  perTorrent = new Map<string, Sample[]>();

  push(snap: EngineSnapshot) {
    const t = performance.now();
    pushRing(this.global, { t, down: snap.stats.downloadRate, up: snap.stats.uploadRate });
    const seen = new Set<string>();
    for (const tor of snap.torrents) {
      seen.add(tor.infoHash);
      let ring = this.perTorrent.get(tor.infoHash);
      if (!ring) this.perTorrent.set(tor.infoHash, (ring = []));
      pushRing(ring, { t, down: tor.downloadRate, up: tor.uploadRate });
    }
    for (const k of this.perTorrent.keys()) if (!seen.has(k)) this.perTorrent.delete(k);
  }
}

function pushRing(ring: Sample[], s: Sample) {
  ring.push(s);
  if (ring.length > SpeedHistory.MAX) ring.splice(0, ring.length - SpeedHistory.MAX);
}

export type Link = "connecting" | "online" | "offline" | "auth";

export function useEngine(focus: string | null) {
  const [snapshot, setSnapshot] = useState<EngineSnapshot | null>(null);
  const [details, setDetails] = useState<TorrentDetails | null>(null);
  const [link, setLink] = useState<Link>("connecting");
  const [lastError, setLastError] = useState<string | null>(null);
  const history = useRef(new SpeedHistory()).current;
  const focusRef = useRef(focus);
  const kick = useRef<() => void>(() => {});

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let alive = true;
    let failures = 0;

    const tick = async () => {
      clearTimeout(timer);
      try {
        const snap = await call<EngineSnapshot>("snapshot");
        if (!alive) return;
        setSnapshot(snap);
        history.push(snap);
        const f = focusRef.current;
        if (f && snap.torrents.some((t) => t.infoHash === f)) {
          const d = await call<TorrentDetails | null>("details", { hash: f });
          if (alive && focusRef.current === f) setDetails(d);
        } else {
          setDetails(null);
        }
        failures = 0;
        setLink("online");
        setLastError(null);
      } catch (e) {
        if (!alive) return;
        failures++;
        if (e instanceof AuthError) setLink("auth");
        else if (failures > 2) setLink("offline");
        setLastError(String(e instanceof Error ? e.message : e));
      } finally {
        if (alive) {
          const hidden = typeof document !== "undefined" && document.hidden;
          timer = setTimeout(tick, hidden ? 2000 : failures ? Math.min(5000, 500 * 2 ** failures) : 500);
        }
      }
    };
    kick.current = () => void tick();
    tick();
    const onVis = () => !document.hidden && tick();
    document.addEventListener("visibilitychange", onVis);
    return () => {
      alive = false;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, [history]);

  // Fetch details immediately when the focused torrent changes.
  useEffect(() => {
    focusRef.current = focus;
    if (!focus) {
      setDetails(null);
      return;
    }
    let alive = true;
    call<TorrentDetails | null>("details", { hash: focus })
      .then((d) => alive && focusRef.current === focus && setDetails(d))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [focus]);

  const refresh = useCallback(() => kick.current(), []);

  return { snapshot, details, link, lastError, history, refresh };
}
