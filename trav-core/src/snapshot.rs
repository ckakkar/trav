//! Serializable views of engine state for UIs (camelCase JSON).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TorrentStatus {
    Paused,
    Queued,
    Checking,
    /// Magnet link: waiting for the info dictionary from peers.
    Metadata,
    Downloading,
    Seeding,
    /// All selected files done, seeding stopped (ratio limit or nothing to share).
    Finished,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStats {
    pub download_rate: u64,
    pub upload_rate: u64,
    pub session_downloaded: u64,
    pub session_uploaded: u64,
    pub download_limit: u64,
    pub upload_limit: u64,
    pub connected_peers: usize,
    pub dht_nodes: usize,
    pub listen_port: u16,
    /// Seen at least one inbound connection: we are reachable.
    pub connectable: bool,
    pub free_space: Option<u64>,
    /// UPnP mapping state, when enabled.
    pub upnp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentSummary {
    pub info_hash: String,
    pub name: String,
    pub status: TorrentStatus,
    /// 0.0–1.0 over selected pieces (or check/metadata progress where relevant).
    pub progress: f64,
    /// Size of the selected files.
    pub size: u64,
    pub total_size: u64,
    pub done_bytes: u64,
    pub downloaded: u64,
    pub uploaded: u64,
    pub download_rate: u64,
    pub upload_rate: u64,
    pub eta: Option<u64>,
    pub ratio: f64,
    pub peers: usize,
    pub seeds: usize,
    pub swarm_peers: Option<u32>,
    pub swarm_seeds: Option<u32>,
    pub added_at: i64,
    pub completed_at: Option<i64>,
    pub save_path: String,
    pub error: Option<String>,
    pub queue_position: usize,
    pub has_metadata: bool,
    pub sequential: bool,
    pub private: bool,
    pub availability: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    pub seq: u64,
    pub torrents: Vec<TorrentSummary>,
    pub stats: GlobalStats,
}

impl EngineSnapshot {
    pub fn empty() -> Self {
        Self { seq: 0, torrents: Vec::new(), stats: GlobalStats::default() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub index: usize,
    pub path: String,
    pub size: u64,
    pub done: u64,
    pub progress: f64,
    /// 0 = skip, 1 = normal, 2 = high.
    pub priority: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    pub addr: String,
    pub client: String,
    /// µTorrent-style flag letters (D d U u K ? I X H O S).
    pub flags: String,
    pub progress: f64,
    pub download_rate: u64,
    pub upload_rate: u64,
    pub downloaded: u64,
    pub uploaded: u64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerInfo {
    pub url: String,
    pub tier: usize,
    /// idle | announcing | working | error
    pub status: String,
    pub message: Option<String>,
    pub peers: usize,
    pub seeds: Option<u32>,
    pub leechers: Option<u32>,
    pub next_announce: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentDetails {
    pub summary: TorrentSummary,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<i64>,
    pub piece_length: u32,
    pub num_pieces: usize,
    /// Base64 bitfield of verified pieces.
    pub pieces: String,
    /// Base64 bitfield of pieces currently being downloaded.
    pub pieces_in_progress: String,
    pub files: Vec<FileInfo>,
    pub peers: Vec<PeerInfo>,
    pub trackers: Vec<TrackerInfo>,
    pub magnet: String,
    pub content_path: Option<String>,
    pub wasted: u64,
    pub hash_fails: u32,
    pub dht_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewFile {
    pub path: String,
    pub size: u64,
}

/// What a `.torrent` contains, shown before the user commits to adding it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentPreview {
    pub info_hash: String,
    pub name: String,
    pub total_size: u64,
    pub piece_length: u32,
    pub num_pieces: usize,
    pub files: Vec<PreviewFile>,
    pub trackers: Vec<String>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub private: bool,
    pub already_added: bool,
}
