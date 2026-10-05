"use client";

import { motion } from "motion/react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Copy, FolderOpen, RefreshCw, Plus, Check } from "lucide-react";
import type { TorrentDetails, FileInfo } from "@/lib/types";
import type { SpeedHistory } from "@/lib/engine";
import { ago, bytes, date, eta, pct, rate } from "@/lib/format";
import { Tween } from "@/lib/tween";
import { copyText, isDesktop, openPath, reveal } from "@/lib/rpc";
import { PieceMap } from "./PieceMap";
import { Graph } from "./Graph";
import { statusLabel } from "./TorrentTable";
import { ease } from "./Modal";

export type DetailTab = "overview" | "files" | "peers" | "trackers" | "speed";
const TABS: { id: DetailTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "files", label: "Files" },
  { id: "peers", label: "Peers" },
  { id: "trackers", label: "Trackers" },
  { id: "speed", label: "Speed" },
];

export interface DetailActions {
  setPriorities: (hash: string, p: number[]) => void;
  addPeers: (hash: string, peers: string[]) => void;
  reannounce: (hash: string) => void;
  toast: (title: string, opts?: { body?: string; tone?: "info" | "ok" | "error" }) => void;
}

export function DetailPanel({
  details,
  tab,
  onTab,
  history,
  actions,
  selectionCount,
}: {
  details: TorrentDetails | null;
  tab: DetailTab;
  onTab: (t: DetailTab) => void;
  history: SpeedHistory;
  actions: DetailActions;
  selectionCount: number;
}) {
  return (
    <section className="detail" aria-label="Torrent details">
      <nav className="tabs" role="tablist">
        {TABS.map((t) => (
          <button key={t.id} role="tab" aria-selected={tab === t.id} className="tab" onClick={() => onTab(t.id)}>
            {t.label}
            {t.id === "peers" && details ? <span className="tab-count">{details.peers.length}</span> : null}
            {t.id === "files" && details && details.files.length > 1 ? <span className="tab-count">{details.files.length}</span> : null}
            {tab === t.id && <motion.span layoutId="tab-underline" className="tab-underline" transition={{ duration: 0.35, ease }} />}
          </button>
        ))}
        <div className="tabs-spacer" />
        {selectionCount > 1 && <span className="mono faint tabs-note">{selectionCount} selected · showing focused</span>}
      </nav>
      <div className="detail-body">
        {!details ? (
          <div className="detail-empty">
            <span className="display">Select a torrent</span>
            <span className="mono faint">its pieces, peers and trackers show up here</span>
          </div>
        ) : (
          <motion.div
            key={tab}
            className="detail-pane"
            initial={{ opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.25, ease }}
          >
            {tab === "overview" && <Overview d={details} actions={actions} />}
            {tab === "files" && <Files d={details} actions={actions} />}
            {tab === "peers" && <Peers d={details} actions={actions} />}
            {tab === "trackers" && <Trackers d={details} actions={actions} />}
            {tab === "speed" && <Speed d={details} history={history} />}
          </motion.div>
        )}
      </div>
    </section>
  );
}

function Stat({ label, children, wide }: { label: string; children: ReactNode; wide?: boolean }) {
  return (
    <div className={`stat${wide ? " wide" : ""}`}>
      <div className="label">{label}</div>
      <div className="stat-v mono">{children}</div>
    </div>
  );
}

function CopyBtn({ text, label }: { text: string; label: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className="icon-btn sm"
      title={label}
      aria-label={label}
      onClick={async () => {
        if (await copyText(text)) {
          setDone(true);
          setTimeout(() => setDone(false), 1200);
        }
      }}
    >
      {done ? <Check size={13} /> : <Copy size={13} />}
    </button>
  );
}

