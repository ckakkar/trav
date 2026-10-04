//! One torrent: lifecycle state machine, connection manager, announcer,
//! choker and the shared state its peer tasks operate on.
//!
//! Concurrency model: a single `parking_lot::Mutex<State>` guards everything
//! peers touch. It is never held across an `.await` (the guard is `!Send`, so
//! the compiler enforces this inside spawned tasks). The torrent's own task
//! serialises lifecycle commands and runs a 1 Hz maintenance tick.

mod metadata;
mod peer;

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::Mutex;
use sha1::{Digest, Sha1};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::bitfield::Bitfield;
use crate::ctx::Ctx;
use crate::limiter::RateMeter;
use crate::magnet::Magnet;
use crate::message::Event;
use crate::metainfo::{Info, InfoHash, Metainfo};
use crate::peer::handshake::{self, Handshake};
use crate::picker::{BlockReq, PeerKey, Picker};
use crate::snapshot::{FileInfo, PeerInfo, TorrentDetails, TorrentStatus, TorrentSummary, TrackerInfo};
use crate::storage::Storage;
use crate::tracker::{self, AnnounceEvent, AnnounceRequest, AnnounceResponse};

use metadata::MetadataAssembly;

const MAX_CANDIDATES: usize = 3000;
const CHOKE_INTERVAL: Duration = Duration::from_secs(10);
const OPTIMISTIC_INTERVAL: Duration = Duration::from_secs(30);
const PEX_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerSource {
    Tracker,
    Dht,
    Pex,
    Incoming,
    Manual,
}

impl PeerSource {
    fn label(self) -> &'static str {
        match self {
            Self::Tracker => "tracker",
            Self::Dht => "dht",
            Self::Pex => "pex",
            Self::Incoming => "incoming",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Paused,
    Queued,
    Checking,
    Metadata,
    Active,
    Finished,
    Error(String),
}

impl Phase {
    fn is_active(&self) -> bool {
        matches!(self, Phase::Metadata | Phase::Active)
    }
}

pub(crate) enum PeerCmd {
    Have(u32),
    Choke,
    Unchoke,
    Cancel(BlockReq),
    /// Metadata/picker changed: recompute interest and requests.
    Refresh,
    Ext(u8, bytes::Bytes),
    Disconnect,
}

pub(crate) struct PeerState {
    addr: SocketAddr,
    tx: mpsc::UnboundedSender<PeerCmd>,
    bitfield: Bitfield,
    peer_id: [u8; 20],
    client: String,
    am_choking: bool,
    am_interested: bool,
    peer_choking: bool,
    peer_interested: bool,
    down: RateMeter,
    up: RateMeter,
    incoming: bool,
    source: PeerSource,
    connected_at: Instant,
    snubbed: bool,
    optimistic: bool,
    ext_pex: Option<u8>,
    listen_port: Option<u16>,
    pex_sent: HashSet<SocketAddr>,
    hash_fails: u8,
}

impl PeerState {
    /// Address other peers can dial (incoming peers advertise their listen port via BEP 10).
    fn dialable(&self) -> Option<SocketAddr> {
        if self.incoming {
            self.listen_port.map(|p| SocketAddr::new(self.addr.ip(), p))
        } else {
            Some(self.addr)
        }
    }
}

struct Candidate {
    source: PeerSource,
    fails: u32,
    next_attempt: Instant,
    connected: bool,
    connecting: bool,
    seed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrackerStatus {
    Idle,
    Announcing,
    Working,
    Error,
}

struct TrackerState {
    url: String,
    tier: usize,
    status: TrackerStatus,
    message: Option<String>,
    seeders: Option<u32>,
    leechers: Option<u32>,
    peers: usize,
    next: Instant,
    min_interval: Option<Duration>,
    fails: u32,
    started: bool,
    send_completed: bool,
    tracker_id: Option<String>,
}

impl TrackerState {
    fn new(url: String, tier: usize) -> Self {
        Self {
            url,
            tier,
            status: TrackerStatus::Idle,
            message: None,
            seeders: None,
            leechers: None,
            peers: 0,
            next: Instant::now(),
            min_interval: None,
            fails: 0,
            started: false,
            send_completed: false,
            tracker_id: None,
        }
    }
}

pub(crate) struct State {
    name: String,
    meta: Option<Arc<Metainfo>>,
    /// Trackers from a magnet link, used until metadata (which may add more) arrives.
    extra_trackers: Vec<Vec<String>>,
    save_path: PathBuf,
    storage: Option<Arc<Storage>>,
    picker: Option<Picker>,
    file_priorities: Vec<u8>,
    sequential: bool,
    phase: Phase,
    user_paused: bool,
    queued: bool,
    ratio_stopped: bool,
    peers: HashMap<PeerKey, PeerState>,
    candidates: HashMap<SocketAddr, Candidate>,
    connecting: usize,
    metadata: Option<MetadataAssembly>,
    trackers: Vec<TrackerState>,
    down: RateMeter,
    up: RateMeter,
    downloaded: u64,
    uploaded: u64,
    wasted: u64,
    hash_fails: u32,
    added_at: i64,
    completed_at: Option<i64>,
    check_progress: Arc<AtomicU32>,
    check_cancel: Arc<AtomicBool>,
    check_gen: u64,
    pending_have: Option<Bitfield>,
    next_key: PeerKey,
    banned: HashSet<IpAddr>,
    dht_next: Instant,
    dht_running: bool,
    last_choke: Instant,
    last_optimistic: Instant,
    last_pex: Instant,
    queue_pos: usize,
    dirty: bool,
}

pub(crate) enum Cmd {
    Pause,
    Resume,
    SetQueued(bool),
    Recheck,
    Reannounce,
    SetFilePriorities(Vec<u8>),
    SetSequential(bool),
    AddPeers(Vec<SocketAddr>, PeerSource),
    Shutdown { delete_files: bool, done: oneshot::Sender<()> },
    ConnectFailed(SocketAddr),
    Announced { idx: usize, res: Result<AnnounceResponse, String> },
    DhtDone(Vec<SocketAddr>),
    CheckDone { have: Bitfield, generation: u64 },
    PieceDone { piece: u32, ok: bool, contributors: Vec<PeerKey>, write_err: Option<String> },
    MetadataDone(Vec<u8>),
}

pub(crate) struct TorrentInit {
    pub meta: Option<Arc<Metainfo>>,
    pub magnet: Option<Magnet>,
    pub save_path: PathBuf,
    pub paused: bool,
    pub have: Option<Bitfield>,
    pub file_priorities: Vec<u8>,
    pub uploaded: u64,
    pub downloaded: u64,
    pub added_at: i64,
    pub completed_at: Option<i64>,
    pub sequential: bool,
    pub queue_pos: usize,
    pub trackers: Vec<Vec<String>>,
}

pub(crate) struct Torrent {
    pub info_hash: InfoHash,
    pub(crate) ctx: Arc<Ctx>,
    pub(crate) st: Mutex<State>,
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Torrent {
    pub fn spawn(ctx: Arc<Ctx>, info_hash: InfoHash, init: TorrentInit) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let name = init
            .meta
            .as_ref()
            .map(|m| m.info.name.clone())
            .or_else(|| init.magnet.as_ref().and_then(|m| m.display_name.clone()))
            .unwrap_or_else(|| hex::encode(info_hash));
        let mut extra = init.trackers;
        let mut peers = Vec::new();
        if let Some(m) = &init.magnet {
            extra.extend(m.trackers.iter().map(|t| vec![t.clone()]));
            peers = m.peers.clone();
        }
        let now = Instant::now();
        let st = State {
            name,
            meta: None,
            extra_trackers: crate::metainfo::normalize_tiers(extra),
            save_path: init.save_path,
            storage: None,
            picker: None,
            file_priorities: init.file_priorities,
            sequential: init.sequential,
            phase: Phase::Paused,
            user_paused: init.paused,
            queued: false,
            ratio_stopped: false,
            peers: HashMap::new(),
            candidates: HashMap::new(),
            connecting: 0,
            metadata: None,
            trackers: Vec::new(),
            down: RateMeter::default(),
            up: RateMeter::default(),
            downloaded: init.downloaded,
            uploaded: init.uploaded,
            wasted: 0,
            hash_fails: 0,
            added_at: init.added_at,
            completed_at: init.completed_at,
            check_progress: Arc::new(AtomicU32::new(0)),
            check_cancel: Arc::new(AtomicBool::new(false)),
            check_gen: 0,
            pending_have: init.have,
            next_key: 1,
            banned: HashSet::new(),
            dht_next: now,
            dht_running: false,
            last_choke: now,
            last_optimistic: now - OPTIMISTIC_INTERVAL,
            last_pex: now,
            queue_pos: init.queue_pos,
            dirty: true,
        };
        let t = Arc::new(Self { info_hash, ctx, st: Mutex::new(st), tx });
        {
            let mut st = t.st.lock();
            if let Some(meta) = init.meta {
                t.install_metadata(&mut st, meta, false);
            }
            t.rebuild_trackers(&mut st);
            for p in peers {
                add_candidate(&mut st, p, PeerSource::Manual);
            }
            t.update_phase(&mut st);
        }
        tokio::spawn(t.clone().run(rx));
        t
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    async fn run(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    None => break,
                    Some(Cmd::Shutdown { delete_files, done }) => {
                        self.shutdown(delete_files);
                        let _ = done.send(());
                        break;
                    }
                    Some(cmd) => self.handle(cmd),
                },
                _ = tick.tick() => self.on_tick(),
            }
        }
        debug!("torrent {} task exited", hex::encode(self.info_hash));
    }

