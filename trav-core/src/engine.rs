//! The session: owns all torrents, the TCP listener, DHT, persistence and the
//! published snapshot. UIs talk to it through the cloneable [`EngineHandle`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use parking_lot::RwLock;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot, watch, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::bitfield::Bitfield;
use crate::ctx::{generate_peer_id, Ctx, HALF_OPEN_LIMIT};
use crate::dht::Dht;
use crate::error::{Error, Result};
use crate::limiter::RateLimiter;
use crate::magnet::Magnet;
use crate::message::Event;
use crate::metainfo::{InfoHash, Metainfo};
use crate::peer::handshake::{self, Handshake};
use crate::persist::Store;
use crate::settings::Settings;
use crate::snapshot::{EngineSnapshot, GlobalStats, PreviewFile, TorrentDetails, TorrentPreview};
use crate::torrent::{now_unix, Cmd, PeerSource, Torrent, TorrentInit};

const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(500);
const SAVE_INTERVAL: Duration = Duration::from_secs(20);

/// Where a torrent comes from.
#[derive(Debug, Clone)]
pub enum TorrentSource {
    /// Raw `.torrent` file contents.
    Bytes(Vec<u8>),
    File(PathBuf),
    /// `magnet:` URI.
    Magnet(String),
}

impl TorrentSource {
    /// Guess from user input: magnet link, or path to a `.torrent`.
    pub fn from_input(s: &str) -> Self {
        let t = s.trim();
        if t.starts_with("magnet:") {
            Self::Magnet(t.to_string())
        } else if t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            Self::Magnet(format!("magnet:?xt=urn:btih:{t}"))
        } else {
            Self::File(PathBuf::from(t))
        }
    }
}

#[derive(Debug, Clone)]
pub struct AddTorrent {
    pub source: TorrentSource,
    pub save_path: Option<PathBuf>,
    pub paused: bool,
    pub sequential: bool,
    /// Per-file priorities (0 skip, 1 normal, 2 high); must match file count.
    pub file_priorities: Option<Vec<u8>>,
}

impl AddTorrent {
    pub fn new(source: TorrentSource) -> Self {
        Self { source, save_path: None, paused: false, sequential: false, file_priorities: None }
    }
}

struct Session {
    ctx: Arc<Ctx>,
    torrents: RwLock<HashMap<InfoHash, Arc<Torrent>>>,
    snapshot_tx: watch::Sender<Arc<EngineSnapshot>>,
    listener: parking_lot::Mutex<Option<JoinHandle<()>>>,
    running: AtomicBool,
    seq: AtomicU64,
}

/// Cloneable handle to a running engine.
#[derive(Clone)]
pub struct EngineHandle {
    s: Arc<Session>,
    snapshot_rx: watch::Receiver<Arc<EngineSnapshot>>,
}

/// Entry point. `Engine::start` boots a session from a state directory.
pub struct Engine;

impl Engine {
    pub async fn start(state_dir: impl Into<PathBuf>) -> Result<EngineHandle> {
        Self::start_with(state_dir, None).await
    }

    /// Start with explicit settings overriding whatever was persisted (tests, CLI flags).
    pub async fn start_with(state_dir: impl Into<PathBuf>, overrides: Option<Settings>) -> Result<EngineHandle> {
        let store = Store::new(state_dir.into())?;
        let settings = overrides
            .or_else(|| store.load::<Settings>("settings.json"))
            .unwrap_or_default()
            .sanitized();
        let _ = store.save("settings.json", &settings);
        info!("Trav engine starting; state in {}", store.root().display());

        let (events, _) = broadcast::channel(256);
        let ctx = Arc::new(Ctx {
            peer_id: generate_peer_id(),
            listen_port: AtomicU16::new(0),
            down: RateLimiter::new(settings.download_limit),
            up: RateLimiter::new(settings.upload_limit),
            settings: RwLock::new(settings),
            dht: RwLock::new(None),
            http: crate::tracker::http_client(),
            events,
            half_open: Arc::new(Semaphore::new(HALF_OPEN_LIMIT)),
            connections: AtomicUsize::new(0),
            store,
            tracker_key: rand::random(),
            connectable: AtomicBool::new(false),
            session_down: AtomicU64::new(0),
            session_up: AtomicU64::new(0),
            upnp_status: RwLock::new(None),
        });

        let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(EngineSnapshot::empty()));
        let s = Arc::new(Session {
            ctx,
            torrents: RwLock::new(HashMap::new()),
            snapshot_tx,
            listener: parking_lot::Mutex::new(None),
            running: AtomicBool::new(true),
            seq: AtomicU64::new(0),
        });

