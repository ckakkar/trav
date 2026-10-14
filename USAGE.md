# Using Trav

Trav is a BitTorrent client for the Mac.

**Contents:**
[Install](#install) ·
[Adding torrents](#adding-torrents) ·
[The list and the details](#the-list-and-the-details) ·
[Menus and keys](#menus-and-keys) ·
[Removing](#removing) ·
[Dock, menu bar and closing](#the-dock-the-menu-bar-and-closing) ·
[Updates](#updates) ·
[Settings](#settings) ·
[Speed](#speed) ·
[Where things live](#where-things-live) ·
[Troubleshooting](#troubleshooting) ·
[Limitations](#limitations)

---

## Install

Download **[Trav.dmg](https://github.com/ckakkar/trav/releases/latest/download/Trav.dmg)**, open it and drag **Trav** into Applications. One download runs on Apple Silicon and Intel Macs (macOS 11 or later). From then on Trav [keeps itself up to date](#updates).

> **First launch:** Trav isn't notarized by Apple (that needs a paid developer account), so the first time macOS says it can't verify Trav. Open **System Settings › Privacy & Security**, scroll to the message about Trav and click **Open Anyway**. You only do this once; updates install without asking again.

To build it yourself instead, see [CONTRIBUTING](CONTRIBUTING.md) (`make install`).

**Uninstall:** turn off **Open at login** if you'd switched it on, then drag Trav to the Bin. Your downloads and [library](#where-things-live) stay; delete the library folder too for a clean slate.

## Adding torrents

Any of these opens the **Add** dialog:

- **Double-click a `.torrent` file** in Finder. Trav is registered as the handler.
- **Click a `magnet:` link** in your browser and allow it to open Trav.
- **Drag** `.torrent` files onto the window.
- **⌘V** anywhere in the window to paste a magnet link (or several, one per line).
- **File › Open Torrent File… (⌘O)**, or **File › Add Magnet Link… (⌘U)**.

In the Add dialog you can untick files you don't want, pick a different save folder, turn on **Sequential download** (to play media while it downloads), and untick **Start immediately** to add it paused.

> **Torrents still open in another app (uTorrent, Transmission, …)?** macOS keeps whichever app was the default first. Select any `.torrent` file in Finder, press ⌘I, choose Trav under **Open with** and click **Change All…**. macOS has no setting for `magnet:` links, so paste those into Trav with ⌘V, or uninstall the other app.

## The list and the details

Click a torrent to select it (⇧/⌘-click for several). **Right-click**, or use the **Transfers** menu, to pause or resume, verify local data, ask trackers for more peers, show in Finder, copy the magnet link, change the queue order or remove. The panel below has five tabs (⌘I shows or hides it):

| Tab | Shows |
|---|---|
| **Overview** | progress, speeds, ratio, ETA, save path and a piece map (downloaded, in flight, missing) |
| **Files** | per-file progress; set a file to *skip*, *normal* or *high* priority at any time |
| **Peers** | who you're connected to, their client, speeds and [flags](#peer-flags) |
| **Trackers** | each tracker's status, peers/seeds it reports and when it's next contacted |
| **Speed** | a live graph of download and upload |

## Menus and keys

| Menu | Items |
|---|---|
| **Trav** | About, **Check for Updates…**, Settings (⌘,), Hide, Quit (⌘Q) |
| **File** | Open Torrent File… (⌘O), Add Magnet Link… (⌘U), Close Window (⌘W) |
| **Edit** | Undo, Cut, Copy, Paste (⌘V also adds magnet links), Select All, Find (⌘F, the filter box) |
| **View** | Command Palette (⌘K), the lists All … Errors (⌘1 – ⌘7), Show/Hide Details (⌘I), Next Theme (⇧⌘L), Full Screen |
| **Transfers** | Pause/Resume (⌘.), Pause All (⌥⌘.), Resume All (⌥⌘/), Show in Finder (⌘R), Copy Magnet Link (⇧⌘C), Download in Order, Move to Top/Bottom of Queue, Verify Local Data, Ask Trackers for More Peers, Remove… (⌘⌫), Remove and Move Data to Trash… (⌥⌘⌫) |
| **Window**, **Help** | Minimize, Zoom, Bring All to Front · Trav Help, Release Notes, Report an Issue |

In the list:

| Keys | Action |
|---|---|
| ↑ ↓ or j k (⇧ extends) · ⌘A | move / select |
| Space | pause ↔ resume selection |
| Del / ⇧Del | remove / remove **and move the files to the Trash** |
| ↵ | show in Finder |
| 1 – 5 | detail tabs |
| / | filter the list |

**Themes:** Ink (dark) → Paper (light) → Phosphor (terminal green), with ⇧⌘L or in Settings.

## Removing

**Remove** keeps the downloaded files where they are. **Remove and Move Data to Trash** puts them in the Trash, so a mistake can be undone from Finder.

## The Dock, the menu bar and closing

The **Dock icon** shows a progress bar across everything that's downloading. The **menu bar icon** shows live speeds and has Pause all / Resume all. Closing the window **keeps Trav running** so it can finish and keep seeding; click the Dock icon (or ⌘0) to bring the window back. **Trav › Quit Trav** (⌘Q) stops it; it saves progress and tells trackers it's leaving first. You get a notification when a download completes (switch it off in Settings).

Trav remembers the window's size and position.

## Updates

Every change published to Trav's GitHub repository becomes a new version within minutes. Trav checks for one shortly after it opens and every hour, and **Trav › Check for Updates…** checks right away. When there's one it shows what changed; **Install & relaunch** downloads it, verifies its signature (Trav refuses anything not signed with the project's key), replaces itself, stops transfers cleanly and reopens. Transfers carry on after the relaunch. Turn the automatic check off in **Settings › App**.

## Settings

Open them with **⌘,**. Engine settings are saved with **Save**; Interface and App settings apply at once.

| Setting | Default | What it does |
|---|---|---|
| Default save folder | `~/Downloads` | where new torrents go unless you pick another |
| Listen port | 51413 | TCP for peers, UDP for DHT; if taken, the next free port is used |
| Download / upload limit | 0 = no limit | bytes per second, for everything combined |
| Keep uploads from slowing downloads | on | with no upload limit set, eases uploads off when they start queueing on your connection (see [Speed](#speed)) |
| Active downloads | 5 | more than this wait in the queue; 0 = no limit |
| Peers per torrent / overall | 150 / 600 | connection caps |
| Upload slots per torrent | 8 | peers you upload to at once |
| DHT | on | trackerless peer discovery; needed for most magnet links |
| Peer exchange | on | learn about peers from peers |
| UPnP port forwarding | on | ask your router to open the listen port |
| Stop seeding at ratio | 0 = seed forever | e.g. `2` stops once you've uploaded twice the size; resuming a stopped torrent overrides it |
| Theme, notifications, confirm before removing | | Interface |
| Check for updates automatically, open at login, keep the Mac awake while transferring | on, off, on | App |

Engine settings live in `settings.json` in the [library folder](#where-things-live).

## Speed

On a healthy torrent Trav downloads as fast as your connection allows. When a download is slow, the cause is usually one of these:

- **Few peers have it.** The Trackers tab shows how many seeds and peers each tracker knows about. Trav keeps looking: while fewer than five peers are sending, it asks the DHT and trackers for more every two minutes, and when it's at its connection limit it swaps idle peers for new ones every 30 seconds.
- **Your connection can't accept incoming peers.** The status bar shows **● open** once another peer has connected *to* you. If it never appears, you can only reach peers that are reachable themselves. Turn on **UPnP** in Settings, or forward the listen port (TCP and UDP, default 51413) on your router. Many mobile and fibre providers share one public address between customers (CGNAT); then no router setting helps, but IPv6 does if your provider and router support it.
- **Uploads are filling your connection.** Uploading at full speed queues up in front of your connection's uplink, and then everything else, downloads included, waits behind it. With no upload limit set, Trav measures how much its connections' round-trip time grows and eases uploads off when they cause queueing; the status bar then shows the upload limit as **auto**. If downloads still crawl while seeding, set a fixed upload limit around 80% of your upload speed.
- **A limit is set.** Check the download/upload limits in Settings (the status bar shows them).

## Where things live

| Where | What |
|---|---|
| `~/Library/Application Support/trav` | your **library**: the list of torrents and their progress |
| `…/trav/settings.json` | your [settings](#settings) |
| `…/trav/torrents/<info-hash>.torrent` / `.json` | each torrent's metadata and resume data |
| `…/trav/dht.json` | known DHT nodes, so the next start finds peers faster |
| `…/trav/logs/` | daily logs, last 7 kept (`trav-desktop.*`) |

The downloaded files themselves are only in your download folders. File names from a torrent are made safe for macOS and unique (two names that differ only in case, or in characters macOS can't store, get a " (2)" suffix).

## Troubleshooting

### "Another Trav instance is using" the library

Only one copy of Trav can have the library open. Quit the other one (**Trav › Quit Trav**; closing the window isn't enough).

### Stuck on "looking for peers…" or 0 peers

- Give it a minute: DHT discovery can take 30–60 s on a cold start.
- Check **DHT** is on in Settings. Torrents without working trackers depend on it.
- The torrent may simply be dead (no one seeding). The Trackers tab shows what each tracker reports.
- Firewalls or VPNs that block outgoing UDP stop both DHT and UDP trackers.

### Stuck on "fetching metadata"

A magnet link has no file list until a peer sends it, so this is the same problem as having no peers (see above). Once one peer answers, it moves on and saves the `.torrent` so it never has to fetch it again.

### A torrent shows "Error"

The reason is shown under its name and in the Overview tab. **disk write failed** usually means the disk is full or the folder isn't writable. Free some space (or fix permissions), then **resume** it; verified data is kept. **refusing to write through a symlink** means something below the save folder is a symlink; Trav won't follow it out of the folder.

### Files were moved or deleted behind Trav's back

Trav notices on the next start and re-verifies, then downloads whatever is missing. To force it now: right-click › **Verify Local Data**.

### Getting logs

Logs are in `~/Library/Application Support/trav/logs`. For much more detail, quit Trav and start it from Terminal with `RUST_LOG=debug /Applications/Trav.app/Contents/MacOS/trav-desktop`, then include the log in a bug report.

### Peer flags

The Peers tab uses µTorrent-style flags: **D** downloading from them · **d** you want data but they're choking you · **U** uploading to them · **u** they want data but you're choking them · **K** they'd send but you're not interested · **?** you'd send but they're not interested · **O** optimistic unchoke · **S** snubbed (stopped sending) · **I** they connected to you · **X** found via peer exchange · **H** found via DHT.

## Limitations

Not supported yet: µTP (peers that only speak µTP can't be reached), protocol encryption (MSE/PE), web seeds (BEP 19) and BitTorrent v2.
