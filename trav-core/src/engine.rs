use crate::message::{Command, Event};
use crate::error::Result;
use crate::snapshot::EngineSnapshot;
use tokio::sync::{mpsc, broadcast};
use tracing::{info, warn, debug};
use std::sync::{Arc, RwLock};
use std::collections::HashMap;

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

        let engine = Self {
            command_rx,
            event_tx: event_tx.clone(),
            snapshot: snapshot.clone(),
        };

        (engine, command_tx, event_rx, snapshot)
    }

    pub async fn run(mut self) -> Result<()> {
        info!("BitTorrent Engine starting…");
        let _ = self.event_tx.send(Event::EngineStarted);

        // Azureus-style peer_id: "-TR2026-<12 pseudo-random digits>"
        let peer_id = {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            let pid = std::process::id();
            format!("-TR2026-{:06}{:06}", ts % 1_000_000, pid % 1_000_000)
        };

        // Periodic tick: compute per-torrent download_hz from bytes_downloaded_total deltas.
        let mut speed_tick = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut last_bytes: HashMap<[u8; 20], u64> = HashMap::new();

        loop {
            tokio::select! {
                _ = speed_tick.tick() => {
                    if let Ok(mut snap) = self.snapshot.write() {
                        let mut total_down = 0u64;
                        for (hash, torrent) in snap.active_torrents.iter_mut() {
                            let prev = last_bytes.get(hash).copied().unwrap_or(torrent.bytes_downloaded_total);
                            let delta = torrent.bytes_downloaded_total.saturating_sub(prev);
                            torrent.download_hz = delta; // bytes received in the last second
                            last_bytes.insert(*hash, torrent.bytes_downloaded_total);
                            total_down = total_down.saturating_add(torrent.download_hz);
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
                                    let info_hash = match torrent.info_hash() {
                                        Ok(h) => h,
                                        Err(e) => {
                                            tracing::error!("info_hash failed: {}", e);
                                            continue;
                                        }
                                    };
                                    let piece_length = torrent.info.piece_length as u32;
                                    let total_length = torrent.info.length.unwrap_or(0);
                                    let num_pieces = (torrent.info.pieces.len() / 20) as u32;

                                    // Reject multi-file torrents (not yet safely supported)
                                    if let Some(files) = &torrent.info.files {
                                        let paths: Vec<Vec<String>> = files.iter().map(|f| f.path.clone()).collect();
                                        if let Err(e) = crate::path_safety::validate_multi_file_paths(&paths) {
                                            tracing::error!("Rejected unsafe multi-file paths: {}", e);
                                            continue;
                                        }
                                        tracing::warn!("Multi-file torrents are not yet supported.");
                                        continue;
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

                                    // Build jailed output path
                                    let download_path = match crate::path_safety::build_jailed_single_file_path(
                                        &download_dir,
                                        &info_hash,
                                        &torrent.info.name,
                                    ) {
                                        Ok(p) => p,
                                        Err(e) => {
                                            tracing::error!("Rejected unsafe torrent path: {}", e);
                                            continue;
                                        }
                                    };
                                    if let Some(parent) = download_path.parent() {
                                        if let Err(e) = tokio::fs::create_dir_all(parent).await {
                                            tracing::error!("Failed to create download directory: {}", e);
                                            continue;
                                        }
                                    }

                                    let disk_tx = match crate::disk::DiskTask::spawn(download_path, piece_length).await {
                                        Ok(tx) => tx,
                                        Err(e) => {
                                            tracing::error!("Failed to spawn disk task: {}", e);
                                            continue;
                                        }
                                    };

                                    let piece_manager = Arc::new(std::sync::Mutex::new(
                                        crate::manager::PieceManager::new(
                                            num_pieces,
                                            piece_length,
                                            total_length,
                                            torrent.info.pieces.clone(),
                                        )
                                    ));

                                    // Try all trackers (main announce + announce-list)
                                    let mut peer_addrs = vec![];
                                    for tracker_url in torrent.all_trackers() {
                                        if !peer_addrs.is_empty() {
                                            break;
                                        }
                                        info!("Trying tracker: {}", tracker_url);
                                        if tracker_url.starts_with("http") {
                                            let client = crate::tracker::TrackerClient::new(&peer_id, 6881);
                                            match client.announce(&tracker_url, &info_hash, 0, 0, total_length).await {
                                                Ok(resp) => {
                                                    peer_addrs = resp.parse_peers()
                                                        .into_iter()
                                                        .map(|p| std::net::SocketAddr::V4(
                                                            std::net::SocketAddrV4::new(p.ip, p.port)
                                                        ))
                                                        .collect();
                                                    info!("Tracker {} returned {} peers", tracker_url, peer_addrs.len());
                                                }
                                                Err(e) => warn!("HTTP tracker {} failed: {}", tracker_url, e),
                                            }
                                        } else if tracker_url.starts_with("udp") {
                                            let client = crate::tracker::UdpTrackerClient::new(&peer_id, 6881);
                                            match client.announce(&tracker_url, &info_hash, 0, 0, total_length).await {
                                                Ok(resp) => {
                                                    peer_addrs = resp.parse_peers()
                                                        .into_iter()
                                                        .map(|p| std::net::SocketAddr::V4(
                                                            std::net::SocketAddrV4::new(p.ip, p.port)
                                                        ))
                                                        .collect();
                                                    info!("UDP tracker {} returned {} peers", tracker_url, peer_addrs.len());
                                                }
                                                Err(e) => warn!("UDP tracker {} failed: {}", tracker_url, e),
                                            }
                                        }
                                    }

                                    if peer_addrs.is_empty() {
                                        warn!("All trackers returned 0 peers for {}", hex::encode(info_hash));
                                        if let Ok(mut s) = self.snapshot.write() {
                                            if let Some(t) = s.active_torrents.get_mut(&info_hash) {
                                                t.state = "No Peers".to_string();
                                            }
                                        }
                                        let _ = self.event_tx.send(Event::TorrentAdded {
                                            hash: info_hash,
                                            name: torrent.info.name,
                                            size_bytes: total_length,
                                        });
                                        continue;
                                    }

                                    {
                                        let mut s = self.snapshot.write().unwrap();
                                        if let Some(t) = s.active_torrents.get_mut(&info_hash) {
                                            t.state = "Downloading".to_string();
                                        }
                                    }

                                    let swarm = Arc::new(crate::swarm::SwarmController::new(
                                        info_hash,
                                        peer_id.clone(),
                                        piece_manager,
                                        disk_tx,
                                        self.snapshot.clone(),
                                        piece_length,
                                        total_length,
                                    ));
                                    tokio::spawn(swarm.start(peer_addrs));

                                    let _ = self.event_tx.send(Event::TorrentAdded {
                                        hash: info_hash,
                                        name: torrent.info.name,
                                        size_bytes: total_length,
                                    });
                                }
                                Err(e) => tracing::error!("Failed to parse .torrent file: {}", e),
                            }
                        }

                        Some(Command::Pause { torrent_hash }) => {
                            info!("Pause: {}", hex::encode(torrent_hash));
                            if let Ok(mut s) = self.snapshot.write() {
                                if let Some(t) = s.active_torrents.get_mut(&torrent_hash) {
                                    t.state = "Paused".to_string();
                                }
                            }
                        }

                        Some(Command::Resume { torrent_hash }) => {
                            info!("Resume: {}", hex::encode(torrent_hash));
                            if let Ok(mut s) = self.snapshot.write() {
                                if let Some(t) = s.active_torrents.get_mut(&torrent_hash) {
                                    t.state = "Downloading".to_string();
                                }
                            }
                        }

                        Some(Command::Quit) => {
                            info!("Quit received — shutting down.");
                            break;
                        }

                        None => {
                            debug!("Command channel closed — shutting down.");
                            break;
                        }
                    }
                }
            }
        }

        info!("Engine shutdown complete.");
        Ok(())
    }
}
