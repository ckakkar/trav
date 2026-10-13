# Trav

A fast, quiet BitTorrent client written in Rust. One headless async engine (`trav-core`) drives three front-ends:

| Front-end | What it is | Run |
|---|---|---|
| **Nova** (desktop) | Tauri 2 app, Next 16 / React 19 UI | `make install-app` (macOS), then open Trav |
| **`trav get`** (one-shot) | download a torrent with a progress bar, then exit | `trav get file.torrent` |
| **Quantum** (terminal) | ratatui dashboard for your library | `trav` |
| **Daemon** (web) | headless engine + the Nova UI over HTTP | `trav --daemon` |

## Quick start: I have a torrent

Install once from a checkout (needs Rust and Node; see [Build](#build)):

```bash
make install        # the `trav` command, into ~/.cargo/bin
make install-app    # macOS: Trav.app, into /Applications
```

Then any of these works:

- **Double-click the `.torrent`** (or click a `magnet:` link in the browser). Trav opens with the add dialog, where you can untick files and pick a folder. Downloads keep seeding from the menu bar after you close the window.
- **One command in a terminal.** It downloads into the current folder, shows progress and exits when done:

  ```bash
  trav get ~/Downloads/ubuntu-26.04-desktop-amd64.iso.torrent
  trav get 'magnet:?xt=urn:btih:…' -o ~/Movies     # magnet link, chosen folder
  ```

  Ctrl-C stops it; run the same command again to resume (data already on disk is verified, not re-downloaded). `--seed` keeps sharing after the download completes, and `--sequential` fetches in order so a video can be previewed early. `trav get` runs on its own, so it works while the desktop app is open and leaves your library alone.
- **Keep it in your library.** `trav file.torrent` opens the terminal UI with the torrent added (press `?` for keys), and `trav --daemon` serves the same UI as the desktop app at http://127.0.0.1:9696.

**[USAGE.md](USAGE.md)** is the full guide: every option and shortcut, settings, where files live, the JSON API and troubleshooting.

The desktop app and the CLI share one library (`~/Library/Application Support/trav` on macOS, `~/.local/share/trav` on Linux, `%APPDATA%\trav` on Windows). The state directory is lock-protected, so only one of them runs the engine at a time.

## Engine

- **Downloading** — rarest-first or sequential picking, per-peer bitfields, adaptive request pipelining (scales with each peer's rate), end-game mode with cancels, per-file priorities (skip / normal / high).
- **Seeding** — inbound listener (dual-stack IPv4/IPv6), choker with optimistic unchoke, upload queue served off the reactor.
- **Discovery** — HTTP and UDP trackers (all tiers, re-announce, `started`/`completed`/`stopped`, IPv6 peers), Mainline DHT (iterative `get_peers` + `announce_peer`; answers queries), PEX, magnet `x.pe` hints, UPnP port mapping.
- **Magnet links** — hex or base32 info-hash; metadata fetched over `ut_metadata` and saved as a `.torrent`.
- **Persistence** — resume data, settings and DHT nodes survive restarts. Resume data is checked against file sizes on start, and missing or truncated files trigger a recheck.
- **Control** — pause/resume, force recheck, reannounce, remove (optionally with data), queueing (`maxActiveDownloads`), a seed-ratio limit, and global rate limits (token bucket).
- **Safety** — every path component from torrent metadata is sanitized and jailed under the save path. The bencode parser has a depth limit, peer frames are size-capped, and peers that send bad data are banned after hash failures.

Not implemented yet: uTP (peers that only speak uTP can't be reached over TCP), MSE/PE encryption, web seeds (BEP 19), BitTorrent v2.

## Nova UI

The design comes from [kkrwhofrags.xyz](https://kkrwhofrags.xyz). There are three themes, cycled with `⌘⇧L`:

- **Ink**: the site's dark tokens (`#0c0d10`, orange `#ff8a3d`).
- **Paper**: the editorial register (`#f5f2ea`, rust `#b93213`, paper grain).
- **Phosphor**: terminal mode (green on black, CRT scanlines).

Type is Geist / Geist Mono with a serif display face. Corners use a 3 px radius, rules are hairlines, and labels are small uppercase mono.

**Motion.** Speeds and percentages ease toward each new value through one shared `requestAnimationFrame` loop, so they update without React re-renders. Progress bars animate their transform across each 500 ms poll. The graphs are time-based canvases that slide continuously. Rows glide when the sort order changes, and switching themes plays a view-transition wipe.

**Features.**
- Sortable, multi-select table with a right-click menu.
- Detail panel with five tabs: Overview (with piece map), Files (priorities), Peers (µTorrent-style flags), Trackers, and Speed.
- Add dialog that lets you pick files before downloading.
- Drag & drop anywhere, and `⌘V` anywhere to paste magnet links.
- Command palette (`⌘K`), settings, and toasts.

**Desktop integration.**
- Native open/folder dialogs and reveal-in-folder.
- Notification when a download completes.
- Tray icon showing live speeds. Closing the window keeps Trav seeding in the tray.
- Single instance: opening a magnet or `.torrent` while Trav is running hands it to the open window.
- Registers as the `magnet:` handler and the `.torrent` file association.

| Keys | |
|---|---|
| `⌘O` / `⌘V` | add .torrent / paste magnet |
| `↑↓` `j k` (`⇧` extends) · `⌘A` | move / select |
| `Space` | pause ↔ resume selection |
| `Del` / `⇧Del` | remove / remove + delete data |
| `↵` | show in folder |
| `1`–`5` | detail tabs |
| `/` · `⌘K` · `⌘,` | filter · palette · settings |

## Build

Requirements:

- Rust 1.90+ (edition 2024)
- Node 24 LTS
- For the desktop app, the [Tauri system dependencies](https://v2.tauri.app/start/prerequisites/). On Linux that means `libwebkit2gtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev`.

```bash
cargo test                                  # unit + loopback swarm integration tests
cd trav-gui && npm install
npm run tauri dev                           # desktop app with hot reload
npm run tauri build                         # installers in target/release/bundle/
```

`cargo build` at the root builds the engine, the TUI and the CLI. The desktop crate is excluded from the default build because it needs WebView libraries. Build it with `cargo build -p trav-desktop`.

### Tests

```bash
make check     # fmt, clippy -D warnings, cargo test, UI typecheck + vitest + build, version sync
make e2e       # Playwright against real daemons (seeder, leecher, token-locked)
```

| Suite | What it covers |
|---|---|
| `trav-core` unit | bencode, metainfo, magnet, picker (incl. 20k-step randomized invariants), storage, wire codec, extensions, DHT, mock HTTP/UDP trackers, rate limiter |
| `trav-core/tests/swarm.rs` | real engines over loopback: download, magnet metadata, magnet over partial data, resume, deleted-file recheck, selective download + HTTP tracker, pause, rate limit, 3-node swarm |
| `trav-core/tests/robustness.rs` | deterministic fuzzing of every untrusted-input parser; path-jail escapes |
| `trav-core/tests/state.rs` | persistence, state-dir locking, settings, the full RPC surface |
| `trav-cli` | HTTP API security (DNS rebinding, CSRF, token), CLI black-box (`--create`, `get` against a live seeder, health check, public-bind guard) |
| `trav-tui` | rendering through ratatui's `TestBackend` |
| `trav-gui` (vitest) | formatting, sorting/filters, transport, history ring |
| `trav-gui/e2e` (Playwright) | add/skip/download, paste-a-magnet, context menu, keyboard, palette, themes, settings, remove + delete, token gate |

CI runs these on Linux, macOS and Windows. It also checks the MSRV (1.90) and supply-chain policy (`cargo-deny`), reports coverage, and builds and smoke-tests the Docker image. Pushing a `v*` tag produces desktop installers (unsigned by default; signing is optional, see CONTRIBUTING), CLI archives with `SHA256SUMS`, and a multi-arch `ghcr.io` image as a draft release.

### Docker (headless / NAS)

```bash
docker run -d --name trav -e TRAV_TOKEN=change-me \
  -p 9696:9696 -p 51413:51413 -p 51413:51413/udp \
  -v trav-data:/data -v ~/Downloads:/downloads ghcr.io/ckakkar/trav:latest
```

The web UI is then at `http://<host>:9696/?token=change-me`. The image refuses to start without `TRAV_TOKEN` because it binds a public address.

### Logs

| Front-end | Location |
|---|---|
| Desktop | `<state dir>/logs/trav-desktop.YYYY-MM-DD.log` |
| TUI | `<state dir>/logs/trav.YYYY-MM-DD.log` |
| `trav get` | `<state dir>/logs/trav-get.YYYY-MM-DD.log` |
| Daemon | stderr |

Logs rotate daily and the last 7 are kept. Set `RUST_LOG=debug` for detail. Panics are logged with a backtrace.

### CLI

```bash
trav get ubuntu.iso.torrent -o ~/Downloads   # download, show progress, exit
trav                                         # terminal UI
trav ubuntu.iso.torrent 'magnet:?xt=…'       # add on start
trav --daemon                                # headless, web UI on http://127.0.0.1:9696
trav --daemon --web-bind 0.0.0.0:9696 --token s3cret   # LAN access (token required)
trav --create ./folder --tracker udp://… -o out.torrent # make a torrent
trav -s ~/Downloads -p 51413                 # set save path / listen port
```

The daemon embeds the static Nova build: run `npm run build` in `trav-gui` before `cargo build --release`. Without a token it only answers requests whose `Host` is loopback, which blocks DNS-rebinding attacks.

### JSON API

Every front-end uses the same API: `POST /api/rpc {"method": …, "params": …}` over HTTP, or `invoke("rpc", …)` in Tauri. Engine events stream as server-sent events at `GET /api/events`.

Methods:

- `snapshot`, `details {hash}`
- `add {torrent|path|magnet, savePath, paused, sequential, filePriorities}`, `inspect`
- `pause` / `resume` / `recheck` / `reannounce` / `remove {hash|hashes, deleteFiles}`
- `pauseAll`, `resumeAll`
- `setFilePriorities`, `setSequential`, `setQueuePosition`, `addPeers`
- `getSettings`, `setSettings`

## Layout

```
trav-core/        engine: bencode, metainfo, magnet, picker, storage, peer wire,
                  extensions (metadata/PEX), DHT, trackers, UPnP, torrent actor, rpc
trav-tui/         ratatui front-end
trav-cli/         `trav` binary: TUI, --daemon web server, --create
trav-gui/         Next.js UI (static export) + src-tauri desktop shell
```

## License

[MIT](LICENSE) · see [USAGE](USAGE.md), [CHANGELOG](CHANGELOG.md), [CONTRIBUTING](CONTRIBUTING.md), [SECURITY](SECURITY.md).
