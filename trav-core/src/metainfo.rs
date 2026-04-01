use serde::{Deserialize, Serialize};
use sha1::{Sha1, Digest};
use std::path::Path;
use tokio::fs;

use crate::error::{BitTorrentError, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Torrent {
    pub announce: String,
    pub info: InfoDict,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InfoDict {
    pub name: String,
    #[serde(rename = "piece length")]
    pub piece_length: u64,
    #[serde(with = "serde_bytes")]
    pub pieces: Vec<u8>,
    pub length: Option<u64>,
    pub files: Option<Vec<FileDict>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileDict {
    pub length: u64,
    pub path: Vec<String>,
}

impl Torrent {
    /// Reads and parses a .torrent file from disk asynchronously.
    pub async fn read_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content = fs::read(path).await?;
        Self::from_bytes(&content)
    }

    /// Parses a Torrent from raw bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let torrent: Torrent = serde_bencode::from_bytes(bytes)?;
        Ok(torrent)
    }

    /// Computes the 20-byte SHA-1 hash of the bencoded `info` dictionary.
    pub fn info_hash(&self) -> Result<[u8; 20]> {
        let info_bytes = serde_bencode::to_bytes(&self.info)?;
        let mut hasher = Sha1::new();
        hasher.update(&info_bytes);
        let hash = hasher.finalize();
        let mut res = [0u8; 20];
        res.copy_from_slice(&hash);
        Ok(res)
    }
}
