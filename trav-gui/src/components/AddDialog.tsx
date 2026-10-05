"use client";

import { useEffect, useMemo, useState } from "react";
import { FolderOpen, Link2, Magnet } from "lucide-react";
import type { TorrentPreview } from "@/lib/types";
import { bytes, looksLikeMagnet } from "@/lib/format";
import { call, isDesktop, pickFolder } from "@/lib/rpc";
import { Modal } from "./Modal";

export type AddItem = { kind: "path"; path: string } | { kind: "bytes"; name: string; b64: string } | { kind: "magnet"; uri: string };

function magnetInfo(uri: string): { name: string; hash: string } {
  try {
    const u = new URL(uri.startsWith("magnet:") ? uri : `magnet:?xt=urn:btih:${uri}`);
    const xt = u.searchParams.getAll("xt").find((x) => x.startsWith("urn:btih:")) ?? "";
    const hash = xt.slice(9);
    return { name: u.searchParams.get("dn") || hash, hash };
  } catch {
    return { name: uri, hash: "" };
  }
}

function source(item: AddItem) {
  switch (item.kind) {
    case "path":
      return { path: item.path };
    case "bytes":
      return { torrent: item.b64 };
    case "magnet":
      return { magnet: item.uri };
  }
}

export function AddDialog({
  open,
  items,
  onClose,
  onConsume,
  onEnqueue,
  defaultSavePath,
  onAdded,
  onError,
}: {
  open: boolean;
  items: AddItem[];
  onClose: () => void;
  onConsume: (n: number) => void;
  onEnqueue: (items: AddItem[]) => void;
  defaultSavePath: string;
  onAdded: (hash: string) => void;
  onError: (msg: string) => void;
}) {
  const item = items[0];
  const [preview, setPreview] = useState<TorrentPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [checked, setChecked] = useState<boolean[]>([]);
  const [savePath, setSavePath] = useState(defaultSavePath);
  const [startNow, setStartNow] = useState(true);
  const [sequential, setSequential] = useState(false);
  const [magnetText, setMagnetText] = useState("");
  const [busy, setBusy] = useState(false);

  // Follow the engine default until the user edits the field.
  const [pathTouched, setPathTouched] = useState(false);
  useEffect(() => {
    if (!pathTouched) setSavePath(defaultSavePath);
  }, [defaultSavePath, pathTouched]);

  useEffect(() => {
    setPreview(null);
    setError(null);
    if (!item || item.kind === "magnet") return;
    setLoading(true);
    call<TorrentPreview>("inspect", source(item))
      .then((p) => {
        setPreview(p);
        setChecked(p.files.map(() => true));
      })
      .catch((e) => setError(String(e?.message ?? e)))
      .finally(() => setLoading(false));
  }, [item]);

  const selectedSize = useMemo(
    () => (preview ? preview.files.reduce((a, f, i) => a + (checked[i] ? f.size : 0), 0) : 0),
    [preview, checked],
  );

  const submit = async (all = false) => {
    if (!item) return;
    setBusy(true);
    const queue = all ? items : [item];
    let consumed = 0;
    for (const it of queue) {
      try {
        const usePrefs = it === item && preview && checked.some((c) => !c);
        const r = await call<{ infoHash: string }>("add", {
          ...source(it),
          savePath,
          paused: !startNow,
          sequential,
          filePriorities: usePrefs ? checked.map((c) => (c ? 1 : 0)) : undefined,
        });
        onAdded(r.infoHash);
      } catch (e) {
        onError(String((e as Error)?.message ?? e));
      }
      consumed++;
    }
    setBusy(false);
    onConsume(consumed);
  };

  // Empty queue: magnet entry mode.
  if (open && !item) {
    const lines = magnetText.split(/\s+/).filter(looksLikeMagnet);
    return (
      <Modal open={open} onClose={onClose} label="Add magnet link" eyebrow="Add · magnet" title="Paste a magnet link" width={620}>
        <div className="modal-body">
          <textarea
            className="input mono magnet-input"
            data-autofocus
            rows={4}
            placeholder="magnet:?xt=urn:btih:…   (one per line, or a 40-char info-hash)"
            value={magnetText}
            onChange={(e) => setMagnetText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && lines.length) {
                e.preventDefault();
                document.getElementById("magnet-next")?.click();
              }
            }}
          />
          <div className="mono faint hint">{lines.length ? `${lines.length} link${lines.length > 1 ? "s" : ""} recognised` : "Tip: ⌘V anywhere in Trav pastes a magnet straight into this dialog."}</div>
        </div>
        <footer className="modal-foot">
          <span />
          <div className="foot-actions">
            <button className="btn ghost" onClick={onClose}>
              Cancel
            </button>
            <button
              id="magnet-next"
              className="btn primary"
              disabled={!lines.length}
              onClick={() => {
                setMagnetText("");
                onEnqueue(lines.map((uri) => ({ kind: "magnet" as const, uri })));
              }}
            >
              Continue
            </button>
          </div>
        </footer>
      </Modal>
    );
  }

  const isMagnet = item?.kind === "magnet";
  const m = isMagnet ? magnetInfo(item.uri) : null;
  const title = preview?.name ?? m?.name ?? (item?.kind === "bytes" ? item.name : item?.kind === "path" ? item.path.split(/[\\/]/).pop() : "");

  return (
    <Modal
      open={open && !!item}
      onClose={onClose}
      label="Add torrent"
      width={720}
      eyebrow={
        <>
          {isMagnet ? <Magnet size={12} /> : <Link2 size={12} />} Add · {isMagnet ? "magnet" : "torrent"}
          {items.length > 1 && <span className="faint"> · 1 of {items.length}</span>}
        </>
      }
      title={<span title={title}>{title || "…"}</span>}
    >
      <div className="modal-body">
        {preview && (
          <div className="add-meta mono">
            <span>{bytes(preview.totalSize)}</span>
            <span>{preview.files.length} file{preview.files.length > 1 ? "s" : ""}</span>
            <span>
              {preview.numPieces} × {bytes(preview.pieceLength, 0)}
            </span>
            {preview.private && <span className="pill">private</span>}
            <span className="faint ellipsis">{preview.infoHash}</span>
          </div>
        )}
        {m && (
          <div className="add-meta mono">
            <span className="faint">{m.hash}</span>
            <span>files can be chosen once metadata arrives</span>
          </div>
        )}
        {preview?.alreadyAdded && <div className="callout warn">This torrent is already in your list.</div>}
        {error && <div className="callout err">{error}</div>}
        {loading && <div className="mono faint">Reading torrent…</div>}

        {preview && preview.files.length > 1 && (
          <div className="add-files">
            <div className="pane-bar">
              <span className="mono faint">
                {checked.filter(Boolean).length} of {preview.files.length} · {bytes(selectedSize)}
              </span>
              <div className="pane-actions">
                <button className="chip" onClick={() => setChecked(preview.files.map(() => true))}>
                  All
                </button>
                <button className="chip" onClick={() => setChecked(preview.files.map(() => false))}>
                  None
                </button>
              </div>
            </div>
            <div className="list add-list">
              {preview.files.map((f, i) => {
                const parts = f.path.split("/");
                const name = parts.pop();
                return (
                  <label key={i} className="file-row add" data-skip={!checked[i] || undefined}>
                    <span className="check">
                      <input
                        type="checkbox"
                        checked={!!checked[i]}
                        onChange={(e) => setChecked((c) => c.map((v, j) => (j === i ? e.target.checked : v)))}
                      />
                      <span />
                    </span>
                    <span className="file-name" title={f.path}>
                      {parts.length > 0 && <span className="faint">{parts.join("/")}/</span>}
                      {name}
                    </span>
                    <span className="mono num faint">{bytes(f.size)}</span>
                  </label>
                );
              })}
            </div>
          </div>
        )}

        <div className="field">
          <label className="label" htmlFor="save-path">
            Save to
          </label>
          <div className="field-row">
            <input
              id="save-path"
              className="input mono"
              value={savePath}
              onChange={(e) => {
                setPathTouched(true);
                setSavePath(e.target.value);
              }}
              spellCheck={false}
            />
            {isDesktop() && (
              <button
                className="btn"
                onClick={async () => {
                  const p = await pickFolder(savePath);
                  if (p) {
                    setPathTouched(true);
                    setSavePath(p);
                  }
                }}
              >
                <FolderOpen size={14} /> Browse
              </button>
            )}
          </div>
        </div>

        <div className="toggles">
          <Toggle checked={startNow} onChange={setStartNow} label="Start immediately" />
          <Toggle checked={sequential} onChange={setSequential} label="Sequential download" hint="stream media while it downloads" />
        </div>
      </div>
      <footer className="modal-foot">
        <span className="mono faint">{preview ? `${bytes(selectedSize)} to download` : ""}</span>
        <div className="foot-actions">
          <button className="btn ghost" onClick={() => onConsume(1)} disabled={busy}>
            {items.length > 1 ? "Skip" : "Cancel"}
          </button>
          {items.length > 1 && (
            <button className="btn" onClick={() => submit(true)} disabled={busy}>
              Add all {items.length}
            </button>
          )}
          <button
            className="btn primary"
            data-autofocus
            onClick={() => submit(false)}
            disabled={busy || loading || !!error || preview?.alreadyAdded || (preview != null && !checked.some(Boolean))}
          >
            {busy ? "Adding…" : "Add torrent"}
          </button>
        </div>
      </footer>
    </Modal>
  );
}

export function Toggle({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string }) {
  return (
    <label className="toggle">
      <input type="checkbox" role="switch" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span className="toggle-track">
        <span className="toggle-thumb" />
      </span>
      <span className="toggle-label">
        {label}
        {hint && <span className="faint mono toggle-hint">{hint}</span>}
      </span>
    </label>
  );
}