function Overview({ d, actions }: { d: TorrentDetails; actions: DetailActions }) {
  const s = d.summary;
  const st = statusLabel(s);
  const path = d.contentPath ?? s.savePath;
  return (
    <div className="overview">
      <div className="ov-head">
        <div className="ov-title">
          <div className="eyebrow">
            <span className={`pill tone-${st.tone}`}>{st.label}</span>
            {s.private && <span className="pill">private</span>}
            {s.sequential && <span className="pill">sequential</span>}
          </div>
          <h3 className="display ov-name" title={s.name}>
            {s.name}
          </h3>
          <div className="ov-hash mono">
            <span>{s.infoHash}</span>
            <CopyBtn text={s.infoHash} label="Copy info-hash" />
            <CopyBtn text={d.magnet} label="Copy magnet link" />
          </div>
        </div>
        <div className="ov-big">
          <Tween key={s.infoHash} className="display ov-pct" value={s.progress} format={(v) => pct(v, v >= 0.999 ? 0 : 1)} />
          <div className="mono faint">
            {s.hasMetadata ? `${bytes(s.doneBytes)} of ${bytes(s.size)}` : "waiting for metadata"}
          </div>
        </div>
      </div>

      <PieceMap pieces={d.pieces} inProgress={d.piecesInProgress} count={d.numPieces} />

      <div className="stats">
        <Stat label="Download">
          <Tween key={s.infoHash} value={s.downloadRate} format={rate} />
        </Stat>
        <Stat label="Upload">
          <Tween key={s.infoHash} value={s.uploadRate} format={rate} />
        </Stat>
        <Stat label="ETA">{s.status === "downloading" ? eta(s.eta) : "—"}</Stat>
        <Stat label="Ratio">{s.ratio.toFixed(3)}</Stat>
        <Stat label="Downloaded">{bytes(s.downloaded)}</Stat>
        <Stat label="Uploaded">{bytes(s.uploaded)}</Stat>
        <Stat label="Seeds">
          {s.seeds} <span className="faint">of {s.swarmSeeds ?? "?"}</span>
        </Stat>
        <Stat label="Peers">
          {s.peers} <span className="faint">of {s.swarmPeers ?? "?"}</span>
        </Stat>
        <Stat label="Availability">{s.availability.toFixed(2)}</Stat>
        <Stat label="Pieces">{d.numPieces ? `${d.numPieces} × ${bytes(d.pieceLength, 0)}` : "—"}</Stat>
        <Stat label="Wasted">
          {bytes(d.wasted)}
          {d.hashFails ? <span className="faint"> · {d.hashFails} fails</span> : null}
        </Stat>
        <Stat label="Added">{ago(s.addedAt)}</Stat>
        <Stat label="Completed">{s.completedAt ? date(s.completedAt) : "—"}</Stat>
        <Stat label="Created">{d.creationDate ? date(d.creationDate) : "—"}</Stat>
        <Stat label="Created by">{d.createdBy ?? "—"}</Stat>
        <Stat label="Save path" wide>
          <span className="path" title={path}>
            {path}
          </span>
          {isDesktop() && (
            <button className="icon-btn sm" title="Show in folder" aria-label="Show in folder" onClick={() => reveal(path).catch((e) => actions.toast("Can't open folder", { body: String(e), tone: "error" }))}>
              <FolderOpen size={13} />
            </button>
          )}
        </Stat>
        {d.comment && (
          <Stat label="Comment" wide>
            <span className="comment">{d.comment}</span>
          </Stat>
        )}
        {s.error && (
          <Stat label="Error" wide>
            <span className="err-text">{s.error}</span>
          </Stat>
        )}
      </div>
    </div>
  );
}

const PRIO_LABEL = ["Skip", "Normal", "High"];

