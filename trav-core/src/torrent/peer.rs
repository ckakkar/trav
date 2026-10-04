//! One peer connection.
//!
//! Three cooperating tasks per peer:
//! * **reader** (this function's loop) — parses messages, drives requests;
//! * **writer** — owns the socket sink, batches frames, enforces a write timeout;
//! * **uploader** — serves block requests from disk under the upload limiter.
//!
//! Splitting them means a slow disk read or a throttled upload never stalls
//! protocol handling, and a stuck socket write cannot wedge the reader.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use tokio_util::codec::Framed;
use tracing::debug;

use super::{Cmd, PeerCmd, Phase, Torrent};
use crate::bitfield::Bitfield;
use crate::error::{proto, Error, Result};
use crate::peer::extension::{ExtHandshake, MetadataMsg, PexMsg, METADATA_PIECE, UT_METADATA, UT_PEX};
use crate::peer::handshake::Handshake;
use crate::peer::protocol::{PeerCodec, PeerMessage, MAX_BLOCK_LEN};
use crate::picker::{BlockReq, OnBlock, PeerKey, BLOCK_SIZE};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SNUB_AFTER: Duration = Duration::from_secs(60);
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const KEEPALIVE: Duration = Duration::from_secs(90);
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_UPLOAD_QUEUE: usize = 500;
const MIN_DEPTH: usize = 8;
const MAX_DEPTH: usize = 500;

struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Default)]
struct UploadQueue {
    q: Mutex<VecDeque<BlockReq>>,
    notify: Notify,
}

pub(super) async fn run(
    t: Arc<Torrent>,
    key: PeerKey,
    stream: TcpStream,
    hs: Handshake,
    addr: SocketAddr,
    rx: mpsc::UnboundedReceiver<PeerCmd>,
) {
    let mut conn = Conn {
        t: t.clone(),
        key,
        addr,
        out: None,
        uploads: Arc::new(UploadQueue::default()),
        pending: Vec::new(),
        am_interested: false,
        peer_choking: true,
        last_piece: Instant::now(),
        ext_metadata: None,
        metadata_req: None,
        metadata_rejects: 0,
        reqq: 250,
        got_bitfield: false,
        snubbed: false,
    };
    if let Err(e) = conn.session(stream, hs, rx).await {
        debug!("peer {addr} closed: {e}");
    }
    // Requests we held go back to the pool for others.
    t.peer_gone(key);
}

struct Conn {
    t: Arc<Torrent>,
    key: PeerKey,
    addr: SocketAddr,
    out: Option<mpsc::Sender<PeerMessage>>,
    uploads: Arc<UploadQueue>,
    pending: Vec<(BlockReq, Instant)>,
    am_interested: bool,
    peer_choking: bool,
    last_piece: Instant,
    ext_metadata: Option<u8>,
    metadata_req: Option<(u32, Instant)>,
    metadata_rejects: u8,
    reqq: usize,
    got_bitfield: bool,
    snubbed: bool,
}

impl Conn {
    async fn send(&self, m: PeerMessage) -> Result<()> {
        self.out
            .as_ref()
            .expect("writer running")
            .send(m)
            .await
            .map_err(|_| Error::Protocol("writer closed".into()))
    }

