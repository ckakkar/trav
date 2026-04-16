use std::collections::{BTreeMap, VecDeque};
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
/// Number of block requests kept in-flight per peer connection.
const PIPELINE_DEPTH: usize = 5;
const MAX_RETRIES_PER_PIECE: u8 = 3;
const PENALTY_DISCONNECT: u32 = 10;
const BACKOFF_BASE: Duration = Duration::from_secs(2);

// ── Peer-local state ─────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct PendingRequest {
    index: u32,
    begin: u32,
    length: u32,
    sent_at: Instant,
}

/// Active piece being assembled. Uses a BTreeMap so blocks arriving out of
/// order (possible with pipelining) are stored and assembled in order.
struct ActivePiece {
    index: u32,
    expected_len: usize,
    /// Next byte offset to include in a Request message.
    next_request_begin: u32,
    /// Received blocks: begin → data.
    blocks: BTreeMap<u32, Vec<u8>>,
}

impl ActivePiece {
    fn bytes_received(&self) -> usize {
        self.blocks.values().map(|b| b.len()).sum()
    }
    fn is_complete(&self) -> bool {
        self.bytes_received() == self.expected_len
    }
    fn assemble(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.expected_len);
        // BTreeMap guarantees ascending key order
        for data in self.blocks.values() {
            buf.extend_from_slice(data);
        }
        buf
    }
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

    fn score(&self) -> u32 {
        self.network_penalty.saturating_add(self.data_penalty.saturating_mul(2))
    }

    fn penalize_network(&mut self, amount: u32) {
        self.network_penalty = self.network_penalty.saturating_add(amount);
        self.timeout_count = self.timeout_count.saturating_add(1);
        let secs = self.score().min(5) as u32;
        self.backoff_until = Some(Instant::now() + BACKOFF_BASE * secs);
    }

    fn penalize_data(&mut self, amount: u32, hash_fail: bool) {
        self.data_penalty = self.data_penalty.saturating_add(amount);
        self.bad_data_count = self.bad_data_count.saturating_add(1);
        if hash_fail {
            self.hash_fail_count = self.hash_fail_count.saturating_add(1);
        }
        let secs = self.score().min(5) as u32;
        self.backoff_until = Some(Instant::now() + BACKOFF_BASE * secs);
    }

    fn reward(&mut self) {
        self.network_penalty = self.network_penalty.saturating_sub(1);
        self.data_penalty = self.data_penalty.saturating_sub(1);
        self.retries_for_piece = 0;
        self.backoff_until = None;
    }

    fn should_backoff(&self) -> bool {
        matches!(self.backoff_until, Some(t) if Instant::now() < t)
    }

    fn should_disconnect(&self) -> bool {
        self.score() >= PENALTY_DISCONNECT
    }
}

// ── SwarmController ───────────────────────────────────────────────────────────

pub struct SwarmController {
    info_hash: [u8; 20],
    peer_id: String,
    piece_manager: Arc<Mutex<PieceManager>>,
    disk_tx: mpsc::Sender<DiskJob>,
    snapshot: Arc<std::sync::RwLock<EngineSnapshot>>,
    piece_length: u32,
}

impl SwarmController {
    pub fn new(
        info_hash: [u8; 20],
        peer_id: String,
        piece_manager: Arc<Mutex<PieceManager>>,
        disk_tx: mpsc::Sender<DiskJob>,
        snapshot: Arc<std::sync::RwLock<EngineSnapshot>>,
        piece_length: u32,
        _total_length: u64,
    ) -> Self {
        Self { info_hash, peer_id, piece_manager, disk_tx, snapshot, piece_length }
    }