function Files({ d, actions }: { d: TorrentDetails; actions: DetailActions }) {
  const hash = d.summary.infoHash;
  const total = d.files.length;
  // Optimistic overrides: the engine confirms on the next poll, but the control
  // must reflect the click immediately (and survive rapid successive edits).
  const [pending, setPending] = useState<Record<number, number>>({});
  useEffect(() => {
    setPending((p) => {
      const left = Object.fromEntries(Object.entries(p).filter(([i, v]) => d.files.find((f) => f.index === Number(i))?.priority !== v));
      return Object.keys(left).length === Object.keys(p).length ? p : left;
    });
  }, [d.files]);
  const prioOf = (f: FileInfo) => pending[f.index] ?? f.priority;
  const prios = useMemo(() => {
    const arr = new Array(Math.max(0, ...d.files.map((f) => f.index + 1))).fill(1);
    d.files.forEach((f) => (arr[f.index] = pending[f.index] ?? f.priority));
    return arr as number[];
  }, [d.files, pending]);
  if (!d.summary.hasMetadata) {
    return <div className="pane-note mono">Files appear once metadata arrives from the swarm.</div>;
  }
  const set = (f: FileInfo, p: number) => {
    const next = [...prios];
    next[f.index] = p;
    setPending((x) => ({ ...x, [f.index]: p }));
    actions.setPriorities(hash, next);
  };
  const setAll = (p: number) => {
    setPending(Object.fromEntries(d.files.map((f) => [f.index, p])));
    actions.setPriorities(hash, prios.map(() => p));
  };
  const wanted = d.files.filter((f) => prioOf(f) > 0).length;
  return (
    <div className="files">
      <div className="pane-bar">
        <span className="mono faint">
          {wanted} of {total} selected · {bytes(d.files.filter((f) => prioOf(f) > 0).reduce((a, f) => a + f.size, 0))}
        </span>
        <div className="pane-actions">
          <button className="chip" onClick={() => setAll(1)}>
            All
          </button>
          <button className="chip" onClick={() => setAll(0)}>
            None
          </button>
        </div>
      </div>
      <div className="list">
        {d.files.map((f) => {
          const parts = f.path.split("/");
          const name = parts.pop();
          return (
            <div key={f.index} className="file-row" data-skip={prioOf(f) === 0 || undefined}>
              <label className="check">
                <input type="checkbox" checked={prioOf(f) > 0} onChange={(e) => set(f, e.target.checked ? 1 : 0)} />
                <span />
              </label>
              <div className="file-name" title={f.path}>
                {parts.length > 0 && <span className="faint">{parts.join("/")}/</span>}
                {name}
              </div>
              <div className="file-prog">
                <div className="bar sm">
                  <div className="bar-fill" style={{ transform: `scaleX(${f.progress})` }} />
                </div>
                <span className="mono faint">{pct(f.progress, 0)}</span>
              </div>
              <div className="mono num file-size">{bytes(f.size)}</div>
              <select className="select sm" value={Math.min(2, prioOf(f))} onChange={(e) => set(f, Number(e.target.value))} aria-label="Priority">
                {PRIO_LABEL.map((l, i) => (
                  <option key={l} value={i}>
                    {l}
                  </option>
                ))}
              </select>
              {isDesktop() && d.contentPath && f.progress >= 1 && (
                <button
                  className="icon-btn sm"
                  title="Open"
                  aria-label={`Open ${name}`}
                  onClick={() => openPath(d.files.length === 1 && !f.path.includes("/") ? d.contentPath! : `${d.contentPath}/${f.path}`)}
                >
                  <FolderOpen size={13} />
                </button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

const FLAG_HELP: Record<string, string> = {
  D: "downloading from peer",
  d: "we want data, peer is choking us",
  U: "uploading to peer",
  u: "peer wants data, we are choking",
  K: "peer unchoked us, we're not interested",
  "?": "we unchoked peer, it's not interested",
  O: "optimistic unchoke",
  S: "snubbed (no data for 60 s)",
  I: "incoming connection",
  X: "found via peer exchange",
  H: "found via DHT",
};

function Peers({ d, actions }: { d: TorrentDetails; actions: DetailActions }) {
  const [add, setAdd] = useState("");
  const peers = [...d.peers].sort((a, b) => b.downloadRate + b.uploadRate - (a.downloadRate + a.uploadRate) || b.progress - a.progress);
  return (
    <div className="peers">
      <div className="pane-bar">
        <span className="mono faint">
          {d.peers.length} connected · {d.summary.seeds} seeds
        </span>
        <form
          className="pane-actions"
          onSubmit={(e) => {
            e.preventDefault();
            const list = add.split(/[\s,]+/).filter(Boolean);
            if (list.length) {
              actions.addPeers(d.summary.infoHash, list);
              setAdd("");
            }
          }}
        >
          <input className="input sm mono" placeholder="add peer ip:port" value={add} onChange={(e) => setAdd(e.target.value)} />
          <button className="icon-btn sm" aria-label="Add peer" type="submit">
            <Plus size={13} />
          </button>
        </form>
      </div>
      <div className="grid-table peers-table">
        <div className="gt-head mono">
          <span>Address</span>
          <span>Client</span>
          <span>Flags</span>
          <span className="num">Has</span>
          <span className="num">Down</span>
          <span className="num">Up</span>
          <span className="num">Received</span>
          <span className="num">Sent</span>
        </div>
        {peers.length === 0 && <div className="pane-note mono">No peers connected yet.</div>}
        {peers.map((p) => (
          <div key={p.addr} className="gt-row">
            <span className="mono">{p.addr}</span>
            <span className="ellipsis">{p.client}</span>
            <span className="mono flags" title={[...p.flags].map((f) => `${f} — ${FLAG_HELP[f] ?? f}`).join("\n")}>
              {p.flags || "—"}
            </span>
            <span className="num peer-has">
              <span className="bar xs">
                <span className="bar-fill" style={{ transform: `scaleX(${p.progress})` }} />
              </span>
              <span className="mono faint">{pct(p.progress, 0)}</span>
            </span>
            <span className="num mono accent-text">{rate(p.downloadRate)}</span>
            <span className="num mono up-text">{rate(p.uploadRate)}</span>
            <span className="num mono faint">{bytes(p.downloaded)}</span>
            <span className="num mono faint">{bytes(p.uploaded)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function Trackers({ d, actions }: { d: TorrentDetails; actions: DetailActions }) {
  return (
    <div className="trackers">
      <div className="pane-bar">
        <span className="mono faint">
          {d.trackers.length} trackers · DHT {d.dhtEnabled ? "on" : "off"}
        </span>
        <div className="pane-actions">
          <button className="chip" onClick={() => actions.reannounce(d.summary.infoHash)}>
            <RefreshCw size={12} /> Reannounce
          </button>
        </div>
      </div>
      <div className="grid-table trackers-table">
        <div className="gt-head mono">
          <span>Status</span>
          <span>Tracker</span>
          <span className="num">Seeds</span>
          <span className="num">Leechers</span>
          <span className="num">Peers</span>
          <span className="num">Next</span>
        </div>
        {[
          { url: "DHT", status: d.dhtEnabled ? "working" : "idle", message: d.dhtEnabled ? "Mainline DHT" : "disabled", seeds: null, leechers: null, peers: null, nextAnnounce: null, tier: -1 },
          ...d.trackers,
        ].map((t, i) => (
          <div key={i} className="gt-row" data-tstatus={t.status}>
            <span className="mono t-status">
              <span className="sdot" />
              {t.status}
            </span>
            <span className="t-url">
              <span className="mono ellipsis" title={t.url}>
                {t.url}
              </span>
              {t.message && <span className="t-msg">{t.message}</span>}
            </span>
            <span className="num mono">{t.seeds ?? "—"}</span>
            <span className="num mono">{t.leechers ?? "—"}</span>
            <span className="num mono">{t.peers ?? "—"}</span>
            <span className="num mono faint">{t.nextAnnounce != null ? eta(t.nextAnnounce) : "—"}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function Speed({ d, history }: { d: TorrentDetails; history: SpeedHistory }) {
  const hash = d.summary.infoHash;
  return (
    <div className="speed">
      <div className="speed-legend">
        <span className="legend down">
          <i /> Download <Tween key={hash + "d"} className="mono" value={d.summary.downloadRate} format={rate} />
        </span>
        <span className="legend up">
          <i /> Upload <Tween key={hash + "u"} className="mono" value={d.summary.uploadRate} format={rate} />
        </span>
        <span className="mono faint">last 3 min</span>
      </div>
      <Graph className="speed-graph" axis windowMs={180_000} source={() => history.perTorrent.get(hash)} />
    </div>
  );
}
