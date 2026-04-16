use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSnapshot {
    pub total_download_hz: u64,
    pub total_upload_hz: u64,
    pub active_torrents: HashMap<[u8; 20], TorrentSnapshot>,
    pub is_running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentSnapshot {
    pub name: String,
    pub info_hash: [u8; 20],
    pub info_hash_hex: String,
    pub size_bytes: u64,
    pub num_pieces: u32,
    pub pieces_downloaded: u32,
    /// Percentage 0.0–100.0 computed from pieces_downloaded / num_pieces.
    pub progress: f32,
    /// Bytes/sec rate, updated by the engine's periodic speed tick.
    pub download_hz: u64,
    pub upload_hz: u64,
    /// Cumulative bytes received (used by engine tick to compute download_hz).
    pub bytes_downloaded_total: u64,
    pub state: String,
    pub peers: Vec<PeerSnapshot>,
    pub piece_map_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSnapshot {
    pub addr: SocketAddr,
    pub is_choked: bool,
    pub is_interested: bool,
    pub peer_choking: bool,
    pub peer_interested: bool,
    pub download_hz: u64,
    pub upload_hz: u64,
    pub penalty_score: u32,
    pub network_penalty: u32,
    pub data_penalty: u32,
    pub timeout_count: u32,
    pub bad_data_count: u32,
    pub hash_fail_count: u32,
}

impl EngineSnapshot {
    pub fn new() -> Self {
        Self {
            total_download_hz: 0,
            total_upload_hz: 0,
            active_torrents: HashMap::new(),
            is_running: true,
        }
    }
}
