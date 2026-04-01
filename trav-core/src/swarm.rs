use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::codec::Framed;
use tokio_stream::StreamExt;
use futures::SinkExt;
use tracing::{info, warn};

use crate::manager::PieceManager;
use crate::disk::DiskJob;
use crate::snapshot::{EngineSnapshot, PeerSnapshot};
use crate::peer::connection::PeerConnection;
use crate::peer::protocol::{PeerCodec, PeerMessage};
use crate::error::Result;

const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
const PIECE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const BLOCK_SIZE: u32 = 16 * 1024;
const MAX_REQUEST_RETRIES_PER_PIECE: u8 = 3;
const PEER_PENALTY_DISCONNECT_THRESHOLD: u32 = 10;
const PEER_BACKOFF_BASE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
struct PendingRequest {
    index: u32,
    begin: u32,
    length: u32,
    sent_at: Instant,
}

struct ActivePiece {
    index: u32,
    expected_len: usize,
    next_begin: u32,
    buffer: Vec<u8>,
}

struct PeerHealth {
    network_penalty: u32,
    data_penalty: u32,
    retries_for_piece: u8,
    backoff_until: Option<Instant>,
    timeout_count: u32,
    bad_data_count: u32,
    hash_fail_count: u32,
}

impl PeerHealth {
    fn new() -> Self {
        Self {
            network_penalty: 0,
            data_penalty: 0,
            retries_for_piece: 0,
            backoff_until: None,
            timeout_count: 0,
            bad_data_count: 0,
            hash_fail_count: 0,
        }
    }

    fn penalty_score(&self) -> u32 {
        self.network_penalty.saturating_add(self.data_penalty.saturating_mul(2))
    }

    fn penalize_network(&mut self, amount: u32) {
        self.network_penalty = self.network_penalty.saturating_add(amount);
        self.timeout_count = self.timeout_count.saturating_add(1);
        let seconds = self.penalty_score().min(5) as u64;
        self.backoff_until = Some(Instant::now() + PEER_BACKOFF_BASE * seconds as u32);
    }

    fn penalize_data(&mut self, amount: u32, hash_fail: bool) {
        self.data_penalty = self.data_penalty.saturating_add(amount);
        self.bad_data_count = self.bad_data_count.saturating_add(1);
        if hash_fail {
            self.hash_fail_count = self.hash_fail_count.saturating_add(1);
        }
        let seconds = self.penalty_score().min(5) as u64;
        self.backoff_until = Some(Instant::now() + PEER_BACKOFF_BASE * seconds as u32);
    }

    fn reward_success(&mut self) {
        self.network_penalty = self.network_penalty.saturating_sub(1);
        self.data_penalty = self.data_penalty.saturating_sub(1);
        self.retries_for_piece = 0;
        self.backoff_until = None;
    }

    fn should_backoff(&self) -> bool {
        matches!(self.backoff_until, Some(until) if Instant::now() < until)
    }

    fn should_disconnect(&self) -> bool {
        self.penalty_score() >= PEER_PENALTY_DISCONNECT_THRESHOLD
    }
}

