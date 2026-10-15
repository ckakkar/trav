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
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::sync::{Notify, mpsc};
use tokio_util::codec::Framed;
use tracing::debug;

use super::{Cmd, PeerCmd, Phase, Torrent};
use crate::bitfield::Bitfield;
use crate::error::{Error, Result, proto};
use crate::governor::RttSample;
use crate::peer::extension::{
    ExtHandshake, HolepunchMsg, METADATA_PIECE, MetadataMsg, PexMsg, UT_HOLEPUNCH, UT_METADATA,
    UT_PEX,
};
use crate::peer::handshake::Handshake;
use crate::peer::protocol::{MAX_BLOCK_LEN, PeerCodec, PeerMessage};
use crate::peer::stream::{PeerStream, RttProbe};
use crate::picker::{BLOCK_SIZE, BlockReq, PeerKey};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SNUB_AFTER: Duration = Duration::from_secs(60);
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const KEEPALIVE: Duration = Duration::from_secs(90);
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_UPLOAD_QUEUE: usize = 500;
/// Requests in flight before a peer's rate is known: enough for a few hundred
/// KB/s on a high-latency (or bloated) path, so new peers ramp up quickly.
const MIN_DEPTH: usize = 16;
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
    /// Both sides speak the Fast extension: refusals are said out loud.
    fast: bool,
    /// Pieces we told the peer it may have while choked (BEP 6 allowed fast).
    allowed: Mutex<Vec<u32>>,
}

/// Allowed-fast pieces given to each peer (libtorrent's default).
const ALLOWED_FAST: usize = 5;

/// BEP 6's canonical allowed-fast set for a peer: derived from its address
/// (the /24 for IPv4) and the info-hash, so every client agrees on it.
fn allowed_fast_set(ip: std::net::IpAddr, info_hash: &[u8; 20], pieces: u32, k: usize) -> Vec<u32> {
    use sha1::{Digest, Sha1};
    let std::net::IpAddr::V4(v4) = ip.to_canonical() else {
        return Vec::new();
    };
    let k = k.min(pieces as usize);
    let mut x: Vec<u8> = (u32::from(v4) & 0xffff_ff00).to_be_bytes().to_vec();
    x.extend_from_slice(info_hash);
    let mut set = Vec::with_capacity(k);
    while set.len() < k {
        x = Sha1::digest(&x).to_vec();
        for chunk in x.chunks(4) {
            if set.len() >= k {
                break;
            }
            let i = u32::from_be_bytes(chunk.try_into().expect("4 bytes")) % pieces;
            if !set.contains(&i) {
                set.push(i);
            }
        }
    }
    set
}

