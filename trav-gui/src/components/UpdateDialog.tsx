"use client";

import { useState } from "react";
import type { UpdateInfo } from "@/lib/rpc";
import { installUpdate } from "@/lib/rpc";
import { bytes } from "@/lib/format";
import { Modal } from "./Modal";

/** Markdown inline marks → plain text (links keep their label). */
const plain = (s: string) => s.replace(/\[([^\]]+)\]\([^)]*\)/g, "$1").replace(/[`*]/g, "");

/** The release notes are the release's CHANGELOG section: headings and bullets. */
function Notes({ text }: { text: string }) {
  const blocks: React.ReactNode[] = [];
  let items: string[] = [];
  const flush = () => {
    if (items.length) blocks.push(<ul key={blocks.length}>{items.map((t, i) => <li key={i}>{t}</li>)}</ul>);
    items = [];
  };
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    if (line.startsWith("#")) {
      flush();
      blocks.push(
        <div key={blocks.length} className="eyebrow">
          {line.replace(/^#+\s*/, "")}
        </div>,
      );
    } else if (line.startsWith("- ")) {
      items.push(plain(line.slice(2)));
    } else if (items.length) {
      items[items.length - 1] += ` ${plain(line)}`;
    } else {
      flush();
      blocks.push(<p key={blocks.length}>{plain(line)}</p>);
    }
  }
  flush();
  return <div className="update-notes">{blocks}</div>;
}

export function UpdateDialog({ update, onClose, onError }: { update: UpdateInfo | null; onClose: () => void; onError: (m: string) => void }) {
  const [progress, setProgress] = useState<{ done: number; total: number | null } | null>(null);

  const install = async () => {
    setProgress({ done: 0, total: null });
    try {
      await installUpdate((done, total) => setProgress({ done, total }));
      // The app relaunches into the new version; nothing more to do here.
    } catch (e) {
      setProgress(null);
      onError(String((e as Error)?.message ?? e));
    }
  };

  const busy = progress !== null;
  const frac = progress?.total ? Math.min(1, progress.done / progress.total) : 0;

  return (
    <Modal
      open={!!update}
      onClose={busy ? () => {} : onClose}
      label="Software update"
      width={560}
      eyebrow={update ? `Update · v${update.currentVersion} → v${update.version}` : "Update"}
      title={update ? `Trav ${update.version} is here.` : ""}
    >
      {update && (
        <>
          <div className="modal-body">
            {update.notes ? <Notes text={update.notes} /> : <p className="mono faint">No release notes.</p>}
            {busy && (
              <div className="update-progress">
                <div className="bar">
                  <div className="bar-fill" style={{ transform: `scaleX(${frac})` }} />
                </div>
                <span className="mono faint">
                  {progress.total ? `${bytes(progress.done)} of ${bytes(progress.total)}` : "starting download…"}
                </span>
              </div>
            )}
          </div>
          <footer className="modal-foot">
            <span className="mono faint">{busy ? "Trav relaunches when it’s done" : "transfers resume after the relaunch"}</span>
            <div className="foot-actions">
              <button className="btn ghost" onClick={onClose} disabled={busy}>
                Later
              </button>
              <button className="btn primary" data-autofocus onClick={install} disabled={busy}>
                {busy ? "Installing…" : "Install & relaunch"}
              </button>
            </div>
          </footer>
        </>
      )}
    </Modal>
  );
}
