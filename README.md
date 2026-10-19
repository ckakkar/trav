# Trav

A fast, quiet BitTorrent client for the Mac. A native app around a headless async Rust engine, with a UI of its own.

**[Download Trav.dmg](https://github.com/ckakkar/trav/releases/latest/download/Trav.dmg)** · universal (Apple Silicon and Intel) · macOS 11+ · updates itself

Open the `.dmg` and drag Trav into Applications. The first launch needs **Open Anyway** once in System Settings › Privacy & Security, because the app isn't notarized ([details](USAGE.md#install)). Then double-click any `.torrent` or click a `magnet:` link.

**[USAGE.md](USAGE.md)** is the guide: adding torrents, menus and shortcuts, settings, speed, where files live and troubleshooting.

## The app

- **Native Mac app.** A full menu bar (File, Edit, View, Transfers, Window, Help) with Mac shortcuts, Show in Finder, native open and folder dialogs, notifications when downloads finish, a progress bar on the Dock icon, and open at login. It is the `magnet:` handler and opens `.torrent` files.
- **Alive in the menu bar.** While downloading, Trav's menu bar icon animates and fills with overall progress, with the speed beside it; its menu lists the transfers in progress and the everyday actions.
- **Motion with purpose.** A short launch sequence where the mark assembles itself; progress bars whose sheen speeds up with the download; a burst when a download completes; new transfers sweep in. Decorative motion rests while the window is in the background, and respects Reduce Motion.
- **Always up to date.** Every push to `main` that passes CI is published as a new version. Trav checks hourly (or **Trav › Check for Updates…**), shows what changed, verifies the update's signature, installs it and relaunches.
- **Careful with your Mac.** "Remove and delete data" moves files to the Trash. It keeps the Mac awake only while transferring, remembers the window, and keeps running when you close it.
- **The Nova UI**, with three themes that differ in material, not just color: **Ink** (dark: floating panels, light that glows with your download speed), **Paper** (light: a risograph print in navy and fluorescent pink, condensed poster type, highlighter selection) and **Phosphor** (the terminal of [kkrwhofrags.xyz](https://kkrwhofrags.xyz): green on black, CRT scanlines). Each moves in its own way, from dialogs and menus to how a finished download is marked. It has a sortable multi-select table, a detail panel (Overview with piece map, Files with priorities, Peers, Trackers, Speed), an add dialog for choosing files, drag & drop, ⌘V to paste magnets, and a command palette (⌘K).

## The engine (`trav-core`)

- **Downloading:** rarest-first or sequential picking, per-peer bitfields, adaptive request pipelining, an end-game that lets faster peers rescue blocks stuck on slow ones, and per-file priorities (skip, normal or high).
- **Web seeds (BEP 19):** HTTP mirrors listed in a torrent (`url-list`) or a magnet (`ws=`) download alongside peers, in runs of whole pieces fetched with range requests, starting where the swarm is thinnest. Their data is hash-checked like any peer's, and a mirror that serves bad data is dropped.
- **Connections:** TCP and µTP (BEP 29, with LEDBAT congestion control, selective acks and fast retransmit) on one port. TCP goes first because it downloads faster; µTP reaches peers TCP can't. **NAT hole punching (BEP 55)** connects to peers behind home routers and carrier-grade NAT, which nothing could reach before, by having a peer connected to both introduce them so both sides dial at once; Trav introduces peers to each other the same way. The **Fast extension (BEP 6)** lets peers that are choking us hand over their allowed-fast pieces, and turns refused requests around at once.
- **Peers:** up to 150 per torrent. Idle peers are swapped for untried ones when a torrent is at its limit, and peer discovery speeds up while a download is starving.
- **Uploads:** an automatic upload limit keeps seeding from flooding your uplink. It works like LEDBAT, measuring each connection's queueing delay from the kernel's TCP round-trip times and easing off when uploads cause it. A choker with optimistic unchoke and an inbound dual-stack listener round this out.
- **Discovery:** HTTP and UDP trackers (all tiers) plus a daily-refreshed list of public trackers for non-private torrents, Mainline DHT (sharing µTP's UDP port; routing-table refresh, protection against fake-node clusters, replies checked against the node asked), PEX with peer flags, local peer discovery on your network (BEP 14), magnet `x.pe` hints, and port mapping by UPnP or NAT-PMP. Magnet metadata is raced across peers, so a magnet starts in seconds.
- **Encryption:** MSE/PE (Diffie-Hellman, then RC4) both ways; encrypted first with a plain fallback, or required.
- **Persistence:** resume data, settings and DHT nodes survive restarts, and missing or truncated files trigger a recheck.
- **Safety:** paths from torrents are sanitised, made unique and kept inside the save folder, never followed through symlinks. Parsers are fuzz-tested, peer frames and tracker replies are size-capped, piece buffers are bounded, and peers that send bad data are banned.

Not implemented yet: BitTorrent v2 and the IPv6 DHT (BEP 32).

## Build from source

Needs macOS, [Rust](https://rustup.rs) 1.90+ and [Node](https://nodejs.org) 24 LTS.

```bash
make install    # build Trav.app and install it in /Applications
make run        # the app with hot reload
make check      # fmt, clippy, engine tests, UI typecheck + unit tests + build (what CI runs)
make bench TORRENT=file.torrent   # measure the engine's download speed on a real swarm
make            # every task
```

```
trav-core/          the engine: bencode, metainfo, magnet, picker, storage, peer wire,
                    encryption, extensions (metadata/PEX/holepunch), µTP, DHT, trackers,
                    web seeds, local discovery, UPnP/NAT-PMP, upload governor, rpc
trav-gui/           the Nova UI (Next.js static export)
trav-gui/src-tauri/ the macOS app: hosts the engine, menu bar, Dock, updates, Trash, login item
scripts/            release manifest, version check, app icon renderer
```

See [CONTRIBUTING](CONTRIBUTING.md) for how releases and signing work, [CHANGELOG](CHANGELOG.md) and [SECURITY](SECURITY.md).

## License

[MIT](LICENSE)
