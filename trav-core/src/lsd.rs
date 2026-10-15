//! Local Service Discovery (BEP 14): find peers for our torrents on the
//! local network by multicast, so machines in the same house or office swap
//! data at LAN speed instead of each pulling it through the internet.
//!
//! Every few minutes (and soon after a torrent starts) we multicast which
//! info-hashes we're on; other clients do the same, and each announcement
//! for a torrent we have becomes a peer to dial.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use crate::metainfo::InfoHash;

pub const GROUP: Ipv4Addr = Ipv4Addr::new(239, 192, 152, 143);
pub const PORT: u16 = 6771;

/// One announcement (several info-hashes may share it).
pub fn message(port: u16, hashes: &[InfoHash], cookie: &str) -> String {
    let mut m = format!("BT-SEARCH * HTTP/1.1\r\nHost: {GROUP}:{PORT}\r\nPort: {port}\r\n");
    for h in hashes {
        m.push_str(&format!("Infohash: {}\r\n", hex::encode(h)));
    }
    m.push_str(&format!("cookie: {cookie}\r\n\r\n\r\n"));
    m
}

#[derive(Debug, PartialEq, Eq)]
pub struct Announce {
    pub port: u16,
    pub hashes: Vec<InfoHash>,
    pub cookie: Option<String>,
}

pub fn parse(msg: &[u8]) -> Option<Announce> {
    let text = std::str::from_utf8(msg).ok()?;
    let mut lines = text.split("\r\n");
    if !lines.next()?.starts_with("BT-SEARCH * HTTP/1.1") {
        return None;
    }
    let (mut port, mut hashes, mut cookie) = (None, Vec::new(), None);
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "port" => port = v.parse::<u16>().ok().filter(|&p| p != 0),
            "infohash" => {
                if let Some(h) = hex::decode(v)
                    .ok()
                    .and_then(|b| <InfoHash>::try_from(b).ok())
                    && hashes.len() < 64
                {
                    hashes.push(h);
                }
            }
            "cookie" => cookie = Some(v.to_string()),
            _ => {}
        }
    }
    Some(Announce {
        port: port?,
        hashes,
        cookie,
    })
}

/// The multicast socket, shared with other clients on this machine.
pub fn bind() -> std::io::Result<tokio::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(unix)]
    s.set_reuse_port(true)?;
    s.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    s.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    s.set_multicast_loop_v4(true)?;
    s.set_multicast_ttl_v4(1)?;
    s.set_nonblocking(true)?;
    tokio::net::UdpSocket::from_std(std::net::UdpSocket::from(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let m = message(51413, &[[0xab; 20], [1; 20]], "c00k1e");
        assert!(m.starts_with("BT-SEARCH * HTTP/1.1\r\nHost: 239.192.152.143:6771\r\n"));
        assert!(m.ends_with("\r\n\r\n\r\n"));
        let a = parse(m.as_bytes()).unwrap();
        assert_eq!(a.port, 51413);
        assert_eq!(a.hashes, vec![[0xab; 20], [1; 20]]);
        assert_eq!(a.cookie.as_deref(), Some("c00k1e"));
    }

    #[test]
    fn lenient_about_case_and_rejects_junk() {
        let a = parse(b"BT-SEARCH * HTTP/1.1\r\nhost: x\r\nPORT:6881\r\nINFOHASH: ABABABABABABABABABABABABABABABABABABABAB\r\n\r\n").unwrap();
        assert_eq!((a.port, a.hashes.len(), a.cookie), (6881, 1, None));
        assert!(parse(b"NOTIFY * HTTP/1.1\r\nPort: 1\r\n").is_none());
        assert!(parse(b"BT-SEARCH * HTTP/1.1\r\nInfohash: zz\r\n").is_none());
        assert!(parse(&[0xff, 0xfe]).is_none());
    }
}
