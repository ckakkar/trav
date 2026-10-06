# Common tasks; run `make` for the list. `make check` mirrors CI (minus e2e and desktop bundling).
.DEFAULT_GOAL := help
.PHONY: help install install-app uninstall setup check fmt lint test ui-check e2e build desktop desktop-build docker clean

APP_DIR ?= /Applications

help: ## Show this list
	@awk 'BEGIN {FS = ":.*## "} /^[a-z0-9-]+:.*## / {printf "  \033[1m%-14s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

install: ## Install the `trav` command (with the web UI built in) to ~/.cargo/bin
	cd trav-gui && npm ci && npm run build
	cargo install --locked --path trav-cli
	@echo "Installed: try \`trav get <file.torrent>\` or just \`trav\`."

install-app: ## macOS: build Trav.app and install it in /Applications
	cd trav-gui && npm ci && npm run tauri build -- --bundles app
	rm -rf "$(APP_DIR)/Trav.app"
	cp -R target/release/bundle/macos/Trav.app "$(APP_DIR)/"
	@echo "Installed $(APP_DIR)/Trav.app: double-click any .torrent or magnet link."

uninstall: ## Remove the `trav` command (your downloads and library are kept)
	cargo uninstall trav-cli

setup: ## Install UI dependencies
	cd trav-gui && npm ci

fmt: ## Format Rust code
	cargo fmt --all

lint: ## rustfmt check + clippy (warnings are errors)
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

test: ## Rust unit + integration tests
	cargo test

ui-check: ## UI typecheck, unit tests and production build
	cd trav-gui && npm run typecheck && npm test && npm run build

check: lint test ui-check ## Everything CI runs, minus e2e
	scripts/check-versions.sh

e2e: ## Playwright against real daemons
	cd trav-gui && npm run build
	cargo build -p trav-cli
	cd trav-gui && npx playwright test

build: ## Release build of `trav` with the web UI embedded
	cd trav-gui && npm run build
	cargo build --release -p trav-cli

desktop: ## Run the desktop app with hot reload
	cd trav-gui && npm run tauri dev

desktop-build: ## Desktop installers in target/release/bundle/
	cd trav-gui && npm run tauri build

docker: ## Build the headless Docker image
	docker build -t trav:local .

clean: ## Remove build output
	cargo clean
	rm -rf trav-gui/.next trav-gui/out trav-gui/test-results trav-gui/playwright-report