pub struct SwarmController {
    info_hash: [u8; 20],
    peer_id: String,
    piece_manager: Arc<Mutex<PieceManager>>,
    disk_tx: mpsc::Sender<DiskJob>,
    snapshot: Arc<std::sync::RwLock<EngineSnapshot>>,
    piece_length: u32,
    _total_length: u64,
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
            _total_length: total_length,
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
                        penalty_score: 0,
                        network_penalty: 0,
                        data_penalty: 0,
                        timeout_count: 0,
                        bad_data_count: 0,
                        hash_fail_count: 0,
                    });
                }
            }
        }

        // Send Interested to unchoke
        tokio::time::timeout(WRITE_TIMEOUT, framed.send(PeerMessage::Interested))
            .await
            .map_err(|_| crate::error::BitTorrentError::Engine(format!("Interested write timeout for peer {}", addr)))?
            .map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;

        let mut pending_request: Option<PendingRequest> = None;
        let mut active_piece: Option<ActivePiece> = None;
        let mut peer_health = PeerHealth::new();

        loop {
            // Update UI Snapshot Speed
            let mut download_delta = 0;

            if let Some(pending) = pending_request {
                if pending.sent_at.elapsed() >= PIECE_REQUEST_TIMEOUT {
                    if let Ok(mut pm) = self.piece_manager.lock() {
                        pm.mark_piece_timed_out(pending.index);
                    }
                    pending_request = None;
                    active_piece = None;
                    peer_health.retries_for_piece = peer_health.retries_for_piece.saturating_add(1);
                    peer_health.penalize_network(2);
                    self.sync_peer_health(addr, &peer_health);
                    if peer_health.should_disconnect() {
                        return Err(crate::error::BitTorrentError::Engine(format!(
                            "Peer {} disconnected due to repeated request timeouts",
                            addr
                        )));
                    }
                }
            }

            match tokio::time::timeout(IDLE_TIMEOUT, framed.next()).await {
                Ok(Some(Ok(msg))) => {
                    match msg {
                        PeerMessage::Choke => self.update_peer_state(addr, |p| p.peer_choking = true),
                        PeerMessage::Unchoke => {
                            self.update_peer_state(addr, |p| p.peer_choking = false);
                            self.request_next_block(&mut framed, &mut active_piece, &mut pending_request, &peer_health).await?;
                        }
                        PeerMessage::Have { piece_index } => {
                            self.piece_manager.lock().unwrap().handle_have(piece_index);
                        }
                        PeerMessage::Bitfield { payload } => {
                            self.piece_manager.lock().unwrap().handle_bitfield(&payload);
                        }
                        PeerMessage::Piece { index, begin, block } => {
                            if let Some(pending) = pending_request {
                                if pending.index != index || pending.begin != begin || block.len() as u32 > pending.length {
                                    peer_health.penalize_data(1, false);
                                    self.sync_peer_health(addr, &peer_health);
                                    continue;
                                }
                            } else {
                                peer_health.penalize_data(1, false);
                                self.sync_peer_health(addr, &peer_health);
                                continue;
                            }
                            pending_request = None;
                            download_delta += block.len() as u64;

                            let piece_done;
                            let mut verified_piece: Option<(u32, Vec<u8>)> = None;
                            {
                                let Some(active) = active_piece.as_mut() else {
                                    continue;
                                };
                                if active.index != index || active.next_begin != begin {
                                    peer_health.penalize_data(1, false);
                                    self.sync_peer_health(addr, &peer_health);
                                    continue;
                                }

                                if active.buffer.len() + block.len() > active.expected_len {
                                    if let Ok(mut pm) = self.piece_manager.lock() {
                                        pm.mark_piece_timed_out(active.index);
                                    }
                                    active_piece = None;
                                    peer_health.retries_for_piece = peer_health.retries_for_piece.saturating_add(1);
                                    peer_health.penalize_data(2, false);
                                    self.sync_peer_health(addr, &peer_health);
                                    continue;
                                }

                                active.buffer.extend_from_slice(&block);
                                active.next_begin = active.next_begin.saturating_add(block.len() as u32);
                                piece_done = active.buffer.len() == active.expected_len;
                                if piece_done {
                                    verified_piece = Some((active.index, active.buffer.clone()));
                                }
                            }

                            if let Some((piece_index, piece_data)) = verified_piece {
                                let expected_hash = {
                                    let pm = self.piece_manager.lock().unwrap();
                                    pm.expected_hash(piece_index)
                                };
                                let Some(expected_hash) = expected_hash else {
                                    active_piece = None;
                                    continue;
                                };
                                let piece_data_for_verify = piece_data.clone();

                                let verified = tokio::task::spawn_blocking(move || {
                                    PieceManager::verify_piece_data(&expected_hash, &piece_data_for_verify)
                                })
                                .await
                                .map_err(|e| crate::error::BitTorrentError::Engine(format!("Hash verify task failed: {}", e)))?;

                                {
                                    let mut pm = self.piece_manager.lock().unwrap();
                                    pm.mark_piece_verification(piece_index, verified);
                                }

                                if verified {
                                    self.disk_tx.send(DiskJob::WriteBlock {
                                        piece_index,
                                        begin: 0,
                                        data: piece_data,
                                    }).await.map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;
                                    peer_health.reward_success();
                                    self.sync_peer_health(addr, &peer_health);
                                } else {
                                    peer_health.retries_for_piece = peer_health.retries_for_piece.saturating_add(1);
                                    peer_health.penalize_data(3, true);
                                    self.sync_peer_health(addr, &peer_health);
                                }
                                active_piece = None;
                            }

                            if peer_health.retries_for_piece > MAX_REQUEST_RETRIES_PER_PIECE {
                                if let Some(active) = active_piece.take() {
                                    if let Ok(mut pm) = self.piece_manager.lock() {
                                        pm.mark_piece_timed_out(active.index);
                                    }
                                }
                                peer_health.penalize_data(2, false);
                                self.sync_peer_health(addr, &peer_health);
                                peer_health.retries_for_piece = 0;
                            }
                            if peer_health.should_disconnect() {
                                return Err(crate::error::BitTorrentError::Engine(format!(
                                    "Peer {} disconnected due to high penalty score",
                                    addr
                                )));
                            }
                            self.request_next_block(&mut framed, &mut active_piece, &mut pending_request, &peer_health).await?;
                        }
                        _ => {}
                    }
                }
                Ok(Some(Err(e))) => return Err(e),
                Ok(None) => return Err(crate::error::BitTorrentError::Engine("Peer closed connection gracefully".into())),
                Err(_) => {
                    // Send KeepAlive on timeout
                    tokio::time::timeout(WRITE_TIMEOUT, framed.send(PeerMessage::KeepAlive))
                        .await
                        .map_err(|_| crate::error::BitTorrentError::Engine(format!("KeepAlive write timeout for peer {}", addr)))?
                        .map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;
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

    fn sync_peer_health(&self, addr: SocketAddr, health: &PeerHealth) {
        self.update_peer_state(addr, |p| {
            p.network_penalty = health.network_penalty;
            p.data_penalty = health.data_penalty;
            p.penalty_score = health.penalty_score();
            p.timeout_count = health.timeout_count;
            p.bad_data_count = health.bad_data_count;
            p.hash_fail_count = health.hash_fail_count;
        });
    }

    async fn request_next_block(
        &self,
        framed: &mut Framed<tokio::net::TcpStream, PeerCodec>,
        active_piece: &mut Option<ActivePiece>,
        pending_request: &mut Option<PendingRequest>,
        peer_health: &PeerHealth,
    ) -> Result<()> {
        if pending_request.is_some() {
            return Ok(());
        }
        if peer_health.should_backoff() {
            return Ok(());
        }

        if active_piece.is_none() {
            let next_piece = {
                let mut pm = self.piece_manager.lock().unwrap();
                pm.pick_rarest_piece()
            };
            if let Some(index) = next_piece {
                let expected_len = {
                    let pm = self.piece_manager.lock().unwrap();
                    pm.piece_size(index).unwrap_or(self.piece_length) as usize
                };
                *active_piece = Some(ActivePiece {
                    index,
                    expected_len,
                    next_begin: 0,
                    buffer: Vec::with_capacity(expected_len),
                });
            } else {
                return Ok(());
            }
        }

        let Some(active) = active_piece.as_ref() else {
            return Ok(());
        };
        if active.next_begin as usize >= active.expected_len {
            return Ok(());
        }
        let remaining = (active.expected_len as u32).saturating_sub(active.next_begin);
        let req_len = std::cmp::min(BLOCK_SIZE, remaining);

        tokio::time::timeout(WRITE_TIMEOUT, framed.send(PeerMessage::Request {
            index: active.index,
            begin: active.next_begin,
            length: req_len,
        }))
        .await
        .map_err(|_| crate::error::BitTorrentError::Engine(format!("Request write timeout for piece {}", active.index)))?
        .map_err(|e| crate::error::BitTorrentError::Engine(e.to_string()))?;

        *pending_request = Some(PendingRequest {
            index: active.index,
            begin: active.next_begin,
            length: req_len,
            sent_at: Instant::now(),
        });
        Ok(())
    }
}