        s.bind_listener().await;
        s.start_dht().await;
        crate::upnp::spawn(Arc::downgrade(&s.ctx));
        s.load_persisted().await;
        s.apply_queue();
        s.publish();

        let weak = Arc::downgrade(&s);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SNAPSHOT_INTERVAL);
            let mut n = 0u64;
            loop {
                tick.tick().await;
                let Some(s) = weak.upgrade() else { break };
                if !s.running.load(Ordering::Relaxed) {
                    break;
                }
                n += 1;
                s.apply_queue();
                s.publish();
                if n % (SAVE_INTERVAL.as_millis() / SNAPSHOT_INTERVAL.as_millis()) as u64 == 0 {
                    s.save_all(false);
                }
            }
        });

        Ok(EngineHandle { s, snapshot_rx })
    }

    /// Parse a `.torrent` without adding it (for the "add torrent" dialog).
    pub fn inspect(bytes: &[u8]) -> Result<TorrentPreview> {
        let m = Metainfo::from_bytes(bytes)?;
        Ok(TorrentPreview {
            info_hash: hex::encode(m.info_hash),
            name: m.info.name.clone(),
            total_size: m.info.total_length,
            piece_length: m.info.piece_length,
            num_pieces: m.info.num_pieces(),
            files: m
                .info
                .files
                .iter()
                .filter(|f| !f.pad)
                .map(|f| PreviewFile { path: f.display_path(), size: f.length })
                .collect(),
            trackers: m.all_trackers().cloned().collect(),
            comment: m.comment.clone(),
            created_by: m.created_by.clone(),
            private: m.info.private,
            already_added: false,
        })
    }
}

