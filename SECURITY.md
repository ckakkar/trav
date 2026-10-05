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
- Every path component from torrent metadata is sanitised and the result is
  verified to stay inside the save directory.
- Peers that send data failing hash checks are banned.
- The web API (`trav --daemon`) only answers loopback `Host` headers unless a
  token is configured, rejects cross-origin requests, compares tokens in
  constant time, and refuses to bind a non-loopback address without a token.
  Put it behind TLS (a reverse proxy) when exposing it beyond your LAN.
- The desktop app ships a restrictive Content-Security-Policy and a minimal
  Tauri capability set (`trav-gui/src-tauri/capabilities/default.json`).

Supported versions: the latest release.
