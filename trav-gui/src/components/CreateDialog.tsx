"use client";

import { useEffect, useRef, useState } from "react";
import { Check, FilePlus2, FolderOpen } from "lucide-react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { bytes } from "@/lib/format";
import { call, reveal } from "@/lib/rpc";
import { Modal } from "./Modal";
import { Toggle } from "./AddDialog";

export interface CreatedTorrent {
  infoHash: string;
  out: string;
  name: string;
  totalSize: number;
  pieceLength: number;
  numPieces: number;
}

type ProgressEvent = { type: "createProgress"; id: string; hashed: number; total: number };
type Phase = "edit" | "saving" | "hashing" | "success";
type Run = { id: string; started: boolean; cancelRequested: boolean; closeAfterCancel: boolean; startedAt: number; off?: UnlistenFn };

function release(run: Run) {
  const off = run.off;
  run.off = undefined;
  off?.();
}

/** Blank lines separate tracker tiers; adjacent lines belong to one tier. */
function trackerTiers(text: string): string[][] {
  const tiers: string[][] = [];
  let tier: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const url = line.trim();
    if (url) tier.push(url);
    else if (tier.length) { tiers.push(tier); tier = []; }
  }
  if (tier.length) tiers.push(tier);
  return tiers;
}

const errorText = (error: unknown) => String(error instanceof Error ? error.message : error);
const baseName = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "Untitled";
const PIECES = Array.from({ length: 11 }, (_, index) => 16 * 1024 * 2 ** index);

