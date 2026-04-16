use crate::message::{Command, Event};
use crate::error::Result;
use crate::snapshot::EngineSnapshot;
use crate::disk::FileRegion;
use tokio::sync::{mpsc, broadcast};
use tracing::{info, warn, debug};
use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Engine {
    command_rx: mpsc::Receiver<Command>,
    event_tx: broadcast::Sender<Event>,
    snapshot: Arc<RwLock<EngineSnapshot>>,
}

impl Engine {
    pub fn new() -> (Self, mpsc::Sender<Command>, broadcast::Receiver<Event>, Arc<RwLock<EngineSnapshot>>) {
        let (command_tx, command_rx) = mpsc::channel(32);
        let (event_tx, event_rx) = broadcast::channel(32);
        let snapshot = Arc::new(RwLock::new(EngineSnapshot::new()));
        let engine = Self { command_rx, event_tx: event_tx.clone(), snapshot: snapshot.clone() };
        (engine, command_tx, event_rx, snapshot)
    }

    pub async fn run(mut self) -> Result<()> {
        info!("Engine starting…");
        let _ = self.event_tx.send(Event::EngineStarted);

        // Randomised Azureus-style peer_id: "-TR2026-<12 digits>"
        let peer_id = {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            let pid = std::process::id();
            format!("-TR2026-{:06}{:06}", ts % 1_000_000, pid % 1_000_000)
        };

        // 1-second tick: derive per-torrent download_hz from bytes_downloaded_total deltas
        let mut speed_tick = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut last_bytes: HashMap<[u8; 20], u64> = HashMap::new();

        loop {
            tokio::select! {
                _ = speed_tick.tick() => {
                    if let Ok(mut snap) = self.snapshot.write() {
                        let mut total_down = 0u64;
                        for (hash, t) in snap.active_torrents.iter_mut() {
                            let prev = last_bytes.get(hash).copied()
                                .unwrap_or(t.bytes_downloaded_total);
                            t.download_hz = t.bytes_downloaded_total.saturating_sub(prev);
                            last_bytes.insert(*hash, t.bytes_downloaded_total);
                            total_down = total_down.saturating_add(t.download_hz);
                        }
                        snap.total_download_hz = total_down;
                    }
                }

                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(Command::AddTorrent { file_path, download_dir }) => {
                            info!("AddTorrent: {:?}", file_path);
                            match crate::metainfo::Torrent::read_from_file(&file_path).await {
                                Ok(torrent) => {
                                    if let Err(e) = self.start_torrent(
                                        torrent, download_dir, &peer_id
                                    ).await {
                                        tracing::error!("Failed to start torrent: {}", e);
                                    }
                                }
                                Err(e) => tracing::error!("Failed to parse .torrent: {}", e),
                            }
                        }

                        Some(Command::Pause { torrent_hash }) => {
                            if let Ok(mut s) = self.snapshot.write() {
                                if let Some(t) = s.active_torrents.get_mut(&torrent_hash) {
                                    t.state = "Paused".to_string();
                                }
                            }
                        }

                        Some(Command::Resume { torrent_hash }) => {
                            if let Ok(mut s) = self.snapshot.write() {
                                if let Some(t) = s.active_torrents.get_mut(&torrent_hash) {
                                    t.state = "Downloading".to_string();
                                }
                            }
                        }

                        Some(Command::Quit) => {
                            info!("Quit — shutting down.");
                            break;
                        }

                        None => {
                            debug!("Command channel closed.");
                            break;
                        }
                    }
                }
            }
        }

        info!("Engine shutdown complete.");
        Ok(())
    }

    async fn start_torrent(
        &self,
        torrent: crate::metainfo::Torrent,
        download_dir: PathBuf,
        peer_id: &str,
    ) -> Result<()> {
        let info_hash = torrent.info_hash()?;
        let piece_length = torrent.info.piece_length as u32;
        let num_pieces = (torrent.info.pieces.len() / 20) as u32;

        // Compute total length and build FileRegion list
        let (total_length, regions) = build_file_regions(
            &torrent,
            &download_dir,
            &info_hash,
        )?;

        if total_length == 0 {
            tracing::error!("Torrent has zero total length — skipping.");
            return Ok(());
        }

        // Register in snapshot
        {
            let mut s = self.snapshot.write().unwrap();
            s.active_torrents.insert(info_hash, crate::snapshot::TorrentSnapshot {
                name: torrent.info.name.clone(),
                info_hash,
                info_hash_hex: hex::encode(info_hash),
                size_bytes: total_length,
                num_pieces,
                pieces_downloaded: 0,
                progress: 0.0,
                download_hz: 0,
                upload_hz: 0,
                bytes_downloaded_total: 0,
                state: "Connecting".to_string(),
                peers: vec![],
                piece_map_base64: String::new(),
            });
        }

        // Spawn disk task (handles multi-file automatically)
        let disk_tx = crate::disk::DiskTask::spawn_regions(regions, piece_length).await?;

        let piece_manager = Arc::new(std::sync::Mutex::new(
            crate::manager::PieceManager::new(
                num_pieces,
                piece_length,
                total_length,
                torrent.info.pieces.clone(),
            )
        ));

        // Announce to all trackers in order until we get peers
        let mut peer_addrs = vec![];
        for tracker_url in torrent.all_trackers() {
            if !peer_addrs.is_empty() {
                break;
            }
            info!("Trying tracker: {}", tracker_url);
            if tracker_url.starts_with("http") {
                let client = crate::tracker::TrackerClient::new(peer_id, 6881);
                match client.announce(&tracker_url, &info_hash, 0, 0, total_length).await {
                    Ok(resp) => {
                        peer_addrs = to_socket_addrs(resp.parse_peers());
                        info!("HTTP tracker returned {} peers", peer_addrs.len());
                    }
                    Err(e) => warn!("HTTP tracker {} failed: {}", tracker_url, e),
                }
            } else if tracker_url.starts_with("udp") {
                let client = crate::tracker::UdpTrackerClient::new(peer_id, 6881);
                match client.announce(&tracker_url, &info_hash, 0, 0, total_length).await {
                    Ok(resp) => {
                        peer_addrs = to_socket_addrs(resp.parse_peers());
                        info!("UDP tracker returned {} peers", peer_addrs.len());
                    }
                    Err(e) => warn!("UDP tracker {} failed: {}", tracker_url, e),
                }
            }
        }

        if peer_addrs.is_empty() {
            warn!("No peers found for {}", hex::encode(info_hash));
            if let Ok(mut s) = self.snapshot.write() {
                if let Some(t) = s.active_torrents.get_mut(&info_hash) {
                    t.state = "No Peers".to_string();
                }
            }
        } else {
            {
                let mut s = self.snapshot.write().unwrap();
                if let Some(t) = s.active_torrents.get_mut(&info_hash) {
                    t.state = "Downloading".to_string();
                }
            }
            let swarm = Arc::new(crate::swarm::SwarmController::new(
                info_hash,
                peer_id.to_string(),
                piece_manager,
                disk_tx,
                self.snapshot.clone(),
                piece_length,
                total_length,
            ));
            tokio::spawn(swarm.start(peer_addrs));
        }

        let _ = self.event_tx.send(Event::TorrentAdded {
            hash: info_hash,
            name: torrent.info.name,
            size_bytes: total_length,
        });

        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Builds the list of FileRegion entries and returns (total_length, regions).
/// Supports both single-file and multi-file torrents.
fn build_file_regions(
    torrent: &crate::metainfo::Torrent,
    download_dir: &Path,
    info_hash: &[u8; 20],
) -> Result<(u64, Vec<FileRegion>)> {
    let jail = download_dir.join(hex::encode(info_hash));

    match &torrent.info.files {
        // ── Multi-file torrent ───────────────────────────────────────────────
        Some(files) => {
            let safe_root = crate::path_safety::sanitize_segment(&torrent.info.name)?;
            let torrent_root = jail.join(&safe_root);

            let mut offset = 0u64;
            let mut regions = Vec::with_capacity(files.len());

            for file in files {
                let mut path = torrent_root.clone();
                for seg in &file.path {
                    let safe = crate::path_safety::sanitize_segment(seg)?;
                    path = path.join(safe);
                }
                // Verify the assembled path is still inside the jail
                crate::path_safety::ensure_within_jail(&jail, &path)?;

                regions.push(FileRegion {
                    start: offset,
                    end: offset + file.length,
                    path,
                });
                offset += file.length;
            }
            Ok((offset, regions))
        }

        // ── Single-file torrent ──────────────────────────────────────────────
        None => {
            let length = torrent.info.length.unwrap_or(0);
            let path = crate::path_safety::build_jailed_single_file_path(
                download_dir,
                info_hash,
                &torrent.info.name,
            )?;
            Ok((length, vec![FileRegion { start: 0, end: length, path }]))
        }
    }
}

fn to_socket_addrs(peers: Vec<crate::tracker::PeerInfo>) -> Vec<std::net::SocketAddr> {
    peers
        .into_iter()
        .map(|p| {
            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(p.ip, p.port))
        })
        .collect()
}