pub(super) async fn run(
    t: Arc<Torrent>,
    key: PeerKey,
    stream: PeerStream,
    hs: Handshake,
    addr: SocketAddr,
    rx: mpsc::UnboundedReceiver<PeerCmd>,
) {
    let mut conn = Conn {
        t: t.clone(),
        key,
        addr,
        rtt: stream.rtt_probe(),
        base_rtt: u32::MAX,
        out: None,
        fast: hs.supports_fast(),
        allowed_fast: Vec::new(),
        uploads: Arc::new(UploadQueue {
            fast: hs.supports_fast(),
            ..UploadQueue::default()
        }),
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
        last_idle_check: Instant::now(),
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
    /// Reads the connection's RTT estimate (the kernel's for TCP, ours for µTP).
    rtt: RttProbe,
    /// Lowest smoothed RTT seen on this connection: the path without queueing.
    base_rtt: u32,
    out: Option<mpsc::Sender<PeerMessage>>,
    /// Both sides speak the Fast extension (BEP 6).
    fast: bool,
    /// Pieces this peer lets us download while it chokes us.
    allowed_fast: Vec<u32>,
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
    /// Throttles the O(pieces) interest re-check when nothing is pickable.
    last_idle_check: Instant,
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

    async fn session(
        &mut self,
        stream: PeerStream,
        hs: Handshake,
        mut rx: mpsc::UnboundedReceiver<PeerCmd>,
    ) -> Result<()> {
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
        let uploader = tokio::spawn(uploader(
            self.t.clone(),
            self.key,
            self.uploads.clone(),
            out_tx,
        ));
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
            let holepunch = s.enable_utp && !private && self.t.ctx.utp.read().is_some();
            let ext = hs.supports_extensions().then(|| {
                ExtHandshake::ours(
                    st.meta.as_ref().map(|m| m.info.raw.len()),
                    self.t.ctx.public_port(),
                    self.addr.ip(),
                    s.enable_pex && !private,
                    holepunch,
                )
            });
            let have = st.picker.as_ref().map_or(0, |p| p.have().count_ones());
            let bf = match st.picker.as_ref() {
                Some(p) if self.fast && p.is_complete() => Some(PeerMessage::HaveAll),
                Some(p) if have > 0 => Some(PeerMessage::Bitfield {
                    payload: Bytes::copy_from_slice(p.have().as_bytes()),
                }),
                _ if self.fast => Some(PeerMessage::HaveNone),
                _ => None,
            };
            // Pieces it may take from us even while choked (only ones we have).
            if self.fast
                && let Some(p) = st.picker.as_ref()
                && have > 0
            {
                let set = allowed_fast_set(
                    self.addr.ip(),
                    &self.t.info_hash,
                    p.num_pieces() as u32,
                    ALLOWED_FAST,
                );
                *self.uploads.allowed.lock() = set
                    .into_iter()
                    .filter(|&i| p.have().get(i as usize))
                    .collect();
            }
            let dht = (hs.supports_dht() && s.enable_dht && !private)
                .then(|| self.t.ctx.dht().map(|d| d.port()))
                .flatten();
            (ext, bf, dht)
        };
        if let Some(payload) = ext {
            self.send(PeerMessage::Extended {
                extended_id: 0,
                payload,
            })
            .await?;
        }
        if let Some(m) = bitfield {
            self.send(m).await?;
        }
        let allowed = self.uploads.allowed.lock().clone();
        for piece_index in allowed {
            self.send(PeerMessage::AllowedFast { piece_index }).await?;
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
                self.send(PeerMessage::Choke).await?;
                // Requests for allowed-fast pieces stand; the rest are refused.
                let dropped: Vec<BlockReq> = {
                    let allowed = self.uploads.allowed.lock().clone();
                    let mut q = self.uploads.q.lock();
                    let (keep, drop): (VecDeque<_>, VecDeque<_>) =
                        q.drain(..).partition(|r| allowed.contains(&r.piece));
                    *q = keep;
                    drop.into()
                };
                if self.fast {
                    for r in dropped {
                        self.reject(r).await?;
                    }
                }
            }
            PeerCmd::Unchoke => self.send(PeerMessage::Unchoke).await?,
            PeerCmd::Cancel(r) => {
                if let Some(i) = self.pending.iter().position(|(p, _)| *p == r) {
                    self.pending.swap_remove(i);
                    self.send(PeerMessage::Cancel {
                        index: r.piece,
                        begin: r.begin,
                        length: r.len,
                    })
                    .await?;
                }
            }
            PeerCmd::Refresh => {
                self.update_interest().await?;
                self.request_metadata().await?;
            }
            PeerCmd::Ext(id, payload) => {
                self.send(PeerMessage::Extended {
                    extended_id: id,
                    payload,
                })
                .await?
            }
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
                if self.fast && !self.allowed_fast.is_empty() {
                    // Allowed-fast requests survive a choke (BEP 6); the rest
                    // go back to the pool rather than waiting for rejects.
                    let (keep, gone): (Vec<_>, Vec<_>) = self
                        .pending
                        .drain(..)
                        .partition(|(r, _)| self.allowed_fast.contains(&r.piece));
                    self.pending = keep;
                    if let Some(pk) = self.t.st.lock().picker.as_mut() {
                        for (r, _) in &gone {
                            pk.release(self.key, r);
                        }
                    }
                } else {
                    self.release_all();
                }
                self.with_peer(|p| p.peer_choking = true);
            }
            PeerMessage::HaveAll | PeerMessage::HaveNone => {
                if !self.fast {
                    return Err(proto("have-all/none without the fast extension"));
                }
                if self.got_bitfield {
                    return Err(proto("duplicate bitfield"));
                }
                self.got_bitfield = true;
                if matches!(msg, PeerMessage::HaveAll) {
                    let mut st = self.t.st.lock();
                    let n = st.meta.as_ref().map(|m| m.info.num_pieces());
                    let st = &mut *st;
                    if let Some(p) = st.peers.get_mut(&self.key) {
                        p.have_all = true;
                        if let Some(n) = n {
                            let bf = Bitfield::full(n);
                            if let Some(pk) = st.picker.as_mut() {
                                pk.remove_bitfield(&p.bitfield);
                                pk.add_bitfield(&bf);
                            }
                            p.bitfield = bf;
                        }
                    }
                }
                self.update_interest().await?;
            }
            PeerMessage::AllowedFast { piece_index } => {
                let n = self.t.st.lock().meta.as_ref().map(|m| m.info.num_pieces());
                if self.fast
                    && n.is_none_or(|n| (piece_index as usize) < n)
                    && self.allowed_fast.len() < 64
                    && !self.allowed_fast.contains(&piece_index)
                {
                    self.allowed_fast.push(piece_index);
                }
            }
            PeerMessage::Reject {
                index,
                begin,
                length,
            } => {
                let r = BlockReq {
                    piece: index,
                    begin,
                    len: length,
                };
                if let Some(i) = self.pending.iter().position(|(p, _)| *p == r) {
                    self.pending.swap_remove(i);
                    if let Some(pk) = self.t.st.lock().picker.as_mut() {
                        pk.release(self.key, &r);
                    }
                }
            }
            PeerMessage::Suggest { .. } => {}
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
                    let Some(p) = st.peers.get_mut(&self.key) else {
                        return Ok(());
                    };
                    let changed = if n.is_some() {
                        p.bitfield.set(piece_index as usize)
                    } else {
                        p.bitfield.set_grow(piece_index as usize)
                    };
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
                        Some(n) if payload.len() != n.div_ceil(8) => {
                            return Err(proto("bitfield length mismatch"));
                        }
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
            PeerMessage::Request {
                index,
                begin,
                length,
            } => self.on_request(index, begin, length).await?,
            PeerMessage::Cancel {
                index,
                begin,
                length,
            } => {
                let r = BlockReq {
                    piece: index,
                    begin,
                    len: length,
                };
                let was_queued = {
                    let mut q = self.uploads.q.lock();
                    let before = q.len();
                    q.retain(|q| *q != r);
                    q.len() != before
                };
                // With the Fast extension every request gets a piece or a reject.
                if was_queued && self.fast {
                    self.reject(r).await?;
                }
            }
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => self.on_piece(index, begin, block).await?,
            PeerMessage::Port { listen_port } => {
                if let Some(dht) = self.t.ctx.dht() {
                    let addr = SocketAddr::new(self.addr.ip(), listen_port);
                    tokio::spawn(async move { dht.ping(addr).await });
                }
            }
            PeerMessage::Extended {
                extended_id,
                payload,
            } => self.on_extended(extended_id, payload).await?,
        }
        Ok(())
    }

    fn with_peer(&self, f: impl FnOnce(&mut super::PeerState)) {
        if let Some(p) = self.t.st.lock().peers.get_mut(&self.key) {
            f(p);
        }
    }

    async fn reject(&self, r: BlockReq) -> Result<()> {
        self.send(PeerMessage::Reject {
            index: r.piece,
            begin: r.begin,
            length: r.len,
        })
        .await
    }

    async fn on_request(&mut self, index: u32, begin: u32, length: u32) -> Result<()> {
        let allowed = self.uploads.allowed.lock().contains(&index);
        let ok = {
            let st = self.t.st.lock();
            let choking = st.peers.get(&self.key).is_none_or(|p| p.am_choking) && !allowed;
            match st.picker.as_ref() {
                Some(pk) if !choking && st.phase == Phase::Active => {
                    length > 0
                        && length <= MAX_BLOCK_LEN
                        && (index as usize) < pk.num_pieces()
                        && pk.have().get(index as usize)
                        && begin
                            .checked_add(length)
                            .is_some_and(|e| e <= pk.piece_size(index))
                }
                _ => false,
            }
        };
        let r = BlockReq {
            piece: index,
            begin,
            len: length,
        };
        if ok {
            let mut q = self.uploads.q.lock();
            if q.len() < MAX_UPLOAD_QUEUE {
                q.push_back(r);
                drop(q);
                self.uploads.notify.notify_one();
                return Ok(());
            }
        }
        if self.fast {
            self.reject(r).await?;
        }
        Ok(())
    }

    async fn on_piece(&mut self, index: u32, begin: u32, block: Bytes) -> Result<()> {
        let len = block.len();
        self.t.ctx.down.acquire(len).await;
        if let Some(i) = self
            .pending
            .iter()
            .position(|(r, _)| r.piece == index && r.begin == begin)
        {
            self.pending.swap_remove(i);
        }
        self.last_piece = Instant::now();
        if self.snubbed {
            self.snubbed = false;
            self.with_peer(|p| p.snubbed = false);
        }
        self.t
            .ctx
            .session_down
            .fetch_add(len as u64, Ordering::Relaxed);

        let complete = {
            let mut st = self.t.st.lock();
            if let Some(p) = st.peers.get_mut(&self.key) {
                p.down.add(len as u64);
                p.last_exchange = Instant::now();
            }
            super::take_block(&mut st, self.key, index, begin, &block)
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
                    if need_meta
                        && st.metadata.is_none()
                        && let (Some(size), Some(_)) = (hs.metadata_size, self.ext_metadata)
                    {
                        st.metadata = Some(super::MetadataAssembly::new(size));
                    }
                    if let Some(p) = st.peers.get_mut(&self.key) {
                        p.ext_pex = hs.m.get("ut_pex").copied();
                        p.ext_holepunch = hs.m.get("ut_holepunch").copied();
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
                                    data: Bytes::copy_from_slice(
                                        &raw[start..(start + METADATA_PIECE).min(raw.len())],
                                    ),
                                })
                            }
                            None => None,
                        }
                    }
                    .unwrap_or(MetadataMsg::Reject { piece });
                    if let Some(their) = self.ext_metadata {
                        self.send(PeerMessage::Extended {
                            extended_id: their,
                            payload: reply.encode(),
                        })
                        .await?;
                    }
                }
                MetadataMsg::Data { piece, data, .. } => {
                    self.metadata_req = None;
                    let done = self
                        .t
                        .st
                        .lock()
                        .metadata
                        .as_mut()
                        .and_then(|m| m.on_data(piece, data));
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
                    let private = self
                        .t
                        .st
                        .lock()
                        .meta
                        .as_ref()
                        .is_some_and(|m| m.info.private);
                    if !private && !(pex.added.is_empty() && pex.dropped.is_empty()) {
                        self.t.send(Cmd::Pex {
                            from: self.key,
                            added: pex.added,
                            dropped: pex.dropped,
                        });
                    }
                }
            }
            UT_HOLEPUNCH => {
                if self.t.ctx.utp().is_none() {
                    return Ok(());
                }
                match HolepunchMsg::decode(&payload)? {
                    HolepunchMsg::Rendezvous(target) => self.t.relay_holepunch(self.key, target),
                    HolepunchMsg::Connect(addr) => self.t.send(Cmd::HolepunchConnect(addr)),
                    HolepunchMsg::Error(addr, code) => {
                        debug!(
                            "peer {}: can't introduce us to {addr} (error {code})",
                            self.addr
                        )
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn request_metadata(&mut self) -> Result<()> {
        let Some(their) = self.ext_metadata else {
            return Ok(());
        };
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
            self.send(PeerMessage::Extended {
                extended_id: their,
                payload: MetadataMsg::Request { piece }.encode(),
            })
            .await?;
        }
        Ok(())
    }

    // ── Download pipeline ────────────────────────────────────────────────────

    async fn set_interested(&mut self, on: bool) -> Result<()> {
        self.am_interested = on;
        self.with_peer(|p| p.am_interested = on);
        self.send(if on {
            PeerMessage::Interested
        } else {
            PeerMessage::NotInterested
        })
        .await
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
        let rate = self
            .t
            .st
            .lock()
            .peers
            .get(&self.key)
            .map(|p| p.down.rate())
            .unwrap_or(0);
        ((rate as usize * 3) / BLOCK_SIZE as usize)
            .clamp(MIN_DEPTH, MAX_DEPTH)
            .min(self.reqq.max(1))
    }

    async fn fill_requests(&mut self) -> Result<()> {
        if !self.am_interested {
            return Ok(());
        }
        if self.peer_choking {
            return self.fill_allowed_fast().await;
        }
        let depth = self.target_depth();
        // Refill once a quarter has come back: keeps the pipe full while still
        // batching picker scans.
        if self.pending.len() * 4 > depth * 3 {
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
            // Nothing this peer can give us right now; re-evaluate interest at most once a second.
            if self.last_idle_check.elapsed() >= Duration::from_secs(1) {
                self.last_idle_check = Instant::now();
                return self.update_interest().await;
            }
            return Ok(());
        }
        let now = Instant::now();
        for r in reqs {
            self.pending.push((r, now));
            self.send(PeerMessage::Request {
                index: r.piece,
                begin: r.begin,
                length: r.len,
            })
            .await?;
        }
        Ok(())
    }

    /// While choked: ask for blocks of the pieces it allows us anyway.
    async fn fill_allowed_fast(&mut self) -> Result<()> {
        if !self.fast || self.allowed_fast.is_empty() || self.pending.len() >= MIN_DEPTH / 2 {
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
                pk.pick_from(
                    self.key,
                    &p.bitfield,
                    &self.allowed_fast,
                    MIN_DEPTH - self.pending.len(),
                    &mut reqs,
                );
            }
        }
        for r in reqs {
            self.pending.push((r, Instant::now()));
            self.send(PeerMessage::Request {
                index: r.piece,
                begin: r.begin,
                length: r.len,
            })
            .await?;
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
            self.pending
                .retain(|(_, at)| now.duration_since(*at) <= REQUEST_TIMEOUT);
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
        if let Some((_, at)) = self.metadata_req
            && at.elapsed() > Duration::from_secs(8)
        {
            self.metadata_req = None;
            self.metadata_rejects += 1;
        }
        self.request_metadata().await?;

        // A seed connected to a seed has nothing to do.
        let st = self.t.st.lock();
        let we_done = st.picker.as_ref().is_some_and(|p| p.is_complete());
        let peer = st.peers.get(&self.key);
        let they_done = peer.is_some_and(|p| !p.bitfield.is_empty() && p.bitfield.all());
        let busy = peer.is_some_and(|p| p.up.rate() + p.down.rate() >= 4096);
        drop(st);
        self.sample_rtt(busy);
        Ok(we_done && they_done)
    }

    /// Feed the upload governor: how far this connection's RTT sits above
    /// the lowest seen on it (queueing delay), while it carries traffic.
    fn sample_rtt(&mut self, busy: bool) {
        let Some(rtt) = self.rtt.srtt_ms() else {
            return;
        };
        self.base_rtt = self.base_rtt.min(rtt);
        if busy {
            let mut samples = self.t.ctx.rtt_samples.lock();
            if samples.len() < 4096 {
                samples.push(RttSample {
                    srtt_ms: rtt,
                    base_ms: self.base_rtt,
                });
            }
        }
    }
}

async fn uploader(
    t: Arc<Torrent>,
    key: PeerKey,
    uploads: Arc<UploadQueue>,
    out: mpsc::Sender<PeerMessage>,
) {
    loop {
        let next = uploads.q.lock().pop_front();
        let Some(r) = next else {
            uploads.notify.notified().await;
            continue;
        };
        let target = {
            let st = t.st.lock();
            let choking = st.peers.get(&key).is_none_or(|p| p.am_choking)
                && !uploads.allowed.lock().contains(&r.piece);
            match (&st.storage, &st.meta) {
                (Some(s), Some(m)) if !choking => Some((
                    s.clone(),
                    m.info.piece_length as u64 * r.piece as u64 + r.begin as u64,
                )),
                _ => None,
            }
        };
        let Some((storage, offset)) = target else {
            if uploads.fast {
                let _ = out
                    .send(PeerMessage::Reject {
                        index: r.piece,
                        begin: r.begin,
                        length: r.len,
                    })
                    .await;
            }
            continue;
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
        if out
            .send(PeerMessage::Piece {
                index: r.piece,
                begin: r.begin,
                block: Bytes::from(data),
            })
            .await
            .is_err()
        {
            return;
        }
        t.ctx.session_up.fetch_add(n, Ordering::Relaxed);
        let mut st = t.st.lock();
        st.up.add(n);
        st.uploaded += n;
        st.dirty = true;
        if let Some(p) = st.peers.get_mut(&key) {
            p.up.add(n);
            p.last_exchange = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn allowed_fast_matches_bep6() {
        // The worked example from BEP 6: 80.4.4.200, info-hash of 0xaa, 1313 pieces.
        let set = super::allowed_fast_set("80.4.4.200".parse().unwrap(), &[0xaa; 20], 1313, 7);
        assert_eq!(set, [1059, 431, 808, 1217, 287, 376, 1188]);
        let nine = super::allowed_fast_set("80.4.4.200".parse().unwrap(), &[0xaa; 20], 1313, 9);
        assert_eq!(nine, [1059, 431, 808, 1217, 287, 376, 1188, 353, 508]);
        assert!(super::allowed_fast_set("::1".parse().unwrap(), &[0; 20], 10, 5).is_empty());
    }
}
