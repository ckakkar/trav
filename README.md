# Trav - Advanced Async BitTorrent Engine

Trav is a high-performance, fully asynchronous BitTorrent client engineered entirely in Rust. It was built from the ground up prioritizing a highly decoupled, panic-free architecture powered by the `tokio` runtime, emphasizing stability, strict protocol implementation, and modularity.

The project splits the BitTorrent protocol implementation (headless core) cleanly away from the presentation layers. This allows multiple user interfaces—like the provided modern Terminal User Interface (TUI)—to connect and control the engine through robust asynchronous message-passing channels.

## 🌟 Key Features

### Phase 1: Core Protocol & Engine
- **Headless Async Engine**: All networking, peer communication, and disk I/O run securely inside dedicated `tokio` tasks using bounded `mpsc` channels and strict non-blocking models avoiding typical race conditions.
- **Robust Bencode Serialization**: Strongly typed decoding/encoding of `.torrent` files leveraging `serde_bencode` and computing SHA-1 integrity checks.
- **Peer Wire Protocol Integration**: Utilizes `tokio_util::codec` to apply length-prefixed streaming delimitation for peer networking handshakes, choked/unchoked messaging, and dynamic payload handling.
- **Concurrent Piece Strategy**: An established Rarest-First piece-picker algorithm working dynamically alongside asynchronous disk operations.
- **Panic-Free Architecture**: Strict adherence to idiomatic Rust abstractions running without `unwrap()` or `expect()`. Robust internal error propagation powered by `thiserror`.

### Phase 2: Advanced Metadata & DHT Discoverability
- **Tracker Diversification**: Extends classic HTTP Announcements (`reqwest`) with custom asynchronous binary UDP Tracker implementations (BEP 15) leveraging `tokio::net::UdpSocket`.
- **Extension Protocols**: Compliant implementations of the Extended Handshake payload (BEP 10), explicitly supporting capabilities to stream `.torrent` metadata directly from remote peers (`ut_metadata` - BEP 9) and discovering active swarms via Peer Exchange (`ut_pex` - BEP 11).
- **Kademlia DHT Scaffolding**: Built-in capabilities tracking XOR-distance metrics and Bencoded UDP Krpc requests to establish future Trackerless discoverability models.
- **Magnet URIs**: Native URL interpreters converting arbitrary `xt=urn:btih:<hash>` magnet payloads rapidly into connected BitTorrent swarms.

### Phase 3: The Dashboard (TUI)
- **Multi-Pane Visualizer**: An extremely snappy, responsive `ratatui`-driven terminal interface dynamically bounded by `crossterm` terminal constraints.
- **Live Statistics**: Translates cross-thread data channels instantly, parsing engine global download speeds (MB/s), live peer-counts, and concurrent hash progress seamlessly into dedicated `ratatui::widgets::Table` displays.

## 🏗️ Workspace Architecture

The system is constructed as a Cargo Workspace separated logically into three main crates:

1. **`trav-core`**: The nucleus backend encapsulating every piece of the BitTorrent wire specification. Runs entirely headless.
2. **`trav-tui`**: The visual presentation application running constraint-based terminal dashboard widgets querying state updates from `trav-core`.
3. **`trav-cli`**: The universal binary entry point that spins up the multithreaded `tokio` runtime, spawns the core engine into the background loop, and binds the TUI gracefully over the main thread.

## 🚀 Getting Started

### Prerequisites

You need the standard Rust toolchain installed:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Installation

Clone the repository and jump into the directory:

```bash
git clone https://github.com/yourusername/trav.git
cd trav
```

### Running the TUI Client

Currently, the primary way to interact with the async engine is via the robust dashboard interface:

```bash
cargo run --release -p trav-cli
```

### Navigation

Within the TUI context:
- Use **↑ ( Up Arrow )** or **k** to scroll up the Active Torrent table.
- Use **↓ ( Down Arrow )** or **j** to scroll down the Active Torrent table.
- Press **q** to gracefully emit shutdown signals dropping all async sockets and wiping the alternate terminal screen correctly.

## 🔧 Logging

Internal `tracing` behaviors operate continuously beneath the TUI interface. To prevent complex asynchronous logs from physically corrupting constraint grids on the terminal screen, all diagnostic messages are redirected immediately either into standard log files (e.g. `trav.log`), or selectively piped directly to the TUI Event Logs bounded buffers.

## 🛡️ Stability Notice

The current version implements structural components of Phase 1, Phase 2, and Phase 3. The foundational parsing, protocol messaging capabilities, DHT structures, and UI dashboards compile completely, preparing for impending massive connection integrations to launch live asynchronous downloading swarms dynamically.

## ⚖️ License

Distributed under the MIT License.
