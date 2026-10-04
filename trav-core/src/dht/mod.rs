//! Mainline DHT (BEP 5): a Kademlia node that can find and announce peers
//! for an info-hash, and answers the standard queries so we are a good citizen.
//!
//! IPv4 only. The routing table is a classic 160-bucket K=8 layout; lookups are
//! iterative with α concurrent queries and terminate once the K closest
//! responsive nodes have all been asked.

use std::collections::{BTreeMap, HashMap};
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};
use parking_lot::Mutex;
use sha1::{Digest, Sha1};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tracing::{debug, info};

use crate::bencode::{self, DictBuilder, Value};
use crate::error::{Error, Result};

pub type NodeId = [u8; 20];

const K: usize = 8;
const ALPHA: usize = 4;
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const LOOKUP_BUDGET: usize = 160;
const LOOKUP_DEADLINE: Duration = Duration::from_secs(25);
const STALE: Duration = Duration::from_secs(15 * 60);
const MAX_STORED_PEERS: usize = 200;

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
    addr: SocketAddrV4,
    last_seen: Instant,
    fails: u8,
}

struct Table {
    own: NodeId,
    buckets: Vec<Vec<Node>>,
}

fn distance(a: &NodeId, b: &NodeId) -> NodeId {
    let mut d = [0u8; 20];
    for i in 0..20 {
        d[i] = a[i] ^ b[i];
    }
    d
}

fn bucket_index(own: &NodeId, id: &NodeId) -> Option<usize> {
    let d = distance(own, id);
    let lz = d.iter().position(|&b| b != 0).map(|i| i * 8 + d[i].leading_zeros() as usize)?;
    Some(lz.min(159))
}

impl Table {
    fn new(own: NodeId) -> Self {
        Self { own, buckets: vec![Vec::new(); 160] }
    }

    fn len(&self) -> usize {
        self.buckets.iter().map(Vec::len).sum()
    }

    fn insert(&mut self, id: NodeId, addr: SocketAddrV4) {
        if addr.port() == 0 || addr.ip().is_unspecified() {
            return;
        }
        let Some(bi) = bucket_index(&self.own, &id) else { return };
        let bucket = &mut self.buckets[bi];
        if let Some(n) = bucket.iter_mut().find(|n| n.id == id) {
            n.addr = addr;
            n.last_seen = Instant::now();
            n.fails = 0;
            return;
        }
        let node = Node { id, addr, last_seen: Instant::now(), fails: 0 };
        if bucket.len() < K {
            bucket.push(node);
        } else if let Some(slot) = bucket.iter_mut().find(|n| n.fails >= 2 || n.last_seen.elapsed() > STALE) {
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
        let mut all: Vec<&Node> = self.buckets.iter().flatten().filter(|n| n.fails < 2).collect();
        all.sort_by_key(|node| distance(&node.id, target));
        all.into_iter().take(n).cloned().collect()
    }

    fn all_addrs(&self) -> Vec<SocketAddrV4> {
        self.buckets.iter().flatten().filter(|n| n.fails == 0).map(|n| n.addr).collect()
    }
}

fn encode_nodes(nodes: &[Node]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nodes.len() * 26);
    for n in nodes {
        out.extend_from_slice(&n.id);
        out.extend_from_slice(&n.addr.ip().octets());
        out.extend_from_slice(&n.addr.port().to_be_bytes());
    }
    out
}

fn decode_nodes(b: &[u8]) -> impl Iterator<Item = (NodeId, SocketAddrV4)> + '_ {
    b.chunks_exact(26).filter_map(|c| {
        let id: NodeId = c[..20].try_into().ok()?;
        let ip = std::net::Ipv4Addr::new(c[20], c[21], c[22], c[23]);
        let port = u16::from_be_bytes([c[24], c[25]]);
        (port != 0).then_some((id, SocketAddrV4::new(ip, port)))
    })
}

fn id_of(v: &Value) -> Option<NodeId> {
    v.get("id").and_then(Value::as_bytes).and_then(|b| b.try_into().ok())
}

struct Inner {
    socket: UdpSocket,
    id: NodeId,
    table: Mutex<Table>,
    pending: Mutex<HashMap<u16, oneshot::Sender<Result<Value>>>>,
    tid: AtomicU16,
    secrets: Mutex<([u8; 16], [u8; 16])>,
    store: Mutex<HashMap<NodeId, Vec<(SocketAddrV4, Instant)>>>,
}

