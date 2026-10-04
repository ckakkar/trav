"use client";

import { motion } from "motion/react";
import type { ReactNode } from "react";
import { Settings2, SunMoon, Command as CommandIcon } from "lucide-react";
import type { GlobalStats } from "@/lib/types";
import type { SpeedHistory } from "@/lib/engine";
import { bytes, bytesParts } from "@/lib/format";
import { Tween } from "@/lib/tween";
import { Graph } from "./Graph";
import { ease } from "./Modal";

export interface FilterDef {
  id: string;
  label: string;
  icon: ReactNode;
  count: number;
}

function Speed({ label, value, tone }: { label: string; value: number; tone: "down" | "up" }) {
  return (
    <div className={`speed-num ${tone}`}>
      <span className="label">{label}</span>
      <span className="speed-val">
        <Tween className="display speed-big" value={value} format={(v) => bytesParts(v)[0]} />
        <Tween className="mono speed-unit" value={value} format={(v) => `${bytesParts(v)[1] || "B"}/s`} />
      </span>
    </div>
  );
}

export function Sidebar({
  filters,
  active,
  onFilter,
  stats,
  history,
  onSettings,
  onTheme,
  onPalette,
  mac,
}: {
  filters: FilterDef[];
  active: string;
  onFilter: (id: string) => void;
  stats: GlobalStats | null;
  history: SpeedHistory;
  onSettings: () => void;
  onTheme: (e: React.MouseEvent) => void;
  onPalette: () => void;
  mac: boolean;
}) {
  return (
    <aside className="sidebar" data-mac={mac || undefined}>
      <div className="brand" data-tauri-drag-region>
        <span className="brand-mark display" data-tauri-drag-region>
          Trav
        </span>
        <span className="brand-sub mono" data-tauri-drag-region>
          nova · bittorrent
        </span>
      </div>

      <nav className="filters" aria-label="Filters">
        <div className="eyebrow side-eyebrow">Library</div>
        {filters.map((f) => (
          <button key={f.id} className="filter" aria-current={active === f.id} onClick={() => onFilter(f.id)}>
            {active === f.id && <motion.span layoutId="filter-active" className="filter-active" transition={{ duration: 0.38, ease }} />}
            <span className="filter-icon">{f.icon}</span>
            <span className="filter-label">{f.label}</span>
            <span className="filter-count mono">{f.count}</span>
          </button>
        ))}
      </nav>

      <div className="speedcard">
        <div className="eyebrow side-eyebrow">Throughput</div>
        <Speed label="Down" value={stats?.downloadRate ?? 0} tone="down" />
        <Graph className="side-graph" source={() => history.global} windowMs={90_000} />
        <Speed label="Up" value={stats?.uploadRate ?? 0} tone="up" />
        <div className="session mono faint">
          <span>session</span>
          <span>
            ↓ {bytes(stats?.sessionDownloaded ?? 0)} · ↑ {bytes(stats?.sessionUploaded ?? 0)}
          </span>
        </div>
      </div>

      <div className="side-foot">
        <button className="icon-btn" onClick={onSettings} title="Settings (⌘,)" aria-label="Settings">
          <Settings2 size={16} />
        </button>
        <button className="icon-btn" onClick={onTheme} title="Cycle theme (⌘⇧L)" aria-label="Cycle theme">
          <SunMoon size={16} />
        </button>
        <button className="icon-btn" onClick={onPalette} title="Command palette (⌘K)" aria-label="Command palette">
          <CommandIcon size={16} />
        </button>
        <span className="mono faint side-ver">v0.2</span>
      </div>
    </aside>
  );
}
