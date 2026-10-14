//! The µTP socket: one UDP socket on the listen port, shared by every µTP
//! connection and the DHT and split by the first byte of each datagram (µTP
//! headers start with a version nibble of 1, DHT messages with `d`).
//!
//! A dispatcher task reads the socket and routes µTP packets by (address,
//! connection id) to per-connection driver tasks, which own the timers and
//! do the sending. Streams and drivers share the connection state under a
//! mutex.

use std::collections::HashMap;
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::conn::{Conn, Dial};
use super::packet::{self, Header, Type};

const MAX_CONNECTIONS: usize = 4000;
const ACCEPT_BACKLOG: usize = 128;
/// Datagrams queued for one connection before we start dropping them.
const INBOX: usize = 2048;
/// Datagrams handled per wakeup before the driver flushes (acks go out per batch).
const BATCH: usize = 64;
const RESETS_PER_SEC: u32 = 50;

struct Datagram {
    data: Bytes,
    at: Instant,
}

fn canonical(a: SocketAddr) -> SocketAddr {
    SocketAddr::new(a.ip().to_canonical(), a.port())
}

/// Sends on the shared socket, addressing IPv4 peers through a dual-stack one.
#[derive(Clone)]
pub struct UdpSender {
    socket: Arc<UdpSocket>,
    v6: bool,
}

impl UdpSender {
    pub fn new(socket: Arc<UdpSocket>) -> Self {
        let v6 = socket.local_addr().is_ok_and(|a| a.is_ipv6());
        Self { socket, v6 }
    }

    fn map(&self, addr: SocketAddr) -> SocketAddr {
        match addr {
            SocketAddr::V4(a) if self.v6 => {
                SocketAddr::new(a.ip().to_ipv6_mapped().into(), a.port())
            }
            a => a,
        }
    }

    pub async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<usize> {
        self.socket.send_to(buf, self.map(addr)).await
    }

    fn try_send_to(&self, buf: &[u8], addr: SocketAddr) -> io::Result<usize> {
        self.socket.try_send_to(buf, self.map(addr))
    }

    pub fn local_port(&self) -> u16 {
        self.socket.local_addr().map(|a| a.port()).unwrap_or(0)
    }
}

type Key = (SocketAddr, u16);
/// Non-µTP datagrams (DHT), with their sender.
pub type Other = (Bytes, SocketAddr);

struct Inner {
    udp: UdpSender,
    epoch: Instant,
    conns: Mutex<HashMap<Key, mpsc::Sender<Datagram>>>,
    accept_tx: mpsc::Sender<(UtpStream, SocketAddr)>,
    accept_rx: tokio::sync::Mutex<mpsc::Receiver<(UtpStream, SocketAddr)>>,
    other: Mutex<Option<mpsc::Sender<Other>>>,
    accepting: AtomicBool,
    cancel: CancellationToken,
    resets: Mutex<(Instant, u32)>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Handle to the shared socket (cheap to clone).
#[derive(Clone)]
pub struct UtpSocket {
    inner: Arc<Inner>,
}

impl UtpSocket {
    /// Bind `port` on every interface: IPv6 and IPv4 on one socket where the
    /// system allows, IPv4 alone otherwise.
    pub async fn bind(port: u16) -> io::Result<Self> {
        let socket = match bind_dual_stack(port) {
            Some(s) => s,
            None => UdpSocket::bind(("0.0.0.0", port)).await?,
        };
        Ok(Self::from_socket(socket))
    }

    fn from_socket(socket: UdpSocket) -> Self {
        let socket = Arc::new(socket);
        let (accept_tx, accept_rx) = mpsc::channel(ACCEPT_BACKLOG);
        let inner = Arc::new(Inner {
            udp: UdpSender::new(socket.clone()),
            epoch: Instant::now(),
            conns: Mutex::new(HashMap::new()),
            accept_tx,
            accept_rx: tokio::sync::Mutex::new(accept_rx),
            other: Mutex::new(None),
            accepting: AtomicBool::new(true),
            cancel: CancellationToken::new(),
            resets: Mutex::new((Instant::now(), 0)),
        });
        tokio::spawn(dispatch(
            Arc::downgrade(&inner),
            socket,
            inner.cancel.clone(),
        ));
        Self { inner }
    }

    pub fn local_port(&self) -> u16 {
        self.inner.udp.local_port()
    }

    /// For other protocols on the same port (the DHT).
    pub fn sender(&self) -> UdpSender {
        self.inner.udp.clone()
    }

    /// Where datagrams that aren't µTP go (or `None` to drop them).
    pub fn set_other(&self, tx: Option<mpsc::Sender<Other>>) {
        *self.inner.other.lock() = tx;
    }

