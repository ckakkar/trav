# Using Trav

Trav is a BitTorrent client with four ways in, all driven by the same engine:

| You want to… | Use |
|---|---|
| Download one torrent and be done | [`trav get`](#1-download-a-torrent-in-one-command-trav-get) |
| Manage downloads in a window | [the desktop app](#2-the-desktop-app) |
| Manage downloads in a terminal | [the terminal UI](#3-the-terminal-ui) |
| Run it on a server or NAS and use it from a browser | [the daemon](#4-the-daemon-and-web-ui) |

**Contents:**
[Install](#install) ·
[`trav get`](#1-download-a-torrent-in-one-command-trav-get) ·
[Desktop app](#2-the-desktop-app) ·
[Terminal UI](#3-the-terminal-ui) ·
[Daemon & web UI](#4-the-daemon-and-web-ui) ·
[Settings](#settings) ·
[Where things live](#where-things-live) ·
[Making a torrent](#making-a-torrent) ·
[Scripting](#scripting-the-json-api) ·
[Troubleshooting](#troubleshooting) ·
[Limitations](#limitations)

---

## Install

### From a checkout

You need [Rust](https://rustup.rs) 1.90 or newer and [Node](https://nodejs.org) 24 LTS. For the desktop app on Linux you also need the [Tauri system libraries](https://v2.tauri.app/start/prerequisites/) (`libwebkit2gtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev`).

```bash
git clone https://github.com/ckakkar/trav && cd trav
make install        # the `trav` command → ~/.cargo/bin (make sure that is on your PATH)
make install-app    # macOS only: Trav.app → /Applications
```

Run `make` on its own to see every task. On Windows and Linux, `make desktop-build` produces installers in `target/release/bundle/`.

### Docker (headless)

```bash
docker run -d --name trav -e TRAV_TOKEN=pick-a-secret \
  -p 9696:9696 -p 51413:51413 -p 51413:51413/udp \
  -v trav-data:/data -v ~/Downloads:/downloads ghcr.io/ckakkar/trav:latest
```

Then open `http://<host>:9696/?token=pick-a-secret`. See [the daemon](#4-the-daemon-and-web-ui).

### Uninstall

`make uninstall` removes the `trav` command; drag `Trav.app` to the Bin. Neither touches your downloads or your [library](#where-things-live). Delete the library folder too if you want a clean slate.

---

## 1. Download a torrent in one command: `trav get`

```bash
trav get ubuntu-26.04-desktop-amd64.iso.torrent
```

That downloads into the **current folder**, shows live progress, and exits when the download is complete:

```
Downloading to /Users/you/Downloads
ubuntu-26.04-desktop-amd64.iso  ████████████▋·······  63.4%  3.8 GiB / 6.0 GiB  ↓ 11.2 MiB/s  48 peers · 3m21s left
✓ ubuntu-26.04-desktop-amd64.iso  6.0 GiB  →  /Users/you/Downloads/ubuntu-26.04-desktop-amd64.iso
```

### More examples

```bash
trav get 'magnet:?xt=urn:btih:…'                  # a magnet link (quote it: it contains & and ?)
trav get c12fe1c06bba254a9dc9f519b335aa7c1367a88a  # a bare info-hash works too
trav get movie.torrent -o ~/Movies                # choose the folder
trav get a.torrent b.torrent 'magnet:?…'          # several at once
trav get show.torrent --sequential                # fetch in order, to start watching early
trav get linux.iso.torrent --seed                 # keep sharing afterwards until Ctrl-C
```

| Option | Meaning |
|---|---|
| `-o, --output DIR` | Folder to download into. Created if missing. Default: the current folder. |
| `--seed` | Don't exit when done; keep uploading until you press Ctrl-C (or your [ratio limit](#settings) is reached). |
| `--sequential` | Download pieces in order instead of rarest-first, so the start of a video is playable early. Slightly worse for the swarm; use it when you need it. |

### Stopping and resuming

Press **Ctrl-C** at any time. Trav tells the trackers it is leaving and exits. To carry on later, **run the same command again**: Trav verifies what is already on disk (`verifying data on disk …%`) and downloads only what is missing. Nothing is downloaded twice.

### What it does and doesn't touch

- It runs a private, throwaway engine. It works **while the desktop app or a daemon is open**, and the torrent does **not** appear in your library afterwards. If you want it in the library, open it in the app or use `trav file.torrent`.
- It uses your saved [settings](#settings) (rate limits, port, DHT, …) but never changes them.
- Exit status: `0` everything finished, `1` something failed (the reason is printed), `130` you stopped it first. Handy in scripts:

  ```bash
  trav get "$TORRENT" -o /srv/media && notify-send "done"
  ```

- When output isn't a terminal (piped, cron, CI) it prints a plain status line every 10 seconds instead of redrawing.

---

## 2. The desktop app

Open **Trav** from Applications (or the Start menu). Downloads go to `~/Downloads` unless you change it.

> **First launch of a downloaded (not self-built) build on macOS:** the release builds aren't notarized, so macOS says it "can't be opened". Right-click Trav.app → **Open** → **Open** once; after that it opens normally. Apps you build with `make install-app` don't need this.

### Adding torrents

Any of these opens the **Add** dialog:

- **Double-click a `.torrent` file** in Finder/Explorer. Trav is registered as the handler.
- **Click a `magnet:` link** in your browser. Trav is the `magnet:` handler; allow the browser to open it.
- **Drag** `.torrent` files onto the window.
- **⌘V** anywhere in the window to paste a magnet link (or several, one per line).
- **⌘O** to pick `.torrent` files.

In the Add dialog you can untick files you don't want, pick a different save folder, turn on **Sequential download**, and untick **Start immediately** to add it paused. If Trav is already running, opening a torrent from outside hands it to the open window.

### The list and the detail panel

Click a torrent to select it (⇧/⌘-click for several) and **right-click** for pause/resume, force recheck, reannounce, show in folder, copy magnet link or info-hash, queue order and remove. The panel below has five tabs:

| Tab | Shows |
|---|---|
| **Overview** | progress, speeds, ratio, ETA, save path and a piece map (downloaded, in flight, missing) |
| **Files** | per-file progress; set a file to *skip*, *normal* or *high* priority at any time |
| **Peers** | who you're connected to, their client, speeds and [flags](#peer-flags) |
| **Trackers** | each tracker's status, peers/seeds it reports and when it's next contacted |
| **Speed** | a live graph of download and upload |

### Keyboard

⌘ is Ctrl on Windows and Linux.

| Keys | Action |
|---|---|
| ⌘O / ⌘V | add `.torrent` / paste magnet |
| ↑ ↓ or j k (⇧ extends) · ⌘A | move / select |
| Space | pause ↔ resume selection |
| Del / ⇧Del | remove / remove **and delete the files** |
| ↵ | show in Finder / Explorer |
| 1 – 5 | detail tabs |
| / | filter the list |
| ⌘K | command palette (every action, searchable) |
| ⌘, | settings |
| ⌘⇧L | cycle theme: Ink (dark) → Paper (light) → Phosphor (terminal green) |

### Closing vs quitting

Closing the window **keeps Trav running in the menu bar / system tray** so it can keep seeding; the tray icon shows live speeds and has Pause all / Resume all. Use **Quit Trav** in the tray menu (or ⌘Q) to stop it; it saves progress and tells trackers it's leaving first. You get a notification when a download completes (switch it off in Settings).

---

## 3. The terminal UI

```bash
trav                          # open your library
trav file.torrent 'magnet:?…' # …and add these first
trav -s ~/Media               # change the default download folder (saved)
trav --web                    # also serve the web UI on http://127.0.0.1:9696 while it runs
```

Press **a** (or **o**), paste a magnet link, an info-hash or a path to a `.torrent` (`~` works), then Enter. **?** shows every key:

| Keys | Action |
|---|---|
| j k ↑ ↓ · g G | move · first / last |
| a o | add a magnet / `.torrent` path |
| space p | pause / resume |
| P U | pause all / resume all |
| d / D | remove (keep data) / remove **and delete data** (asks y/n) |
| r | force recheck |
| R | reannounce to trackers and DHT |
| s | toggle sequential download |
| tab 1–5 · J K | detail panel tabs · scroll it |
| q esc | quit (progress is saved) |

The terminal UI and the desktop app share one library, but only one can have it open at a time (see [troubleshooting](#another-trav-instance-is-using-the-library)).

---

## 4. The daemon and web UI

`trav --daemon` runs the engine without a terminal UI and serves the same interface as the desktop app in a browser.

```bash
trav --daemon                                          # http://127.0.0.1:9696, this machine only
trav --daemon --web-bind 0.0.0.0:9696 --token s3cret   # reachable from your network
```

- Without `--token`, the web UI is **loopback-only**: Trav refuses to listen on a network address unless a token is set.
- With a token, open `http://<host>:9696/?token=s3cret` once; the browser remembers it. Scripts send it as `Authorization: Bearer s3cret` (or an `X-Trav-Token` header).
- `TRAV_TOKEN` and `TRAV_STATE_DIR` can be set as environment variables instead of flags.
- `trav --health-check --web-bind 127.0.0.1:9696` exits 0 if a daemon is answering (used by the Docker image).
- Logs go to stderr (`docker logs trav`). Stop with Ctrl-C or `SIGTERM`; it saves state first.

**Docker volumes:** `/data` holds the library (settings, resume data), `/downloads` is where files go. Publish port 51413 over TCP and UDP so peers can reach you.

---

## Settings

Change settings in the app or web UI (**⌘,**), or in the terminal with `-s/--save-path` and `-p/--port` (both are saved). They live in `settings.json` in the [library folder](#where-things-live), which you can also edit while Trav is closed.

| Setting (`settings.json` key) | Default | What it does |
|---|---|---|
| Default save folder (`downloadDir`) | `~/Downloads` | where new torrents go unless you pick another |
| Listen port (`listenPort`) | 51413 | TCP for peers, UDP for DHT; if taken, the next free port is used |
| Download / upload limit (`downloadLimit`, `uploadLimit`) | 0 = unlimited | bytes per second, for everything combined |
| Active downloads (`maxActiveDownloads`) | 5 | more than this wait in the queue; 0 = no limit |
| Peers per torrent / overall (`maxPeersPerTorrent`, `maxPeersGlobal`) | 80 / 500 | connection caps |
| Upload slots per torrent (`uploadSlots`) | 8 | peers you upload to at once |
| DHT (`enableDht`) | on | trackerless peer discovery; needed for most magnet links |
| Peer exchange (`enablePex`) | on | learn about peers from peers |
| UPnP port forwarding (`enableUpnp`) | on | ask your router to open the listen port |
| Stop seeding at ratio (`seedRatioLimit`) | 0 = seed forever | e.g. `2` stops once you've uploaded twice the size; resuming a stopped torrent overrides it |

---

## Where things live

Your **library** (the list of torrents and their progress) is stored here:

| OS | Folder |
|---|---|
| macOS | `~/Library/Application Support/trav` |
| Linux | `~/.local/share/trav` |
| Windows | `%APPDATA%\trav` |
| Docker | `/data` |

Set `TRAV_STATE_DIR` (or `--state-dir`) to use another folder, for example a second, separate library.

| Inside | Contents |
|---|---|
| `settings.json` | your [settings](#settings) |
| `torrents/<info-hash>.torrent` / `.json` | each torrent's metadata and resume data (which pieces you have, priorities, totals) |
| `dht.json` | known DHT nodes, so the next start finds peers faster |
| `logs/` | daily logs, last 7 kept: `trav-desktop.*`, `trav.*` (terminal UI), `trav-get.*` |
| `.lock` | held while a Trav has the library open |

The downloaded files themselves are only in your download folders. Removing a torrent keeps them unless you choose **remove and delete data**.

---

## Making a torrent

```bash
trav --create ./my-album -o my-album.torrent --tracker udp://tracker.example:1337/announce
```

Works on a file or a folder; the piece size is picked automatically. Add `--private` for private trackers (disables DHT and PEX for it). The info-hash is printed first. To seed it, open the `.torrent` in Trav and point the save folder at the folder that *contains* `my-album`.

---

## Scripting: the JSON API

The desktop app, web UI and daemon all speak the same API. With a daemon running:

```bash
api() { curl -s http://127.0.0.1:9696/api/rpc -H 'content-type: application/json' -d "$1"; }

api '{"method":"add","params":{"magnet":"magnet:?xt=urn:btih:…","savePath":"/srv/media"}}'
api '{"method":"snapshot"}'                         # every torrent + global stats
api '{"method":"details","params":{"hash":"<info-hash>"}}'
api '{"method":"pause","params":{"hashes":["<hash1>","<hash2>"]}}'
api '{"method":"remove","params":{"hash":"<info-hash>","deleteFiles":true}}'
api '{"method":"setSettings","params":{"downloadLimit":5000000}}'
```

Responses are `{"ok":true,"result":…}` or `{"ok":false,"error":"…"}`. Add `-H 'authorization: Bearer <token>'` if the daemon has a token.

| Method | Params |
|---|---|
| `snapshot` | none |
| `details` | `hash` |
| `add` | one of `magnet`, `path` (a `.torrent` on the server), `torrent` (base64 contents); optional `savePath`, `paused`, `sequential`, `filePriorities` |
| `inspect` | same sources as `add`; returns the file list without adding |
| `pause` `resume` `recheck` `reannounce` | `hash` or `hashes` |
| `remove` | `hash` or `hashes`, `deleteFiles` |
| `pauseAll` `resumeAll` | none |
| `setFilePriorities` | `hash`, `priorities` (one per file: 0 skip, 1 normal, 2 high) |
| `setSequential` | `hash`, `enabled` |
| `setQueuePosition` | `hash`, `position` (0 = top) |
| `addPeers` | `hash`, `peers` (`["1.2.3.4:6881", …]`) |
| `getSettings` / `setSettings` | none / the [settings](#settings) object (camelCase keys) |

Live events (added, metadata received, completed, error, removed) stream as server-sent events from `GET /api/events`.

---

## Troubleshooting

### "Another Trav instance is using" the library

The error reads *another Trav instance is using …*. Only one Trav can have the library open: the desktop app, the terminal UI or a daemon. Either add the torrent in the one that's running, quit it first (desktop: tray icon → **Quit Trav**; closing the window isn't enough), or use [`trav get`](#1-download-a-torrent-in-one-command-trav-get), which doesn't need the library.

### Stuck on "looking for peers…" or 0 peers

- Give it a minute: DHT discovery can take 30–60 s on a cold start.
- Check **DHT** is on in Settings. Torrents without working trackers depend on it.
- The torrent may simply be dead (no one seeding). The Trackers tab shows what each tracker reports.
- Firewalls or VPNs that block outgoing UDP stop both DHT and UDP trackers.

### Stuck on "fetching metadata"

A magnet link has no file list until a peer sends it, so this is the same problem as having no peers (see above). Once one peer answers, it moves on and saves the `.torrent` so it never has to fetch it again.

### Slow, or "not connectable"

The status bar shows **● open** once another peer has connected *to* you. If it never appears, you can only reach peers that are themselves reachable, which roughly halves your options. Turn on **UPnP** in Settings, or forward the listen port (TCP and UDP, default 51413) on your router to this machine. Also check you haven't set a download/upload limit, and that the [upload limit](#settings) isn't tiny: peers favour those who upload back.

### A torrent shows "Error"

The reason is shown under its name and in the Overview tab. **disk write failed** usually means the disk is full or the folder isn't writable. Free some space (or fix permissions), then **resume** it; verified data is kept. **invalid file layout** means the torrent's file names can't be stored safely on this system.

### Files were moved or deleted behind Trav's back

Trav notices on the next start and re-verifies, then downloads whatever is missing. To force it now: **recheck** (`r` in the terminal UI, right-click in the app).

### "Web UI not built"

The `trav` binary embeds the web UI at build time. Build with `make install` (or `make build`), not plain `cargo build`. The JSON API works either way.

### Getting logs

Logs are in `logs/` in the [library folder](#where-things-live) (the daemon logs to stderr). For much more detail, start with `RUST_LOG=debug`, e.g. `RUST_LOG=debug trav get file.torrent`, and include the log in a bug report.

### Peer flags

The Peers tab uses µTorrent-style flags: **D** downloading from them · **d** you want data but they're choking you · **U** uploading to them · **u** they want data but you're choking them · **K** they'd send but you're not interested · **?** you'd send but they're not interested · **O** optimistic unchoke · **S** snubbed (stopped sending) · **I** they connected to you · **X** found via peer exchange · **H** found via DHT.

---

## Limitations

Not supported yet: µTP (peers that only speak µTP can't be reached), protocol encryption (MSE/PE), web seeds (BEP 19) and BitTorrent v2.
