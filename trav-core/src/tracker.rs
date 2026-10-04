//! HTTP (BEP 3/23) and UDP (BEP 15) tracker announces.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use bytes::{Buf, BufMut, BytesMut};
use tokio::net::UdpSocket;
use url::Url;

use crate::bencode::{self, Value};
use crate::error::{Error, Result};
use crate::metainfo::InfoHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceEvent {
    None,
    Started,
    Completed,
    Stopped,
}

impl AnnounceEvent {
    fn http(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Started => Some("started"),
            Self::Completed => Some("completed"),
            Self::Stopped => Some("stopped"),
        }
    }

    fn udp(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Completed => 1,
            Self::Started => 2,
            Self::Stopped => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnnounceRequest {
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub event: AnnounceEvent,
    pub num_want: u32,
    pub key: u32,
    pub tracker_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AnnounceResponse {
    pub interval: Duration,
    pub min_interval: Option<Duration>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub peers: Vec<SocketAddr>,
    pub warning: Option<String>,
    pub tracker_id: Option<String>,
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("Trav/", env!("CARGO_PKG_VERSION")))
        .gzip(true)
        .build()
        .unwrap_or_default()
}

pub async fn announce(
    url: &str,
    req: &AnnounceRequest,
    http: &reqwest::Client,
) -> Result<AnnounceResponse> {
    if url.starts_with("udp://") {
        announce_udp(url, req).await
    } else if url.starts_with("http://") || url.starts_with("https://") {
        announce_http(url, req, http).await
    } else {
        Err(Error::Tracker(format!("unsupported tracker scheme: {url}")))
    }
}

fn pct(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

async fn announce_http(
    base: &str,
    req: &AnnounceRequest,
    http: &reqwest::Client,
) -> Result<AnnounceResponse> {
    let sep = if base.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{base}{sep}info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1&no_peer_id=1&numwant={}&key={:08x}",
        pct(&req.info_hash),
        pct(&req.peer_id),
        req.port,
        req.uploaded,
        req.downloaded,
        req.left,
        req.num_want,
        req.key,
    );
    if let Some(ev) = req.event.http() {
        url.push_str("&event=");
        url.push_str(ev);
    }
    if let Some(id) = &req.tracker_id {
        url.push_str("&trackerid=");
        url.push_str(&pct(id.as_bytes()));
    }

    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| Error::Tracker(short_reqwest(&e)))?;
    let status = resp.status();
    let body = resp
        .bytes()
        .await
        .map_err(|e| Error::Tracker(short_reqwest(&e)))?;
    let v = bencode::decode(&body).map_err(|_| {
        if status.is_success() {
            Error::Tracker("malformed tracker response".into())
        } else {
            Error::Tracker(format!("HTTP {status}"))
        }
    })?;
    parse_http_response(&v)
}

fn short_reqwest(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection failed".into()
    } else {
        e.to_string()
    }
}

pub(crate) fn parse_http_response(v: &Value) -> Result<AnnounceResponse> {
    if let Some(f) = v.get("failure reason").and_then(Value::as_string_lossy) {
        return Err(Error::Tracker(f));
    }
    let secs = |k: &str| {
        v.get(k)
            .and_then(Value::as_int)
            .filter(|&i| i > 0)
            .map(|i| Duration::from_secs(i as u64))
    };
    let mut peers = Vec::new();
    match v.get("peers") {
        Some(Value::Bytes(b)) => peers.extend(parse_compact_v4(b)),
        Some(Value::List(list)) => {
            for p in list {
                let ip = p
                    .get("ip")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<IpAddr>().ok());
                let port = p
                    .get("port")
                    .and_then(Value::as_int)
                    .and_then(|p| u16::try_from(p).ok());
                if let (Some(ip), Some(port)) = (ip, port) {
                    peers.push(SocketAddr::new(ip, port));
                }
            }
        }
        _ => {}
    }
    if let Some(b) = v.get("peers6").and_then(Value::as_bytes) {
        peers.extend(parse_compact_v6(b));
    }
    Ok(AnnounceResponse {
        interval: secs("interval").unwrap_or(Duration::from_secs(1800)),
        min_interval: secs("min interval"),
        seeders: v
            .get("complete")
            .and_then(Value::as_int)
            .map(|i| i.max(0) as u32),
        leechers: v
            .get("incomplete")
            .and_then(Value::as_int)
            .map(|i| i.max(0) as u32),
        peers,
        warning: v.get("warning message").and_then(Value::as_string_lossy),
        tracker_id: v.get("tracker id").and_then(Value::as_string_lossy),
    })
}

pub fn parse_compact_v4(b: &[u8]) -> impl Iterator<Item = SocketAddr> + '_ {
    b.chunks_exact(6).filter_map(|c| {
        let port = u16::from_be_bytes([c[4], c[5]]);
        (port != 0).then(|| SocketAddr::new(Ipv4Addr::new(c[0], c[1], c[2], c[3]).into(), port))
    })
}

pub fn parse_compact_v6(b: &[u8]) -> impl Iterator<Item = SocketAddr> + '_ {
    b.chunks_exact(18).filter_map(|c| {
        let ip: [u8; 16] = c[..16].try_into().ok()?;
        let port = u16::from_be_bytes([c[16], c[17]]);
        (port != 0).then(|| SocketAddr::new(Ipv6Addr::from(ip).into(), port))
    })
}