    /// Whether to take incoming µTP connections (refused with a reset if not).
    pub fn set_accepting(&self, on: bool) {
        self.inner.accepting.store(on, Ordering::Relaxed);
    }

    pub fn connections(&self) -> usize {
        self.inner.conns.lock().len()
    }

    /// Stop everything: the socket closes once the tasks notice.
    pub fn close(&self) {
        self.inner.cancel.cancel();
    }

    pub async fn connect(&self, addr: SocketAddr, dial: Dial) -> io::Result<UtpStream> {
        let addr = canonical(addr);
        if self.inner.cancel.is_cancelled() {
            return Err(io::ErrorKind::NotConnected.into());
        }
        let stream = {
            let mut conns = self.inner.conns.lock();
            if conns.len() >= MAX_CONNECTIONS {
                return Err(io::Error::other("too many µTP connections"));
            }
            let id = loop {
                let id: u16 = rand::random();
                if !conns.contains_key(&(addr, id)) {
                    break id;
                }
            };
            let conn = Conn::connect(id, self.inner.epoch, Instant::now(), dial);
            register(&self.inner, &mut conns, (addr, id), conn, addr)
        };
        poll_fn(|cx| {
            let mut slot = stream.shared.slot.lock();
            if let Some(e) = slot.conn.error() {
                return Poll::Ready(Err(io::Error::from(e)));
            }
            if slot.conn.connected() {
                return Poll::Ready(Ok(()));
            }
            slot.write_waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await?;
        Ok(stream)
    }

    /// The next incoming connection (`None` once the socket is closed).
    pub async fn accept(&self) -> Option<(UtpStream, SocketAddr)> {
        let mut rx = self.inner.accept_rx.lock().await;
        tokio::select! {
            _ = self.inner.cancel.cancelled() => None,
            r = rx.recv() => r,
        }
    }
}

fn bind_dual_stack(port: u16) -> Option<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)).ok()?;
    s.set_only_v6(false).ok()?;
    s.bind(&SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())
        .ok()?;
    // Room for bursts at line rate while the dispatcher catches up.
    let _ = s.set_recv_buffer_size(4 << 20);
    let _ = s.set_send_buffer_size(2 << 20);
    s.set_nonblocking(true).ok()?;
    UdpSocket::from_std(std::net::UdpSocket::from(s)).ok()
}

fn register(
    inner: &Arc<Inner>,
    conns: &mut HashMap<Key, mpsc::Sender<Datagram>>,
    key: Key,
    conn: Conn,
    remote: SocketAddr,
) -> UtpStream {
    let (tx, rx) = mpsc::channel(INBOX);
    let shared = Arc::new(Shared {
        slot: Mutex::new(Slot {
            conn,
            read_waker: None,
            write_waker: None,
        }),
        notify: Notify::new(),
    });
    conns.insert(key, tx);
    tokio::spawn(drive(inner.clone(), shared.clone(), key, remote, rx));
    UtpStream { shared, remote }
}

async fn dispatch(inner: Weak<Inner>, socket: Arc<UdpSocket>, cancel: CancellationToken) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let r = tokio::select! {
            _ = cancel.cancelled() => break,
            r = socket.recv_from(&mut buf) => r,
        };
        let (n, from) = match r {
            Ok(x) => x,
            Err(e) => {
                // ICMP errors surface here on some systems; the socket is fine.
                debug!("µTP socket: {e}");
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
        };
        let at = Instant::now();
        let Some(inner) = inner.upgrade() else { break };
        let from = canonical(from);
        let data = &buf[..n];
        match packet::parse(data) {
            Some(p) => inner.route(p.h, data, from, at),
            None if data.first() == Some(&b'd') => {
                if let Some(tx) = inner.other.lock().as_ref() {
                    let _ = tx.try_send((Bytes::copy_from_slice(data), from));
                }
            }
            None => {}
        }
    }
}