    // ── Lifecycle ────────────────────────────────────────────────────────────

    fn handle(self: &Arc<Self>, cmd: Cmd) {
        let mut st = self.st.lock();
        match cmd {
            Cmd::Pause => {
                st.user_paused = true;
                st.dirty = true;
                self.update_phase(&mut st);
            }
            Cmd::Resume => {
                st.user_paused = false;
                st.ratio_stopped = false;
                if matches!(st.phase, Phase::Error(_)) {
                    st.phase = Phase::Paused;
                }
                st.dirty = true;
                self.update_phase(&mut st);
            }
            Cmd::SetQueued(q) => {
                if st.queued != q {
                    st.queued = q;
                    self.update_phase(&mut st);
                }
            }
            Cmd::Recheck => {
                if st.meta.is_some() {
                    st.pending_have = None;
                    st.picker = None;
                    self.enter_inactive(&mut st);
                    self.start_check(&mut st);
                }
            }
            Cmd::Reannounce => {
                let now = Instant::now();
                for t in st.trackers.iter_mut() {
                    t.next = now;
                }
                st.dht_next = now;
            }
            Cmd::SetFilePriorities(prio) => {
                if let Some(meta) = st.meta.clone() {
                    if prio.len() == meta.info.files.len() {
                        let was_complete = st.picker.as_ref().is_some_and(Picker::wanted_complete);
                        st.file_priorities = prio;
                        let piece_prio = piece_priorities(&meta.info, &st.file_priorities);
                        if let Some(p) = st.picker.as_mut() {
                            p.set_piece_priorities(piece_prio);
                        }
                        st.dirty = true;
                        if was_complete && !st.picker.as_ref().is_some_and(Picker::wanted_complete) {
                            st.ratio_stopped = false;
                            st.completed_at = None;
                            self.update_phase(&mut st);
                        }
                        broadcast(&st, || PeerCmd::Refresh);
                    }
                }
            }
            Cmd::SetSequential(on) => {
                st.sequential = on;
                if let Some(p) = st.picker.as_mut() {
                    p.set_sequential(on);
                }
                st.dirty = true;
            }
            Cmd::AddPeers(addrs, src) => {
                for a in addrs {
                    add_candidate(&mut st, a, src);
                }
            }
            Cmd::ConnectFailed(addr) => {
                st.connecting = st.connecting.saturating_sub(1);
                if let Some(c) = st.candidates.get_mut(&addr) {
                    c.connecting = false;
                    c.fails += 1;
                    c.next_attempt = Instant::now() + backoff(c.fails);
                }
            }
            Cmd::Announced { idx, res } => self.on_announced(&mut st, idx, res),
            Cmd::DhtDone(peers) => {
                st.dht_running = false;
                let n = peers.len();
                for p in peers {
                    add_candidate(&mut st, p, PeerSource::Dht);
                }
                let few = st.peers.len() < 10;
                st.dht_next = Instant::now() + if few { Duration::from_secs(120) } else { Duration::from_secs(900) };
                debug!("{}: DHT returned {n} peers", st.name);
            }
            Cmd::CheckDone { have, generation } => {
                if generation == st.check_gen && st.phase == Phase::Checking {
                    st.pending_have = Some(have);
                    let meta = st.meta.clone().expect("checking implies metadata");
                    self.install_picker(&mut st, &meta);
                    st.dirty = true;
                    if st.picker.as_ref().is_some_and(Picker::wanted_complete) && st.completed_at.is_none() {
                        st.completed_at = Some(now_unix());
                    }
                    st.phase = Phase::Paused;
                    self.update_phase(&mut st);
                }
            }
            Cmd::PieceDone { piece, ok, contributors, write_err } => {
                self.on_piece_done(&mut st, piece, ok, contributors, write_err)
            }
            Cmd::MetadataDone(raw) => self.on_metadata(&mut st, raw),
            Cmd::Shutdown { .. } => unreachable!("handled in run loop"),
        }
    }

