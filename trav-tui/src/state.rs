use std::collections::{HashMap, VecDeque};
use ratatui::widgets::TableState;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Connecting,
    Downloading,
    Seeding,
    Paused,
    NoPeers,
    Error,
}

impl Status {
    pub fn from_str(s: &str) -> Self {
        match s {
            "Downloading" => Self::Downloading,
            "Seeding" => Self::Seeding,
            "Paused" => Self::Paused,
            "No Peers" => Self::NoPeers,
            "Error" => Self::Error,
            _ => Self::Connecting,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Connecting => "CONN",
            Self::Downloading => "DL",
            Self::Seeding => "SEED",
            Self::Paused => "PAUSED",
            Self::NoPeers => "NO PEERS",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TorrentState {
    pub hash: [u8; 20],
    pub name: String,
    pub size_bytes: u64,
    pub num_pieces: u32,
    pub pieces_downloaded: u32,
    pub progress: f32,
    pub status: Status,
    pub peers: usize,
    pub download_hz: u64,
    pub upload_hz: u64,
    pub health_badge: String,
}

impl TorrentState {
    pub fn eta_secs(&self) -> Option<u64> {
        if self.download_hz == 0 || self.progress >= 100.0 {
            return None;
        }
        let remaining_bytes = (self.size_bytes as f64 * (1.0 - self.progress as f64 / 100.0)) as u64;
        Some(remaining_bytes / self.download_hz.max(1))
    }
}

#[derive(Debug, Clone)]
pub struct PeerHealthState {
    pub addr: String,
    pub penalty_score: u32,
    pub network_penalty: u32,
    pub data_penalty: u32,
    pub timeout_count: u32,
    pub bad_data_count: u32,
    pub hash_fail_count: u32,
}

pub struct TuiState {
    pub torrents_map: HashMap<[u8; 20], usize>,
    pub torrents: Vec<TorrentState>,
    pub peer_health_map: HashMap<[u8; 20], Vec<PeerHealthState>>,
    pub logs: VecDeque<String>,
    pub table_state: TableState,
    pub global_down_history: Vec<u64>,
    pub global_up_history: Vec<u64>,
}

impl TuiState {
    pub fn new() -> Self {
        Self {
            torrents_map: HashMap::new(),
            torrents: Vec::new(),
            peer_health_map: HashMap::new(),
            logs: VecDeque::with_capacity(200),
            table_state: TableState::default(),
            global_down_history: Vec::with_capacity(120),
            global_up_history: Vec::with_capacity(120),
        }
    }

    pub fn log(&mut self, msg: String) {
        if self.logs.len() >= 200 {
            self.logs.pop_back();
        }
        self.logs.push_front(msg);
    }

    pub fn next(&mut self) {
        let len = self.torrents.len();
        if len == 0 {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => (i + 1) % len,
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    pub fn previous(&mut self) {
        let len = self.torrents.len();
        if len == 0 {
            return;
        }
        let i = match self.table_state.selected() {
            Some(0) | None => len.saturating_sub(1),
            Some(i) => i - 1,
        };
        self.table_state.select(Some(i));
    }
}