impl Inner {
    fn route(self: &Arc<Self>, h: Header, data: &[u8], from: SocketAddr, at: Instant) {
        let dg = || Datagram {
            data: Bytes::copy_from_slice(data),
            at,
        };
        let mut conns = self.conns.lock();
        match h.ty {
            Type::Syn => {
                if let Some(tx) = conns.get(&(from, h.conn_id.wrapping_add(1))) {
                    let _ = tx.try_send(dg());
                    return;
                }
                if !self.accepting.load(Ordering::Relaxed) || conns.len() >= MAX_CONNECTIONS {
                    drop(conns);
                    self.reset(from, &h);
                    return;
                }
                let conn = Conn::accept(&h, self.epoch, at);
                let key = (from, conn.recv_id);
                let stream = register(self, &mut conns, key, conn, from);
                drop(conns);
                if let Err(e) = self.accept_tx.try_send((stream, from)) {
                    let (stream, _) = e.into_inner();
                    stream.abort();
                }
            }
            Type::Reset => {
                // The id may be either of ours, depending on who sent it.
                for id in [
                    h.conn_id,
                    h.conn_id.wrapping_sub(1),
                    h.conn_id.wrapping_add(1),
                ] {
                    if let Some(tx) = conns.get(&(from, id)) {
                        let _ = tx.try_send(dg());
                        return;
                    }
                }
            }
            _ => {
                if let Some(tx) = conns.get(&(from, h.conn_id)) {
                    let _ = tx.try_send(dg());
                    return;
                }
                drop(conns);
                self.reset(from, &h);
            }
        }
    }

    /// Tell a peer it's talking to a connection we don't have (rate limited).
    fn reset(&self, to: SocketAddr, h: &Header) {
        {
            let mut r = self.resets.lock();
            let now = Instant::now();
            if now.duration_since(r.0) >= Duration::from_secs(1) {
                *r = (now, 0);
            }
            if r.1 >= RESETS_PER_SEC {
                return;
            }
            r.1 += 1;
        }
        let rst = Header {
            ty: Type::Reset,
            conn_id: h.conn_id,
            ts: Instant::now()
                .saturating_duration_since(self.epoch)
                .as_micros() as u32,
            ts_diff: 0,
            wnd: 0,
            seq: rand::random(),
            ack: h.seq,
        };
        let mut buf = Vec::with_capacity(packet::HEADER_LEN);
        packet::encode(&rst, None, &[], &mut buf);
        let _ = self.udp.try_send_to(&buf, to);
    }
}

struct Slot {
    conn: Conn,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
}

impl Slot {
    fn wakers(&mut self) -> [Option<Waker>; 2] {
        [
            self.conn
                .readable()
                .then(|| self.read_waker.take())
                .flatten(),
            self.conn
                .writable()
                .then(|| self.write_waker.take())
                .flatten(),
        ]
    }
}

struct Shared {
    slot: Mutex<Slot>,
    /// Wakes the driver: the application wrote, read or closed.
    notify: Notify,
}

async fn drive(
    inner: Arc<Inner>,
    shared: Arc<Shared>,
    key: Key,
    remote: SocketAddr,
    mut rx: mpsc::Receiver<Datagram>,
) {
    let mut out: Vec<Vec<u8>> = Vec::new();
    loop {
        let (deadline, finished, wake) = {
            let mut slot = shared.slot.lock();
            slot.conn.flush(Instant::now(), &mut out);
            (
                slot.conn.next_timeout(),
                slot.conn.finished(),
                slot.wakers(),
            )
        };
        for w in wake.into_iter().flatten() {
            w.wake();
        }
        for dg in out.drain(..) {
            // A send error is as good as a lost packet: timers retry.
            let _ = inner.udp.send_to(&dg, remote).await;
        }
        if finished {
            break;
        }
        let sleep = async move {
            match deadline {
                Some(d) => tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = inner.cancel.cancelled() => {
                shared.slot.lock().conn.abort();
            }
            d = rx.recv() => {
                let Some(d) = d else { break };
                let mut slot = shared.slot.lock();
                slot.conn.on_datagram(&d.data, d.at);
                for _ in 1..BATCH {
                    match rx.try_recv() {
                        Ok(d) => slot.conn.on_datagram(&d.data, d.at),
                        Err(_) => break,
                    }
                }
            }
            _ = shared.notify.notified() => {}
            _ = sleep => {
                shared.slot.lock().conn.on_timeout(Instant::now());
            }
        }
    }
    inner.conns.lock().remove(&key);
}

/// One µTP connection, as a byte stream.
pub struct UtpStream {
    shared: Arc<Shared>,
    remote: SocketAddr,
}

/// Reads a connection's round-trip estimate after the stream itself has
/// been moved into a codec.
#[derive(Clone)]
pub struct UtpProbe(Arc<Shared>);

impl UtpProbe {
    pub fn srtt(&self) -> Option<Duration> {
        self.0.slot.lock().conn.srtt()
    }
}

impl std::fmt::Debug for UtpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UtpStream")
            .field("remote", &self.remote)
            .finish()
    }
}

impl UtpStream {
    pub fn peer_addr(&self) -> SocketAddr {
        self.remote
    }