    /// Recompute the phase from user intent and progress, connecting or disconnecting as needed.
    fn update_phase(self: &Arc<Self>, st: &mut State) {
        match st.phase {
            Phase::Error(_) => return,
            Phase::Checking if st.user_paused || st.queued => {
                // Abandon the check; it restarts on resume.
                st.check_cancel.store(true, Ordering::Relaxed);
                st.phase = Phase::Paused;
            }
            Phase::Checking => return,
            _ => {}
        }
        let was_active = st.phase.is_active();
        let target = if st.user_paused {
            Phase::Paused
        } else if st.ratio_stopped {
            Phase::Finished
        } else if st.queued && (st.picker.is_some() || st.meta.is_none()) {
            // Queue only gates downloading; verifying existing data proceeds regardless.
            Phase::Queued
        } else if st.meta.is_none() {
            Phase::Metadata
        } else if st.picker.is_none() {
            // Metadata present but never checked/installed.
            let meta = st.meta.clone().expect("checked above");
            let storage_has_files = st.storage.as_ref().is_some_and(|s| s.any_file_exists());
            match st.pending_have.as_ref() {
                Some(h) if h.count_ones() > 0 && !storage_has_files => {
                    // Resume data claims pieces but the files are gone.
                    warn!("{}: payload missing on disk, starting over", st.name);
                    st.pending_have = None;
                    self.install_picker(st, &meta);
                    if st.queued { Phase::Queued } else { Phase::Active }
                }
                Some(_) => {
                    self.install_picker(st, &meta);
                    if st.queued { Phase::Queued } else { Phase::Active }
                }
                None if storage_has_files => {
                    if was_active {
                        self.enter_inactive(st);
                    }
                    self.start_check(st);
                    return;
                }
                None => {
                    self.install_picker(st, &meta);
                    if st.queued { Phase::Queued } else { Phase::Active }
                }
            }
        } else {
            Phase::Active
        };

        if target == st.phase {
            return;
        }
        st.phase = target;
        if was_active && !st.phase.is_active() {
            self.enter_inactive(st);
        }
        if st.phase.is_active() {
            let now = Instant::now();
            for t in st.trackers.iter_mut() {
                if !t.started {
                    t.next = now;
                }
            }
            st.dht_next = now;
            broadcast(st, || PeerCmd::Refresh);
        }
        info!("{}: {:?}", st.name, st.phase);
    }

