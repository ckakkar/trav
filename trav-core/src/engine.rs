use crate::message::{Command, Event};
use crate::error::Result;
use crate::snapshot::EngineSnapshot;
use tokio::sync::{mpsc, broadcast};
use tracing::{info, debug};
use std::sync::{Arc, RwLock};

/// The core BitTorrent Engine.
pub struct Engine {
    command_rx: mpsc::Receiver<Command>,
    event_tx: broadcast::Sender<Event>,
    snapshot: Arc<RwLock<EngineSnapshot>>,
}

impl Engine {
    /// Creates a new Engine and returns the Channels and Snapshot to communicate with it.
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

    /// Spawns the engine's main event loop on the current Tokio runtime.
    pub async fn run(mut self) -> Result<()> {
        info!("Starting BitTorrent Engine...");
        let _ = self.event_tx.send(Event::EngineStarted);

        loop {
            tokio::select! {
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(Command::AddTorrent { file_path, download_dir }) => {
                            info!("Command received: AddTorrent {:?}", file_path);
                            // 1. Read Torrent File
                            match crate::metainfo::Torrent::read_from_file(&file_path).await {
                                Ok(torrent) => {
                                    let info_hash = torrent.info_hash().unwrap_or([0u8; 20]);
                                    let piece_length = torrent.info.piece_length as u32;
                                    let total_length = torrent.info.length.unwrap_or(0); // Simplified for single-file
                                    let num_pieces = (torrent.info.pieces.len() / 20) as u32;
                                    
                                    // 2. Register Torrent in Snapshot for UI
                                    {
                                        let mut s = self.snapshot.write().unwrap();
                                        s.active_torrents.insert(info_hash, crate::snapshot::TorrentSnapshot {
                                            name: torrent.info.name.clone(),
                                            info_hash,
                                            info_hash_hex: hex::encode(info_hash),
                                            size_bytes: total_length,
                                            progress: 0.0,
                                            download_hz: 0,
                                            upload_hz: 0,
                                            state: "Initializing".to_string(),
                                            peers: vec![],
                                            piece_map_base64: "".to_string(),
                                        });
                                    }

                                    // 3. Initialize Disk I/O & Piece Manager
                                    let download_path = download_dir.join(&torrent.info.name);
                                    if let Err(e) = tokio::fs::create_dir_all(&download_dir).await {
                                        tracing::error!("Failed to create downloads directory: {}", e);
                                    }
                                    
                                    match crate::disk::DiskTask::spawn(download_path, piece_length).await {
                                        Ok(disk_tx) => {
                                            let piece_manager = std::sync::Arc::new(std::sync::Mutex::new(
                                                crate::manager::PieceManager::new(
                                                    num_pieces, 
                                                    piece_length, 
                                                    total_length, 
                                                    torrent.info.pieces.clone()
                                                )
                                            ));

                                            // 4. Query Tracker
                                            // The announce can be HTTP or UDP. We check prefix.
                                            let announce_url = torrent.announce.clone();
                                            let peer_id = "-TR2026-N1O2V3A4G5U6";
                                            let mut peer_addrs = vec![];
                                            
                                            if announce_url.starts_with("http") {
                                                let client = crate::tracker::TrackerClient::new(peer_id, 6881);
                                                if let Ok(resp) = client.announce(&announce_url, &info_hash, 0, 0, total_length).await {
                                                    peer_addrs = resp.parse_peers().into_iter().map(|p| std::net::SocketAddr::V4(std::net::SocketAddrV4::new(p.ip, p.port))).collect();
                                                }
                                            } else if announce_url.starts_with("udp") {
                                                let client = crate::tracker::UdpTrackerClient::new(peer_id, 6881);
                                                if let Ok(resp) = client.announce(&announce_url, &info_hash, 0, 0, total_length).await {
                                                    peer_addrs = resp.parse_peers().into_iter().map(|p| std::net::SocketAddr::V4(std::net::SocketAddrV4::new(p.ip, p.port))).collect();
                                                }
                                            }

                                            // 5. Spawn Swarm Controller
                                            if !peer_addrs.is_empty() {
                                                let swarm = std::sync::Arc::new(crate::swarm::SwarmController::new(
                                                    info_hash,
                                                    peer_id.to_string(),
                                                    piece_manager,
                                                    disk_tx,
                                                    self.snapshot.clone(),
                                                    piece_length,
                                                    total_length
                                                ));
                                                
                                                {
                                                    let mut s = self.snapshot.write().unwrap();
                                                    if let Some(t) = s.active_torrents.get_mut(&info_hash) {
                                                        t.state = "Downloading".to_string();
                                                    }
                                                }
                                                
                                                tokio::spawn(swarm.start(peer_addrs));
                                                let _ = self.event_tx.send(Event::TorrentAdded { hash: info_hash, name: torrent.info.name, size_bytes: total_length });
                                            } else {
                                                tracing::warn!("Tracker returned 0 peers.");
                                            }
                                        }
                                        Err(e) => tracing::error!("Failed to spawn disk task: {}", e),
                                    }
                                }
                                Err(e) => tracing::error!("Failed to read .torrent file: {}", e),
                            }
                        }
                        Some(Command::Pause { torrent_hash }) => {
                            info!("Command received: Pause {}", hex::encode(torrent_hash));
                        }
                        Some(Command::Resume { torrent_hash }) => {
                            info!("Command received: Resume {}", hex::encode(torrent_hash));
                        }
                        Some(Command::Quit) => {
                            info!("Quit command received. Shutting down engine...");
                            break;
                        }
                        None => {
                            debug!("Command channel closed. Shutting down.");
                            break;
                        }
                    }
                }
            }
        }

        info!("BitTorrent Engine fully shutdown.");
        Ok(())
    }
}
