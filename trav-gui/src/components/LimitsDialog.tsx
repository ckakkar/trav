"use client";

import { useEffect, useState } from "react";
import { Modal } from "./Modal";

/** Bytes per second ↔ the KB/s the fields show (0 = no limit). */
const kb = (b: number) => Math.round(b / 1024);

/** Speed limits for one or more torrents, on top of the overall ones. */
export function LimitsDialog({
  target,
  onClose,
  onSave,
}: {
  target: { hashes: string[]; name: string; download: number; upload: number } | null;
  onClose: () => void;
  onSave: (hashes: string[], download: number, upload: number) => void;
}) {
  const [down, setDown] = useState(0);
  const [up, setUp] = useState(0);
  useEffect(() => {
    if (!target) return;
    setDown(kb(target.download));
    setUp(kb(target.upload));
  }, [target]);
  const many = (target?.hashes.length ?? 0) > 1;
  const field = (label: string, value: number, set: (n: number) => void) => (
    <label className="field">
      <span className="label">{label}</span>
      <span className="num-input">
        <input
          className="input mono"
          type="number"
          min={0}
          value={value}
          data-autofocus={label.startsWith("Download") || undefined}
          onChange={(e) => set(Math.max(0, Math.round(Number(e.target.value) || 0)))}
        />
        <span className="mono faint">KB/s</span>
      </span>
    </label>
  );
  return (
    <Modal
      open={!!target}
      onClose={onClose}
      label="Speed limits"
      width={460}
      eyebrow={many ? `${target!.hashes.length} torrents` : "Speed limits"}
      title={many ? "Limit these torrents" : target?.name}
    >
      {target && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            onSave(target.hashes, down * 1024, up * 1024);
          }}
        >
          <div className="modal-body">
            <div className="limits-fields">
              {field("Download", down, setDown)}
              {field("Upload", up, setUp)}
            </div>
            <p className="mono faint hint">0 = no limit of its own. The overall limits in Settings still apply.</p>
          </div>
          <footer className="modal-foot">
            <button type="button" className="btn ghost" onClick={() => onSave(target.hashes, 0, 0)}>
              Remove limits
            </button>
            <div className="foot-actions">
              <button type="button" className="btn ghost" onClick={onClose}>
                Cancel
              </button>
              <button type="submit" className="btn primary">
                Set limits
              </button>
            </div>
          </footer>
        </form>
      )}
    </Modal>
  );
}
