# Contributing

## Setup

- Rust 1.90+ (`rustup`), Node 24 LTS.
- Desktop app only: [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
  (Linux: `libwebkit2gtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev`).

```bash
make            # list every task
make setup      # npm ci
make check      # everything CI runs, minus e2e
make e2e        # Playwright against real daemons (builds first)
make desktop    # run the desktop app with hot reload
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

1. Bump the version in `Cargo.toml` (workspace), `trav-gui/package.json`,
   `trav-gui/src-tauri/Cargo.toml` and `trav-gui/src-tauri/tauri.conf.json`
   (`make check` verifies they match), move *Unreleased* in the changelog.
2. Tag `vX.Y.Z` and push. The release workflow builds desktop installers,
   CLI archives (+ `SHA256SUMS`) and a multi-arch image on
   `ghcr.io/ckakkar/trav` into a draft GitHub release; review and publish it.

### Code signing (optional)

Builds are unsigned by default (macOS: right-click → Open on first launch).
To sign and notarize, add repository secrets `APPLE_CERTIFICATE` (base64
.p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
`APPLE_PASSWORD` (app-specific) and `APPLE_TEAM_ID`, then add matching
`env:` entries to the `tauri-action` step in `.github/workflows/release.yml`.
See the [Tauri signing guide](https://v2.tauri.app/distribute/sign/macos/).
