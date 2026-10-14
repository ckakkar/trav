# Security policy

## Reporting a vulnerability

Please report security issues privately to **cyrus@kkrwhofrags.xyz** (or a
GitHub private security advisory on this repository) rather than opening a
public issue. Include a description, reproduction steps and the affected
version. You should receive an acknowledgement within 72 hours.

## Scope and design notes

Trav processes untrusted input from the network (peer wire messages, DHT
packets, tracker responses) and from `.torrent` files. Hardening in place:

- Bencode decoding has a nesting limit and bounds-checked lengths; peer frames
  are capped at 2 MiB; metadata at 64 MiB. Parsers are fuzz-tested in CI
  (`trav-core/tests/robustness.rs`).
- Every path component from torrent metadata is sanitised, made unique
  (case-insensitively) and verified to stay inside the save folder; writes
  never follow a symlink below the save folder.
- DHT replies are only accepted from the node that was asked; tracker replies
  are capped at 2 MiB; partial pieces stay within a fixed memory budget.
- Peers that send data failing hash checks are banned.
- The app ships a restrictive Content-Security-Policy and a minimal Tauri
  capability set (`trav-gui/src-tauri/capabilities/default.json`).
- Updates are only installed if their signature verifies against the public
  key built into the app (`plugins.updater.pubkey` in `tauri.conf.json`).

Supported versions: the latest release (the app updates itself to it).
