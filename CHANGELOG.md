# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] — 2026-10-04

### Added
- Complete BitTorrent engine: per-peer piece selection (rarest-first or sequential), adaptive pipelining, end-game, seeding with a choker and optimistic unchoke, inbound dual-stack listener.
- Magnet links (hex/base32, `x.pe`) with `ut_metadata`, Mainline DHT, PEX, UPnP port mapping.
- Multi-tracker HTTP/UDP announces with re-announce and `started`/`completed`/`stopped` events.
- Persistence (resume data, settings, DHT nodes), pause/resume/recheck/remove, queueing, ratio limit, global rate limits, per-file priorities.
- Shared JSON-RPC surface used by the desktop app (IPC) and `trav --daemon` (HTTP + SSE).
- Nova desktop app on Tauri 2: native dialogs, tray with live speeds, close-to-tray, notifications, single instance, `magnet:` handler, `.torrent` association.
- New UI in the kkrwhofrags.xyz design language with Ink, Paper and Phosphor themes.
- Rewritten terminal UI; `trav --create` to make torrents.
- Docker image for headless/NAS use.
- Test suites: engine unit + loopback swarm integration, parser fuzzing, HTTP security, CLI, TUI rendering, frontend unit, Playwright end-to-end; CI on Linux/macOS/Windows and tagged release builds.

### Security
- Torrent paths are sanitised and jailed under the save directory.
- Web API: loopback `Host` check (DNS rebinding), cross-origin rejection (CSRF), constant-time token comparison, security headers, refuses public binds without a token.

### Fixed
- The previous GUI failed to build (`window is not defined` during static export).
- The previous engine requested pieces peers did not have, never uploaded, and lost all progress on restart.
