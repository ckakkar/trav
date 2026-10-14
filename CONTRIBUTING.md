# Contributing

## Setup

- macOS with the Xcode Command Line Tools, Rust 1.90+ (`rustup`), Node 24 LTS.

```bash
make            # list every task
make setup      # npm ci
make run        # the app with hot reload (quit the installed Trav first: they share the library)
make check      # everything CI runs, except bundling the app
make install    # build Trav.app and install it in /Applications
make bench TORRENT=…   # the engine's download speed on a real swarm (trav-core/examples/fetch.rs)
make icons      # re-render the app icon from scripts/app-icon.swift
```

## Expectations

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `npm run typecheck`
  must be clean. CI enforces all of them.
- Engine changes need tests. Prefer the loopback swarm harness in
  `trav-core/tests/swarm.rs` over mocks for anything protocol-level; measure
  speed changes with `make bench` against a real swarm, before and after.
- Parsers of untrusted input must never panic; extend
  `trav-core/tests/robustness.rs` when adding one.
- UI changes: keep pure logic in `trav-gui/src/lib/` with vitest coverage, and
  try the change in the app.
- Record user-facing changes in `CHANGELOG.md` under *Unreleased*.

## Releases

There is nothing to do by hand: every push to `main` that passes CI is
released by `.github/workflows/release.yml` as version
`<major.minor>.<commits on main>`. The major.minor comes from the version in
`Cargo.toml` (workspace), `trav-gui/package.json` and
`trav-gui/src-tauri/tauri.conf.json` (`make check` verifies they agree); bump
them there for a bigger step. Each release carries a universal `Trav.dmg`, the
signed update archive and `latest.json`, and installed apps pick it up within
the hour. Run the workflow by hand (Actions › Release › Run workflow) to
rebuild the current commit if a release failed.

### Update signing key

Updates are signed with a minisign key. The public half is in
`tauri.conf.json` (`plugins.updater.pubkey`); the private half is the
`TAURI_SIGNING_PRIVATE_KEY` repository secret, with a backup in the
maintainer's `~/.tauri/trav-updater.key`. Keep that backup: without it no
future update can be signed, and installed apps would need a manual
reinstall. Local builds skip the update archive and don't need the key.

### Apple code signing (optional)

Without an Apple Developer ID the app is ad-hoc signed and not notarized, so
users click **Open Anyway** once (System Settings › Privacy & Security);
updates then install without prompts. To sign and notarize, add repository
secrets `APPLE_CERTIFICATE` (base64 .p12), `APPLE_CERTIFICATE_PASSWORD`,
`APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (app-specific) and
`APPLE_TEAM_ID`; the release workflow picks up whichever are set. See the
[Tauri signing guide](https://v2.tauri.app/distribute/sign/macos/).