pub fn encode_compact(addr: &SocketAddr, out: &mut Vec<u8>) {
    match addr {
        SocketAddr::V4(a) => out.extend_from_slice(&a.ip().octets()),
        SocketAddr::V6(a) => out.extend_from_slice(&a.ip().octets()),
    }
    out.extend_from_slice(&addr.port().to_be_bytes());
}

const UDP_MAGIC: u64 = 0x0417_2710_1980;

async fn announce_udp(url: &str, req: &AnnounceRequest) -> Result<AnnounceResponse> {
    let parsed = Url::parse(url).map_err(|e| Error::Tracker(format!("bad URL: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| Error::Tracker("missing host".into()))?;
    let port = parsed
        .port()
        .ok_or_else(|| Error::Tracker("missing port".into()))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');

    let addrs: Vec<SocketAddr> = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| Error::Tracker("DNS timed out".into()))?
    .map_err(|_| Error::Tracker("DNS lookup failed".into()))?
    .collect();
    let addr = addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.first())
        .copied()
        .ok_or_else(|| Error::Tracker("no address for host".into()))?;

    let bind: SocketAddr = if addr.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let sock = UdpSocket::bind(bind).await?;
    sock.connect(addr).await?;

    let mut buf = vec![0u8; 4096];

    // BEP 15 retransmission, shortened: 4s, 8s, 12s.
    let conn_id = {
        let mut out = None;
        for attempt in 1..=3u64 {
            let tid: u32 = rand::random();
            let mut pkt = BytesMut::with_capacity(16);
            pkt.put_u64(UDP_MAGIC);
            pkt.put_u32(0);
            pkt.put_u32(tid);
            sock.send(&pkt).await?;
            match recv_matching(&sock, &mut buf, tid, Duration::from_secs(4 * attempt)).await? {
                Some((0, body)) if body.len() >= 8 => {
                    out = Some((&body[..8]).get_u64());
                    break;
                }
                Some((3, body)) => {
                    return Err(Error::Tracker(String::from_utf8_lossy(&body).into_owned()));
                }
                _ => continue,
            }
        }
        out.ok_or(Error::Tracker("timed out".into()))?
    };

    for attempt in 1..=3u64 {
        let tid: u32 = rand::random();
        let mut pkt = BytesMut::with_capacity(98);
        pkt.put_u64(conn_id);
        pkt.put_u32(1);
        pkt.put_u32(tid);
        pkt.put_slice(&req.info_hash);
        pkt.put_slice(&req.peer_id);
        pkt.put_u64(req.downloaded);
        pkt.put_u64(req.left);
        pkt.put_u64(req.uploaded);
        pkt.put_u32(req.event.udp());
        pkt.put_u32(0); // IP: default
        pkt.put_u32(req.key);
        pkt.put_i32(req.num_want.min(i32::MAX as u32) as i32);
        pkt.put_u16(req.port);
        sock.send(&pkt).await?;

        match recv_matching(&sock, &mut buf, tid, Duration::from_secs(4 * attempt)).await? {
            Some((1, body)) if body.len() >= 12 => {
                let mut b = &body[..];
                let interval = b.get_u32();
                let leechers = b.get_u32();
                let seeders = b.get_u32();
                let peers = if addr.is_ipv4() {
                    parse_compact_v4(b).collect()
                } else {
                    parse_compact_v6(b).collect()
                };
                return Ok(AnnounceResponse {
                    interval: Duration::from_secs(interval.max(60) as u64),
                    min_interval: None,
                    seeders: Some(seeders),
                    leechers: Some(leechers),
                    peers,
                    warning: None,
                    tracker_id: None,
                });
            }
            Some((3, body)) => {
                return Err(Error::Tracker(String::from_utf8_lossy(&body).into_owned()));
            }
            _ => continue,
        }
    }
    Err(Error::Tracker("timed out".into()))
}

/// Wait for a datagram carrying `tid`; returns (action, body-after-header).
async fn recv_matching(
    sock: &UdpSocket,
    buf: &mut [u8],
    tid: u32,
    wait: Duration,
) -> Result<Option<(u32, Vec<u8>)>> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let n = match tokio::time::timeout_at(deadline, sock.recv(buf)).await {
            Err(_) => return Ok(None),
            Ok(Err(e)) => return Err(Error::Tracker(format!("udp: {e}"))),
            Ok(Ok(n)) => n,
        };
        if n < 8 {
            continue;
        }
        let mut h = &buf[..8];
        let action = h.get_u32();
        if h.get_u32() == tid {
            return Ok(Some((action, buf[8..n].to_vec())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::DictBuilder;

    #[test]
    fn parses_compact_and_dict_peers() {
        let v = DictBuilder::new()
            .int("interval", 900)
            .int("complete", 5)
            .bytes("peers", vec![127, 0, 0, 1, 0x1a, 0xe1])
            .build();
        let r = parse_http_response(&v).unwrap();
        assert_eq!(r.peers, vec!["127.0.0.1:6881".parse().unwrap()]);
        assert_eq!(r.seeders, Some(5));

        let v = DictBuilder::new()
            .value(
                "peers",
                Value::List(vec![
                    DictBuilder::new()
                        .bytes("ip", "10.0.0.2")
                        .int("port", 51413)
                        .build(),
                ]),
            )
            .build();
        assert_eq!(
            parse_http_response(&v).unwrap().peers,
            vec!["10.0.0.2:51413".parse().unwrap()]
        );

        let f = DictBuilder::new()
            .bytes("failure reason", "unregistered torrent")
            .build();
        assert!(parse_http_response(&f).is_err());
    }
}
