# Headless Trav: engine + web UI. For NAS/servers.
#
#   docker run -d --name trav \
#     -e TRAV_TOKEN=change-me \
#     -p 9696:9696 -p 51413:51413 -p 51413:51413/udp \
#     -v trav-data:/data -v /path/to/downloads:/downloads \
#     ghcr.io/ckakkar/trav:latest

# ── UI (static export, embedded into the binary) ─────────────────────────────
FROM node:22-bookworm-slim AS ui
WORKDIR /src/trav-gui
COPY trav-gui/package.json trav-gui/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY trav-gui/ ./
RUN npm run build

# ── Engine + CLI ─────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
COPY --from=ui /src/trav-gui/out trav-gui/out
RUN cargo build --release --locked -p trav-cli \
 && install -Dm755 target/release/trav /out/trav

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 1000 --create-home --home-dir /data trav \
 && mkdir -p /downloads && chown trav /downloads
COPY --from=build /out/trav /usr/local/bin/trav
USER trav
ENV TRAV_STATE_DIR=/data RUST_LOG=info
VOLUME ["/data", "/downloads"]
EXPOSE 9696 51413 51413/udp
HEALTHCHECK --interval=30s --timeout=5s CMD ["trav", "--health-check", "--web-bind", "127.0.0.1:9696"]
ENTRYPOINT ["/usr/bin/tini", "--", "trav", "--daemon", "--web-bind", "0.0.0.0:9696", "--save-path", "/downloads", "--port", "51413"]
