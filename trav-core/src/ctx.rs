//! Session-wide context shared by every torrent.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize};

use parking_lot::RwLock;
use tokio::sync::{Semaphore, broadcast};

use crate::dht::Dht;
use crate::limiter::RateLimiter;
use crate::message::Event;
use crate::persist::Store;
use crate::settings::Settings;

/// Outbound TCP handshakes allowed in flight at once across the session.
pub const HALF_OPEN_LIMIT: usize = 64;

pub(crate) struct Ctx {
    pub peer_id: [u8; 20],
    pub listen_port: AtomicU16,
    pub settings: RwLock<Settings>,
    pub down: RateLimiter,
    pub up: RateLimiter,
    pub dht: RwLock<Option<Dht>>,
    pub http: reqwest::Client,
    pub events: broadcast::Sender<Event>,
    pub half_open: Arc<Semaphore>,
    pub connections: AtomicUsize,
    pub store: Store,
    pub tracker_key: u32,
    pub connectable: AtomicBool,
    pub session_down: AtomicU64,
    pub session_up: AtomicU64,
    pub upnp_status: RwLock<Option<String>>,
}

impl Ctx {
    pub fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub fn dht(&self) -> Option<Dht> {
        self.dht.read().clone()
    }
}

/// Azureus-style peer id: `-TV0200-` + 12 random alphanumerics.
pub fn generate_peer_id() -> [u8; 20] {
    const ALNUM: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut id = [0u8; 20];
    let ver = env!("CARGO_PKG_VERSION").replace('.', "");
    let prefix = format!("-TV{:0<4}-", &ver[..ver.len().min(4)]);
    id[..8].copy_from_slice(&prefix.as_bytes()[..8]);
    for b in &mut id[8..] {
        *b = ALNUM[rand::random_range(0..ALNUM.len())];
    }
    id
}