    pub async fn start(self: Arc<Self>, peers: Vec<SocketAddr>) {
        info!(
            "Swarm {} starting with {} peers",
            hex::encode(self.info_hash),
            peers.len()
        );
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

    // ── Per-peer download loop ────────────────────────────────────────────────

    async fn handle_peer(&self, addr: SocketAddr) -> Result<()> {
        let mut conn = PeerConnection::connect(addr).await?;
        conn.handshake(&self.info_hash, &self.peer_id).await?;

        let mut framed = Framed::new(conn.stream, PeerCodec);

        self.register_peer(addr);

        // Signal interest immediately after handshake
        tokio::time::timeout(WRITE_TIMEOUT, framed.send(PeerMessage::Interested))
            .await
            .map_err(|_| crate::error::BitTorrentError::Engine(
                format!("Interested write timeout for {}", addr)
            ))??;

        let mut pending: VecDeque<PendingRequest> = VecDeque::with_capacity(PIPELINE_DEPTH);
        let mut active_piece: Option<ActivePiece> = None;
        let mut health = PeerHealth::new();

        loop {
            // ── Timeout check: drop the piece if any request has stalled ──────
            let any_timed_out = pending.iter().any(|r| r.sent_at.elapsed() >= PIECE_REQUEST_TIMEOUT);
            if any_timed_out {
                if let Some(ref ap) = active_piece {
                    self.piece_manager.lock().unwrap().mark_piece_timed_out(ap.index);
                }
                active_piece = None;
                pending.clear();
                health.retries_for_piece = health.retries_for_piece.saturating_add(1);
                health.penalize_network(2);
                self.sync_health(addr, &health);
                if health.should_disconnect() {
                    return Err(crate::error::BitTorrentError::Engine(
                        format!("Peer {} exceeded timeout penalty", addr)
                    ));
                }
            }

            match tokio::time::timeout(IDLE_TIMEOUT, framed.next()).await {
                Ok(Some(Ok(msg))) => {
                    match msg {
                        PeerMessage::Choke => {
                            self.update_peer(addr, |p| p.peer_choking = true);
                            // Cancel all in-flight requests: peer won't respond to them
                            if let Some(ref ap) = active_piece {
                                self.piece_manager.lock().unwrap().mark_piece_timed_out(ap.index);
                            }
                            active_piece = None;
                            pending.clear();
                        }
                        PeerMessage::Unchoke => {
                            self.update_peer(addr, |p| p.peer_choking = false);
                            self.fill_pipeline(
                                &mut framed, &mut active_piece, &mut pending, &health
                            ).await?;
                        }
                        PeerMessage::Have { piece_index } => {
                            self.piece_manager.lock().unwrap().handle_have(piece_index);
                        }
                        PeerMessage::Bitfield { payload } => {
                            self.piece_manager.lock().unwrap().handle_bitfield(&payload);
                        }
                        PeerMessage::Piece { index, begin, block } => {
                            // Find matching in-flight request
                            let matched = pending
                                .iter()
                                .position(|r| r.index == index && r.begin == begin);

                            if matched.is_none() {
                                // Unsolicited or duplicate block — penalize and skip
                                health.penalize_data(1, false);
                                self.sync_health(addr, &health);
                                continue;
                            }
                            pending.remove(matched.unwrap());

                            let block_len = block.len() as u64;

                            // Store block in active piece
                            let piece_complete = if let Some(ap) = active_piece.as_mut() {
                                if ap.index != index {
                                    // Block for a piece we're not tracking — ignore
                                    health.penalize_data(1, false);
                                    self.sync_health(addr, &health);
                                    false
                                } else if ap.blocks.len() * BLOCK_SIZE as usize + block.len()
                                    > ap.expected_len
                                {
                                    // Overflow: corrupt
                                    self.piece_manager.lock().unwrap().mark_piece_timed_out(ap.index);
                                    active_piece = None;
                                    pending.clear();
                                    health.penalize_data(2, false);
                                    self.sync_health(addr, &health);
                                    false
                                } else {
                                    ap.blocks.insert(begin, block);
                                    ap.is_complete()
                                }
                            } else {
                                // No active piece — stale response
                                health.penalize_data(1, false);
                                self.sync_health(addr, &health);
                                false
                            };

                            // Accumulate bytes for speed metric
                            if block_len > 0 {
                                if let Ok(mut snap) = self.snapshot.write() {
                                    if let Some(t) = snap.active_torrents.get_mut(&self.info_hash) {
                                        t.bytes_downloaded_total =
                                            t.bytes_downloaded_total.saturating_add(block_len);
                                    }
                                }
                            }

                            if piece_complete {
                                let ap = active_piece.take().unwrap();
                                let piece_data = ap.assemble();
                                let piece_index = ap.index;
                                pending.clear(); // all blocks received

                                let expected = {
                                    self.piece_manager.lock().unwrap().expected_hash(piece_index)
                                };
                                if let Some(expected_hash) = expected {
                                    let data_for_verify = piece_data.clone();
                                    let verified =
                                        tokio::task::spawn_blocking(move || {
                                            PieceManager::verify_piece_data(
                                                &expected_hash,
                                                &data_for_verify,
                                            )
                                        })
                                        .await
                                        .map_err(|e| {
                                            crate::error::BitTorrentError::Engine(e.to_string())
                                        })?;

                                    {
                                        let mut pm = self.piece_manager.lock().unwrap();
                                        pm.mark_piece_verification(piece_index, verified);
                                    }

                                    if verified {
                                        self.disk_tx
                                            .send(DiskJob::WriteBlock {
                                                piece_index,
                                                begin: 0,
                                                data: piece_data,
                                            })
                                            .await
                                            .map_err(|e| {
                                                crate::error::BitTorrentError::Engine(e.to_string())
                                            })?;

                                        let (dl, total, done) = {
                                            let pm = self.piece_manager.lock().unwrap();
                                            (
                                                pm.downloaded_count() as u32,
                                                pm.num_pieces,
                                                pm.is_complete(),
                                            )
                                        };
                                        if let Ok(mut snap) = self.snapshot.write() {
                                            if let Some(t) =
                                                snap.active_torrents.get_mut(&self.info_hash)
                                            {
                                                t.pieces_downloaded = dl;
                                                t.progress = if total > 0 {
                                                    (dl as f32 / total as f32 * 100.0).min(100.0)
                                                } else {
                                                    0.0
                                                };
                                                if done {
                                                    t.state = "Seeding".to_string();
                                                }
                                            }
                                        }
                                        health.reward();
                                        self.sync_health(addr, &health);
                                    } else {
                                        health.retries_for_piece =
                                            health.retries_for_piece.saturating_add(1);
                                        health.penalize_data(3, true);
                                        self.sync_health(addr, &health);

                                        if health.retries_for_piece > MAX_RETRIES_PER_PIECE {
                                            health.retries_for_piece = 0;
                                        }
                                    }
                                }
                            }

                            if health.should_disconnect() {
                                return Err(crate::error::BitTorrentError::Engine(
                                    format!("Peer {} penalty threshold exceeded", addr)
                                ));
                            }

                            // Keep the pipeline full
                            self.fill_pipeline(
                                &mut framed, &mut active_piece, &mut pending, &health
                            ).await?;
                        }
                        PeerMessage::Unknown { .. } => {} // silently absorb
                        _ => {}
                    }
                }
                Ok(Some(Err(e))) => return Err(e),
                Ok(None) => {
                    return Err(crate::error::BitTorrentError::Engine(
                        "Peer closed connection".into()
                    ));
                }
                Err(_) => {
                    // Idle timeout — send KeepAlive to stay alive
                    tokio::time::timeout(WRITE_TIMEOUT, framed.send(PeerMessage::KeepAlive))
                        .await
                        .map_err(|_| crate::error::BitTorrentError::Engine(
                            format!("KeepAlive timeout for {}", addr)
                        ))??;
                }
            }
        }
    }

    // ── Pipeline filling ─────────────────────────────────────────────────────
    //
    // Sends block Request messages until PIPELINE_DEPTH in-flight requests exist
    // or there is nothing more to request on the current piece.

    async fn fill_pipeline(
        &self,
        framed: &mut Framed<tokio::net::TcpStream, PeerCodec>,
        active_piece: &mut Option<ActivePiece>,
        pending: &mut VecDeque<PendingRequest>,
        health: &PeerHealth,
    ) -> Result<()> {
        if health.should_backoff() {
            return Ok(());
        }

        while pending.len() < PIPELINE_DEPTH {
            // Ensure we have a piece to download
            if active_piece.is_none() {
                let next = {
                    let mut pm = self.piece_manager.lock().unwrap();
                    pm.pick_rarest_piece()
                };
                match next {
                    Some(index) => {
                        let expected_len = {
                            let pm = self.piece_manager.lock().unwrap();
                            pm.piece_size(index).unwrap_or(self.piece_length) as usize
                        };
                        *active_piece = Some(ActivePiece {
                            index,
                            expected_len,
                            next_request_begin: 0,
                            blocks: BTreeMap::new(),
                        });
                    }
                    None => break, // No more pieces available
                }
            }

            let ap = active_piece.as_mut().unwrap();

            // All blocks of this piece have been requested — wait for responses
            if ap.next_request_begin as usize >= ap.expected_len {
                break;
            }

            let remaining = (ap.expected_len as u32).saturating_sub(ap.next_request_begin);
            let req_len = remaining.min(BLOCK_SIZE);
            let begin = ap.next_request_begin;
            let index = ap.index;

            tokio::time::timeout(
                WRITE_TIMEOUT,
                framed.send(PeerMessage::Request { index, begin, length: req_len }),
            )
            .await
            .map_err(|_| crate::error::BitTorrentError::Engine(
                format!("Request write timeout piece {}", index)
            ))??;

            pending.push_back(PendingRequest {
                index,
                begin,
                length: req_len,
                sent_at: Instant::now(),
            });
            ap.next_request_begin += req_len;
        }
        Ok(())
    }

    // ── Snapshot helpers ─────────────────────────────────────────────────────

    fn register_peer(&self, addr: SocketAddr) {
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

    fn update_peer<F>(&self, addr: SocketAddr, f: F)
    where
        F: FnOnce(&mut PeerSnapshot),
    {
        if let Ok(mut snap) = self.snapshot.write() {
            if let Some(t) = snap.active_torrents.get_mut(&self.info_hash) {
                if let Some(p) = t.peers.iter_mut().find(|p| p.addr == addr) {
                    f(p);
                }
            }
        }
    }

    fn sync_health(&self, addr: SocketAddr, h: &PeerHealth) {
        self.update_peer(addr, |p| {
            p.network_penalty = h.network_penalty;
            p.data_penalty = h.data_penalty;
            p.penalty_score = h.score();
            p.timeout_count = h.timeout_count;
            p.bad_data_count = h.bad_data_count;
            p.hash_fail_count = h.hash_fail_count;
        });
    }
}
