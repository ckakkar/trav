# Trav - Headless BitTorrent Engine with Premium Interfaces

Trav is a Rust BitTorrent project built around a shared, headless engine (`trav-core`) with multiple frontends:
- `trav-tui` (Quantum terminal dashboard)
- `trav-gui` (Nova desktop app via Tauri + Next.js + TypeScript + Tailwind)
- `trav-cli` (runtime bootstrap + TUI launcher)

The design goal is one core engine, multiple interfaces, with non-blocking shared telemetry.

## Workspace

Cargo workspace members:
- `trav-core`
- `trav-tui`
- `trav-cli`
- `trav-gui/src-tauri`

## Phase Status

### Phase 1 - Core Protocol & Engine

#### Objective
Build a robust asynchronous headless BitTorrent engine with clean module boundaries and stable runtime behavior.

#### Delivered
- `trav-core` engine loop with command/event channels.
- Bencode `.torrent` parsing and info-hash generation.
- Peer wire protocol framing and handshake baseline.
- Tracker integration foundations (HTTP and UDP paths).
- Piece manager with rarest-first selection primitives.
- Async disk task abstraction with bounded queue.

### Phase 2 - Metadata, Magnet, and Discovery Expansion

#### Objective
Expand discovery and metadata capability beyond basic single tracker flows.

#### Delivered
- Magnet parsing module and extension protocol scaffolding.
- DHT/KRPC scaffolding modules for iterative evolution.
- Expanded tracker support and peer parsing paths.
- Core module split for protocol surface growth (`tracker`, `dht`, `magnet`, `peer`).

### Phase 3 - Operational Interface Baseline

#### Objective
Introduce a usable live operator interface on top of the headless engine.

#### Delivered
- Initial `trav-tui` dashboard using `ratatui` + `crossterm`.
- CLI bootstrap (`trav-cli`) to run runtime, engine, and interface together.
- Real-time table/log display and keyboard navigation loop.

### Phase 4 - Premium Interfaces

#### Objective
Upgrade the terminal interface into a dense operator dashboard and introduce a modern desktop GUI, both powered by the same `trav-core`.

#### Delivered
- Added `trav-gui/src-tauri` to the Cargo workspace.
- Introduced Nova GUI stack with:
  - Next.js + TypeScript + Tailwind CSS
  - Dark-themed dashboard style
  - Live telemetry cards and charts
  - Piece map visualization panel
- Upgraded Quantum TUI with:
  - Constraint-based, high-density layout
  - Speed sparklines
  - Torrent table and live logs
- Established shared state exposure from `trav-core` using `Arc<RwLock<EngineSnapshot>>`.

#### Pending / Partial
- Native OS drag-and-drop for `.torrent` files is wired in Nova (desktop runtime).
- System tray support is wired (`Show/Hide`, `Quit`).

### Phase 5 - Live Engine Wiring

#### Objective
Wire both interfaces to live engine state and event flow without UI-specific logic inside core networking paths.

#### Delivered
- `trav-core` emits and updates shared real-time snapshot state.
- `trav-tui` reads snapshot on periodic ticks and renders live torrent/peer metrics.
- `trav-gui` reads snapshot through Tauri IPC (`get_snapshot`) and updates UI continuously.
- Swarm loop integrated with tracker-discovered peers and request pipeline.

### Phase 6 - Security & Stability Audit

#### Objective
Harden core runtime against path traversal, async stalls, unbounded memory pressure, and abusive peers.

#### Delivered
- **Path traversal/jail hardening**
  - Added strict metadata path sanitization.
  - Enforced jailed output paths under per-torrent root (`download_dir/<info_hash>/...`).
  - Rejects unsafe segments (`..`, absolute/prefix paths, separators, NUL, unsafe trailing characters).
- **Tokio blocking mitigation**
  - Piece SHA-1 verification moved to `tokio::task::spawn_blocking`.
- **Backpressure**
  - Disk queue remains bounded; explicit queue capacity constant introduced.
- **Timeouts aligned for global swarms**
  - Handshake timeout: 15s
  - Idle/read timeout: 90s
  - Piece request timeout: 30s
  - Write operations wrapped with explicit timeout paths.
- **Protocol and abuse safeguards**
  - Peer codec max message length guard.
  - Retry budget per piece.
  - Adaptive peer penalty scoring with category split:
    - network penalties (timeouts)
    - data penalties (bad/mismatched/hash-failed payloads)
  - Peer backoff and disconnect threshold logic.
- **Observability**
  - Peer health metrics exposed in snapshot and surfaced in GUI/TUI views.

## In-Depth Usage Guide

### 1) Prerequisites
- Rust toolchain (`rustup`, `cargo`)
- Node.js 18+ and npm (for `trav-gui`)
- Platform dependencies required by Tauri (WebView toolchain)

### 2) Build / check the workspace
From repo root:
```bash
cargo check -p trav-core -p trav-tui -p trav-cli -p app
```

### 3) Run the terminal client (Quantum TUI)
TUI is launched through `trav-cli`:
```bash
cargo run --release -p trav-cli
```

Start with a torrent immediately:
```bash
cargo run --release -p trav-cli -- /path/to/file.torrent /path/to/downloads
```

Behavior:
- boots Tokio runtime
- starts `trav-core` in background
- renders TUI on main thread
- writes logs to `trav.log`

### 4) Use TUI controls
- `j` / Down Arrow: next torrent row
- `k` / Up Arrow: previous torrent row
- `q`: graceful shutdown

### 5) Run Nova GUI (web layer only)
Inside `trav-gui`:
```bash
npm install
npm run dev
```

This starts Next.js UI at `http://localhost:3000` (without desktop APIs like file-drop/system tray).

### 6) Run Nova as desktop app (Tauri + Next.js)
From repo root:
```bash
cargo run -p app
```

Or from `trav-gui/src-tauri`:
```bash
cargo run
```

Desktop-only behavior:
- uses Tauri IPC to poll `EngineSnapshot`
- accepts drag-and-drop of `.torrent` files
- forwards dropped files to `trav-core` via `Command::AddTorrent`
- exposes system tray menu:
  - `Show/Hide`
  - `Quit`

### 7) Add torrents in practice
- **TUI/CLI path:** launch with startup args:
  - `cargo run --release -p trav-cli -- /path/file.torrent /path/downloads`
- **GUI desktop path:** drag a `.torrent` file onto the Nova window.

### 8) Download path and safety model
- Output is jailed under:
  - `download_dir/<info_hash_hex>/<sanitized_name>`
- Unsafe paths from torrent metadata are rejected.
- Multi-file torrents are validated for path safety and currently fail closed where unsupported.

### 9) Peer health and stability behavior
- Adaptive penalties track:
  - network faults (timeouts)
  - data faults (invalid/mismatched/hash-failed blocks)
- Slow/bad peers are backoff-throttled, then disconnected above threshold.
- Piece verification runs off reactor threads via `spawn_blocking`.
- Channels between swarm and disk remain bounded for backpressure.

### 10) Troubleshooting
- **No GUI updates:** ensure you are running desktop (`cargo run -p app`), not only `npm run dev`.
- **Dropped file ignored:** confirm extension is `.torrent` and path exists.
- **No peers discovered:** verify tracker reachability and torrent health.
- **Tauri build issues:** install required OS-level WebView/Tauri dependencies.

## Notes

- Logs are written to `trav.log` by the CLI runtime to avoid corrupting the terminal UI.
- Current implementation is intentionally iterative; some advanced BitTorrent behaviors are still being expanded.

## License

MIT
