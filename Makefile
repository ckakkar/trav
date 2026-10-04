# Common tasks. `make check` mirrors CI (minus e2e and desktop bundling).
.PHONY: setup check fmt lint test ui-check e2e build release desktop desktop-build docker clean

setup:
	cd trav-gui && npm ci

fmt:
	cargo fmt --all

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test

ui-check:
	cd trav-gui && npm run typecheck && npm test && npm run build

check: lint test ui-check
	scripts/check-versions.sh

e2e:
	cd trav-gui && npm run build
	cargo build -p trav-cli
	cd trav-gui && npx playwright test

build:
	cd trav-gui && npm run build
	cargo build --release -p trav-cli

desktop:
	cd trav-gui && npm run tauri dev

desktop-build:
	cd trav-gui && npm run tauri build

docker:
	docker build -t trav:local .

clean:
	cargo clean
	rm -rf trav-gui/.next trav-gui/out trav-gui/test-results trav-gui/playwright-report
