use serde::{Deserialize, Serialize};
use sha1::{Sha1, Digest};
use std::path::Path;
use tokio::fs;

use crate::error::{BitTorrentError, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Torrent {
    #[serde(default)]
    pub announce: String,
    #[serde(rename = "announce-list", default)]
    pub announce_list: Vec<Vec<String>>,
    pub info: InfoDict,
    /// Raw file bytes preserved for correct info_hash extraction.
    #[serde(skip)]
    pub raw_bytes: Vec<u8>,
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
    pub async fn read_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content = fs::read(path).await?;
        Self::from_bytes(&content)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut torrent: Torrent = serde_bencode::from_bytes(bytes)?;
        torrent.raw_bytes = bytes.to_vec();
        Ok(torrent)
    }

    /// Computes the 20-byte SHA-1 info_hash by hashing the raw bencoded info dict.
    /// Extracting raw bytes avoids losing extra fields (private, source, etc.) that
    /// would change the hash if we re-serialized from our parsed struct.
    pub fn info_hash(&self) -> Result<[u8; 20]> {
        let info_bytes = find_info_bytes(&self.raw_bytes)
            .ok_or_else(|| BitTorrentError::Engine(
                "Could not locate info dict in torrent file bytes".into()
            ))?;
        let mut hasher = Sha1::new();
        hasher.update(info_bytes);
        let hash = hasher.finalize();
        let mut res = [0u8; 20];
        res.copy_from_slice(&hash);
        Ok(res)
    }

    /// Returns all unique tracker URLs from announce + announce-list tiers.
    pub fn all_trackers(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        let candidates = std::iter::once(self.announce.clone())
            .chain(self.announce_list.iter().flatten().cloned());
        for url in candidates {
            let trimmed = url.trim().to_string();
            if !trimmed.is_empty() && seen.insert(trimmed.clone()) {
                result.push(trimmed);
            }
        }
        result
    }
}

/// Locates the raw bencoded bytes of the "info" value in a bencoded top-level dict.
fn find_info_bytes(data: &[u8]) -> Option<&[u8]> {
    if data.first() != Some(&b'd') {
        return None;
    }
    let mut pos = 1usize;
    while pos < data.len() {
        if data[pos] == b'e' {
            break;
        }
        // Each dict entry: <key-string> <value>
        let (key, key_end) = parse_bencode_string(data, pos)?;
        pos = key_end;
        let val_start = pos;
        if key == b"info" {
            skip_bencode_value(data, &mut pos)?;
            return Some(&data[val_start..pos]);
        }
        skip_bencode_value(data, &mut pos)?;
    }
    None
}

fn parse_bencode_string(data: &[u8], pos: usize) -> Option<(&[u8], usize)> {
    let colon = data[pos..].iter().position(|&b| b == b':')?;
    let len: usize = std::str::from_utf8(&data[pos..pos + colon]).ok()?.parse().ok()?;
    let start = pos + colon + 1;
    let end = start + len;
    if end > data.len() {
        return None;
    }
    Some((&data[start..end], end))
}

fn skip_bencode_value(data: &[u8], pos: &mut usize) -> Option<()> {
    if *pos >= data.len() {
        return None;
    }
    match data[*pos] {
        b'i' => {
            *pos += 1;
            while *pos < data.len() && data[*pos] != b'e' {
                *pos += 1;
            }
            if *pos >= data.len() {
                return None;
            }
            *pos += 1;
            Some(())
        }
        b'l' | b'd' => {
            *pos += 1;
            while *pos < data.len() && data[*pos] != b'e' {
                skip_bencode_value(data, pos)?;
            }
            if *pos >= data.len() {
                return None;
            }
            *pos += 1;
            Some(())
        }
        b'0'..=b'9' => {
            let start = *pos;
            while *pos < data.len() && data[*pos] != b':' {
                *pos += 1;
            }
            if *pos >= data.len() {
                return None;
            }
            let len: usize = std::str::from_utf8(&data[start..*pos]).ok()?.parse().ok()?;
            *pos += 1 + len;
            Some(())
        }
        _ => None,
    }
}
