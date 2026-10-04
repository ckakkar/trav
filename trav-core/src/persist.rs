//! On-disk session state: settings, per-torrent metainfo + resume data, DHT nodes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::metainfo::InfoHash;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ResumeData {
    pub name: String,
    pub magnet: Option<String>,
    pub save_path: PathBuf,
    pub paused: bool,
    /// Base64 bitfield of verified pieces.
    pub have: Option<String>,
    pub file_priorities: Vec<u8>,
    pub uploaded: u64,
    pub downloaded: u64,
    pub added_at: i64,
    pub completed_at: Option<i64>,
    pub sequential: bool,
    pub queue_position: usize,
    pub trackers: Vec<Vec<String>>,
}

pub struct Store {
    root: PathBuf,
    /// Exclusive lock on `<root>/.lock` so two engines never share a state dir.
    lock: parking_lot::Mutex<Option<std::fs::File>>,
}

impl Store {
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(root.join("torrents"))?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(".lock"))?;
        if f.try_lock().is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("another Trav instance is using {}", root.display()),
            ));
        }
        Ok(Self {
            root,
            lock: parking_lot::Mutex::new(Some(f)),
        })
    }

    pub fn unlock(&self) {
        if let Some(f) = self.lock.lock().take() {
            let _ = f.unlock();
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn torrent_path(&self, ih: &InfoHash, ext: &str) -> PathBuf {
        self.root
            .join("torrents")
            .join(format!("{}.{ext}", hex::encode(ih)))
    }

    pub fn save_torrent(&self, ih: &InfoHash, bytes: &[u8]) -> std::io::Result<()> {
        write_atomic(&self.torrent_path(ih, "torrent"), bytes)
    }

    pub fn load_torrent(&self, ih: &InfoHash) -> Option<Vec<u8>> {
        std::fs::read(self.torrent_path(ih, "torrent")).ok()
    }

    pub fn save_resume(&self, ih: &InfoHash, r: &ResumeData) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(r).map_err(std::io::Error::other)?;
        write_atomic(&self.torrent_path(ih, "json"), &json)
    }

    pub fn remove(&self, ih: &InfoHash) {
        let _ = std::fs::remove_file(self.torrent_path(ih, "torrent"));
        let _ = std::fs::remove_file(self.torrent_path(ih, "json"));
    }

    /// All persisted torrents, in queue order.
    pub fn load_all(&self) -> Vec<(InfoHash, ResumeData)> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.root.join("torrents")) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(ih) = p
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| hex::decode(s).ok())
                .and_then(|v| <[u8; 20]>::try_from(v).ok())
            else {
                continue;
            };
            if let Some(r) = read_json::<ResumeData>(&p) {
                out.push((ih, r));
            }
        }
        out.sort_by_key(|(_, r)| (r.queue_position, r.added_at));
        out
    }

    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        read_json(&self.root.join(name))
    }

    pub fn save<T: Serialize>(&self, name: &str, v: &T) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(v).map_err(std::io::Error::other)?;
        write_atomic(&self.root.join(name), &json)
    }
}

fn read_json<T: DeserializeOwned>(p: &Path) -> Option<T> {
    let data = std::fs::read(p).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Write-then-rename so a crash never leaves a truncated state file.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}
