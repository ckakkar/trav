"use client";

import { useEffect, useState, type ReactNode } from "react";
import { Dices, FolderOpen } from "lucide-react";
import type { GlobalStats, Settings } from "@/lib/types";
import { bytes } from "@/lib/format";
import { call, isDesktop, pickFolder } from "@/lib/rpc";
import { Modal } from "./Modal";
import { Toggle } from "./AddDialog";
import { THEMES, type Theme } from "./theme";

export interface UiPrefs {
  theme: Theme;
  notify: boolean;
  confirmRemove: boolean;
  scanlines: boolean;
}

const SECTIONS = ["Downloads", "Bandwidth", "Network", "Interface"] as const;
type Section = (typeof SECTIONS)[number];

function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="set-row">
      <div className="set-label">
        <div>{label}</div>
        {hint && <div className="mono faint set-hint">{hint}</div>}
      </div>
      <div className="set-control">{children}</div>
    </div>
  );
}

function Num({ value, onChange, suffix, min = 0, max, step = 1 }: { value: number; onChange: (n: number) => void; suffix?: string; min?: number; max?: number; step?: number }) {
  return (
    <div className="num-input">
      <input
        className="input mono"
        type="number"
        min={min}
        max={max}
        step={step}
        value={Number.isFinite(value) ? value : 0}
        onChange={(e) => onChange(Math.max(min, Number(e.target.value) || 0))}
      />
      {suffix && <span className="mono faint">{suffix}</span>}
    </div>
  );
}

