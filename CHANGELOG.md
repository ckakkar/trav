# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `trav get <torrent|magnet>…`: download into the current folder (or `-o DIR`) with a live progress display, then exit. Works while the desktop app is open, leaves the library alone, resumes by verifying data already on disk; `--seed` and `--sequential` options.
- `make install` (the `trav` command) and `make install-app` (macOS `Trav.app` in /Applications); bare `make` lists every task.
- A README quick start for "I have a torrent".

### Changed
- Announces that become due (start, completion, reannounce) go out immediately instead of on the next 1 s tick.
- Starting the terminal UI while another Trav holds the library now says so and suggests `trav get`.
- Dependencies upgraded to their latest stable releases: reqwest 0.13 (rustls + aws-lc-rs, OS trust store), rand 0.10, thiserror 2, sha1 0.11, igd-next 0.18, base64 0.23, fs4 1, dirs 7; Next 16.4, TypeScript 7, Playwright 1.63. The desktop crate moves to edition 2024.
- CI, releases and the Docker image build with Node 24 LTS; the image is based on Debian 13 (trixie); GitHub Actions on their current majors.

### Fixed
- Adding a magnet whose files were partly on disk already (or rechecking a torrent) dropped every peer and then backed off from them for a minute or more, stalling the download.
- CI failed on current stable clippy (`chunks_exact_to_as_chunks`), and an HTTP tracker swarm test was flaky on macOS and Windows.
- `scripts/check-versions.sh`, and with it `make check`, failed on macOS (GNU-only `sed` syntax).
- First runs printed "Error reading the log directory" because the log directory did not exist yet.

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