#[derive(Clone)]
pub struct Dht {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct LookupResult {
    peers: Vec<SocketAddr>,
    /// Closest responsive nodes with their announce tokens.
    closest: Vec<(SocketAddrV4, Vec<u8>)>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CandState {
    New,
    Asked,
    Responded,
    Failed,
}

impl Dht {
    /// Bind the UDP socket and start the receive + maintenance loops.
    pub async fn start(port: u16, known_nodes: Vec<SocketAddr>) -> Result<Self> {
        let socket = match UdpSocket::bind(("0.0.0.0", port)).await {
            Ok(s) => s,
            Err(e) => {
                debug!("DHT port {port} unavailable ({e}); using ephemeral");
                UdpSocket::bind(("0.0.0.0", 0)).await?
            }
        };
        let id: NodeId = rand::random();
        let inner = Arc::new(Inner {
            socket,
            id,
            table: Mutex::new(Table::new(id)),
            pending: Mutex::new(HashMap::new()),
            tid: AtomicU16::new(rand::random()),
            secrets: Mutex::new((rand::random(), rand::random())),
            store: Mutex::new(HashMap::new()),
        });
        let dht = Self { inner };

        let weak = Arc::downgrade(&dht.inner);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                let Some(inner) = weak.upgrade() else { break };
                let res = inner.socket.recv_from(&mut buf).await;
                match res {
                    Ok((n, SocketAddr::V4(from))) => Dht { inner }.on_packet(&buf[..n], from).await,
                    Ok(_) => {}
                    Err(e) => {
                        debug!("DHT recv error: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        });

        let weak = Arc::downgrade(&dht.inner);
        tokio::spawn(async move {
            let mut seeds = known_nodes;
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            let mut rounds = 0u64;
            loop {
                tick.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                let dht = Dht { inner };
                if dht.node_count() < 32 {
                    dht.bootstrap(std::mem::take(&mut seeds)).await;
                    info!("DHT bootstrapped: {} nodes", dht.node_count());
                }
                rounds += 1;
                if rounds % 5 == 0 {
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

        Ok(dht)
    }

    pub fn port(&self) -> u16 {
        self.inner.socket.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub fn node_count(&self) -> usize {
        self.inner.table.lock().len()
    }

    pub fn known_nodes(&self) -> Vec<SocketAddr> {
        self.inner.table.lock().all_addrs().into_iter().map(SocketAddr::V4).collect()
    }

    /// Seed the routing table from a peer's BEP 5 `port` message.
    pub async fn ping(&self, addr: SocketAddr) {
        let SocketAddr::V4(a) = addr else { return };
        let args = DictBuilder::new().bytes("id", self.inner.id.to_vec()).build();
        let _ = self.query(a, "ping", args).await;
    }

    async fn bootstrap(&self, seeds: Vec<SocketAddr>) {
        let mut targets: Vec<SocketAddrV4> = seeds
            .into_iter()
            .filter_map(|a| match a {
                SocketAddr::V4(a) => Some(a),
                _ => None,
            })
            .collect();
        for host in BOOTSTRAP {
            if let Ok(Ok(addrs)) = tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host(*host)).await {
                targets.extend(addrs.filter_map(|a| match a {
                    SocketAddr::V4(a) => Some(a),
                    _ => None,
                }));
            }
        }
        let own = self.inner.id;
        let mut futs: FuturesUnordered<_> = targets
            .into_iter()
            .take(64)
            .map(|addr| {
                let me = self.clone();
                async move {
                    let args = DictBuilder::new().bytes("id", own.to_vec()).bytes("target", own.to_vec()).build();
                    me.query(addr, "find_node", args).await
                }
            })
            .collect();
        while futs.next().await.is_some() {}
        self.lookup(own, false).await;
    }

    /// Find peers for `info_hash`; when `announce_port` is set, also announce ourselves.
    pub async fn get_peers(&self, info_hash: NodeId, announce_port: Option<u16>) -> Vec<SocketAddr> {
        if self.node_count() == 0 {
            self.bootstrap(Vec::new()).await;
        }
        let res = self.lookup(info_hash, true).await;
        if let Some(port) = announce_port {
            let futs: FuturesUnordered<_> = res
                .closest
                .iter()
                .take(K)
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

    async fn lookup(&self, target: NodeId, get_peers: bool) -> LookupResult {
        let started = Instant::now();
        let mut cands: BTreeMap<NodeId, (NodeId, SocketAddrV4, CandState, Option<Vec<u8>>)> = BTreeMap::new();
        for n in self.inner.table.lock().closest(&target, K * 2) {
            cands.insert(distance(&n.id, &target), (n.id, n.addr, CandState::New, None));
        }
        let mut result = LookupResult::default();
        let mut seen_peers = std::collections::HashSet::new();
        let mut inflight = FuturesUnordered::new();
        let mut asked = 0usize;

        loop {
            // Launch queries to the closest unasked candidates among the K best live ones.
            while inflight.len() < ALPHA && asked < LOOKUP_BUDGET {
                let next = cands
                    .iter_mut()
                    .filter(|(_, c)| c.2 != CandState::Failed)
                    .take(K)
                    .find(|(_, c)| c.2 == CandState::New)
                    .map(|(d, c)| {
                        c.2 = CandState::Asked;
                        (*d, c.1)
                    });
                let Some((dist, addr)) = next else { break };
                asked += 1;
                let me = self.clone();
                let method = if get_peers { "get_peers" } else { "find_node" };
                let mut args = DictBuilder::new().bytes("id", self.inner.id.to_vec());
                args = if get_peers {
                    args.bytes("info_hash", target.to_vec())
                } else {
                    args.bytes("target", target.to_vec())
                };
                let args = args.build();
                inflight.push(async move { (dist, me.query(addr, method, args).await) });
            }
            if inflight.is_empty() || started.elapsed() > LOOKUP_DEADLINE {
                break;
            }
            let Some((dist, resp)) = inflight.next().await else { break };
            match resp {
                Ok(r) => {
                    if let Some(c) = cands.get_mut(&dist) {
                        c.2 = CandState::Responded;
                        c.3 = r.get("token").and_then(Value::as_bytes).map(<[u8]>::to_vec);
                    }
                    if let Some(vals) = r.get("values").and_then(Value::as_list) {
                        for v in vals.iter().filter_map(Value::as_bytes) {
                            for p in crate::tracker::parse_compact_v4(v) {
                                if seen_peers.insert(p) {
                                    result.peers.push(p);
                                }
                            }
                        }
                    }
                    if let Some(nodes) = r.get("nodes").and_then(Value::as_bytes) {
                        for (id, addr) in decode_nodes(nodes) {
                            if id == self.inner.id {
                                continue;
                            }
                            cands.entry(distance(&id, &target)).or_insert((id, addr, CandState::New, None));
                        }
                    }
                }
                Err(_) => {
                    if let Some(c) = cands.get_mut(&dist) {
                        c.2 = CandState::Failed;
                    }
                }
            }
        }

        result.closest = cands
            .values()
            .filter(|c| c.2 == CandState::Responded)
            .filter_map(|c| c.3.clone().map(|t| (c.1, t)))
            .take(K)
            .collect();
        debug!(
            "DHT lookup {} done: {} queries, {} peers, {:?}",
            hex::encode(target),
            asked,
            result.peers.len(),
            started.elapsed()
        );
        result
    }

    async fn query(&self, addr: SocketAddrV4, method: &str, args: Value) -> Result<Value> {
        let tid = self.inner.tid.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().insert(tid, tx);
        let msg = DictBuilder::new()
            .value("a", args)
            .bytes("q", method)
            .bytes("t", tid.to_be_bytes().to_vec())
            .bytes("y", "q")
            .build()
            .encode();
        if let Err(e) = self.inner.socket.send_to(&msg, addr).await {
            self.inner.pending.lock().remove(&tid);
            return Err(e.into());
        }
        let out = match tokio::time::timeout(QUERY_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            _ => Err(Error::Timeout("dht query")),
        };
        self.inner.pending.lock().remove(&tid);
        match &out {
            Ok(r) => {
                if let Some(id) = id_of(r) {
                    self.inner.table.lock().insert(id, addr);
                }
            }
            Err(_) => {
                // We may not know the id (bootstrap routers); match by address.
                let mut t = self.inner.table.lock();
                let id = t.buckets.iter().flatten().find(|n| n.addr == addr).map(|n| n.id);
                if let Some(id) = id {
                    t.failed(&id);
                }
            }
        }
        out
    }

    async fn on_packet(&self, data: &[u8], from: SocketAddrV4) {
        let Ok(msg) = bencode::decode(data) else { return };
        let Some(t) = msg.get("t").and_then(Value::as_bytes) else { return };
        match msg.get("y").and_then(Value::as_str) {
            Some("r") | Some("e") => {
                let Ok(tid) = <[u8; 2]>::try_from(t).map(u16::from_be_bytes) else { return };
                if let Some(tx) = self.inner.pending.lock().remove(&tid) {
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
                    let out = DictBuilder::new().value("r", reply).bytes("t", t).bytes("y", "r").build();
                    let _ = self.inner.socket.send_to(&out.encode(), from).await;
                }
            }
            _ => {}
        }
    }

    fn token_for(&self, secret: &[u8; 16], ip: &std::net::Ipv4Addr) -> Vec<u8> {
        let mut h = Sha1::new();
        h.update(secret);
        h.update(ip.octets());
        h.finalize()[..8].to_vec()
    }

    fn answer(&self, msg: &Value, from: SocketAddrV4) -> Option<Value> {
        let q = msg.get("q")?.as_str()?;
        let a = msg.get("a")?;
        let their_id = id_of(a)?;
        let read_only = a.get("ro").and_then(Value::as_int) == Some(1);
        if !read_only {
            self.inner.table.lock().insert(their_id, from);
        }
        let own = self.inner.id.to_vec();
        let closest = |target: &NodeId| encode_nodes(&self.inner.table.lock().closest(target, K));

        match q {
            "ping" => Some(DictBuilder::new().bytes("id", own).build()),
            "find_node" => {
                let target: NodeId = a.get("target")?.as_bytes()?.try_into().ok()?;
                Some(DictBuilder::new().bytes("id", own).bytes("nodes", closest(&target)).build())
            }
            "get_peers" => {
                let ih: NodeId = a.get("info_hash")?.as_bytes()?.try_into().ok()?;
                let token = self.token_for(&self.inner.secrets.lock().0, from.ip());
                let mut d = DictBuilder::new().bytes("id", own).bytes("token", token).bytes("nodes", closest(&ih));
                if let Some(peers) = self.inner.store.lock().get(&ih) {
                    let vals = peers
                        .iter()
                        .take(50)
                        .map(|(p, _)| {
                            let mut b = Vec::with_capacity(6);
                            b.extend_from_slice(&p.ip().octets());
                            b.extend_from_slice(&p.port().to_be_bytes());
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
                if token != self.token_for(&cur, from.ip()) && token != self.token_for(&prev, from.ip()) {
                    return None;
                }
                let implied = a.get("implied_port").and_then(Value::as_int) == Some(1);
                let port = if implied {
                    from.port()
                } else {
                    a.get("port").and_then(Value::as_int).and_then(|p| u16::try_from(p).ok())?
                };
                let peer = SocketAddrV4::new(*from.ip(), port);
                let mut store = self.inner.store.lock();
                if store.len() < 10_000 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_and_closest() {
        let own = [0u8; 20];
        let mut t = Table::new(own);
        for i in 1..=50u8 {
            let mut id = [0u8; 20];
            id[19] = i;
            t.insert(id, SocketAddrV4::new([10, 0, 0, i].into(), 6881));
        }
        let mut target = [0u8; 20];
        target[19] = 3;
        let c = t.closest(&target, 3);
        assert_eq!(c[0].id[19], 3);
        assert!(t.len() <= 50);
        assert_eq!(bucket_index(&own, &[0x80; 20]), Some(0));
    }

    #[tokio::test]
    async fn two_nodes_find_each_other_and_share_peers() {
        let a = Dht::start(0, vec![]).await.unwrap();
        let b = Dht::start(0, vec![]).await.unwrap();
        let b_addr: SocketAddr = format!("127.0.0.1:{}", b.port()).parse().unwrap();
        a.ping(b_addr).await;
        assert_eq!(a.node_count(), 1);
        let ih = [7u8; 20];
        // a announces into b, then b's store answers a lookup from a fresh node c.
        let _ = a.lookup(ih, true).await;
        let peers = a.get_peers(ih, Some(4242)).await;
        assert!(peers.is_empty());
        let c = Dht::start(0, vec![]).await.unwrap();
        c.ping(b_addr).await;
        let found = c.get_peers(ih, None).await;
        assert_eq!(found, vec!["127.0.0.1:4242".parse::<SocketAddr>().unwrap()]);
    }
}