impl Session {
    async fn bind_listener(self: &Arc<Self>) {
        if let Some(h) = self.listener.lock().take() {
            h.abort();
        }
        let want = self.ctx.settings.read().listen_port;
        let mut bound = None;
        for port in std::iter::once(want).chain((1..=10).map(|i| want.wrapping_add(i))).chain(std::iter::once(0)) {
            if let Ok(l) = TcpListener::bind(("0.0.0.0", port)).await {
                bound = Some(l);
                break;
            }
        }
        let Some(listener) = bound else {
            warn!("could not bind any listen port; incoming connections disabled");
            return;
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        self.ctx.listen_port.store(port, Ordering::Relaxed);
        info!("listening for peers on TCP {port}");

        let weak = Arc::downgrade(self);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, addr)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                let Some(s) = weak.upgrade() else { break };
                tokio::spawn(async move { s.accept(stream, addr).await });
            }
        });
        *self.listener.lock() = Some(handle);
    }

    async fn accept(self: Arc<Self>, mut stream: tokio::net::TcpStream, addr: SocketAddr) {
        let global = self.ctx.settings.read().max_peers_global;
        if self.ctx.connections.load(Ordering::Relaxed) >= global {
            return;
        }
        stream.set_nodelay(true).ok();
        let theirs = match handshake::read_handshake(&mut stream).await {
            Ok(h) => h,
            Err(e) => {
                debug!("incoming {addr}: {e}");
                return;
            }
        };
        let torrent = self.torrents.read().get(&theirs.info_hash).cloned();
        let Some(t) = torrent.filter(|t| t.accepts_incoming()) else { return };
        if theirs.peer_id == self.ctx.peer_id {
            return;
        }
        let ours = Handshake::ours(theirs.info_hash, self.ctx.peer_id);
        if handshake::write_handshake(&mut stream, &ours).await.is_ok() {
            t.attach(stream, theirs, addr, PeerSource::Incoming);
        }
    }

    async fn start_dht(self: &Arc<Self>) {
        let enabled = self.ctx.settings.read().enable_dht;
        if !enabled {
            *self.ctx.dht.write() = None;
            return;
        }
        let port = self.ctx.listen_port.load(Ordering::Relaxed);
        let known: Vec<SocketAddr> = self.ctx.store.load("dht.json").unwrap_or_default();
        match Dht::start(port, known).await {
            Ok(d) => *self.ctx.dht.write() = Some(d),
            Err(e) => warn!("DHT failed to start: {e}"),
        }
    }

    async fn load_persisted(self: &Arc<Self>) {
        let b64 = base64::engine::general_purpose::STANDARD;
        for (ih, r) in self.ctx.store.load_all() {
            let meta = self
                .ctx
                .store
                .load_torrent(&ih)
                .and_then(|b| Metainfo::from_bytes(&b).ok())
                .filter(|m| m.info_hash == ih)
                .map(Arc::new);
            let magnet = r.magnet.as_deref().and_then(|m| Magnet::parse(m).ok());
            if meta.is_none() && magnet.is_none() {
                warn!("dropping unrecoverable torrent {}", hex::encode(ih));
                continue;
            }
            let have = match (&meta, &r.have) {
                (Some(m), Some(h)) => b64.decode(h).ok().map(|b| Bitfield::from_bytes(&b, m.info.num_pieces())),
                _ => None,
            };
            let t = Torrent::spawn(
                self.ctx.clone(),
                ih,
                TorrentInit {
                    meta,
                    magnet,
                    save_path: r.save_path,
                    paused: r.paused,
                    have,
                    file_priorities: r.file_priorities,
                    uploaded: r.uploaded,
                    downloaded: r.downloaded,
                    added_at: r.added_at,
                    completed_at: r.completed_at,
                    sequential: r.sequential,
                    queue_pos: r.queue_position,
                    trackers: r.trackers,
                },
            );
            self.torrents.write().insert(ih, t);
        }
        let n = self.torrents.read().len();
        if n > 0 {
            info!("restored {n} torrents");
        }
    }

    /// Keep at most `max_active_downloads` downloading; the rest wait in queue order.
    fn apply_queue(&self) {
        let max = self.ctx.settings.read().max_active_downloads;
        let mut list: Vec<Arc<Torrent>> = self.torrents.read().values().cloned().collect();
        list.sort_by_key(|t| t.queue_pos());
        let mut active = 0usize;
        for t in list {
            if t.is_download_candidate() {
                let queue = max > 0 && active >= max;
                if !queue {
                    active += 1;
                }
                t.send(Cmd::SetQueued(queue));
            } else {
                t.send(Cmd::SetQueued(false));
            }
        }
    }

    fn publish(&self) {
        let mut torrents: Vec<_> = self.torrents.read().values().map(|t| t.summary()).collect();
        torrents.sort_by(|a, b| a.queue_position.cmp(&b.queue_position).then(a.added_at.cmp(&b.added_at)));
        let settings = self.ctx.settings.read().clone();
        let stats = GlobalStats {
            download_rate: torrents.iter().map(|t| t.download_rate).sum(),
            upload_rate: torrents.iter().map(|t| t.upload_rate).sum(),
            session_downloaded: self.ctx.session_down.load(Ordering::Relaxed),
            session_uploaded: self.ctx.session_up.load(Ordering::Relaxed),
            download_limit: settings.download_limit,
            upload_limit: settings.upload_limit,
            connected_peers: self.ctx.connections.load(Ordering::Relaxed),
            dht_nodes: self.ctx.dht().map(|d| d.node_count()).unwrap_or(0),
            listen_port: self.ctx.listen_port.load(Ordering::Relaxed),
            connectable: self.ctx.connectable.load(Ordering::Relaxed),
            free_space: free_space(&settings.download_dir),
            upnp: settings.enable_upnp.then(|| self.ctx.upnp_status.read().clone()).flatten(),
        };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.snapshot_tx.send(Arc::new(EngineSnapshot { seq, torrents, stats }));
    }

    fn save_all(&self, force: bool) {
        let list: Vec<Arc<Torrent>> = self.torrents.read().values().cloned().collect();
        for t in list {
            if let Some(r) = t.take_resume(force) {
                if let Err(e) = self.ctx.store.save_resume(&t.info_hash, &r) {
                    warn!("failed to save resume data: {e}");
                }
            }
        }
        if let Some(d) = self.ctx.dht() {
            let nodes = d.known_nodes();
            if !nodes.is_empty() {
                let _ = self.ctx.store.save("dht.json", &nodes);
            }
        }
    }

    fn get(&self, hash: &str) -> Result<Arc<Torrent>> {
        let ih = parse_hash(hash)?;
        self.torrents.read().get(&ih).cloned().ok_or_else(|| Error::Engine("no such torrent".into()))
    }
}

