//! Trav core: a headless, async BitTorrent engine.
//!
//! Start a session with [`Engine::start`] and drive it through the returned
//! [`EngineHandle`]. UIs poll [`EngineHandle::snapshot`] (refreshed every
//! 500 ms) and fetch [`EngineHandle::details`] for the selected torrent.

pub mod bencode;
pub mod bitfield;
mod ctx;
pub mod dht;
pub mod engine;
pub mod error;
pub mod limiter;
pub mod magnet;
pub mod message;
pub mod metainfo;
pub mod path_safety;
pub mod peer;
pub mod persist;
pub mod picker;
pub mod settings;
pub mod snapshot;
pub mod storage;
mod torrent;
pub mod tracker;
mod upnp;

pub use engine::{AddTorrent, Engine, EngineHandle, TorrentSource};
pub use error::{Error, Result};
pub use message::Event;
pub use settings::Settings;
pub use snapshot::{EngineSnapshot, TorrentDetails, TorrentPreview, TorrentStatus, TorrentSummary};
