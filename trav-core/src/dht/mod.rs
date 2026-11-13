//! Mainline DHT (BEP 5): a Kademlia node that can find and announce peers
//! for an info-hash, and answers the standard queries so we are a good citizen.
//!
//! IPv4 and IPv6 (BEP 32), with independent 160-bucket K=8 routing tables, kept
//! filled across the whole id space by refresh lookups; lookups are iterative
//! with α concurrent queries and terminate once the K closest responsive nodes
//! have all been asked.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::stream::{FuturesUnordered, StreamExt};
use parking_lot::Mutex;
use sha1::{Digest, Sha1};
use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc, oneshot};
use tracing::{debug, info};

use crate::bencode::{self, DictBuilder, Value};
use crate::error::{Error, Result};
use crate::utp::UdpSender;

pub type NodeId = [u8; 20];

const K: usize = 8;
const ALPHA: usize = 4;
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const LOOKUP_BUDGET: usize = 160;
/// Queries per bucket-refresh lookup: enough to reach the bucket's region and fill it.
const REFRESH_BUDGET: usize = 24;
/// An under-filled bucket is looked up again at most this often.
const REFRESH_EVERY: Duration = Duration::from_secs(15 * 60);
const LOOKUP_DEADLINE: Duration = Duration::from_secs(25);
const STALE: Duration = Duration::from_secs(15 * 60);
const MAX_STORED_PEERS: usize = 200;
const MAX_CANDIDATES: usize = K * LOOKUP_BUDGET;
const MAX_PENDING: usize = 4096;
const IPV6_BACKOFF: Duration = Duration::from_secs(5 * 60);
const MAX_PACKET: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    V4 = 0,
    V6 = 1,
}

impl Family {
    fn of(addr: SocketAddr) -> Self {
        if addr.is_ipv4() { Self::V4 } else { Self::V6 }
    }
}

fn canonical(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V6(a) if a.ip().to_ipv4_mapped().is_some() => {
            SocketAddr::new(a.ip().to_canonical(), a.port())
        }
        a => a,
    }
}

fn valid_addr(addr: SocketAddr) -> bool {
    addr.port() != 0
        && !addr.ip().is_unspecified()
        && !addr.ip().is_multicast()
        && !matches!(addr.ip(), IpAddr::V4(ip) if ip.is_broadcast())
}

// One public IPv4 host or IPv6 /64 may not crowd out real nodes with fake IDs.
// Local addresses are exempt so LANs and loopback can host several nodes.
fn public_prefix(ip: IpAddr) -> Option<IpAddr> {
    match ip {
        IpAddr::V4(ip) if ip.is_loopback() || ip.is_private() || ip.is_link_local() => None,
        IpAddr::V6(ip)
            if ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local() =>
        {
            None
        }
        IpAddr::V4(ip) => Some(ip.into()),
        IpAddr::V6(ip) => {
            let mut bytes = ip.octets();
            bytes[8..].fill(0);
            Some(Ipv6Addr::from(bytes).into())
        }
    }
}

fn bind_dual_stack(port: u16) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_only_v6(false)?;
    socket.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into())?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(std::net::UdpSocket::from(socket))
}

async fn bind(port: u16) -> io::Result<UdpSocket> {
    match bind_dual_stack(port) {
        Ok(socket) => Ok(socket),
        Err(_) => UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).await,
    }
}

pub const BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
    "dht.libtorrent.org:25401",
    "dht.aelitis.com:6881",
];

#[derive(Debug, Clone)]
struct Node {
    id: NodeId,
    addr: SocketAddr,
    last_seen: Instant,
    fails: u8,
}

struct Table {
    family: Family,
    own: NodeId,
    buckets: Vec<Vec<Node>>,
    refreshed: Vec<Option<Instant>>,
}

fn distance(a: &NodeId, b: &NodeId) -> NodeId {
    let mut d = [0u8; 20];
    for i in 0..20 {
        d[i] = a[i] ^ b[i];
    }
    d
}

/// A random id that lands in bucket `i` of `own`'s table: the first `i` bits
/// match `own`, bit `i` differs, the rest are random.
fn random_id_in_bucket(own: &NodeId, i: usize) -> NodeId {
    let mut id: NodeId = rand::random();
    let (byte, bit) = (i / 8, i % 8);
    id[..byte].copy_from_slice(&own[..byte]);
    let keep = !(0xFFu8 >> bit);
    let flip = 0x80u8 >> bit;
    id[byte] = (own[byte] & keep) | (!own[byte] & flip) | (id[byte] & !keep & !flip);
    id
}