fn parse_hash(hash: &str) -> Result<InfoHash> {
    hex::decode(hash.trim())
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| Error::Engine(format!("invalid info-hash '{hash}'")))
}

/// Free bytes on the volume holding `path` (walks up to the nearest existing ancestor).
fn free_space(path: &std::path::Path) -> Option<u64> {
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    fs4::available_space(p).ok()
}

impl EngineHandle {
    pub async fn add(&self, req: AddTorrent) -> Result<String> {
        let s = &self.s;
        let (meta, magnet) = match req.source {
            TorrentSource::Bytes(b) => (Some(Metainfo::from_bytes(&b)?), None),
            TorrentSource::File(p) => (Some(Metainfo::read_file(&p).await?), None),
            TorrentSource::Magnet(m) => (None, Some(Magnet::parse(&m)?)),
        };
        let ih = meta.as_ref().map(|m| m.info_hash).or(magnet.as_ref().map(|m| m.info_hash)).expect("one is set");

        if let Some(existing) = s.torrents.read().get(&ih).cloned() {
            // Merge new trackers/peers into the existing torrent rather than erroring.
            if let Some(m) = &magnet {
                existing.send(Cmd::AddPeers(m.peers.clone(), PeerSource::Manual));
            }
            return Err(Error::Engine("torrent is already in the list".into()));
        }

        let save_path = req.save_path.unwrap_or_else(|| s.ctx.settings.read().download_dir.clone());
        let file_priorities = match (&meta, req.file_priorities) {
            (Some(m), Some(p)) if p.len() == m.info.files.len() => p,
            _ => Vec::new(),
        };
        if let Some(m) = &meta {
            s.ctx.store.save_torrent(&ih, &m.to_torrent_bytes())?;
        }
        let queue_pos = s.torrents.read().values().map(|t| t.queue_pos() + 1).max().unwrap_or(0);
        let name = meta
            .as_ref()
            .map(|m| m.info.name.clone())
            .or_else(|| magnet.as_ref().and_then(|m| m.display_name.clone()))
            .unwrap_or_else(|| hex::encode(ih));

        let t = Torrent::spawn(
            s.ctx.clone(),
            ih,
            TorrentInit {
                meta: meta.map(Arc::new),
                magnet,
                save_path,
                paused: req.paused,
                have: None,
                file_priorities,
                uploaded: 0,
                downloaded: 0,
                added_at: now_unix(),
                completed_at: None,
                sequential: req.sequential,
                queue_pos,
                trackers: Vec::new(),
            },
        );
        if let Some(r) = t.take_resume(true) {
            let _ = s.ctx.store.save_resume(&ih, &r);
        }
        s.torrents.write().insert(ih, t);
        s.apply_queue();
        s.publish();
        let hex = hex::encode(ih);
        s.ctx.emit(Event::TorrentAdded { info_hash: hex.clone(), name });
        Ok(hex)
    }

    /// Parse a `.torrent`, flagging whether it is already in the session.
    pub fn inspect(&self, bytes: &[u8]) -> Result<TorrentPreview> {
        let mut p = Engine::inspect(bytes)?;
        let ih = parse_hash(&p.info_hash)?;
        p.already_added = self.s.torrents.read().contains_key(&ih);
        Ok(p)
    }

    pub fn pause(&self, hash: &str) -> Result<()> {
        self.s.get(hash)?.send(Cmd::Pause);
        Ok(())
    }

    pub fn resume(&self, hash: &str) -> Result<()> {
        self.s.get(hash)?.send(Cmd::Resume);
        Ok(())
    }

    pub fn pause_all(&self) {
        for t in self.s.torrents.read().values() {
            t.send(Cmd::Pause);
        }
    }

    pub fn resume_all(&self) {
        for t in self.s.torrents.read().values() {
            t.send(Cmd::Resume);
        }
    }

    pub fn recheck(&self, hash: &str) -> Result<()> {
        self.s.get(hash)?.send(Cmd::Recheck);
        Ok(())
    }

