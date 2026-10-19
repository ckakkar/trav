//! Session-wide context shared by every torrent.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize};

use parking_lot::{Mutex, RwLock};
use tokio::sync::{Semaphore, broadcast};

use crate::dht::Dht;
use crate::governor::RttSample;
use crate::limiter::RateLimiter;
use crate::message::Event;
use crate::metainfo::InfoHash;
use crate::path_safety::{fs_key, key_within};
use crate::persist::Store;
use crate::settings::Settings;
use crate::utp::UtpSocket;

/// Outbound connection attempts allowed in flight at once across the session.
pub const HALF_OPEN_LIMIT: usize = 100;

pub(crate) struct Ctx {
    pub peer_id: [u8; 20],
    pub listen_port: AtomicU16,
    /// The outside port a NAT-PMP router gave us, when not `listen_port` (0).
    pub public_port: AtomicU16,
    pub settings: RwLock<Settings>,
    pub down: RateLimiter,
    pub up: RateLimiter,
    pub dht: RwLock<Option<Dht>>,
    /// The UDP socket on the listen port (µTP, shared with the DHT).
    pub utp: RwLock<Option<UtpSocket>>,
    pub http: reqwest::Client,
    /// For web seeds (see `tracker::web_seed_client`).
    pub web: reqwest::Client,
    pub events: broadcast::Sender<Event>,
    pub half_open: Arc<Semaphore>,
    pub connections: AtomicUsize,
    pub store: Store,
    pub tracker_key: u32,
    pub connectable: AtomicBool,
    pub session_down: AtomicU64,
    pub session_up: AtomicU64,
    pub upnp_status: RwLock<Option<String>>,
    /// Round-trip samples from busy connections since the governor last ran.
    pub rtt_samples: Mutex<Vec<RttSample>>,
    /// The automatic upload limit in force (bytes/s; 0 = none).
    pub auto_upload_cap: AtomicU64,
    /// Public trackers added to non-private torrents (see `public_trackers`).
    pub public_trackers: RwLock<Vec<String>>,
    /// Where each torrent keeps its data (see [`Ctx::claim`]).
    pub claims: Mutex<HashMap<InfoHash, Claim>>,
}

/// A torrent's file or folder, by [`fs_key`], and its name.
pub(crate) struct Claim {
    key: String,
    name: String,
}

impl Ctx {
    pub fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    /// The port to tell trackers, the DHT and peers to reach us on.
    pub fn public_port(&self) -> u16 {
        match self.public_port.load(std::sync::atomic::Ordering::Relaxed) {
            0 => self.listen_port.load(std::sync::atomic::Ordering::Relaxed),
            p => p,
        }
    }

    /// The torrent other than `ih` whose data is at `content`, inside it or
    /// around it: two torrents writing one place would overwrite each other.
    pub fn claimed_by(&self, ih: &InfoHash, content: &Path) -> Option<String> {
        holder(&self.claims.lock(), ih, &fs_key(content)).map(|c| c.name.clone())
    }

    /// Reserve `content` for `ih`; if another torrent has it, that torrent's name.
    pub fn claim(&self, ih: InfoHash, content: &Path, name: &str) -> Result<(), String> {
        let key = fs_key(content);
        let mut claims = self.claims.lock();
        if let Some(c) = holder(&claims, &ih, &key) {
            return Err(c.name.clone());
        }
        let name = name.to_string();
        claims.insert(ih, Claim { key, name });
        Ok(())
    }

    pub fn release(&self, ih: &InfoHash) {
        self.claims.lock().remove(ih);
    }

    pub fn dht(&self) -> Option<Dht> {
        self.dht.read().clone()
    }

    /// The µTP socket to dial with, if µTP is on.
    pub fn utp(&self) -> Option<UtpSocket> {
        if !self.settings.read().enable_utp {
            return None;
        }
        self.utp.read().clone()
    }
}

fn holder<'a>(claims: &'a HashMap<InfoHash, Claim>, ih: &InfoHash, key: &str) -> Option<&'a Claim> {
    claims
        .iter()
        .find(|(h, c)| *h != ih && (key_within(&c.key, key) || key_within(key, &c.key)))
        .map(|(_, c)| c)
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