fn bucket_index(own: &NodeId, id: &NodeId) -> Option<usize> {
    let d = distance(own, id);
    let lz = d
        .iter()
        .position(|&b| b != 0)
        .map(|i| i * 8 + d[i].leading_zeros() as usize)?;
    Some(lz.min(159))
}

impl Table {
    fn new(own: NodeId, family: Family) -> Self {
        Self {
            family,
            own,
            buckets: vec![Vec::new(); 160],
            refreshed: vec![None; 160],
        }
    }

    fn len(&self) -> usize {
        self.buckets.iter().map(Vec::len).sum()
    }

    fn insert(&mut self, id: NodeId, addr: SocketAddr) {
        if Family::of(addr) != self.family || !valid_addr(addr) {
            return;
        }
        let Some(bi) = bucket_index(&self.own, &id) else {
            return;
        };
        if let Some(prefix) = public_prefix(addr.ip())
            && self
                .buckets
                .iter()
                .flatten()
                .any(|n| n.id != id && public_prefix(n.addr.ip()) == Some(prefix))
        {
            return;
        }
        if let Some(n) = self.buckets[bi].iter_mut().find(|n| n.id == id) {
            n.addr = addr;
            n.last_seen = Instant::now();
            n.fails = 0;
            return;
        }
        let bucket = &mut self.buckets[bi];
        let node = Node {
            id,
            addr,
            last_seen: Instant::now(),
            fails: 0,
        };
        if bucket.len() < K {
            bucket.push(node);
        } else if let Some(slot) = bucket
            .iter_mut()
            .find(|n| n.fails >= 2 || n.last_seen.elapsed() > STALE)
        {
            *slot = node;
        }
    }

    fn failed(&mut self, id: &NodeId) {
        if let Some(bi) = bucket_index(&self.own, id) {
            let bucket = &mut self.buckets[bi];
            if let Some(pos) = bucket.iter().position(|n| &n.id == id) {
                bucket[pos].fails += 1;
                if bucket[pos].fails >= 4 {
                    bucket.remove(pos);
                }
            }
        }
    }

    fn closest(&self, target: &NodeId, n: usize) -> Vec<Node> {
        let mut all: Vec<&Node> = self
            .buckets
            .iter()
            .flatten()
            .filter(|n| n.fails < 2)
            .collect();
        all.sort_by_key(|node| distance(&node.id, target));
        all.into_iter().take(n).cloned().collect()
    }

    /// Random targets for the buckets worth a refresh lookup: under-filled, not
    /// refreshed lately, and no deeper than the closest nodes we know of.
    fn refresh_targets(&mut self, now: Instant) -> Vec<NodeId> {
        let depth = self
            .buckets
            .iter()
            .rposition(|b| !b.is_empty())
            .unwrap_or(0);
        let mut out = Vec::new();
        for i in 0..=depth {
            let due = self.refreshed[i].is_none_or(|at| now.duration_since(at) >= REFRESH_EVERY);
            if self.buckets[i].len() < K && due {
                self.refreshed[i] = Some(now);
                out.push(random_id_in_bucket(&self.own, i));
            }
        }
        out
    }

    /// Nodes we have not heard from in a while, oldest first.
    fn questionable(&self, n: usize) -> Vec<SocketAddr> {
        let mut old: Vec<&Node> = self
            .buckets
            .iter()
            .flatten()
            .filter(|n| n.last_seen.elapsed() > STALE)
            .collect();
        old.sort_by_key(|n| n.last_seen);
        old.into_iter().take(n).map(|n| n.addr).collect()
    }

    fn all_addrs(&self) -> Vec<SocketAddr> {
        self.buckets
            .iter()
            .flatten()
            .filter(|n| n.fails == 0)
            .map(|n| n.addr)
            .collect()
    }
}

fn encode_nodes(nodes: &[Node]) -> Vec<u8> {
    let mut out = Vec::new();
    for n in nodes {
        out.extend_from_slice(&n.id);
        crate::tracker::encode_compact(&n.addr, &mut out);
    }
    out
}

fn decode_nodes(bytes: &[u8], family: Family) -> impl Iterator<Item = (NodeId, SocketAddr)> + '_ {
    let size = if family == Family::V4 { 26 } else { 38 };
    bytes.chunks_exact(size).filter_map(move |chunk| {
        let id = chunk[..20].try_into().ok()?;
        let addr = decode_peer(&chunk[20..])?;
        (Family::of(addr) == family).then_some((id, addr))
    })
}