    pub fn reannounce(&self, hash: &str) -> Result<()> {
        self.s.get(hash)?.send(Cmd::Reannounce);
        Ok(())
    }

    pub fn set_sequential(&self, hash: &str, on: bool) -> Result<()> {
        self.s.get(hash)?.send(Cmd::SetSequential(on));
        Ok(())
    }

    pub fn set_file_priorities(&self, hash: &str, priorities: Vec<u8>) -> Result<()> {
        self.s.get(hash)?.send(Cmd::SetFilePriorities(priorities));
        Ok(())
    }

    /// Manually add peer addresses (e.g. `1.2.3.4:6881`).
    pub fn add_peers(&self, hash: &str, peers: Vec<SocketAddr>) -> Result<()> {
        self.s.get(hash)?.send(Cmd::AddPeers(peers, PeerSource::Manual));
        Ok(())
    }

    /// Move a torrent in the download queue (0 = top).
    pub fn set_queue_position(&self, hash: &str, pos: usize) -> Result<()> {
        let target = self.s.get(hash)?;
        let mut list: Vec<Arc<Torrent>> = self.s.torrents.read().values().cloned().collect();
        list.sort_by_key(|t| t.queue_pos());
        list.retain(|t| !Arc::ptr_eq(t, &target));
        list.insert(pos.min(list.len()), target);
        for (i, t) in list.iter().enumerate() {
            t.set_queue_pos(i);
        }
        self.s.apply_queue();
        Ok(())
    }

    pub async fn remove(&self, hash: &str, delete_files: bool) -> Result<()> {
        let ih = parse_hash(hash)?;
        let t = self.s.torrents.write().remove(&ih).ok_or_else(|| Error::Engine("no such torrent".into()))?;
        let (tx, rx) = oneshot::channel();
        t.send(Cmd::Shutdown { delete_files, done: tx });
        let _ = tokio::time::timeout(Duration::from_secs(10), rx).await;
        self.s.ctx.store.remove(&ih);
        self.s.ctx.emit(Event::TorrentRemoved { info_hash: hex::encode(ih) });
        self.s.apply_queue();
        self.s.publish();
        Ok(())
    }

    pub fn settings(&self) -> Settings {
        self.s.ctx.settings.read().clone()
    }

    pub async fn update_settings(&self, new: Settings) -> Result<()> {
        let new = new.sanitized();
        let old = std::mem::replace(&mut *self.s.ctx.settings.write(), new.clone());
        self.s.ctx.down.set_rate(new.download_limit);
        self.s.ctx.up.set_rate(new.upload_limit);
        self.s.ctx.store.save("settings.json", &new)?;
        if old.listen_port != new.listen_port {
            self.s.bind_listener().await;
        }
        if old.listen_port != new.listen_port || old.enable_dht != new.enable_dht {
            self.s.start_dht().await;
        }
        self.s.apply_queue();
        self.s.publish();
        Ok(())
    }

    /// Latest snapshot (refreshed every 500 ms).
    pub fn snapshot(&self) -> Arc<EngineSnapshot> {
        self.snapshot_rx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<EngineSnapshot>> {
        self.snapshot_rx.clone()
    }

    pub fn details(&self, hash: &str) -> Option<TorrentDetails> {
        self.s.get(hash).ok().map(|t| t.details())
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.s.ctx.events.subscribe()
    }

    pub fn listen_port(&self) -> u16 {
        self.s.ctx.listen_port.load(Ordering::Relaxed)
    }

    pub fn state_dir(&self) -> PathBuf {
        self.s.ctx.store.root().to_path_buf()
    }

    /// Flush resume data and stop all torrents (trackers get a `stopped` announce).
    pub async fn shutdown(&self) {
        if !self.s.running.swap(false, Ordering::Relaxed) {
            return;
        }
        info!("engine shutting down");
        self.s.save_all(true);
        let list: Vec<Arc<Torrent>> = self.s.torrents.read().values().cloned().collect();
        let mut waits = Vec::new();
        for t in list {
            let (tx, rx) = oneshot::channel();
            t.send(Cmd::Shutdown { delete_files: false, done: tx });
            waits.push(rx);
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(waits)).await;
        if let Some(h) = self.s.listener.lock().take() {
            h.abort();
        }
        // Give the fire-and-forget "stopped" announces a moment to leave.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