export function SettingsDialog({
  open,
  onClose,
  prefs,
  onPrefs,
  stats,
  onSaved,
  onError,
}: {
  open: boolean;
  onClose: () => void;
  prefs: UiPrefs;
  onPrefs: (p: UiPrefs) => void;
  stats: GlobalStats | null;
  onSaved: () => void;
  onError: (m: string) => void;
}) {
  const [s, setS] = useState<Settings | null>(null);
  const [orig, setOrig] = useState<string>("");
  const [section, setSection] = useState<Section>("Downloads");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    call<Settings>("getSettings")
      .then((v) => {
        setS(v);
        setOrig(JSON.stringify(v));
      })
      .catch((e) => onError(String(e?.message ?? e)));
  }, [open, onError]);

  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setS((x) => (x ? { ...x, [k]: v } : x));
  const dirty = s && JSON.stringify(s) !== orig;

  const save = async () => {
    if (!s) return;
    setSaving(true);
    try {
      const v = await call<Settings>("setSettings", s);
      setS(v);
      setOrig(JSON.stringify(v));
      onSaved();
    } catch (e) {
      onError(String((e as Error)?.message ?? e));
    } finally {
      setSaving(false);
    }
  };

  const kb = (b: number) => Math.round(b / 1024);

  return (
    <Modal open={open} onClose={onClose} label="Settings" eyebrow="Preferences" title="Settings" width={760}>
      <div className="settings">
        <nav className="set-nav">
          {SECTIONS.map((x) => (
            <button key={x} className="set-nav-item" aria-current={section === x} onClick={() => setSection(x)}>
              {x}
            </button>
          ))}
          <div className="set-nav-foot mono faint">
            {stats && (
              <>
                <div>port {stats.listenPort}</div>
                <div>{stats.connectable ? "reachable ●" : "not yet reachable ○"}</div>
                <div>dht {stats.dhtNodes} nodes</div>
                {stats.upnp && <div className="ellipsis" title={stats.upnp}>upnp {stats.upnp}</div>}
              </>
            )}
          </div>
        </nav>
        <div className="set-body">
          {!s ? (
            <div className="mono faint">Loading…</div>
          ) : section === "Downloads" ? (
            <>
              <Row label="Default save folder" hint="new torrents land here unless you pick another">
                <div className="field-row">
                  <input className="input mono" value={s.downloadDir} onChange={(e) => set("downloadDir", e.target.value)} spellCheck={false} />
                  {isDesktop() && (
                    <button
                      className="btn"
                      onClick={async () => {
                        const p = await pickFolder(s.downloadDir);
                        if (p) set("downloadDir", p);
                      }}
                    >
                      <FolderOpen size={14} />
                    </button>
                  )}
                </div>
              </Row>
              <Row label="Active downloads" hint="the rest wait in the queue · 0 = no limit">
                <Num value={s.maxActiveDownloads} onChange={(v) => set("maxActiveDownloads", v)} max={500} />
              </Row>
              <Row label="Stop seeding at ratio" hint="0 = seed forever">
                <Num value={s.seedRatioLimit} onChange={(v) => set("seedRatioLimit", v)} step={0.1} suffix="×" />
              </Row>
            </>
          ) : section === "Bandwidth" ? (
            <>
              <Row label="Download limit" hint="0 = unlimited">
                <Num value={kb(s.downloadLimit)} onChange={(v) => set("downloadLimit", v * 1024)} suffix="KB/s" />
              </Row>
              <Row label="Upload limit" hint="0 = unlimited">
                <Num value={kb(s.uploadLimit)} onChange={(v) => set("uploadLimit", v * 1024)} suffix="KB/s" />
              </Row>
              <Row label="Upload slots per torrent" hint="peers unchoked at once">
                <Num value={s.uploadSlots} onChange={(v) => set("uploadSlots", v)} min={1} max={200} />
              </Row>
              <div className="presets">
                {[0, 512, 1024, 5 * 1024, 20 * 1024].map((v) => (
                  <button key={v} className="chip" onClick={() => set("downloadLimit", v * 1024)}>
                    ↓ {v === 0 ? "∞" : `${bytes(v * 1024, 0)}/s`}
                  </button>
                ))}
              </div>
            </>
          ) : section === "Network" ? (
            <>
              <Row label="Listen port" hint="TCP for peers, UDP for DHT">
                <div className="field-row">
                  <Num value={s.listenPort} onChange={(v) => set("listenPort", Math.min(65535, v))} max={65535} />
                  <button className="btn" title="Random port" onClick={() => set("listenPort", 20000 + Math.floor(Math.random() * 40000))}>
                    <Dices size={14} />
                  </button>
                </div>
              </Row>
              <Row label="UPnP port forwarding" hint="ask the router to open the listen port">
                <Toggle checked={s.enableUpnp} onChange={(v) => set("enableUpnp", v)} label="" />
              </Row>
              <Row label="DHT" hint="trackerless peer discovery (needed for most magnets)">
                <Toggle checked={s.enableDht} onChange={(v) => set("enableDht", v)} label="" />
              </Row>
              <Row label="Peer exchange" hint="learn peers from peers">
                <Toggle checked={s.enablePex} onChange={(v) => set("enablePex", v)} label="" />
              </Row>
              <Row label="Peers per torrent">
                <Num value={s.maxPeersPerTorrent} onChange={(v) => set("maxPeersPerTorrent", v)} min={1} max={2000} />
              </Row>
              <Row label="Peers overall">
                <Num value={s.maxPeersGlobal} onChange={(v) => set("maxPeersGlobal", v)} min={10} max={20000} />
              </Row>
            </>
          ) : (
            <>
              <Row label="Theme" hint="from kkrwhofrags.xyz">
                <div className="theme-picks">
                  {THEMES.map((t) => (
                    <button key={t.id} className="theme-pick" data-pick={t.id} aria-pressed={prefs.theme === t.id} onClick={() => onPrefs({ ...prefs, theme: t.id })}>
                      <span className="swatch" style={{ background: t.bg, borderColor: t.line }}>
                        <i style={{ background: t.accent }} />
                        <i style={{ background: t.text }} />
                      </span>
                      <span className="mono">{t.label}</span>
                    </button>
                  ))}
                </div>
              </Row>
              <Row label="Notify when a download completes">
                <Toggle checked={prefs.notify} onChange={(v) => onPrefs({ ...prefs, notify: v })} label="" />
              </Row>
              <Row label="Confirm before removing">
                <Toggle checked={prefs.confirmRemove} onChange={(v) => onPrefs({ ...prefs, confirmRemove: v })} label="" />
              </Row>
              <Row label="CRT scanlines" hint="phosphor theme only">
                <Toggle checked={prefs.scanlines} onChange={(v) => onPrefs({ ...prefs, scanlines: v })} label="" />
              </Row>
            </>
          )}
        </div>
      </div>
      <footer className="modal-foot">
        <span className="mono faint">{section === "Interface" ? "interface changes apply instantly" : dirty ? "unsaved changes" : "engine settings"}</span>
        <div className="foot-actions">
          <button className="btn ghost" onClick={onClose}>
            Close
          </button>
          <button className="btn primary" disabled={!dirty || saving} onClick={save}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </footer>
    </Modal>
  );
}