    async fn session(&mut self, stream: TcpStream, hs: Handshake, mut rx: mpsc::UnboundedReceiver<PeerCmd>) -> Result<()> {
        let (mut sink, mut source) = Framed::with_capacity(stream, PeerCodec, 64 * 1024).split();

        // Writer: drain the channel, flushing only when it runs dry (natural batching).
        let (out_tx, mut out_rx) = mpsc::channel::<PeerMessage>(512);
        let writer = tokio::spawn(async move {
            while let Some(m) = out_rx.recv().await {
                let mut batch = vec![m];
                while let Ok(m) = out_rx.try_recv() {
                    batch.push(m);
                    if batch.len() >= 64 {
                        break;
                    }
                }
                let write = async {
                    for m in batch {
                        sink.feed(m).await?;
                    }
                    sink.flush().await
                };
                match tokio::time::timeout(WRITE_TIMEOUT, write).await {
                    Ok(Ok(())) => {}
                    _ => break,
                }
            }
        });
        let writer_guard = AbortOnDrop(writer.abort_handle());
        self.out = Some(out_tx.clone());

        // Uploader: serve queued requests from disk.
        let uploader = tokio::spawn(uploader(self.t.clone(), self.key, self.uploads.clone(), out_tx));
        let _uploader_guard = AbortOnDrop(uploader.abort_handle());

        self.greet(&hs).await?;

        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_recv = Instant::now();
        let mut last_keepalive = Instant::now();

        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    None | Some(PeerCmd::Disconnect) => return Ok(()),
                    Some(cmd) => self.on_cmd(cmd).await?,
                },
                msg = source.next() => match msg {
                    None => return Err(proto("connection closed")),
                    Some(Err(e)) => return Err(e),
                    Some(Ok(m)) => {
                        last_recv = Instant::now();
                        self.on_message(m).await?;
                    }
                },
                _ = tick.tick() => {
                    if last_recv.elapsed() > IDLE_TIMEOUT {
                        return Err(Error::Timeout("peer idle"));
                    }
                    if writer_guard.0.is_finished() {
                        return Err(Error::Timeout("peer write"));
                    }
                    if last_keepalive.elapsed() > KEEPALIVE {
                        last_keepalive = Instant::now();
                        self.send(PeerMessage::KeepAlive).await?;
                    }
                    if self.on_tick().await? {
                        return Ok(());
                    }
                }
            }
            self.fill_requests().await?;
        }
    }

    async fn greet(&mut self, hs: &Handshake) -> Result<()> {
        let (ext, bitfield, dht_port) = {
            let st = self.t.st.lock();
            let s = self.t.ctx.settings.read();
            let private = st.meta.as_ref().is_some_and(|m| m.info.private);
            let ext = hs.supports_extensions().then(|| {
                ExtHandshake::ours(
                    st.meta.as_ref().map(|m| m.info.raw.len()),
                    self.t.ctx.listen_port.load(Ordering::Relaxed),
                    self.addr.ip(),
                    s.enable_pex && !private,
                )
            });
            let bf = st
                .picker
                .as_ref()
                .filter(|p| p.have().count_ones() > 0)
                .map(|p| Bytes::copy_from_slice(p.have().as_bytes()));
            let dht = (hs.supports_dht() && s.enable_dht && !private)
                .then(|| self.t.ctx.dht().map(|d| d.port()))
                .flatten();
            (ext, bf, dht)
        };
        if let Some(payload) = ext {
            self.send(PeerMessage::Extended { extended_id: 0, payload }).await?;
        }
        if let Some(payload) = bitfield {
            self.send(PeerMessage::Bitfield { payload }).await?;
        }
        if let Some(port) = dht_port {
            self.send(PeerMessage::Port { listen_port: port }).await?;
        }
        Ok(())
    }

    // ── Commands from the torrent ────────────────────────────────────────────

    async fn on_cmd(&mut self, cmd: PeerCmd) -> Result<()> {
        match cmd {
            PeerCmd::Have(i) => {
                self.send(PeerMessage::Have { piece_index: i }).await?;
                if self.am_interested {
                    self.update_interest().await?;
                }
            }
            PeerCmd::Choke => {
                self.uploads.q.lock().clear();
                self.send(PeerMessage::Choke).await?;
            }
            PeerCmd::Unchoke => self.send(PeerMessage::Unchoke).await?,
            PeerCmd::Cancel(r) => {
                if let Some(i) = self.pending.iter().position(|(p, _)| *p == r) {
                    self.pending.swap_remove(i);
                    self.send(PeerMessage::Cancel { index: r.piece, begin: r.begin, length: r.len }).await?;
                }
            }
            PeerCmd::Refresh => {
                self.update_interest().await?;
                self.request_metadata().await?;
            }
            PeerCmd::Ext(id, payload) => self.send(PeerMessage::Extended { extended_id: id, payload }).await?,
            PeerCmd::Disconnect => unreachable!("handled by caller"),
        }
        Ok(())
    }

    // ── Wire messages ────────────────────────────────────────────────────────

    async fn on_message(&mut self, msg: PeerMessage) -> Result<()> {
        match msg {
            PeerMessage::KeepAlive | PeerMessage::Unknown { .. } => {}
            PeerMessage::Choke => {
                self.peer_choking = true;
                self.release_all();
                self.with_peer(|p| p.peer_choking = true);
            }
            PeerMessage::Unchoke => {
                self.peer_choking = false;
                self.last_piece = Instant::now();
                self.with_peer(|p| p.peer_choking = false);
            }
            PeerMessage::Interested => {
                let unchoke = {
                    let mut st = self.t.st.lock();
                    if let Some(p) = st.peers.get_mut(&self.key) {
                        p.peer_interested = true;
                    }
                    st.phase == Phase::Active && self.t.try_fast_unchoke(&mut st, self.key)
                };
                if unchoke {
                    self.send(PeerMessage::Unchoke).await?;
                }
            }
            PeerMessage::NotInterested => self.with_peer(|p| p.peer_interested = false),
            PeerMessage::Have { piece_index } => {
                let interesting = {
                    let mut st = self.t.st.lock();
                    let n = st.meta.as_ref().map(|m| m.info.num_pieces());
                    if n.is_some_and(|n| piece_index as usize >= n) {
                        return Err(proto("have index out of range"));
                    }
                    let st = &mut *st;
                    let Some(p) = st.peers.get_mut(&self.key) else { return Ok(()) };
                    let changed = if n.is_some() { p.bitfield.set(piece_index as usize) } else { p.bitfield.set_grow(piece_index as usize) };
                    match st.picker.as_mut() {
                        Some(pk) if changed => {
                            pk.add_have(piece_index);
                            pk.is_wanted(piece_index)
                        }
                        _ => false,
                    }
                };
                if interesting && !self.am_interested {
                    self.set_interested(true).await?;
                }
            }
            PeerMessage::Bitfield { payload } => {
                if self.got_bitfield {
                    return Err(proto("duplicate bitfield"));
                }
                self.got_bitfield = true;
                {
                    let mut st = self.t.st.lock();
                    let n = st.meta.as_ref().map(|m| m.info.num_pieces());
                    let bf = match n {
                        Some(n) if payload.len() != n.div_ceil(8) => return Err(proto("bitfield length mismatch")),
                        Some(n) => Bitfield::from_bytes(&payload, n),
                        None => Bitfield::from_bytes_unsized(&payload),
                    };
                    let st = &mut *st;
                    if let Some(p) = st.peers.get_mut(&self.key) {
                        if let Some(pk) = st.picker.as_mut() {
                            pk.remove_bitfield(&p.bitfield);
                            pk.add_bitfield(&bf);
                        }
                        p.bitfield = bf;
                    }
                }
                self.update_interest().await?;
            }
            PeerMessage::Request { index, begin, length } => self.on_request(index, begin, length),
            PeerMessage::Cancel { index, begin, length } => {
                let r = BlockReq { piece: index, begin, len: length };
                self.uploads.q.lock().retain(|q| *q != r);
            }
            PeerMessage::Piece { index, begin, block } => self.on_piece(index, begin, block).await?,
            PeerMessage::Port { listen_port } => {
                if let Some(dht) = self.t.ctx.dht() {
                    let addr = SocketAddr::new(self.addr.ip(), listen_port);
                    tokio::spawn(async move { dht.ping(addr).await });
                }
            }
            PeerMessage::Extended { extended_id, payload } => self.on_extended(extended_id, payload).await?,
        }
        Ok(())
    }

    fn with_peer(&self, f: impl FnOnce(&mut super::PeerState)) {
        if let Some(p) = self.t.st.lock().peers.get_mut(&self.key) {
            f(p);
        }
    }

    fn on_request(&mut self, index: u32, begin: u32, length: u32) {
        let ok = {
            let st = self.t.st.lock();
            let choking = st.peers.get(&self.key).is_none_or(|p| p.am_choking);
            match st.picker.as_ref() {
                Some(pk) if !choking && st.phase == Phase::Active => {
                    length > 0
                        && length <= MAX_BLOCK_LEN
                        && (index as usize) < pk.num_pieces()
                        && pk.have().get(index as usize)
                        && begin.checked_add(length).is_some_and(|e| e <= pk.piece_size(index))
                }
                _ => false,
            }
        };
        if ok {
            let mut q = self.uploads.q.lock();
            if q.len() < MAX_UPLOAD_QUEUE {
                q.push_back(BlockReq { piece: index, begin, len: length });
                drop(q);
                self.uploads.notify.notify_one();
            }
        }
    }

    async fn on_piece(&mut self, index: u32, begin: u32, block: Bytes) -> Result<()> {
        let len = block.len();
        self.t.ctx.down.acquire(len).await;
        if let Some(i) = self.pending.iter().position(|(r, _)| r.piece == index && r.begin == begin) {
            self.pending.swap_remove(i);
        }
        self.last_piece = Instant::now();
        if self.snubbed {
            self.snubbed = false;
            self.with_peer(|p| p.snubbed = false);
        }
        self.t.ctx.session_down.fetch_add(len as u64, Ordering::Relaxed);

        let complete = {
            let mut st = self.t.st.lock();
            let st = &mut *st;
            st.down.add(len as u64);
            st.downloaded += len as u64;
            if let Some(p) = st.peers.get_mut(&self.key) {
                p.down.add(len as u64);
            }
            let outcome = match st.picker.as_mut() {
                Some(pk) => pk.on_block(self.key, index, begin, &block),
                None => OnBlock::Rejected,
            };
            let cancel_peers = |ks: &[PeerKey]| {
                let r = BlockReq { piece: index, begin, len: len as u32 };
                for k in ks {
                    if let Some(p) = st.peers.get(k) {
                        let _ = p.tx.send(PeerCmd::Cancel(r));
                    }
                }
            };
            match outcome {
                OnBlock::Accepted { cancel } => {
                    cancel_peers(&cancel);
                    None
                }
                OnBlock::Complete { piece, data, contributors, cancel } => {
                    cancel_peers(&cancel);
                    Some((piece, data, contributors))
                }
                OnBlock::Rejected | OnBlock::Duplicate => {
                    st.wasted += len as u64;
                    None
                }
            }
        };
        if let Some((piece, data, contributors)) = complete {
            self.t.spawn_verify(piece, data, contributors);
        }
        Ok(())
    }

    async fn on_extended(&mut self, id: u8, payload: Bytes) -> Result<()> {
        match id {
            0 => {
                let hs = ExtHandshake::decode(&payload)?;
                self.ext_metadata = hs.m.get("ut_metadata").copied();
                if let Some(q) = hs.reqq {
                    self.reqq = q as usize;
                }
                {
                    let mut st = self.t.st.lock();
                    let need_meta = st.meta.is_none();
                    if need_meta && st.metadata.is_none() {
                        if let (Some(size), Some(_)) = (hs.metadata_size, self.ext_metadata) {
                            st.metadata = Some(super::MetadataAssembly::new(size));
                        }
                    }
                    if let Some(p) = st.peers.get_mut(&self.key) {
                        p.ext_pex = hs.m.get("ut_pex").copied();
                        p.listen_port = hs.listen_port;
                        if let Some(v) = hs.client.filter(|v| !v.trim().is_empty()) {
                            p.client = v.chars().take(40).collect();
                        }
                    }
                }
                self.request_metadata().await?;
            }
            UT_METADATA => match MetadataMsg::decode(&payload)? {
                MetadataMsg::Request { piece } => {
                    let reply = {
                        let st = self.t.st.lock();
                        match st.meta.as_ref() {
                            Some(m) => {
                                let raw = &m.info.raw;
                                let start = piece as usize * METADATA_PIECE;
                                (start < raw.len()).then(|| MetadataMsg::Data {
                                    piece,
                                    total_size: raw.len(),
                                    data: Bytes::copy_from_slice(&raw[start..(start + METADATA_PIECE).min(raw.len())]),
                                })
                            }
                            None => None,
                        }
                    }
                    .unwrap_or(MetadataMsg::Reject { piece });
                    if let Some(their) = self.ext_metadata {
                        self.send(PeerMessage::Extended { extended_id: their, payload: reply.encode() }).await?;
                    }
                }
                MetadataMsg::Data { piece, data, .. } => {
                    self.metadata_req = None;
                    let done = self.t.st.lock().metadata.as_mut().and_then(|m| m.on_data(piece, data));
                    match done {
                        Some(raw) => self.t.send(Cmd::MetadataDone(raw)),
                        None => self.request_metadata().await?,
                    }
                }
                MetadataMsg::Reject { piece } => {
                    self.metadata_req = None;
                    self.metadata_rejects += 1;
                    if let Some(m) = self.t.st.lock().metadata.as_mut() {
                        m.reject(piece);
                    }
                }
            },
            UT_PEX => {
                let pex = PexMsg::decode(&payload)?;
                let allowed = self.t.ctx.settings.read().enable_pex;
                if allowed {
                    let private = self.t.st.lock().meta.as_ref().is_some_and(|m| m.info.private);
                    if !private && !pex.added.is_empty() {
                        self.t.send(Cmd::AddPeers(pex.added, super::PeerSource::Pex));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn request_metadata(&mut self) -> Result<()> {
        let Some(their) = self.ext_metadata else { return Ok(()) };
        if self.metadata_req.is_some() || self.metadata_rejects >= 3 {
            return Ok(());
        }
        let piece = {
            let mut st = self.t.st.lock();
            if st.meta.is_some() {
                return Ok(());
            }
            st.metadata.as_mut().and_then(|m| m.next_request(self.key))
        };
        if let Some(piece) = piece {
            self.metadata_req = Some((piece, Instant::now()));
            self.send(PeerMessage::Extended { extended_id: their, payload: MetadataMsg::Request { piece }.encode() })
                .await?;
        }
        Ok(())
    }

    // ── Download pipeline ────────────────────────────────────────────────────

    async fn set_interested(&mut self, on: bool) -> Result<()> {
        self.am_interested = on;
        self.with_peer(|p| p.am_interested = on);
        self.send(if on { PeerMessage::Interested } else { PeerMessage::NotInterested }).await
    }

    async fn update_interest(&mut self) -> Result<()> {
        let interesting = {
            let st = self.t.st.lock();
            match (&st.picker, st.peers.get(&self.key)) {
                (Some(pk), Some(p)) if st.phase == Phase::Active => pk.is_interesting(&p.bitfield),
                _ => false,
            }
        };
        if interesting != self.am_interested {
            self.set_interested(interesting).await?;
        }
        Ok(())
    }

    fn release_all(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        self.pending.clear();
        if let Some(pk) = self.t.st.lock().picker.as_mut() {
            pk.release_peer(self.key);
        }
    }

    /// Requests to keep in flight: ~3 s worth of this peer's rate, bounded by its `reqq`.
    fn target_depth(&self) -> usize {
        if self.snubbed {
            return 1;
        }
        let rate = self.t.st.lock().peers.get(&self.key).map(|p| p.down.rate()).unwrap_or(0);
        ((rate as usize * 3) / BLOCK_SIZE as usize).clamp(MIN_DEPTH, MAX_DEPTH).min(self.reqq.max(1))
    }

    async fn fill_requests(&mut self) -> Result<()> {
        if self.peer_choking || !self.am_interested {
            return Ok(());
        }
        let depth = self.target_depth();
        // Refill in batches to amortise picker scans.
        if self.pending.len() > depth / 2 {
            return Ok(());
        }
        let mut reqs = Vec::new();
        {
            let mut st = self.t.st.lock();
            let st = &mut *st;
            if st.phase != Phase::Active {
                return Ok(());
            }
            if let (Some(pk), Some(p)) = (st.picker.as_mut(), st.peers.get(&self.key)) {
                pk.pick(self.key, &p.bitfield, depth - self.pending.len(), &mut reqs);
            }
        }
        if reqs.is_empty() && self.pending.is_empty() {
            // Nothing this peer can give us right now.
            return self.update_interest().await;
        }
        let now = Instant::now();
        for r in reqs {
            self.pending.push((r, now));
            self.send(PeerMessage::Request { index: r.piece, begin: r.begin, length: r.len }).await?;
        }
        Ok(())
    }

    /// Returns true when the connection should close gracefully.
    async fn on_tick(&mut self) -> Result<bool> {
        // Expire stalled requests so other peers can pick those blocks up.
        let now = Instant::now();
        let expired: Vec<BlockReq> = self
            .pending
            .iter()
            .filter(|(_, at)| now.duration_since(*at) > REQUEST_TIMEOUT)
            .map(|(r, _)| *r)
            .collect();
        if !expired.is_empty() {
            self.pending.retain(|(_, at)| now.duration_since(*at) <= REQUEST_TIMEOUT);
            if let Some(pk) = self.t.st.lock().picker.as_mut() {
                for r in &expired {
                    pk.release(self.key, r);
                }
            }
        }
        if !self.pending.is_empty() && self.last_piece.elapsed() > SNUB_AFTER && !self.snubbed {
            self.snubbed = true;
            self.with_peer(|p| p.snubbed = true);
            self.release_all();
        }
        if let Some((_, at)) = self.metadata_req {
            if at.elapsed() > Duration::from_secs(20) {
                self.metadata_req = None;
                self.metadata_rejects += 1;
            }
        }
        self.request_metadata().await?;

        // A seed connected to a seed has nothing to do.
        let st = self.t.st.lock();
        let we_done = st.picker.as_ref().is_some_and(|p| p.is_complete());
        let they_done = st.peers.get(&self.key).is_some_and(|p| !p.bitfield.is_empty() && p.bitfield.all());
        Ok(we_done && they_done)
    }
}

async fn uploader(t: Arc<Torrent>, key: PeerKey, uploads: Arc<UploadQueue>, out: mpsc::Sender<PeerMessage>) {
    loop {
        let next = uploads.q.lock().pop_front();
        let Some(r) = next else {
            uploads.notify.notified().await;
            continue;
        };
        let (storage, offset) = {
            let st = t.st.lock();
            let choking = st.peers.get(&key).is_none_or(|p| p.am_choking);
            match (&st.storage, &st.meta) {
                (Some(s), Some(m)) if !choking => {
                    (s.clone(), m.info.piece_length as u64 * r.piece as u64 + r.begin as u64)
                }
                _ => continue,
            }
        };
        let data = match storage.read_async(offset, r.len as usize).await {
            Ok(d) => d,
            Err(e) => {
                debug!("upload read failed: {e}");
                continue;
            }
        };
        t.ctx.up.acquire(data.len()).await;
        let n = data.len() as u64;
        if out.send(PeerMessage::Piece { index: r.piece, begin: r.begin, block: Bytes::from(data) }).await.is_err() {
            return;
        }
        t.ctx.session_up.fetch_add(n, Ordering::Relaxed);
        let mut st = t.st.lock();
        st.up.add(n);
        st.uploaded += n;
        st.dirty = true;
        if let Some(p) = st.peers.get_mut(&key) {
            p.up.add(n);
        }
    }
}
