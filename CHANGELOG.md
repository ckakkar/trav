# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed
- Quitting Trav (⌘Q, the Dock, or relaunching after an update) no longer leaves every transfer paused the next time it opens.
- A tracker whose announce was out while the tracker list changed (the daily public-list refresh, which also runs at launch) could stay "announcing" for the rest of the session and never announce again, while another tracker showed its results. Replies are now matched by URL.
- A magnet could stall for good if the first peer reported the wrong metadata size or sent a bad piece. Metadata is now assembled separately for each size peers report, and a dictionary that fails its hash check starts over at once, with the peers that sent it sitting out briefly.
- A magnet whose info dictionary can't be read now stops with an error instead of sitting at "fetching metadata".
- Adding a torrent that is already in the list now also adds the new link's or file's trackers, not only its peers.
- Stopping or removing a transfer flushes or deletes its files off the engine's async threads, and NAT-PMP no longer blocks one while looking up the router.

## [0.3.43] - 2026-10-15

Every way other clients reach more peers and download faster: µTP and NAT hole punching, web seeds, the Fast extension, local peer discovery and NAT-PMP.

### Added
- µTP (BEP 29): BitTorrent over UDP with LEDBAT congestion control, selective acks, fast retransmit and slow start, on the same port as TCP (the DHT shares its socket). Checked against libtorrent, the engine inside qBittorrent and Deluge, both ways over TCP and µTP. Settings › Network › µTP & hole punching; the Peers tab marks µTP peers **P**.
- NAT hole punching (BEP 55): peers behind home routers and carrier-grade NAT, which couldn't be reached at all before, are connected through a peer connected to both; both sides dial at once over µTP. Trav introduces peers to each other the same way. Marked **h**.
- Web seeds (BEP 19): HTTP mirrors from a torrent's `url-list` or a magnet's `ws=` download alongside peers, four range requests at a time, in runs of whole pieces where the swarm is thinnest. Their data is hash-checked; a mirror that serves bad data, is gone or ignores ranges is dropped. They appear in the Peers tab (**W** while sending). An Internet Archive torrent with no peers online downloaded at about 1.7 MiB/s.
- Fast extension (BEP 6): Have All/Have None, Reject and Allowed Fast both ways, so peers that choke us still hand over a few pieces and refused requests go back to the pool at once.
- Local peer discovery (BEP 14): other clients on your network are found by multicast and dialled first (Settings › Network; marked **L**).
- NAT-PMP port mapping when the router doesn't do UPnP (Settings › Network › Port forwarding).

### Changed
- Connections go TCP first, with µTP joining a second later only if TCP hasn't connected, or at once when TCP is refused or accepted and dropped. On the same Ubuntu swarm this averaged 4.4 to 4.6 MiB/s (the full line), against 2.9 TCP-only and 1.8 µTP-first.
- PEX messages carry peer flags (encryption, seed, µTP, hole punching).
- Two connections to the same peer (as hole punching makes) are resolved the way libtorrent does, so both ends keep the same one.

## [0.3.37] - 2026-10-14

Faster starts and finishes, encryption, a menu bar icon that shows what's happening, and motion throughout.

### Added
- Protocol encryption (MSE/PE): connections are encrypted when the peer supports it, with a plain fallback, which reaches peers that refuse plain BitTorrent and hides the traffic from throttling. Settings › Network › Encryption (Prefer, Require, Off); the Peers tab marks encrypted peers **E**.
- Public trackers: non-private torrents also announce to a list of well-known open trackers, refreshed daily (Settings › Network). In a test with a bare magnet link this found twice the peers and downloaded 3× more in the first minute.
- The menu bar icon animates while downloading (the arrow dips, the line fills with progress and a glint runs along it), shows the speed beside it, and pulses when a download finishes. Its menu lists live totals and the transfers in progress, plus Open Trav, Add Torrent File…, Add Magnet from Clipboard, Pause/Resume All, Settings and Check for Updates.
- "app by cyrus" in the sidebar, linking to kkrwhofrags.xyz, and in About Trav.
- A launch sequence, progress bars whose sheen follows download speed, a sweeping bar while fetching metadata, moving stripes while verifying, a burst when a download completes, and a highlight for new transfers. Decorative motion pauses while the window is in the background.

### Changed
- End-game: up to four peers may race a block that has waited more than 1.5 s, so faster peers rescue blocks stuck on slow ones. The last 5% of a download went from 36 s to 12 s in testing.
- Magnet metadata pieces are asked of a second peer when the first is slow (and given up on after 6 s, was 15 s): magnets start in about 3 to 5 seconds instead of 18 to 24.
- Dead peers are given up on after 6 s (was 10 s), and up to 100 connection attempts run at once (was 64).
- rustls 0.23.46 (RUSTSEC-2026-0346 to 0349).

### Fixed
- Opening a `.torrent` from Finder showed the Add dialog twice.
- Quit (⌘Q) ended the app at once, without saving progress or telling trackers; Dock › Quit, logout and restart now save progress too.
- The last lines of the log were lost when quitting.
- Peers that trackers report at `127.0.0.1` and other unreachable addresses are no longer dialled.

## [0.3.34] - 2026-10-14

Trav is now only the Mac app, and every push to `main` ships to it.

