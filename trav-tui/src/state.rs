use std::collections::{HashMap, VecDeque};
use ratatui::widgets::TableState;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Downloading,
    Seeding,
    Paused,
    Error,
}

#[derive(Debug, Clone)]
pub struct TorrentState {
    pub hash: [u8; 20],
    pub name: String,
    pub size_bytes: u64,
    pub progress: f32,
    pub status: Status,
    pub peers: usize,
    pub download_hz: u64,
    pub upload_hz: u64,
}

pub struct TuiState {
    pub torrents_map: HashMap<[u8; 20], usize>,
    pub torrents: Vec<TorrentState>,
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
            logs: VecDeque::with_capacity(100),
            table_state: TableState::default(),
            global_down_history: Vec::with_capacity(100),
            global_up_history: Vec::with_capacity(100),
        }
    }

    pub fn log(&mut self, msg: String) {
        if self.logs.len() >= 100 {
            self.logs.pop_back();
        }
        self.logs.push_front(msg);
    }

    pub fn add_torrent(&mut self, hash: [u8; 20], name: String, size_bytes: u64) {
        if !self.torrents_map.contains_key(&hash) {
            let idx = self.torrents.len();
            self.torrents_map.insert(hash, idx);
            self.torrents.push(TorrentState {
                hash,
                name,
                size_bytes,
                progress: 0.0,
                status: Status::Downloading,
                peers: 0,
                download_hz: 0,
                upload_hz: 0,
            });
            
            // Select the newly added row if none selected
            if self.table_state.selected().is_none() {
                self.table_state.select(Some(0));
            }
        }
    }

    pub fn next(&mut self) {
        let i = match self.table_state.selected() {
            Some(i) => {
                if i >= self.torrents.len().saturating_sub(1) {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    pub fn previous(&mut self) {
        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.torrents.len().saturating_sub(1)
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }
}
