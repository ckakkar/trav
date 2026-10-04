//! Transport-agnostic JSON command surface.
//!
//! The desktop app routes Tauri `invoke("rpc", …)` here and the daemon routes
//! `POST /api/rpc` here, so both frontends speak exactly the same API.

use std::net::SocketAddr;
use std::path::PathBuf;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::engine::{AddTorrent, EngineHandle, TorrentSource};
use crate::settings::Settings;

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AddParams {
    /// Base64 `.torrent` contents.
    torrent: Option<String>,
    path: Option<String>,
    magnet: Option<String>,
    save_path: Option<String>,
    paused: bool,
    sequential: bool,
    file_priorities: Option<Vec<u8>>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Target {
    hash: Option<String>,
    hashes: Vec<String>,
    delete_files: bool,
    enabled: bool,
    position: usize,
    priorities: Vec<u8>,
    peers: Vec<String>,
}

impl Target {
    fn all(&self) -> Vec<String> {
        let mut v = self.hashes.clone();
        if let Some(h) = &self.hash {
            v.push(h.clone());
        }
        v
    }

    fn one(&self) -> Result<&str, String> {
        self.hash.as_deref().or(self.hashes.first().map(String::as_str)).ok_or_else(|| "missing hash".into())
    }
}

fn parse<T: for<'de> Deserialize<'de> + Default>(v: Value) -> Result<T, String> {
    if v.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(v).map_err(|e| format!("bad params: {e}"))
}

fn ok() -> Result<Value, String> {
    Ok(Value::Null)
}

pub async fn dispatch(h: &EngineHandle, method: &str, params: Value) -> Result<Value, String> {
    match method {
        "snapshot" => Ok(to_json(&*h.snapshot())),
        "details" => {
            let t: Target = parse(params)?;
            Ok(h.details(t.one()?).map(|d| to_json(&d)).unwrap_or(Value::Null))
        }
        "add" => {
            let p: AddParams = parse(params)?;
            let source = if let Some(b) = p.torrent {
                TorrentSource::Bytes(base64::engine::general_purpose::STANDARD.decode(b.trim()).map_err(|e| e.to_string())?)
            } else if let Some(m) = p.magnet {
                TorrentSource::from_input(&m)
            } else if let Some(path) = p.path {
                TorrentSource::from_input(&path)
            } else {
                return Err("nothing to add".into());
            };
            let hash = h
                .add(AddTorrent {
                    source,
                    save_path: p.save_path.filter(|s| !s.trim().is_empty()).map(PathBuf::from),
                    paused: p.paused,
                    sequential: p.sequential,
                    file_priorities: p.file_priorities,
                })
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "infoHash": hash }))
        }
        "inspect" => {
            let p: AddParams = parse(params)?;
            let bytes = match (p.torrent, p.path) {
                (Some(b), _) => base64::engine::general_purpose::STANDARD.decode(b.trim()).map_err(|e| e.to_string())?,
                (None, Some(path)) => tokio::fs::read(&path).await.map_err(|e| e.to_string())?,
                _ => return Err("missing torrent".into()),
            };
            h.inspect(&bytes).map(|p| to_json(&p)).map_err(|e| e.to_string())
        }
        "pause" | "resume" | "recheck" | "reannounce" => {
            let t: Target = parse(params)?;
            for hash in t.all() {
                let r = match method {
                    "pause" => h.pause(&hash),
                    "resume" => h.resume(&hash),
                    "recheck" => h.recheck(&hash),
                    _ => h.reannounce(&hash),
                };
                r.map_err(|e| e.to_string())?;
            }
            ok()
        }
        "remove" => {
            let t: Target = parse(params)?;
            for hash in t.all() {
                h.remove(&hash, t.delete_files).await.map_err(|e| e.to_string())?;
            }
            ok()
        }
        "pauseAll" => {
            h.pause_all();
            ok()
        }
        "resumeAll" => {
            h.resume_all();
            ok()
        }
        "setSequential" => {
            let t: Target = parse(params)?;
            h.set_sequential(t.one()?, t.enabled).map_err(|e| e.to_string())?;
            ok()
        }
        "setFilePriorities" => {
            let t: Target = parse(params)?;
            h.set_file_priorities(t.one()?, t.priorities.clone()).map_err(|e| e.to_string())?;
            ok()
        }
        "setQueuePosition" => {
            let t: Target = parse(params)?;
            h.set_queue_position(t.one()?, t.position).map_err(|e| e.to_string())?;
            ok()
        }
        "addPeers" => {
            let t: Target = parse(params)?;
            let peers: Vec<SocketAddr> = t.peers.iter().filter_map(|p| p.trim().parse().ok()).collect();
            h.add_peers(t.one()?, peers).map_err(|e| e.to_string())?;
            ok()
        }
        "getSettings" => Ok(to_json(&h.settings())),
        "setSettings" => {
            let s: Settings = serde_json::from_value(params).map_err(|e| format!("bad settings: {e}"))?;
            h.update_settings(s).await.map_err(|e| e.to_string())?;
            Ok(to_json(&h.settings()))
        }
        _ => Err(format!("unknown method '{method}'")),
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}