    /// Disconnect everyone, tell trackers we stopped, release file handles.
    fn enter_inactive(self: &Arc<Self>, st: &mut State) {
        broadcast(st, || PeerCmd::Disconnect);
        st.check_cancel.store(true, Ordering::Relaxed);
        if let Some(s) = &st.storage {
            let s = s.clone();
            tokio::task::spawn_blocking(move || {
                s.flush();
                s.close();
            });
        }
        let left = st.picker.as_ref().map(Picker::wanted_bytes_left).unwrap_or(0);
        for t in st.trackers.iter_mut().filter(|t| t.started) {
            t.started = false;
            t.status = TrackerStatus::Idle;
            let req = AnnounceRequest {
                info_hash: self.info_hash,
                peer_id: self.ctx.peer_id,
                port: self.ctx.listen_port.load(Ordering::Relaxed),
                uploaded: st.uploaded,
                downloaded: st.downloaded,
                left,
                event: AnnounceEvent::Stopped,
                num_want: 0,
                key: self.ctx.tracker_key,
                tracker_id: t.tracker_id.clone(),
            };
            let url = t.url.clone();
            let http = self.ctx.http.clone();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(5), tracker::announce(&url, &req, &http)).await;
            });
        }
    }

    fn start_check(self: &Arc<Self>, st: &mut State) {
        let (Some(meta), Some(storage)) = (st.meta.clone(), st.storage.clone()) else { return };
        st.phase = Phase::Checking;
        st.check_gen += 1;
        let generation = st.check_gen;
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        st.check_progress = progress.clone();
        st.check_cancel = cancel.clone();
        info!("{}: checking {} pieces", st.name, meta.info.num_pieces());
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let info = &meta.info;
            let n = info.num_pieces();
            let mut have = Bitfield::new(n);
            for i in 0..n {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let (start, end) = info.piece_range(i);
                if storage.verify(start, (end - start) as usize, &info.pieces[i]) {
                    have.set(i);
                }
                progress.store(((i + 1) * 10_000 / n) as u32, Ordering::Relaxed);
            }
            storage.close();
            me.send(Cmd::CheckDone { have, generation });
        });
    }

    fn shutdown(self: &Arc<Self>, delete_files: bool) {
        let mut st = self.st.lock();
        st.user_paused = true;
        if st.phase.is_active() {
            self.enter_inactive(&mut st);
        } else {
            broadcast(&st, || PeerCmd::Disconnect);
            st.check_cancel.store(true, Ordering::Relaxed);
        }
        st.phase = Phase::Paused;
        if let Some(s) = st.storage.clone() {
            if delete_files {
                s.delete_files();
            } else {
                s.flush();
            }
        }
    }

    // ── Metadata ─────────────────────────────────────────────────────────────

    fn on_metadata(self: &Arc<Self>, st: &mut State, raw: Vec<u8>) {
        if st.meta.is_some() {
            return;
        }
        let hash: InfoHash = Sha1::digest(&raw).into();
        if hash != self.info_hash {
            warn!("{}: metadata hash mismatch, retrying", st.name);
            st.metadata = None;
            return;
        }
        match Info::from_bytes(&raw) {
            Ok(info) => {
                let trackers = st.extra_trackers.clone();
                let meta = Arc::new(Metainfo::from_info(info, trackers));
                if let Err(e) = self.ctx.store.save_torrent(&self.info_hash, &meta.to_torrent_bytes()) {
                    warn!("could not persist metadata: {e}");
                }
                self.install_metadata(st, meta, true);
                self.ctx.emit(Event::MetadataReceived { info_hash: hex::encode(self.info_hash), name: st.name.clone() });
                self.update_phase(st);
            }
            Err(e) => {
                warn!("{}: bad metadata from swarm: {e}", st.name);
                st.metadata = None;
            }
        }
    }

    fn install_metadata(self: &Arc<Self>, st: &mut State, meta: Arc<Metainfo>, from_swarm: bool) {
        st.name = meta.info.name.clone();
        match Storage::new(&st.save_path, &meta.info) {
            Ok(s) => st.storage = Some(Arc::new(s)),
            Err(e) => st.phase = Phase::Error(format!("invalid file layout: {e}")),
        }
        if st.file_priorities.len() != meta.info.files.len() {
            st.file_priorities = vec![1; meta.info.files.len()];
        }
        let n = meta.info.num_pieces();
        for p in st.peers.values_mut() {
            p.bitfield.resize(n);
        }
        st.metadata = None;
        st.meta = Some(meta);
        st.dirty = true;
        if from_swarm {
            self.rebuild_trackers(st);
        }
    }

    fn install_picker(self: &Arc<Self>, st: &mut State, meta: &Arc<Metainfo>) {
        let n = meta.info.num_pieces();
        let have = st
            .pending_have
            .take()
            .filter(|h| h.len() == n)
            .unwrap_or_else(|| Bitfield::new(n));
        let mut picker = Picker::new(&meta.info, have);
        picker.set_piece_priorities(piece_priorities(&meta.info, &st.file_priorities));
        picker.set_sequential(st.sequential);
        for p in st.peers.values() {
            picker.add_bitfield(&p.bitfield);
        }
        st.picker = Some(picker);
    }

    fn rebuild_trackers(&self, st: &mut State) {
        let mut tiers: Vec<Vec<String>> = st.meta.as_ref().map(|m| m.trackers.clone()).unwrap_or_default();
        tiers.extend(st.extra_trackers.iter().cloned());
        let tiers = crate::metainfo::normalize_tiers(tiers);
        let mut fresh = Vec::new();
        for (tier, urls) in tiers.into_iter().enumerate() {
            for url in urls {
                let existing = st.trackers.iter().position(|t| t.url == url);
                fresh.push(match existing {
                    Some(i) => {
                        let mut t = std::mem::replace(&mut st.trackers[i], TrackerState::new(String::new(), 0));
                        t.tier = tier;
                        t
                    }
                    None => TrackerState::new(url, tier),
                });
            }
        }
        st.trackers = fresh;
    }

    // ── Peers ────────────────────────────────────────────────────────────────

    pub(crate) fn accepts_incoming(&self) -> bool {
        let st = self.st.lock();
        st.phase.is_active() && st.peers.len() < self.ctx.settings.read().max_peers_per_torrent
    }

    /// Adopt an established connection (either direction).
    pub(crate) fn attach(self: &Arc<Self>, stream: TcpStream, hs: Handshake, addr: SocketAddr, source: PeerSource) {
        let incoming = source == PeerSource::Incoming;
        let max = self.ctx.settings.read().max_peers_per_torrent;
        let mut st = self.st.lock();
        if !incoming {
            st.connecting = st.connecting.saturating_sub(1);
        }
        let reject = !st.phase.is_active()
            || st.peers.len() >= max
            || st.banned.contains(&addr.ip())
            || st.peers.values().any(|p| p.peer_id == hs.peer_id || (!incoming && p.addr == addr));
        if let Some(c) = st.candidates.get_mut(&addr) {
            c.connecting = false;
            c.connected = !reject;
            if reject {
                c.next_attempt = Instant::now() + Duration::from_secs(120);
            }
        }
        if reject {
            return;
        }
        if incoming {
            self.ctx.connectable.store(true, Ordering::Relaxed);
        }
        let key = st.next_key;
        st.next_key += 1;
        let (tx, rx) = mpsc::unbounded_channel();
        let n = st.meta.as_ref().map(|m| m.info.num_pieces());
        st.peers.insert(
            key,
            PeerState {
                addr,
                tx,
                bitfield: n.map(Bitfield::new).unwrap_or_default(),
                peer_id: hs.peer_id,
                client: crate::peer::client::identify(&hs.peer_id),
                am_choking: true,
                am_interested: false,
                peer_choking: true,
                peer_interested: false,
                down: RateMeter::default(),
                up: RateMeter::default(),
                incoming,
                source,
                connected_at: Instant::now(),
                snubbed: false,
                optimistic: false,
                ext_pex: None,
                listen_port: None,
                pex_sent: HashSet::new(),
                hash_fails: 0,
            },
        );
        self.ctx.connections.fetch_add(1, Ordering::Relaxed);
        drop(st);
        tokio::spawn(peer::run(self.clone(), key, stream, hs, addr, rx));
    }

    fn peer_gone(&self, key: PeerKey) {
        let mut st = self.st.lock();
        let Some(p) = st.peers.remove(&key) else { return };
        self.ctx.connections.fetch_sub(1, Ordering::Relaxed);
        if let Some(pk) = st.picker.as_mut() {
            pk.release_peer(key);
            pk.remove_bitfield(&p.bitfield);
        }
        if let Some(m) = st.metadata.as_mut() {
            m.release_peer(key);
        }
        let is_seed = p.bitfield.all() && !p.bitfield.is_empty();
        let short = p.connected_at.elapsed() < Duration::from_secs(30);
        if let Some(addr) = p.dialable() {
            let c = st.candidates.entry(addr).or_insert(Candidate {
                source: p.source,
                fails: 0,
                next_attempt: Instant::now(),
                connected: false,
                connecting: false,
                seed: false,
            });
            c.connected = false;
            c.seed = is_seed;
            if short {
                c.fails += 1;
            }
            c.next_attempt = Instant::now() + backoff(c.fails.max(1));
        }
    }

    fn connect_more(self: &Arc<Self>, st: &mut State) {
        let (per_torrent, global) = {
            let s = self.ctx.settings.read();
            (s.max_peers_per_torrent, s.max_peers_global)
        };
        let room = per_torrent.saturating_sub(st.peers.len() + st.connecting);
        let global_room = global.saturating_sub(self.ctx.connections.load(Ordering::Relaxed) + st.connecting);
        let n = room.min(global_room).min(10).min(
            self.ctx.half_open.available_permits().max(1),
        );
        if n == 0 {
            return;
        }
        let complete = st.picker.as_ref().is_some_and(Picker::wanted_complete);
        let now = Instant::now();
        let mut picks: Vec<(SocketAddr, u32, u8)> = st
            .candidates
            .iter()
            .filter(|(a, c)| {
                !c.connected && !c.connecting && c.next_attempt <= now && !(complete && c.seed) && !st.banned.contains(&a.ip())
            })
            .map(|(a, c)| {
                let rank = match c.source {
                    PeerSource::Manual => 0,
                    PeerSource::Tracker => 1,
                    PeerSource::Pex => 2,
                    PeerSource::Dht => 3,
                    PeerSource::Incoming => 4,
                };
                (*a, c.fails, rank)
            })
            .collect();
        picks.sort_by_key(|&(_, f, r)| (f, r));
        let ours = Handshake::ours(self.info_hash, self.ctx.peer_id);
        for (addr, _, _) in picks.into_iter().take(n) {
            if let Some(c) = st.candidates.get_mut(&addr) {
                c.connecting = true;
            }
            st.connecting += 1;
            let me = self.clone();
            let sem = self.ctx.half_open.clone();
            tokio::spawn(async move {
                let _permit = sem.acquire_owned().await;
                match handshake::connect(addr, &ours).await {
                    Ok((stream, hs)) => {
                        let src = me.st.lock().candidates.get(&addr).map(|c| c.source).unwrap_or(PeerSource::Tracker);
                        me.attach(stream, hs, addr, src);
                    }
                    Err(e) => {
                        debug!("connect {addr}: {e}");
                        me.send(Cmd::ConnectFailed(addr));
                    }
                }
            });
        }
    }

    // ── Pieces ───────────────────────────────────────────────────────────────

    /// Hash and persist a completed piece off the reactor, then report back.
    pub(crate) fn spawn_verify(self: &Arc<Self>, piece: u32, data: Vec<u8>, contributors: Vec<PeerKey>) {
        let (meta, storage) = {
            let st = self.st.lock();
            (st.meta.clone(), st.storage.clone())
        };
        let (Some(meta), Some(storage)) = (meta, storage) else { return };
        let me = self.clone();
        tokio::spawn(async move {
            let expected = meta.info.pieces[piece as usize];
            let offset = meta.info.piece_range(piece as usize).0;
            let res = tokio::task::spawn_blocking(move || {
                if Sha1::digest(&data).as_slice() != expected {
                    return (false, None);
                }
                match storage.write(offset, &data) {
                    Ok(()) => (true, None),
                    Err(e) => (true, Some(e.to_string())),
                }
            })
            .await;
            let (ok, write_err) = res.unwrap_or((false, Some("verification task failed".into())));
            me.send(Cmd::PieceDone { piece, ok, contributors, write_err });
        });
    }

    fn on_piece_done(
        self: &Arc<Self>,
        st: &mut State,
        piece: u32,
        ok: bool,
        contributors: Vec<PeerKey>,
        write_err: Option<String>,
    ) {
        let Some(picker) = st.picker.as_mut() else { return };
        if let Some(err) = write_err {
            picker.verified(piece, false);
            let msg = format!("disk write failed: {err}");
            warn!("{}: {msg}", st.name);
            self.ctx.emit(Event::TorrentError {
                info_hash: hex::encode(self.info_hash),
                name: st.name.clone(),
                message: msg.clone(),
            });
            self.enter_inactive(st);
            st.phase = Phase::Error(msg);
            return;
        }
        if !ok {
            picker.verified(piece, false);
            st.hash_fails += 1;
            st.wasted += picker.piece_size(piece) as u64;
            let sole = contributors.len() == 1;
            for k in contributors {
                if let Some(p) = st.peers.get_mut(&k) {
                    p.hash_fails += 1;
                    if sole || p.hash_fails >= 3 {
                        warn!("{}: banning {} after bad data", st.name, p.addr);
                        st.banned.insert(p.addr.ip());
                        let _ = p.tx.send(PeerCmd::Disconnect);
                    }
                }
            }
            return;
        }
        let was_done = picker.wanted_complete();
        if !picker.verified(piece, true) {
            return;
        }
        st.dirty = true;
        for p in st.peers.values() {
            if !p.bitfield.get(piece as usize) {
                let _ = p.tx.send(PeerCmd::Have(piece));
            }
        }
        let picker = st.picker.as_ref().expect("present");
        if !was_done && picker.wanted_complete() {
            self.on_complete(st);
        }
    }

    fn on_complete(self: &Arc<Self>, st: &mut State) {
        info!("{}: download complete", st.name);
        st.completed_at = Some(now_unix());
        st.dirty = true;
        if let Some(s) = st.storage.clone() {
            tokio::task::spawn_blocking(move || {
                s.create_empty_files();
                s.flush();
            });
        }
        let full = st.picker.as_ref().is_some_and(Picker::is_complete);
        if full {
            let now = Instant::now();
            for t in st.trackers.iter_mut().filter(|t| t.started) {
                t.send_completed = true;
                t.next = now;
            }
            // Seeds are useless to a seed.
            for p in st.peers.values() {
                if p.bitfield.all() {
                    let _ = p.tx.send(PeerCmd::Disconnect);
                }
            }
        }
        broadcast(st, || PeerCmd::Refresh);
        self.ctx.emit(Event::TorrentCompleted { info_hash: hex::encode(self.info_hash), name: st.name.clone() });
    }

    // ── Trackers & DHT ───────────────────────────────────────────────────────

    fn announce_due(self: &Arc<Self>, st: &mut State) {
        let now = Instant::now();
        let left = match st.picker.as_ref() {
            Some(p) => p.wanted_bytes_left(),
            None => st.meta.as_ref().map(|m| m.info.total_length).unwrap_or(1 << 30),
        };
        let port = self.ctx.listen_port.load(Ordering::Relaxed);
        let want_peers = st.peers.len() < self.ctx.settings.read().max_peers_per_torrent;
        for (idx, t) in st.trackers.iter_mut().enumerate() {
            if t.status == TrackerStatus::Announcing || t.next > now {
                continue;
            }
            let event = if !t.started {
                AnnounceEvent::Started
            } else if t.send_completed {
                AnnounceEvent::Completed
            } else {
                AnnounceEvent::None
            };
            let req = AnnounceRequest {
                info_hash: self.info_hash,
                peer_id: self.ctx.peer_id,
                port,
                uploaded: st.uploaded,
                downloaded: st.downloaded,
                left,
                event,
                num_want: if want_peers { 200 } else { 0 },
                key: self.ctx.tracker_key,
                tracker_id: t.tracker_id.clone(),
            };
            t.status = TrackerStatus::Announcing;
            t.started = true;
            t.send_completed = false;
            let url = t.url.clone();
            let http = self.ctx.http.clone();
            let me = self.clone();
            tokio::spawn(async move {
                let res = tracker::announce(&url, &req, &http).await.map_err(|e| match e {
                    crate::error::Error::Tracker(m) => m,
                    other => other.to_string(),
                });
                me.send(Cmd::Announced { idx, res });
            });
        }
    }

    fn on_announced(self: &Arc<Self>, st: &mut State, idx: usize, res: Result<AnnounceResponse, String>) {
        let few_peers = st.peers.len() < 15;
        let complete = st.picker.as_ref().is_some_and(Picker::wanted_complete);
        let active = st.phase.is_active();
        let Some(t) = st.trackers.get_mut(idx) else { return };
        let now = Instant::now();
        let mut found = Vec::new();
        match res {
            Ok(r) => {
                t.status = TrackerStatus::Working;
                t.message = r.warning;
                t.fails = 0;
                t.seeders = r.seeders;
                t.leechers = r.leechers;
                t.peers = r.peers.len();
                t.min_interval = r.min_interval;
                if r.tracker_id.is_some() {
                    t.tracker_id = r.tracker_id;
                }
                let mut wait = r.interval.clamp(Duration::from_secs(60), Duration::from_secs(3 * 3600));
                if few_peers && !complete {
                    let floor = t.min_interval.unwrap_or(Duration::from_secs(120)).max(Duration::from_secs(120));
                    wait = wait.min(floor.max(Duration::from_secs(300)));
                }
                t.next = now + wait;
                found = r.peers;
            }
            Err(e) => {
                t.status = TrackerStatus::Error;
                t.message = Some(e);
                t.fails += 1;
                t.started = t.fails < 2 && t.started;
                t.next = now + backoff(t.fails).max(Duration::from_secs(60));
            }
        }
        if !active {
            return;
        }
        for p in found {
            add_candidate(st, p, PeerSource::Tracker);
        }
    }

    fn dht_due(self: &Arc<Self>, st: &mut State) {
        let private = st.meta.as_ref().is_some_and(|m| m.info.private);
        if private || st.dht_running || Instant::now() < st.dht_next || !self.ctx.settings.read().enable_dht {
            return;
        }
        let Some(dht) = self.ctx.dht() else { return };
        st.dht_running = true;
        let port = self.ctx.listen_port.load(Ordering::Relaxed);
        let ih = self.info_hash;
        let me = self.clone();
        tokio::spawn(async move {
            let peers = dht.get_peers(ih, Some(port)).await;
            me.send(Cmd::DhtDone(peers));
        });
    }

    // ── Choking & PEX ────────────────────────────────────────────────────────

    fn rechoke(&self, st: &mut State) {
        let slots = self.ctx.settings.read().upload_slots;
        let seeding = st.picker.as_ref().is_some_and(Picker::wanted_complete);
        let now = Instant::now();
        let mut ranked: Vec<(PeerKey, u64)> = st
            .peers
            .iter()
            .filter(|(_, p)| p.peer_interested)
            .map(|(k, p)| (*k, if seeding { p.up.rate() } else { p.down.rate() }))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1));
        let regular: HashSet<PeerKey> = ranked.iter().take(slots.saturating_sub(1)).map(|r| r.0).collect();

        let mut optimistic = st.peers.iter().find(|(_, p)| p.optimistic).map(|(k, _)| *k);
        if now.duration_since(st.last_optimistic) >= OPTIMISTIC_INTERVAL || optimistic.is_none() {
            let pool: Vec<PeerKey> = ranked.iter().map(|r| r.0).filter(|k| !regular.contains(k)).collect();
            optimistic = (!pool.is_empty()).then(|| pool[rand::random::<usize>() % pool.len()]);
            st.last_optimistic = now;
        }
        for (k, p) in st.peers.iter_mut() {
            p.optimistic = Some(*k) == optimistic;
            let unchoke = regular.contains(k) || p.optimistic;
            if unchoke && p.am_choking {
                p.am_choking = false;
                let _ = p.tx.send(PeerCmd::Unchoke);
            } else if !unchoke && !p.am_choking {
                p.am_choking = true;
                let _ = p.tx.send(PeerCmd::Choke);
            }
        }
    }

    /// Called by a peer when it becomes interested: unchoke now if a slot is free.
    pub(crate) fn try_fast_unchoke(&self, st: &mut State, key: PeerKey) -> bool {
        let slots = self.ctx.settings.read().upload_slots;
        let unchoked = st.peers.values().filter(|p| !p.am_choking).count();
        match st.peers.get_mut(&key) {
            Some(p) if p.am_choking && unchoked < slots => {
                p.am_choking = false;
                true
            }
            _ => false,
        }
    }

    fn send_pex(&self, st: &mut State) {
        if !self.ctx.settings.read().enable_pex || st.meta.as_ref().is_some_and(|m| m.info.private) {
            return;
        }
        let current: Vec<(PeerKey, SocketAddr)> =
            st.peers.iter().filter_map(|(k, p)| p.dialable().map(|a| (*k, a))).collect();
        for (k, p) in st.peers.iter_mut() {
            let Some(id) = p.ext_pex else { continue };
            let now: HashSet<SocketAddr> = current.iter().filter(|(ck, _)| ck != k).map(|(_, a)| *a).collect();
            let added: Vec<SocketAddr> = now.difference(&p.pex_sent).copied().take(50).collect();
            let dropped: Vec<SocketAddr> = p.pex_sent.difference(&now).copied().take(50).collect();
            if added.is_empty() && dropped.is_empty() {
                continue;
            }
            for a in &added {
                p.pex_sent.insert(*a);
            }
            for a in &dropped {
                p.pex_sent.remove(a);
            }
            let msg = crate::peer::extension::PexMsg { added, dropped };
            let _ = p.tx.send(PeerCmd::Ext(id, msg.encode()));
        }
    }

    // ── Tick ─────────────────────────────────────────────────────────────────

    fn on_tick(self: &Arc<Self>) {
        let mut st = self.st.lock();
        st.down.tick(1.0);
        st.up.tick(1.0);
        for p in st.peers.values_mut() {
            p.down.tick(1.0);
            p.up.tick(1.0);
        }
        if !st.phase.is_active() {
            return;
        }
        self.connect_more(&mut st);
        self.announce_due(&mut st);
        self.dht_due(&mut st);

        let now = Instant::now();
        if st.phase == Phase::Active {
            if now.duration_since(st.last_choke) >= CHOKE_INTERVAL {
                st.last_choke = now;
                self.rechoke(&mut st);
            }
            if now.duration_since(st.last_pex) >= PEX_INTERVAL {
                st.last_pex = now;
                self.send_pex(&mut st);
            }
            let limit = self.ctx.settings.read().seed_ratio_limit;
            let size = st.picker.as_ref().map(Picker::wanted_bytes_total).unwrap_or(0);
            let done = st.picker.as_ref().is_some_and(Picker::wanted_complete);
            if done && limit > 0.0 && size > 0 && st.uploaded as f64 / size as f64 >= limit {
                info!("{}: ratio {limit} reached, stopping", st.name);
                st.ratio_stopped = true;
                st.dirty = true;
                self.update_phase(&mut st);
            }
        }
    }

    // ── Views ────────────────────────────────────────────────────────────────

    fn status_of(st: &State) -> TorrentStatus {
        match &st.phase {
            Phase::Paused => TorrentStatus::Paused,
            Phase::Queued => TorrentStatus::Queued,
            Phase::Checking => TorrentStatus::Checking,
            Phase::Metadata => TorrentStatus::Metadata,
            Phase::Finished => TorrentStatus::Finished,
            Phase::Error(_) => TorrentStatus::Error,
            Phase::Active => {
                if st.picker.as_ref().is_some_and(Picker::wanted_complete) {
                    TorrentStatus::Seeding
                } else {
                    TorrentStatus::Downloading
                }
            }
        }
    }

    pub(crate) fn is_download_candidate(&self) -> bool {
        let st = self.st.lock();
        !st.user_paused
            && !matches!(st.phase, Phase::Error(_) | Phase::Checking)
            && !st.picker.as_ref().is_some_and(Picker::wanted_complete)
    }

    pub(crate) fn queue_pos(&self) -> usize {
        self.st.lock().queue_pos
    }

    pub(crate) fn set_queue_pos(&self, pos: usize) {
        let mut st = self.st.lock();
        if st.queue_pos != pos {
            st.queue_pos = pos;
            st.dirty = true;
        }
    }

    pub(crate) fn summary(&self) -> TorrentSummary {
        let st = self.st.lock();
        summarize(&self.info_hash, &st)
    }

    pub(crate) fn details(&self) -> TorrentDetails {
        let st = self.st.lock();
        let summary = summarize(&self.info_hash, &st);
        let b64 = base64::engine::general_purpose::STANDARD;
        let meta = st.meta.as_ref();
        let (pieces, in_progress) = match (&st.picker, meta) {
            (Some(p), Some(m)) => {
                let mut prog = Bitfield::new(m.info.num_pieces());
                for i in p.in_progress() {
                    prog.set(i as usize);
                }
                (b64.encode(p.have().as_bytes()), b64.encode(prog.as_bytes()))
            }
            _ => (String::new(), String::new()),
        };
        let files = match meta {
            Some(m) => file_progress(&m.info, st.picker.as_ref().map(Picker::have), &st.file_priorities),
            None => Vec::new(),
        };
        let now = Instant::now();
        let peers = st
            .peers
            .values()
            .map(|p| {
                let mut flags = String::new();
                if p.am_interested {
                    flags.push(if p.peer_choking { 'd' } else { 'D' });
                } else if !p.peer_choking {
                    flags.push('K');
                }
                if p.peer_interested {
                    flags.push(if p.am_choking { 'u' } else { 'U' });
                } else if !p.am_choking {
                    flags.push('?');
                }
                if p.optimistic {
                    flags.push('O');
                }
                if p.snubbed {
                    flags.push('S');
                }
                match p.source {
                    PeerSource::Incoming => flags.push('I'),
                    PeerSource::Pex => flags.push('X'),
                    PeerSource::Dht => flags.push('H'),
                    _ => {}
                }
                PeerInfo {
                    addr: p.addr.to_string(),
                    client: p.client.clone(),
                    flags,
                    progress: if p.bitfield.is_empty() { 0.0 } else { p.bitfield.count_ones() as f64 / p.bitfield.len() as f64 },
                    download_rate: p.down.rate(),
                    upload_rate: p.up.rate(),
                    downloaded: p.down.total(),
                    uploaded: p.up.total(),
                    source: p.source.label().into(),
                }
            })
            .collect();
        let trackers = st
            .trackers
            .iter()
            .map(|t| TrackerInfo {
                url: t.url.clone(),
                tier: t.tier,
                status: match t.status {
                    TrackerStatus::Idle => "idle",
                    TrackerStatus::Announcing => "announcing",
                    TrackerStatus::Working => "working",
                    TrackerStatus::Error => "error",
                }
                .into(),
                message: t.message.clone(),
                peers: t.peers,
                seeds: t.seeders,
                leechers: t.leechers,
                next_announce: (st.phase.is_active() && t.status != TrackerStatus::Announcing)
                    .then(|| t.next.saturating_duration_since(now).as_secs()),
            })
            .collect();
        let magnet = Magnet {
            info_hash: self.info_hash,
            display_name: Some(st.name.clone()),
            trackers: st
                .meta
                .as_ref()
                .map(|m| m.all_trackers().cloned().collect())
                .unwrap_or_else(|| st.extra_trackers.iter().flatten().cloned().collect()),
            peers: vec![],
        }
        .to_uri();
        TorrentDetails {
            summary,
            comment: meta.and_then(|m| m.comment.clone()),
            created_by: meta.and_then(|m| m.created_by.clone()),
            creation_date: meta.and_then(|m| m.creation_date),
            piece_length: meta.map(|m| m.info.piece_length).unwrap_or(0),
            num_pieces: meta.map(|m| m.info.num_pieces()).unwrap_or(0),
            pieces,
            pieces_in_progress: in_progress,
            files,
            peers,
            trackers,
            magnet,
            content_path: st.storage.as_ref().map(|s| s.content_root().display().to_string()),
            wasted: st.wasted,
            hash_fails: st.hash_fails,
            dht_enabled: self.ctx.settings.read().enable_dht && !meta.is_some_and(|m| m.info.private),
        }
    }

    /// Resume data if anything changed since the last save.
    pub(crate) fn take_resume(&self, force: bool) -> Option<crate::persist::ResumeData> {
        let mut st = self.st.lock();
        if !st.dirty && !force {
            return None;
        }
        st.dirty = false;
        let b64 = base64::engine::general_purpose::STANDARD;
        let have = match (&st.picker, &st.pending_have) {
            (Some(p), _) => Some(b64.encode(p.have().as_bytes())),
            (None, Some(h)) => Some(b64.encode(h.as_bytes())),
            _ => None,
        };
        Some(crate::persist::ResumeData {
            name: st.name.clone(),
            magnet: st.meta.is_none().then(|| {
                Magnet {
                    info_hash: self.info_hash,
                    display_name: Some(st.name.clone()),
                    trackers: st.extra_trackers.iter().flatten().cloned().collect(),
                    peers: vec![],
                }
                .to_uri()
            }),
            save_path: st.save_path.clone(),
            paused: st.user_paused,
            have,
            file_priorities: st.file_priorities.clone(),
            uploaded: st.uploaded,
            downloaded: st.downloaded,
            added_at: st.added_at,
            completed_at: st.completed_at,
            sequential: st.sequential,
            queue_position: st.queue_pos,
            trackers: st.extra_trackers.clone(),
        })
    }
}

