use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// User-tunable engine settings. Persisted as `settings.json` in the state dir.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub download_dir: PathBuf,
    /// TCP listen port; DHT shares it over UDP.
    pub listen_port: u16,
    pub max_peers_per_torrent: usize,
    pub max_peers_global: usize,
    /// Downloads beyond this many are queued (0 = unlimited).
    pub max_active_downloads: usize,
    /// Bytes per second, 0 = unlimited.
    pub download_limit: u64,
    pub upload_limit: u64,
    pub upload_slots: usize,
    pub enable_dht: bool,
    pub enable_pex: bool,
    pub enable_upnp: bool,
    /// Stop seeding once uploaded/size reaches this (0 = never stop).
    pub seed_ratio_limit: f64,
    /// With no upload limit set, cap uploads automatically whenever they start
    /// queueing on the uplink and slowing everything else (see `governor`).
    pub auto_upload_limit: bool,
    /// Which defaults this file was written with (for migrations). Files from
    /// before versioning have none, so it defaults to 0 rather than current.
    #[serde(default)]
    pub version: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            listen_port: 51413,
            max_peers_per_torrent: 150,
            max_peers_global: 600,
            max_active_downloads: 5,
            download_limit: 0,
            upload_limit: 0,
            upload_slots: 8,
            enable_dht: true,
            enable_pex: true,
            enable_upnp: true,
            seed_ratio_limit: 0.0,
            auto_upload_limit: true,
            version: Self::VERSION,
        }
    }
}

fn default_download_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("downloads"))
}

impl Settings {
    pub const VERSION: u32 = 1;

    /// Bring settings saved by an older version up to date: values the user
    /// never changed move to the new defaults.
    pub fn migrated(mut self) -> Self {
        if self.version < 1 {
            // 0.3: more connections per torrent (80 → 150, 500 → 600 overall).
            if self.max_peers_per_torrent == 80 {
                self.max_peers_per_torrent = 150;
            }
            if self.max_peers_global == 500 {
                self.max_peers_global = 600;
            }
        }
        self.version = Self::VERSION;
        self
    }

    /// Clamp values that would wedge the engine.
    pub fn sanitized(mut self) -> Self {
        self.max_peers_per_torrent = self.max_peers_per_torrent.clamp(1, 2000);
        self.max_peers_global = self
            .max_peers_global
            .clamp(self.max_peers_per_torrent.min(50), 20_000);
        self.upload_slots = self.upload_slots.clamp(1, 200);
        if !self.seed_ratio_limit.is_finite() || self.seed_ratio_limit < 0.0 {
            self.seed_ratio_limit = 0.0;
        }
        self
    }
}