### Added
- Continuous releases: each push to `main` that passes CI is published as `0.3.<commits on main>` with a universal `Trav.dmg`, the signed update and `latest.json`. The app checks hourly.
- An automatic upload limit (on by default, Settings › Bandwidth): with no fixed limit, Trav measures queueing delay from the kernel's TCP round-trip times and eases uploads off when they start flooding the uplink, which on asymmetric home connections slows downloads to a crawl. The status bar shows it as "auto".
- Peer rotation: a download at its connection limit swaps peers that have exchanged no data for a minute for untried ones, every 30 seconds.
- `make bench`: measures the engine's download speed on a real swarm (`trav-core/examples/fetch.rs`).

### Changed
- Peer discovery follows how a download is doing rather than how many peers are connected: while fewer than five peers are sending, the DHT and trackers are asked again every two minutes.
- New peers start with 16 requests in flight (was 8) and are topped up once a quarter have returned (was half), so they reach full speed sooner on high-latency connections.
- Up to 150 peers per torrent and 600 overall (was 80 and 500); settings still at the old defaults move to the new ones.
- Raising or removing a speed limit takes effect at once for transfers already being throttled.

### Removed
- The terminal UI, the `trav` command (`trav get`, the daemon and its web UI) and the Docker image.

### Fixed
- Torrent files whose names became the same after sanitising (`a:b` and `a?b`), or differed only in case, were written to one file and overwrote each other; they now get " (2)"-style names. Files can no longer take a folder's name.
- Writes no longer follow symlinks below the save folder (the save folder itself may still be one).
- DHT replies are only accepted from the node that was asked.
- Turning the DHT off (or changing the port) now really stops it and frees its port; queries in flight fail at once.
- HTTP tracker replies are capped at 2 MiB, and pieces awaiting their hash check count against the memory budget for partial pieces (at least two pieces, so huge pieces no longer allow 1 GiB of buffers).
- `setSettings` with only some fields changed reset the others to defaults.
- Choosing files when adding a torrent was ignored for torrents with padding files.
- "Open" in the Files tab used the name from the torrent rather than the file's name on disk.

## [0.3.0] — 2026-10-14

The desktop app is now **Trav for macOS**: the same UI, with the parts that make it a Mac app.

### Added
- Automatic updates from GitHub releases. Trav checks shortly after it opens and every six hours (**Trav › Check for Updates…** any time), shows what's new, verifies the update's signature, installs it, stops transfers cleanly and relaunches. Releases publish a signed update archive and `latest.json` for it.
- A full macOS menu bar: File (Open Torrent File ⌘O, Add Magnet Link ⌘U), Edit, View (lists ⌘1–⌘7, details ⌘I, palette ⌘K, theme), Transfers (pause/resume ⌘., pause/resume all, Show in Finder ⌘R, copy magnet, download in order, queue order, verify, reannounce, remove ⌘⌫ / ⌥⌘⌫), Window and Help. Items enable and relabel themselves with the selection.
- The Dock icon shows a progress bar across everything that's downloading.
- "Remove and delete data" in the app moves the files to the Trash instead of erasing them.
- Settings › App: check for updates automatically, open at login (starts in the background), keep the Mac awake while transferring.
- The window remembers its size and position.
- A new app icon and menu bar glyph (a "T" whose stem is a download arrow), rendered by `scripts/app-icon.swift` (`make icons`).
- `trav get <torrent|magnet>…`: download into the current folder (or `-o DIR`) with a live progress display, then exit. Works while the desktop app is open, leaves the library alone, resumes by verifying data already on disk; `--seed` and `--sequential` options.
- `make install` (the `trav` command) and `make install-app` (macOS `Trav.app` in /Applications); bare `make` lists every task.
- A README quick start for "I have a torrent", and [USAGE.md](USAGE.md): a complete guide to every front-end, the settings, the library layout, the JSON API and troubleshooting.

### Changed
- Announces that become due (start, completion, reannounce) go out immediately instead of on the next 1 s tick.
- Starting the terminal UI while another Trav holds the library now says so and suggests `trav get`.
- Dependencies upgraded to their latest stable releases: reqwest 0.13 (rustls + aws-lc-rs, OS trust store), rand 0.10, thiserror 2, sha1 0.11, igd-next 0.18, base64 0.23, fs4 1, dirs 7; Next 16.4, TypeScript 7, Playwright 1.64, Tauri 2.12.3. The desktop crate moves to edition 2024.
- CI, releases and the Docker image build with Node 24 LTS; the image is based on Debian 13 (trixie); GitHub Actions on their current majors.
- The app ships as one universal `.dmg` for Apple Silicon and Intel, ad-hoc signed (Developer ID signing and notarization stay optional), and releases publish themselves once every asset is uploaded.

### Removed
- The Windows and Linux builds of the desktop app. The `trav` command (terminal UI, `trav get`, the daemon and web UI) and the Docker image still run there.

### Fixed
- DHT peer searches could come back empty for minutes at a time. The routing table only ever held our own neighbourhood (15–30 nodes, so it re-bootstrapped every minute) and searches could be steered into clusters of fake nodes. Buckets are now filled by refresh lookups (typically 120–140 nodes), quiet nodes are re-checked, and the table and each search accept one node per public IP.
- The Settings dialog discarded unsaved edits: it re-fetched the settings on every app re-render (each 500 ms poll), so typed values reverted.
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