fn summarize(ih: &InfoHash, st: &State) -> TorrentSummary {
    let status = Torrent::status_of(st);
    let total_size = st.meta.as_ref().map(|m| m.info.total_length).unwrap_or(0);
    let (size, left) = match &st.picker {
        Some(p) => (p.wanted_bytes_total(), p.wanted_bytes_left()),
        None => (total_size, total_size),
    };
    let done_bytes = size - left;
    let progress = match status {
        TorrentStatus::Checking => st.check_progress.load(Ordering::Relaxed) as f64 / 10_000.0,
        TorrentStatus::Metadata => st.metadata.as_ref().map(MetadataAssembly::progress).unwrap_or(0.0),
        _ if size > 0 => {
            let partial = st.picker.as_ref().map(Picker::partial_bytes).unwrap_or(0);
            ((done_bytes + partial.min(left)) as f64 / size as f64).min(1.0)
        }
        _ => 0.0,
    };
    let rate = st.down.rate();
    let seeds = st.peers.values().filter(|p| !p.bitfield.is_empty() && p.bitfield.all()).count();
    TorrentSummary {
        info_hash: hex::encode(ih),
        name: st.name.clone(),
        status,
        progress,
        size,
        total_size,
        done_bytes,
        downloaded: st.downloaded,
        uploaded: st.uploaded,
        download_rate: rate,
        upload_rate: st.up.rate(),
        eta: (status == TorrentStatus::Downloading && rate > 0).then(|| left / rate),
        ratio: if done_bytes > 0 { st.uploaded as f64 / size.max(1) as f64 } else { 0.0 },
        peers: st.peers.len() - seeds,
        seeds,
        swarm_peers: st.trackers.iter().filter_map(|t| t.leechers).max(),
        swarm_seeds: st.trackers.iter().filter_map(|t| t.seeders).max(),
        added_at: st.added_at,
        completed_at: st.completed_at,
        save_path: st.save_path.display().to_string(),
        error: match &st.phase {
            Phase::Error(e) => Some(e.clone()),
            _ => None,
        },
        queue_position: st.queue_pos,
        has_metadata: st.meta.is_some(),
        sequential: st.sequential,
        private: st.meta.as_ref().is_some_and(|m| m.info.private),
        availability: st.picker.as_ref().map(Picker::distributed_copies).unwrap_or(0.0),
    }
}

