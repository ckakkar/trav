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
- Native OS drag-and-drop for `.torrent` files (not fully wired yet).
- Full system tray behavior (not fully wired yet).

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

## Run

### TUI (CLI entrypoint)
```bash
cargo run --release -p trav-cli
```

Optional startup torrent:
```bash
cargo run --release -p trav-cli -- /path/to/file.torrent /path/to/downloads
```

### GUI web layer (inside `trav-gui`)
```bash
npm install
npm run dev
```

### Tauri desktop app (inside `trav-gui/src-tauri`)
Use Tauri dev workflow from the GUI project root (expects frontend dev server via `beforeDevCommand`).

## TUI Controls

- `j` / Down Arrow: next torrent row
- `k` / Up Arrow: previous torrent row
- `q`: graceful shutdown

## Notes

- Logs are written to `trav.log` by the CLI runtime to avoid corrupting the terminal UI.
- Current implementation is intentionally iterative; some advanced BitTorrent behaviors are still being expanded.

## License

MIT