export function CreateDialog({ open, onClose, onCreated }: {
  open: boolean;
  onClose(): void;
  onCreated(created: { infoHash: string; out: string; name: string }, seed: boolean, sourcePath: string): void;
}) {
  const [path, setPath] = useState("");
  const [trackers, setTrackers] = useState("");
  const [webSeeds, setWebSeeds] = useState("");
  const [isPrivate, setPrivate] = useState(false);
  const [comment, setComment] = useState("");
  const [source, setSource] = useState("");
  const [pieceLength, setPieceLength] = useState("");
  const [seed, setSeed] = useState(true);
  const [phase, setPhase] = useState<Phase>("edit");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [progress, setProgress] = useState({ hashed: 0, total: 0, rate: 0 });
  const [created, setCreated] = useState<CreatedTorrent | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [picking, setPicking] = useState(false);
  const active = useRef<Run | null>(null);
  const pickToken = useRef(0);

  useEffect(() => {
    if (open) {
      setPhase("edit"); setCreated(null); setError(null); setNotice(null);
      setProgress({ hashed: 0, total: 0, rate: 0 }); setCancelling(false); setPicking(false);
    }
    return () => {
      pickToken.current++;
      const run = active.current;
      active.current = null;
      if (run) release(run);
      if (run?.started) void call("cancelCreate", { id: run.id }).catch(() => {});
    };
  }, [open]);

  const choose = async (directory: boolean) => {
    if (picking) return;
    const token = ++pickToken.current;
    setPicking(true); setError(null);
    try {
      const { open: pick } = await import("@tauri-apps/plugin-dialog");
      const selected = await pick({ directory, multiple: false, title: directory ? "Choose a folder to share" : "Choose a file to share" });
      if (token === pickToken.current && typeof selected === "string") { setPath(selected); setNotice(null); }
    } catch (error) {
      if (token === pickToken.current) setError(errorText(error));
    } finally {
      if (token === pickToken.current) setPicking(false);
    }
  };

  const requestCancel = async (closeAfterCancel = false) => {
    const run = active.current;
    if (!run) { onClose(); return; }
    run.closeAfterCancel ||= closeAfterCancel;
    run.cancelRequested = true;
    if (!run.started) {
      active.current = null; release(run); setPhase("edit");
      if (closeAfterCancel) onClose();
      return;
    }
    setCancelling(true);
    try {
      await call("cancelCreate", { id: run.id });
    } catch (error) {
      if (active.current === run) { setError(errorText(error)); setCancelling(false); run.cancelRequested = false; }
    }
  };

  const submit = async () => {
    if (active.current || !path || picking) return;
    const run: Run = { id: crypto.randomUUID(), started: false, cancelRequested: false, closeAfterCancel: false, startedAt: 0 };
    active.current = run;
    const sourcePath = path;
    const startSeeding = seed;
    setPhase("saving"); setError(null); setNotice(null); setCancelling(false);
    setProgress({ hashed: 0, total: 0, rate: 0 });
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const out = await save({ title: "Create torrent", defaultPath: `${baseName(sourcePath)}.torrent`, filters: [{ name: "Torrent", extensions: ["torrent"] }] });
      if (active.current !== run) return;
      if (!out) { active.current = null; setPhase("edit"); return; }
      // Subscribe before invoking: a small file can finish before invoke returns.
      run.off = await listen<ProgressEvent>("trav://event", ({ payload }) => {
        if (active.current !== run || payload.type !== "createProgress" || payload.id !== run.id) return;
        const total = Math.max(0, Number(payload.total) || 0);
        const hashed = Math.min(total, Math.max(0, Number(payload.hashed) || 0));
        const elapsed = Math.max(0.001, (performance.now() - run.startedAt) / 1000);
        setProgress({ hashed, total, rate: hashed / elapsed });
      });
      if (active.current !== run) { release(run); return; }
      run.started = true; run.startedAt = performance.now(); setPhase("hashing");
      const result = await call<CreatedTorrent>("createTorrent", {
        id: run.id, path: sourcePath, out, trackers: trackerTiers(trackers),
        webSeeds: webSeeds.split(/\r?\n/).map((line) => line.trim()).filter(Boolean),
        private: isPrivate, comment: comment || null, source: source || null,
        pieceLength: pieceLength ? Number(pieceLength) : null,
      });
      if (active.current !== run) return;
      setCreated(result); setPhase("success"); setCancelling(false);
      // Creation is complete even if the parent reports a seeding error.
      try { onCreated(result, startSeeding, sourcePath); }
      catch (error) { setError(errorText(error)); }
    } catch (error) {
      if (active.current !== run) return;
      setPhase("edit"); setCancelling(false);
      if (run.cancelRequested && /cancelled|canceled/i.test(errorText(error))) {
        setNotice("Creation cancelled. No torrent was saved.");
        if (run.closeAfterCancel) onClose();
      } else setError(errorText(error));
    } finally {
      release(run);
      if (active.current === run) active.current = null;
    }
  };

  const busy = phase === "saving" || phase === "hashing";
  const percent = progress.total ? Math.min(100, (progress.hashed / progress.total) * 100) : 0;

  return (
    <Modal open={open} onClose={() => busy ? void requestCancel(true) : onClose()} label="Create torrent" eyebrow={<><FilePlus2 size={12} /> Share · create</>} title={phase === "success" ? "Torrent created" : "Create a torrent"} width={660}>
      <div className="modal-body">
        {error && <div className="callout err" role="alert">{error}</div>}
        {notice && <div className="callout" role="status">{notice}</div>}
        {phase === "edit" && <>
          <div className="field">
            <span className="label">Content to share</span>
            <div className="field-row">
              <button className="btn" data-autofocus disabled={picking} onClick={() => void choose(false)}><FilePlus2 size={14} /> Choose file…</button>
              <button className="btn" disabled={picking} onClick={() => void choose(true)}><FolderOpen size={14} /> Choose folder…</button>
            </div>
            <div className="mono faint hint create-path" title={path}>{path || "Choose a file or folder with something in it."}</div>
          </div>
          <div className="field">
            <label className="label" htmlFor="create-trackers">Trackers</label>
            <textarea id="create-trackers" className="input mono" rows={3} value={trackers} onChange={(event) => setTrackers(event.target.value)} placeholder="https://tracker.example/announce" spellCheck={false} />
            <span className="mono faint hint">One URL per line. A blank line starts a new tier.</span>
          </div>
          <div className="field">
            <label className="label" htmlFor="create-web-seeds">Web seeds</label>
            <textarea id="create-web-seeds" className="input mono" rows={2} value={webSeeds} onChange={(event) => setWebSeeds(event.target.value)} placeholder="https://mirror.example/files/" spellCheck={false} />
            <span className="mono faint hint">Optional HTTP or HTTPS mirrors, one per line.</span>
          </div>
          <div className="field">
            <label className="label" htmlFor="create-comment">Comment</label>
            <input id="create-comment" className="input" value={comment} onChange={(event) => setComment(event.target.value)} />
          </div>
          <div className="create-options">
            <div className="field">
              <label className="label" htmlFor="create-source">Source <span className="faint">· optional</span></label>
              <input id="create-source" className="input" value={source} onChange={(event) => setSource(event.target.value)} />
            </div>
            <div className="field">
              <label className="label" htmlFor="create-piece-length">Piece size</label>
              <select id="create-piece-length" className="select" value={pieceLength} onChange={(event) => setPieceLength(event.target.value)}>
                <option value="">Auto</option>
                {PIECES.map((size) => <option key={size} value={size}>{bytes(size, 0)}</option>)}
              </select>
            </div>
          </div>
          <div className="toggles">
            <Toggle checked={isPrivate} onChange={setPrivate} label="Private torrent" />
            <Toggle checked={seed} onChange={setSeed} label="Start seeding when done" />
          </div>
        </>}
        {busy && <div className="field" role="status" aria-live="polite">
          <div className="create-progress-head"><span>{cancelling ? "Cancelling…" : phase === "saving" ? "Choose where to save your torrent…" : progress.hashed === progress.total && progress.total > 0 ? "Saving torrent…" : "Hashing your files…"}</span><span className="mono faint">{percent.toFixed(0)}%</span></div>
          <div className="bar" role="progressbar" aria-label="Torrent creation" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(percent)}><div className="bar-fill" style={{ width: `${percent}%` }} /></div>
          <div className="create-progress-head mono faint hint"><span>{bytes(progress.hashed)} / {bytes(progress.total)}</span><span>{(progress.rate / 1_000_000).toFixed(1)} MB/s</span></div>
        </div>}
        {phase === "success" && created && <div className="field" role="status">
          <div className="field-row"><Check size={18} /><strong>{created.name}</strong></div>
          <div className="mono faint">{bytes(created.totalSize)} · {created.numPieces.toLocaleString()} pieces</div>
          <div className="mono hint create-path">{created.out}</div>
          <div className="mono faint hint create-path">{created.infoHash}</div>
        </div>}
      </div>
      <footer className="modal-foot">
        <span className="mono faint hint">{phase === "edit" ? "Hidden files and symlinks are skipped." : ""}</span>
        <div className="foot-actions">
          {phase === "success" && created ? <>
            <button className="btn" onClick={() => void reveal(created.out).catch((error) => setError(errorText(error)))}><FolderOpen size={14} /> Show in Finder</button>
            <button className="btn primary" data-autofocus onClick={onClose}>Done</button>
          </> : <>
            <button className="btn ghost" disabled={cancelling} onClick={() => busy ? void requestCancel() : onClose()}>{cancelling ? "Cancelling…" : "Cancel"}</button>
            {!busy && <button className="btn primary" disabled={!path || picking} onClick={() => void submit()}>Create torrent…</button>}
          </>}
        </div>
      </footer>
    </Modal>
  );
}
