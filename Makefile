# Common tasks for Trav for macOS; run `make` for the list. `make check` mirrors CI.
.DEFAULT_GOAL := help
.PHONY: help setup run install install-app dmg check fmt lint test ui-check bench icons clean

APP_DIR ?= /Applications

# Local builds skip the signed update archive (that needs the release key);
# an installed copy still updates itself from GitHub releases.
LOCAL_BUILD = --config '{"bundle":{"createUpdaterArtifacts":false}}'

help: ## Show this list
	@awk 'BEGIN {FS = ":.*## "} /^[a-z0-9-]+:.*## / {printf "  \033[1m%-12s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

setup: ## Install the UI's dependencies
	cd trav-gui && npm ci

run: ## Run the app with hot reload (uses your library; quit the installed Trav first)
	cd trav-gui && npm run tauri dev

install: ## Build Trav.app and install it in /Applications
	@if pgrep -f "$(APP_DIR)/Trav.app/Contents/MacOS/" >/dev/null; then echo "Quit Trav first (Trav › Quit Trav), then run make install again."; exit 1; fi
	cd trav-gui && npm ci && npm run tauri build -- --bundles app $(LOCAL_BUILD)
	rm -rf "$(APP_DIR)/Trav.app"
	cp -R target/release/bundle/macos/Trav.app "$(APP_DIR)/"
	@echo "Installed $(APP_DIR)/Trav.app."

install-app: install

dmg: ## Trav.app and a .dmg in target/release/bundle/
	cd trav-gui && npm ci && npm run tauri build -- $(LOCAL_BUILD)

check: lint test ui-check ## Everything CI runs, except bundling the app
	scripts/check-versions.sh

fmt: ## Format Rust code
	cargo fmt --all

lint: ## rustfmt check + clippy (warnings are errors)
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

test: ## Engine unit and integration tests
	cargo test -p trav-core

ui-check: ## UI typecheck, unit tests and production build
	cd trav-gui && npm run typecheck && npm test && npm run build

bench: ## Download TORRENT for SECS (default 60) with the engine and report the speed
	@test -n "$(TORRENT)" || { echo "usage: make bench TORRENT=file.torrent|magnet [SECS=60]"; exit 1; }
	cargo run --release -p trav-core --example fetch -- "$(TORRENT)" $(or $(SECS),60)

icons: ## Re-render the app icon and menu bar glyph (scripts/app-icon.swift)
	swift scripts/app-icon.swift trav-gui/src-tauri/icons/source.png trav-gui/src-tauri/icons/glyph.png
	sips -Z 64 trav-gui/src-tauri/icons/glyph.png --out trav-gui/src-tauri/icons/tray.png >/dev/null
	sips -Z 64 trav-gui/src-tauri/icons/source.png --out trav-gui/src/app/icon.png >/dev/null
	cd trav-gui && npx tauri icon src-tauri/icons/source.png >/dev/null
	cd trav-gui/src-tauri/icons && rm -rf glyph.png android ios icon.ico icon.png 64x64.png Square*Logo.png StoreLogo.png

clean: ## Remove build output and caches (Rust target, Next build, generated Tauri schemas)
	cargo clean
	rm -rf trav-gui/.next trav-gui/out trav-gui/tsconfig.tsbuildinfo trav-gui/src-tauri/gen