// A values entry is exactly one peer. In particular, an 18-byte IPv6 peer
// must never be interpreted as three IPv4 peers; hybrid lists are legal.
fn decode_peer(bytes: &[u8]) -> Option<SocketAddr> {
    let addr = match bytes.len() {
        6 => crate::tracker::parse_compact_v4(bytes).next()?,
        18 => crate::tracker::parse_compact_v6(bytes).next()?,
        _ => return None,
    };
    let addr = canonical(addr);
    valid_addr(addr).then_some(addr)
}

fn wants(args: &Value, from: SocketAddr) -> (bool, bool) {
    match args.get("want").and_then(Value::as_list) {
        Some(flags) => (
            flags.iter().any(|v| v.as_str() == Some("n4")),
            flags.iter().any(|v| v.as_str() == Some("n6")),
        ),
        None => (from.is_ipv4(), from.is_ipv6()),
    }
}

fn id_of(v: &Value) -> Option<NodeId> {
    v.get("id")
        .and_then(Value::as_bytes)
        .and_then(|b| b.try_into().ok())
}

/// A query awaiting its reply: who we asked, and where the answer goes.
type Pending = (SocketAddr, Option<oneshot::Sender<Result<Value>>>);

struct Inner {
    socket: UdpSender,
    routers: &'static [&'static str],
    id: NodeId,
    tables: [Mutex<Table>; 2],
    ipv6_retry: Mutex<Option<Instant>>,
    ipv6_disabled: Notify,
    /// Queries awaiting a reply, by transaction id.
    pending: Mutex<HashMap<u16, Pending>>,
    tid: AtomicU16,
    secrets: Mutex<([u8; 16], [u8; 16])>,
    store: Mutex<HashMap<NodeId, Vec<(SocketAddr, Instant)>>>,
    /// Stops the receive and maintenance loops and fails queries in flight.
    cancel: tokio_util::sync::CancellationToken,
}