fn broadcast(st: &State, mk: impl Fn() -> PeerCmd) {
    for p in st.peers.values() {
        let _ = p.tx.send(mk());
    }
}

fn add_candidate(st: &mut State, addr: SocketAddr, source: PeerSource) {
    if addr.port() == 0 || addr.ip().is_unspecified() || st.banned.contains(&addr.ip()) {
        return;
    }
    if st.candidates.len() >= MAX_CANDIDATES && !st.candidates.contains_key(&addr) {
        // Evict the worst failed candidate to make room.
        let worst = st
            .candidates
            .iter()
            .filter(|(_, c)| !c.connected && !c.connecting)
            .max_by_key(|(_, c)| c.fails)
            .map(|(a, _)| *a);
        match worst {
            Some(w) => {
                st.candidates.remove(&w);
            }
            None => return,
        }
    }
    st.candidates.entry(addr).or_insert(Candidate {
        source,
        fails: 0,
        next_attempt: Instant::now(),
        connected: false,
        connecting: false,
        seed: false,
    });
}

fn backoff(fails: u32) -> Duration {
    Duration::from_secs(30u64.saturating_mul(1 << fails.min(7)).min(3600))
}

/// Piece priority = max priority of the (non-pad) files it overlaps.
fn piece_priorities(info: &Info, file_prio: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; info.num_pieces()];
    let pl = info.piece_length as u64;
    for (f, &prio) in info.files.iter().zip(file_prio) {
        if f.pad || prio == 0 || f.length == 0 {
            continue;
        }
        let first = (f.offset / pl) as usize;
        let last = ((f.offset + f.length - 1) / pl) as usize;
        for p in &mut out[first..=last.min(info.num_pieces() - 1)] {
            *p = (*p).max(prio);
        }
    }
    out
}

fn file_progress(info: &Info, have: Option<&Bitfield>, prio: &[u8]) -> Vec<FileInfo> {
    let mut done = vec![0u64; info.files.len()];
    if let Some(have) = have {
        for piece in have.iter_ones() {
            let (s, e) = info.piece_range(piece);
            let first = info.files.partition_point(|f| f.offset + f.length <= s);
            for (i, f) in info.files.iter().enumerate().skip(first) {
                if f.offset >= e {
                    break;
                }
                let ov = e.min(f.offset + f.length).saturating_sub(s.max(f.offset));
                done[i] += ov;
            }
        }
    }
    info.files
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.pad)
        .map(|(i, f)| FileInfo {
            index: i,
            path: f.display_path(),
            size: f.length,
            done: done[i],
            progress: if f.length == 0 { 1.0 } else { done[i] as f64 / f.length as f64 },
            priority: prio.get(i).copied().unwrap_or(1),
        })
        .collect()
}