    pub fn probe(&self) -> UtpProbe {
        UtpProbe(self.shared.clone())
    }

    /// Hang up at once (a reset rather than a graceful close).
    pub fn abort(&self) {
        self.shared.slot.lock().conn.abort();
        self.shared.notify.notify_one();
    }
}

impl Drop for UtpStream {
    fn drop(&mut self) {
        self.shared.slot.lock().conn.drop_handle();
        self.shared.notify.notify_one();
    }
}

impl AsyncRead for UtpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut slot = self.shared.slot.lock();
        match slot.conn.read(buf) {
            Some(r) => {
                let update = slot.conn.needs_ack();
                drop(slot);
                if update {
                    self.shared.notify.notify_one();
                }
                Poll::Ready(r)
            }
            None => {
                slot.read_waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl AsyncWrite for UtpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut slot = self.shared.slot.lock();
        match slot.conn.write(data) {
            Some(r) => {
                drop(slot);
                self.shared.notify.notify_one();
                Poll::Ready(r)
            }
            None => {
                slot.write_waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Queued bytes go out as fast as the window allows; nothing to wait for.
        match self.shared.slot.lock().conn.error() {
            Some(e) => Poll::Ready(Err(e.into())),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.shared.slot.lock().conn.close();
        self.shared.notify.notify_one();
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn pair() -> (UtpSocket, UtpSocket, SocketAddr) {
        let a = UtpSocket::bind(0).await.unwrap();
        let b = UtpSocket::bind(0).await.unwrap();
        let addr: SocketAddr = format!("127.0.0.1:{}", b.local_port()).parse().unwrap();
        (a, b, addr)
    }

    #[tokio::test]
    async fn connect_send_close() {
        let (a, b, addr) = pair().await;
        let server = tokio::spawn(async move {
            let (mut s, _) = b.accept().await.unwrap();
            let mut got = Vec::new();
            s.read_to_end(&mut got).await.unwrap();
            s.write_all(&(got.len() as u64).to_be_bytes())
                .await
                .unwrap();
            s.shutdown().await.unwrap();
            got
        });
        let mut c = a.connect(addr, Dial::Normal).await.unwrap();
        let payload: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        c.write_all(&payload).await.unwrap();
        c.shutdown().await.unwrap();
        let mut len = [0u8; 8];
        c.read_exact(&mut len).await.unwrap();
        assert_eq!(u64::from_be_bytes(len), payload.len() as u64);
        let mut rest = Vec::new();
        c.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        assert_eq!(server.await.unwrap(), payload);
    }

    #[tokio::test]
    async fn many_streams_both_directions() {
        let (a, b, addr) = pair().await;
        let server = tokio::spawn(async move {
            let mut tasks = Vec::new();
            for _ in 0..20 {
                let (mut s, _) = b.accept().await.unwrap();
                tasks.push(tokio::spawn(async move {
                    let mut buf = vec![0u8; 200_000];
                    s.read_exact(&mut buf).await.unwrap();
                    s.write_all(&buf).await.unwrap();
                    buf
                }));
            }
            for t in tasks {
                t.await.unwrap();
            }
        });
        let mut clients = Vec::new();
        for i in 0..20u8 {
            let a = a.clone();
            clients.push(tokio::spawn(async move {
                let mut s = a.connect(addr, Dial::Normal).await.unwrap();
                let data = vec![i; 200_000];
                s.write_all(&data).await.unwrap();
                let mut back = vec![0u8; 200_000];
                s.read_exact(&mut back).await.unwrap();
                assert_eq!(back, data);
            }));
        }
        for c in clients {
            c.await.unwrap();
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn refused_when_not_accepting() {
        let (a, b, addr) = pair().await;
        b.set_accepting(false);
        let e = a.connect(addr, Dial::Normal).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
    }

    #[tokio::test]
    async fn dht_datagrams_pass_through() {
        let (a, b, addr) = pair().await;
        let (tx, mut rx) = mpsc::channel(8);
        b.set_other(Some(tx));
        a.sender().send_to(b"d1:y1:qe", addr).await.unwrap();
        let (data, from) = rx.recv().await.unwrap();
        assert_eq!(&data[..], b"d1:y1:qe");
        assert_eq!(from.port(), a.local_port());
        assert!(from.is_ipv4(), "{from}");
    }

    #[tokio::test]
    async fn dropping_the_handles_frees_the_port() {
        let a = UtpSocket::bind(0).await.unwrap();
        let port = a.local_port();
        drop(a);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let b = UtpSocket::bind(port).await.unwrap();
        assert_eq!(b.local_port(), port);
    }
}
