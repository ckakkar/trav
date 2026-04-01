use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::codec::Framed;
use tokio_stream::StreamExt;
use futures::SinkExt;
use tracing::{info, warn, error};

use crate::manager::PieceManager;
use crate::disk::DiskJob;
use crate::snapshot::{EngineSnapshot, TorrentSnapshot, PeerSnapshot};
use crate::peer::connection::PeerConnection;
use crate::peer::protocol::{PeerCodec, PeerMessage};
use crate::error::Result;

pub struct SwarmController {
    info_hash: [u8; 20],
    peer_id: String,
    piece_manager: Arc<Mutex<PieceManager>>,
    disk_tx: mpsc::Sender<DiskJob>,
    snapshot: Arc<std::sync::RwLock<EngineSnapshot>>,
    piece_length: u32,
    total_length: u64,
}

impl SwarmController {
    pub fn new(
        info_hash: [u8; 20],
        peer_id: String,
        piece_manager: Arc<Mutex<PieceManager>>,
        disk_tx: mpsc::Sender<DiskJob>,
        snapshot: Arc<std::sync::RwLock<EngineSnapshot>>,
        piece_length: u32,
        total_length: u64,
    ) -> Self {
        Self {
            info_hash,
            peer_id,
            piece_manager,
            disk_tx,
            snapshot,
            piece_length,
            total_length,
        }
    }

    pub async fn start(self: Arc<Self>, peers: Vec<SocketAddr>) {
        info!("Starting Swarm Controller for {} with {} peers", hex::encode(self.info_hash), peers.len());
        
        let pool = Arc::new(tokio::sync::Semaphore::new(50));
        
        for addr in peers {
            let swarm = self.clone();
            if let Ok(permit) = pool.clone().acquire_owned().await {
                tokio::spawn(async move {
                    if let Err(e) = swarm.handle_peer(addr).await {
                        warn!("Peer {} disconnected: {}", addr, e);
                    }
                    drop(permit);
                });
            }
        }
    }

    async fn handle_peer(&self, addr: SocketAddr) -> Result<()> {
        let mut conn = PeerConnection::connect(addr).await?;
        conn.handshake(&self.info_hash, &self.peer_id).await?;

        // Extract native stream (this drops PeerConnection builder logic)
        let stream = conn.stream;
        let mut framed = Framed::new(stream, PeerCodec);

        // Register peer in snapshot
        {
            if let Ok(mut snap) = self.snapshot.write() {
                if let Some(t) = snap.active_torrents.get_mut(&self.info_hash) {
                    t.peers.push(PeerSnapshot {
                        addr,
                        is_choked: true,
                        is_interested: false,
                        peer_choking: true,
                        peer_interested: false,
                        download_hz: 0,
                        upload_hz: 0,
                    });
                }
            }
        }

        // Send Interested to unchoke
        framed.send(PeerMessage::Interested).await.map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;

        loop {
            // Update UI Snapshot Speed
            let mut download_delta = 0;

            match tokio::time::timeout(std::time::Duration::from_secs(30), framed.next()).await {
                Ok(Some(Ok(msg))) => {
                    match msg {
                        PeerMessage::Choke => self.update_peer_state(addr, |p| p.peer_choking = true),
                        PeerMessage::Unchoke => {
                            self.update_peer_state(addr, |p| p.peer_choking = false);
                            // Request a piece
                            self.request_next_block(addr, &mut framed).await?;
                        }
                        PeerMessage::Have { piece_index } => {
                            self.piece_manager.lock().unwrap().handle_have(piece_index);
                        }
                        PeerMessage::Bitfield { payload } => {
                            self.piece_manager.lock().unwrap().handle_bitfield(&payload);
                        }
                        PeerMessage::Piece { index, begin, block } => {
                            download_delta += block.len() as u64;
                            self.disk_tx.send(DiskJob::WriteBlock {
                                piece_index: index,
                                begin,
                                data: block.clone(),
                            }).await.map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;
                            
                            // Let the manager verify if needed then ask for more
                            self.request_next_block(addr, &mut framed).await?;
                        }
                        _ => {}
                    }
                }
                Ok(Some(Err(e))) => return Err(e),
                Ok(None) => return Err(crate::error::BitTorrentError::Engine("Peer closed connection gracefully".into())),
                Err(_) => {
                    // Send KeepAlive on timeout
                    framed.send(PeerMessage::KeepAlive).await.map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;
                }
            }

            // Sync metrics to TUI
            if download_delta > 0 {
                if let Ok(mut snap) = self.snapshot.write() {
                    if let Some(t) = snap.active_torrents.get_mut(&self.info_hash) {
                        t.download_hz = t.download_hz.wrapping_add(download_delta);
                        if let Some(p) = t.peers.iter_mut().find(|p| p.addr == addr) {
                            p.download_hz = p.download_hz.wrapping_add(download_delta);
                        }
                        
                        // Fake progress update for UI mapping disk rights
                        if t.progress < 100.0 {
                            t.progress += (download_delta as f32 / t.size_bytes as f32) * 100.0;
                        }
                    }
                }
            }
        }
    }

    fn update_peer_state<F>(&self, addr: SocketAddr, f: F) 
    where F: FnOnce(&mut PeerSnapshot) {
        if let Ok(mut snap) = self.snapshot.write() {
            if let Some(t) = snap.active_torrents.get_mut(&self.info_hash) {
                if let Some(p) = t.peers.iter_mut().find(|p| p.addr == addr) {
                    f(p);
                }
            }
        }
    }

    async fn request_next_block(&self, addr: SocketAddr, framed: &mut Framed<tokio::net::TcpStream, PeerCodec>) -> Result<()> {
        let next_piece = {
            let mut pm = self.piece_manager.lock().unwrap();
            pm.pick_rarest_piece()
        };

        if let Some(index) = next_piece {
            // Simplified: Requesting a full 16KB block
            // A real engine iterates 16KB sub-blocks until the piece_length is fulfilled
            let length = std::cmp::min(16384, self.piece_length);
            framed.send(PeerMessage::Request {
                index,
                begin: 0,
                length,
            }).await.map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;
        }
        Ok(())
    }
}