#[derive(Clone)]
pub struct Dht {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct LookupResult {
    peers: Vec<SocketAddr>,
    /// Closest responsive nodes with their announce tokens.
    closest: Vec<(SocketAddr, Vec<u8>)>,
}

/// Lookup candidate: (node id, address, state, announce token).
type Cand = (NodeId, SocketAddr, CandState, Option<Vec<u8>>);

/// The nodes a lookup may ask, closest to its target first.
#[derive(Default)]
struct Candidates {
    by_distance: BTreeMap<NodeId, Cand>,
    ips: HashSet<IpAddr>,
}

impl Candidates {
    /// Adds a node unless its id or (public) IP is already a candidate. Fake
    /// nodes that claim ids right next to every target come many to an IP; one
    /// candidate per IP stops them crowding out the real closest nodes.
    fn add(&mut self, target: &NodeId, id: NodeId, addr: SocketAddr) {
        let d = distance(&id, target);
        if !valid_addr(addr) || self.by_distance.contains_key(&d) {
            return;
        }
        if let Some(prefix) = public_prefix(addr.ip())
            && self.ips.contains(&prefix)
        {
            return;
        }
        if self.by_distance.len() >= MAX_CANDIDATES {
            if self
                .by_distance
                .last_key_value()
                .is_some_and(|(last, _)| d >= *last)
            {
                return;
            }
            if let Some((_, removed)) = self.by_distance.pop_last()
                && let Some(prefix) = public_prefix(removed.1.ip())
            {
                self.ips.remove(&prefix);
            }
        }
        if let Some(prefix) = public_prefix(addr.ip()) {
            self.ips.insert(prefix);
        }
        self.by_distance.insert(d, (id, addr, CandState::New, None));
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CandState {
    New,
    Asked,
    Responded,
    Failed,
}

impl Dht {
    /// Bind a UDP socket of its own and start (tests, and when µTP is off).
    pub async fn start(port: u16, known_nodes: Vec<SocketAddr>) -> Result<Self> {
        Self::start_with_routers(port, known_nodes, BOOTSTRAP).await
    }

    async fn start_with_routers(
        port: u16,
        known_nodes: Vec<SocketAddr>,
        routers: &'static [&'static str],
    ) -> Result<Self> {
        // A DHT that was just stopped may still be releasing the port.
        let mut bound = None;
        for attempt in 0..10 {
            match bind(port).await {
                Ok(s) => {
                    bound = Some(s);
                    break;
                }
                Err(e) if attempt == 9 => {
                    debug!("DHT port {port} unavailable ({e}); using ephemeral")
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        let socket = Arc::new(match bound {
            Some(s) => s,
            None => bind(0).await?,
        });
        let (tx, rx) = mpsc::channel(1024);
        let dht =
            Self::start_on_with_routers(UdpSender::new(socket.clone()), rx, known_nodes, routers);
        let cancel = dht.inner.cancel.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let res = tokio::select! {
                    _ = cancel.cancelled() => break,
                    r = socket.recv_from(&mut buf) => r,
                };
                match res {
                    Ok((n, from)) => {
                        if tx
                            .try_send((Bytes::copy_from_slice(&buf[..n]), canonical(from)))
                            .is_err()
                            && tx.is_closed()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        debug!("DHT recv error: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        });
        Ok(dht)
    }

    /// Run on a socket shared with µTP: `udp` sends, `incoming` carries the
    /// datagrams the µTP socket didn't claim.
    pub fn start_on(
        udp: UdpSender,
        incoming: mpsc::Receiver<(Bytes, SocketAddr)>,
        known_nodes: Vec<SocketAddr>,
    ) -> Self {
        Self::start_on_with_routers(udp, incoming, known_nodes, BOOTSTRAP)
    }

    fn start_on_with_routers(
        udp: UdpSender,
        mut incoming: mpsc::Receiver<(Bytes, SocketAddr)>,
        known_nodes: Vec<SocketAddr>,
        routers: &'static [&'static str],
    ) -> Self {
        let id: NodeId = rand::random();
        let inner = Arc::new(Inner {
            socket: udp,
            routers,
            id,
            tables: [
                Mutex::new(Table::new(id, Family::V4)),
                Mutex::new(Table::new(id, Family::V6)),
            ],
            ipv6_retry: Mutex::new(None),
            ipv6_disabled: Notify::new(),
            pending: Mutex::new(HashMap::new()),
            tid: AtomicU16::new(rand::random()),
            secrets: Mutex::new((rand::random(), rand::random())),
            store: Mutex::new(HashMap::new()),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
        let dht = Self { inner };

        let weak = Arc::downgrade(&dht.inner);
        let cancel = dht.inner.cancel.clone();
        tokio::spawn(async move {
            loop {
                let res = tokio::select! {
                    _ = cancel.cancelled() => break,
                    r = incoming.recv() => r,
                };
                let Some((data, from)) = res else { break };
                let Some(inner) = weak.upgrade() else { break };
                Dht { inner }.on_packet(&data, canonical(from)).await;
            }
        });

        let weak = Arc::downgrade(&dht.inner);
        let cancel = dht.inner.cancel.clone();
        tokio::spawn(async move {
            let mut seeds = known_nodes;
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            let mut rounds = 0u64;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tick.tick() => {}
                }
                let Some(inner) = weak.upgrade() else { break };
                let dht = Dht { inner };
                let round = async {
                    if dht.inner.tables[0].lock().len() < 32
                        || (dht.ipv6_usable() && dht.inner.tables[1].lock().len() < 32)
                    {
                        dht.bootstrap(std::mem::take(&mut seeds)).await;
                        info!("DHT bootstrapped: {} nodes", dht.node_count());
                    }
                    dht.maintain().await;
                };
                // Dropping the round mid-way (e.g. during a DNS lookup) releases the socket.
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = round => {}
                }
                rounds += 1;
                if rounds.is_multiple_of(5) {
                    let mut s = dht.inner.secrets.lock();
                    s.1 = s.0;
                    s.0 = rand::random();
                    dht.inner.store.lock().retain(|_, v| {
                        v.retain(|(_, t)| t.elapsed() < Duration::from_secs(30 * 60));
                        !v.is_empty()
                    });
                }
            }
        });

        dht
    }

    /// Stop for good: the loops end, queries in flight fail at once, and the
    /// socket closes as soon as the last lookup notices.
    pub fn stop(&self) {
        self.inner.cancel.cancel();
        self.inner.pending.lock().clear();
    }

    pub fn port(&self) -> u16 {
        self.inner.socket.local_port()
    }

    pub fn node_count(&self) -> usize {
        self.inner.tables.iter().map(|t| t.lock().len()).sum()
    }

    pub fn known_nodes(&self) -> Vec<SocketAddr> {
        self.inner
            .tables
            .iter()
            .flat_map(|t| t.lock().all_addrs())
            .collect()
    }

    fn table(&self, addr: SocketAddr) -> &Mutex<Table> {
        &self.inner.tables[Family::of(addr) as usize]
    }

    fn ipv6_usable(&self) -> bool {
        self.inner
            .ipv6_retry
            .lock()
            .is_none_or(|at| Instant::now() >= at)
    }

    async fn send(&self, data: &[u8], addr: SocketAddr) -> io::Result<usize> {
        if data.len() > MAX_PACKET {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if addr.is_ipv6() && !self.ipv6_usable() {
            return Err(io::ErrorKind::NetworkUnreachable.into());
        }
        let result = self.inner.socket.send_to(data, addr).await;
        if addr.is_ipv6()
            && let Err(error) = &result
            && (matches!(
                error.kind(),
                io::ErrorKind::NetworkUnreachable
                    | io::ErrorKind::HostUnreachable
                    | io::ErrorKind::AddrNotAvailable
                    | io::ErrorKind::Unsupported
            ) || matches!(
                error.raw_os_error(),
                Some(libc::EAFNOSUPPORT | libc::EPROTONOSUPPORT)
            ))
        {
            let mut retry = self.inner.ipv6_retry.lock();
            if retry.is_none_or(|at| Instant::now() >= at) {
                *retry = Some(Instant::now() + IPV6_BACKOFF);
                self.inner.ipv6_disabled.notify_waiters();
                debug!(
                    "DHT IPv6 unavailable; backing off for {} seconds",
                    IPV6_BACKOFF.as_secs()
                );
            }
        }
        result
    }

    /// Seed the routing table from a peer's BEP 5 `port` message.
    pub async fn ping(&self, addr: SocketAddr) {
        let addr = canonical(addr);
        let args = DictBuilder::new()
            .bytes("id", self.inner.id.to_vec())
            .build();
        let _ = self.query(addr, "ping", args).await;
    }

    async fn seed_nodes(&self, targets: Vec<SocketAddr>) {
        let mut seen = HashSet::new();
        let mut counts = [0; 2];
        let mut futs: FuturesUnordered<_> = targets
            .into_iter()
            .map(canonical)
            .filter(|addr| {
                let family = Family::of(*addr) as usize;
                if !valid_addr(*addr) || !seen.insert(*addr) || counts[family] >= 64 {
                    return false;
                }
                counts[family] += 1;
                true
            })
            .map(|addr| async move {
                let args = DictBuilder::new()
                    .bytes("id", self.inner.id.to_vec())
                    .bytes("target", self.inner.id.to_vec())
                    .build();
                if let Ok(reply) = self.query(addr, "find_node", args).await {
                    // Referrals remain untrusted until they answer a query.
                    for (key, family) in [("nodes", Family::V4), ("nodes6", Family::V6)] {
                        if let Some(bytes) = reply.get(key).and_then(Value::as_bytes) {
                            let mut candidates = Candidates::default();
                            for (id, addr) in decode_nodes(bytes, family).take(K) {
                                if id != self.inner.id {
                                    candidates.add(&self.inner.id, id, addr);
                                }
                            }
                            let probes = candidates
                                .by_distance
                                .values()
                                .map(|node| self.ping(node.1));
                            futures::future::join_all(probes).await;
                        }
                    }
                }
            })
            .collect();
        while futs.next().await.is_some() {}
    }

    async fn bootstrap(&self, seeds: Vec<SocketAddr>) {
        let resolve = async {
            let resolutions = self.inner.routers.iter().map(|host| async move {
                tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host(*host)).await
            });
            let mut targets = Vec::new();
            for addrs in futures::future::join_all(resolutions).await {
                if let Ok(Ok(addrs)) = addrs {
                    targets.extend(addrs);
                }
            }
            targets
        };
        // Persisted nodes are usable immediately, even while DNS is slow.
        let (_, resolved) = tokio::join!(self.seed_nodes(seeds), resolve);
        self.seed_nodes(resolved).await;
        self.lookup(self.inner.id, false, LOOKUP_BUDGET).await;
    }

    /// Keeps the table healthy: fills under-populated buckets with lookups for
    /// random ids in their range (otherwise it only ever holds our own
    /// neighbourhood, and every search starts far from its target), and
    /// re-checks nodes that have gone quiet so dead ones get replaced.
    async fn maintain(&self) {
        let mut targets = Vec::new();
        let mut stale = Vec::new();
        for family in [Family::V4, Family::V6] {
            if family == Family::V6 && !self.ipv6_usable() {
                continue;
            }
            let mut table = self.inner.tables[family as usize].lock();
            targets.extend(table.refresh_targets(Instant::now()));
            stale.extend(table.questionable(16));
        }
        let pings = stale.into_iter().map(|a| {
            let me = self.clone();
            async move { me.ping(a).await }
        });
        futures::future::join_all(pings).await;
        let mut lookups = futures::stream::iter(targets.into_iter().map(|id| {
            let me = self.clone();
            async move { me.lookup(id, false, REFRESH_BUDGET).await }
        }))
        .buffer_unordered(4);
        while lookups.next().await.is_some() {}
        debug!("DHT table: {} nodes", self.node_count());
    }

    /// Find peers for `info_hash`; when `announce_port` is set, also announce ourselves.
    pub async fn get_peers(
        &self,
        info_hash: NodeId,
        announce_port: Option<u16>,
    ) -> Vec<SocketAddr> {
        if self.node_count() == 0 {
            self.bootstrap(Vec::new()).await;
        }
        let res = self.lookup(info_hash, true, LOOKUP_BUDGET).await;
        if let Some(port) = announce_port {
            let futs: FuturesUnordered<_> = res
                .closest
                .iter()
                .map(|(addr, token)| {
                    let args = DictBuilder::new()
                        .bytes("id", self.inner.id.to_vec())
                        .bytes("info_hash", info_hash.to_vec())
                        .int("port", port as i64)
                        .int("implied_port", 0)
                        .bytes("token", token.clone())
                        .build();
                    self.query(*addr, "announce_peer", args)
                })
                .collect();
            let _ = futs.collect::<Vec<_>>().await;
        }
        res.peers
    }

    async fn lookup(&self, target: NodeId, get_peers: bool, budget: usize) -> LookupResult {
        let started = Instant::now();
        let mut candidates = [Candidates::default(), Candidates::default()];
        for family in [Family::V4, Family::V6] {
            for node in self.inner.tables[family as usize]
                .lock()
                .closest(&target, K * 2)
            {
                candidates[family as usize].add(&target, node.id, node.addr);
            }
        }
        let mut result = LookupResult::default();
        let mut seen_peers = HashSet::new();
        let mut inflight = FuturesUnordered::new();
        let mut asked = [0usize; 2];
        let mut active = [0usize; 2];

        loop {
            // Independent K/alpha/budget per family: IPv6 never takes an IPv4 slot.
            for family in [Family::V4, Family::V6] {
                let index = family as usize;
                if family == Family::V6 && !self.ipv6_usable() {
                    continue;
                }
                while active[index] < ALPHA && asked[index] < budget {
                    let next = candidates[index]
                        .by_distance
                        .iter_mut()
                        .filter(|(_, c)| c.2 != CandState::Failed)
                        .take(K)
                        .find(|(_, c)| c.2 == CandState::New)
                        .map(|(d, c)| {
                            c.2 = CandState::Asked;
                            (*d, c.1)
                        });
                    let Some((dist, addr)) = next else { break };
                    asked[index] += 1;
                    active[index] += 1;
                    let me = self.clone();
                    let method = if get_peers { "get_peers" } else { "find_node" };
                    let mut args = DictBuilder::new().bytes("id", self.inner.id.to_vec());
                    args = if get_peers {
                        args.bytes("info_hash", target.to_vec())
                    } else {
                        args.bytes("target", target.to_vec())
                    };
                    let args = args.build();
                    inflight.push(async move { (index, dist, me.query(addr, method, args).await) });
                }
            }
            if inflight.is_empty() || started.elapsed() > LOOKUP_DEADLINE {
                break;
            }
            let Some((index, dist, resp)) = inflight.next().await else {
                break;
            };
            active[index] -= 1;
            match resp {
                Ok(r) => {
                    if let Some(cand) = candidates[index].by_distance.get_mut(&dist) {
                        cand.2 = CandState::Responded;
                        cand.3 = r.get("token").and_then(Value::as_bytes).map(<[u8]>::to_vec);
                    }
                    if let Some(vals) = r.get("values").and_then(Value::as_list) {
                        for v in vals.iter().filter_map(Value::as_bytes) {
                            if let Some(peer) = decode_peer(v)
                                && seen_peers.insert(peer)
                            {
                                result.peers.push(peer);
                            }
                        }
                    }
                    for (key, family) in [("nodes", Family::V4), ("nodes6", Family::V6)] {
                        if let Some(nodes) = r.get(key).and_then(Value::as_bytes) {
                            for (id, addr) in decode_nodes(nodes, family) {
                                if id != self.inner.id {
                                    candidates[family as usize].add(&target, id, addr);
                                }
                            }
                        }
                    }
                }
                Err(_) => {
                    if let Some(cand) = candidates[index].by_distance.get_mut(&dist) {
                        cand.2 = CandState::Failed;
                    }
                }
            }
        }

        result.closest = candidates
            .iter()
            .flat_map(|c| {
                c.by_distance
                    .values()
                    .filter(|c| c.2 == CandState::Responded)
                    .filter_map(|c| c.3.clone().map(|token| (c.1, token)))
                    .take(K)
            })
            .collect();
        debug!(
            "DHT lookup {} done: {} queries, {} peers, {:?}",
            hex::encode(target),
            asked.iter().sum::<usize>(),
            result.peers.len(),
            started.elapsed()
        );
        result
    }

    async fn query(&self, addr: SocketAddr, method: &str, mut args: Value) -> Result<Value> {
        let addr = canonical(addr);
        if self.inner.cancel.is_cancelled() {
            return Err(Error::Timeout("dht stopped"));
        }
        let (tx, rx) = oneshot::channel();
        let tid = {
            let mut pending = self.inner.pending.lock();
            if pending.len() >= MAX_PENDING {
                return Err(Error::Protocol("too many DHT queries".into()));
            }
            let tid = loop {
                let tid = self.inner.tid.fetch_add(1, Ordering::Relaxed);
                if !pending.contains_key(&tid) {
                    break tid;
                }
            };
            pending.insert(tid, (addr, Some(tx)));
            tid
        };
        let _guard = PendingGuard {
            inner: &self.inner,
            tid,
        };
        if matches!(method, "find_node" | "get_peers")
            && self.ipv6_usable()
            && let Value::Dict(dict) = &mut args
        {
            dict.insert(
                b"want".to_vec(),
                Value::List(vec![
                    Value::Bytes(b"n4".to_vec()),
                    Value::Bytes(b"n6".to_vec()),
                ]),
            );
        }
        let msg = DictBuilder::new()
            .value("a", args)
            .bytes("q", method)
            .bytes("t", tid.to_be_bytes().to_vec())
            .bytes("y", "q")
            .build()
            .encode();
        if let Err(e) = self.send(&msg, addr).await {
            return Err(e.into());
        }
        let out = tokio::select! {
            _ = self.inner.cancel.cancelled() => Err(Error::Timeout("dht stopped")),
            _ = self.inner.ipv6_disabled.notified(), if addr.is_ipv6() => Err(Error::Timeout("dht IPv6 unavailable")),
            r = tokio::time::timeout(QUERY_TIMEOUT, rx) => match r {
                Ok(Ok(r)) => r,
                _ => Err(Error::Timeout("dht query")),
            },
        };
        match &out {
            Ok(r) => {
                debug!(%addr, method,
                    nodes = ?r.get("nodes").and_then(|v| v.as_bytes()).map(|b| b.len() / 26),
                    nodes6 = ?r.get("nodes6").and_then(|v| v.as_bytes()).map(|b| b.len() / 38),
                    values = ?r.get("values").and_then(|v| v.as_list()).map(|v| v.iter().filter_map(|v| v.as_bytes()).map(<[u8]>::len).collect::<Vec<_>>()),
                    "DHT response");
                if let Some(id) = id_of(r) {
                    self.table(addr).lock().insert(id, addr);
                }
            }
            Err(_) => {
                // We may not know the id (bootstrap routers); match by address.
                let mut t = self.table(addr).lock();
                let id = t
                    .buckets
                    .iter()
                    .flatten()
                    .find(|n| n.addr == addr)
                    .map(|n| n.id);
                if let Some(id) = id {
                    t.failed(&id);
                }
            }
        }
        out
    }

    async fn on_packet(&self, data: &[u8], from: SocketAddr) {
        let Ok(msg) = bencode::decode(data) else {
            return;
        };
        let Some(t) = msg.get("t").and_then(Value::as_bytes) else {
            return;
        };
        match msg.get("y").and_then(Value::as_str) {
            Some("r") | Some("e") => {
                let Ok(tid) = <[u8; 2]>::try_from(t).map(u16::from_be_bytes) else {
                    return;
                };
                // Only the node we asked may answer: a matching transaction id
                // from anywhere else is ignored (it would let a spoofer inject
                // nodes and peers into our lookups).
                let tx = {
                    let mut pending = self.inner.pending.lock();
                    match pending.get_mut(&tid) {
                        Some((asked, tx)) if *asked == from => tx.take(),
                        _ => None,
                    }
                };
                if let Some(tx) = tx {
                    let res = match msg.get("r") {
                        Some(r) => Ok(r.clone()),
                        None => Err(Error::Protocol("dht error response".into())),
                    };
                    let _ = tx.send(res);
                }
            }
            Some("q") => {
                let t = t.to_vec();
                if let Some(reply) = self.answer(&msg, from) {
                    let out = DictBuilder::new()
                        .value("r", reply)
                        .bytes("t", t)
                        .bytes("y", "r")
                        .build();
                    let bytes = out.encode();
                    if bytes.len() <= MAX_PACKET {
                        debug!(%from, query = ?msg.get("q").and_then(|v| v.as_str()),
                            nodes = ?out.get("r").and_then(|r| r.get("nodes")).and_then(|v| v.as_bytes()).map(|b| b.len() / 26),
                            nodes6 = ?out.get("r").and_then(|r| r.get("nodes6")).and_then(|v| v.as_bytes()).map(|b| b.len() / 38),
                            values = ?out.get("r").and_then(|r| r.get("values")).and_then(|v| v.as_list()).map(|v| v.iter().filter_map(|v| v.as_bytes()).map(<[u8]>::len).collect::<Vec<_>>()),
                            "DHT reply");
                        let _ = self.send(&bytes, from).await;
                    }
                }
            }
            _ => {}
        }
    }

    fn token_for(&self, secret: &[u8; 16], ip: IpAddr) -> Vec<u8> {
        let mut h = Sha1::new();
        h.update(secret);
        match ip {
            IpAddr::V4(ip) => h.update(ip.octets()),
            IpAddr::V6(ip) => h.update(ip.octets()),
        }
        h.finalize()[..8].to_vec()
    }

    fn answer(&self, msg: &Value, from: SocketAddr) -> Option<Value> {
        let q = msg.get("q")?.as_str()?;
        let a = msg.get("a")?;
        let their_id = id_of(a)?;
        let read_only = a.get("ro").and_then(Value::as_int) == Some(1);
        if !read_only {
            self.table(from).lock().insert(their_id, from);
        }
        let own = self.inner.id.to_vec();
        let closest = |mut reply: DictBuilder, target: &NodeId| {
            let (v4, v6) = wants(a, from);
            for (include, key, family) in [(v4, "nodes", Family::V4), (v6, "nodes6", Family::V6)] {
                if include {
                    reply = reply.bytes(
                        key,
                        encode_nodes(&self.inner.tables[family as usize].lock().closest(target, K)),
                    );
                }
            }
            reply
        };

        match q {
            "ping" => Some(DictBuilder::new().bytes("id", own).build()),
            "find_node" => {
                let target: NodeId = a.get("target")?.as_bytes()?.try_into().ok()?;
                Some(closest(DictBuilder::new().bytes("id", own), &target).build())
            }
            "get_peers" => {
                let ih: NodeId = a.get("info_hash")?.as_bytes()?.try_into().ok()?;
                let token = self.token_for(&self.inner.secrets.lock().0, from.ip());
                let mut d = closest(
                    DictBuilder::new().bytes("id", own).bytes("token", token),
                    &ih,
                );
                if let Some(peers) = self.inner.store.lock().get(&ih) {
                    let vals = peers
                        .iter()
                        .filter(|(peer, at)| {
                            peer.is_ipv4() == from.is_ipv4()
                                && at.elapsed() < Duration::from_secs(30 * 60)
                        })
                        // Leave room for both node tables and the KRPC envelope.
                        .take(if from.is_ipv6() { 16 } else { 40 })
                        .map(|(p, _)| {
                            let mut b = Vec::with_capacity(6);
                            crate::tracker::encode_compact(p, &mut b);
                            Value::Bytes(b)
                        })
                        .collect();
                    d = d.value("values", Value::List(vals));
                }
                Some(d.build())
            }
            "announce_peer" => {
                let ih: NodeId = a.get("info_hash")?.as_bytes()?.try_into().ok()?;
                let token = a.get("token")?.as_bytes()?;
                let (cur, prev) = *self.inner.secrets.lock();
                if token != self.token_for(&cur, from.ip())
                    && token != self.token_for(&prev, from.ip())
                {
                    return None;
                }
                let implied = a.get("implied_port").and_then(Value::as_int) == Some(1);
                let port = if implied {
                    from.port()
                } else {
                    a.get("port")
                        .and_then(Value::as_int)
                        .and_then(|p| u16::try_from(p).ok())?
                };
                let peer = SocketAddr::new(from.ip(), port);
                if !valid_addr(peer) {
                    return None;
                }
                let mut store = self.inner.store.lock();
                if store.len() < 10_000 || store.contains_key(&ih) {
                    let list = store.entry(ih).or_default();
                    list.retain(|(p, _)| *p != peer);
                    list.push((peer, Instant::now()));
                    if list.len() > MAX_STORED_PEERS {
                        list.remove(0);
                    }
                }
                Some(DictBuilder::new().bytes("id", own).build())
            }
            _ => None,
        }
    }
}

// Lookups may be dropped at their deadline; retire their transaction IDs too.
struct PendingGuard<'a> {
    inner: &'a Inner,
    tid: u16,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.inner.pending.lock().remove(&self.tid);
    }
}

#[cfg(test)]
mod tests;
