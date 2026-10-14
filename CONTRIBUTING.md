# Contributing

## Setup

- Rust 1.90+ (`rustup`), Node 24 LTS.
- The app is macOS-only and needs the Xcode Command Line Tools. The engine,
  terminal UI and CLI build anywhere.

```bash
make            # list every task
make setup      # npm ci
make check      # everything CI runs, minus e2e
make e2e        # Playwright against real daemons (builds first)
make desktop    # run the Mac app with hot reload
make icons      # re-render the app icon from scripts/app-icon.swift
```

## Expectations

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `npm run typecheck`
  must be clean. CI enforces all of them.
- Engine changes need tests. Prefer the loopback swarm harness in
  `trav-core/tests/swarm.rs` over mocks for anything protocol-level.
- Parsers of untrusted input must never panic; extend
  `trav-core/tests/robustness.rs` when adding one.
- UI changes: add or update a Playwright flow in `trav-gui/e2e/` when behaviour
  changes; keep pure logic in `src/lib/` with vitest coverage.
- Keep versions in sync (`scripts/check-versions.sh`); record user-facing
  changes in `CHANGELOG.md` under *Unreleased*.

## Releasing

1. Bump the version in `Cargo.toml` (workspace), `trav-gui/package.json` and
   `trav-gui/src-tauri/tauri.conf.json` (`make check` verifies they match),
   and move *Unreleased* in the changelog to the new version.
2. Tag `vX.Y.Z` and push. The release workflow builds Trav for macOS (a
   universal `.dmg`, the signed update archive and `latest.json`), CLI
   archives (+ `SHA256SUMS`) and a multi-arch image on `ghcr.io/ckakkar/trav`,
   then publishes the release.

Installed apps check `releases/latest/download/latest.json`, so publishing is
what ships the update; they install it only if its signature verifies.

### Update signing key

Updates are signed with a minisign key. The public half is in
`tauri.conf.json` (`plugins.updater.pubkey`); the private half is the
`TAURI_SIGNING_PRIVATE_KEY` repository secret, with a backup in the
maintainer's `~/.tauri/trav-updater.key`. Keep that backup: without it no
future update can be signed, and installed apps would need a manual
reinstall. Local builds (`make install-app`, `make desktop-build`) skip the
update archive and don't need the key.

### Apple code signing (optional)

Without an Apple Developer ID the app is ad-hoc signed and not notarized, so
users click **Open Anyway** once (System Settings › Privacy & Security);
updates then install without prompts. To sign and notarize, add repository
secrets `APPLE_CERTIFICATE` (base64 .p12), `APPLE_CERTIFICATE_PASSWORD`,
`APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (app-specific) and
`APPLE_TEAM_ID`; the release workflow picks up whichever are set. See the
[Tauri signing guide](https://v2.tauri.app/distribute/sign/macos/).
