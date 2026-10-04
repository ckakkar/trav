use std::net::SocketAddr;

use url::Url;

use crate::error::{Error, Result};
use crate::metainfo::InfoHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Magnet {
    pub info_hash: InfoHash,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
    /// BEP 9 `x.pe` direct peer hints.
    pub peers: Vec<SocketAddr>,
}

/// Back-compat alias for the pre-0.2 name.
pub type MagnetUri = Magnet;

impl Magnet {
    pub fn parse(uri: &str) -> Result<Self> {
        let bad = |m: String| Error::Engine(format!("invalid magnet link: {m}"));
        let url = Url::parse(uri.trim()).map_err(|e| bad(e.to_string()))?;
        if url.scheme() != "magnet" {
            return Err(bad("scheme must be magnet:".into()));
        }

        let mut info_hash = None;
        let mut display_name = None;
        let mut trackers = Vec::new();
        let mut peers = Vec::new();

        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                k if k == "xt" || k.starts_with("xt.") => {
                    if let Some(h) = value.strip_prefix("urn:btih:") {
                        info_hash = Some(parse_btih(h).ok_or_else(|| bad(format!("bad btih '{h}'")))?);
                    }
                }
                "dn" => display_name = Some(value.into_owned()),
                k if k == "tr" || k.starts_with("tr.") => trackers.push(value.into_owned()),
                "x.pe" => {
                    if let Ok(a) = value.parse::<SocketAddr>() {
                        peers.push(a);
                    }
                }
                _ => {}
            }
        }

        let info_hash = info_hash.ok_or_else(|| bad("missing xt=urn:btih:".into()))?;
        Ok(Self { info_hash, display_name, trackers, peers })
    }

    pub fn to_uri(&self) -> String {
        let mut s = format!("magnet:?xt=urn:btih:{}", hex::encode(self.info_hash));
        if let Some(n) = &self.display_name {
            s.push_str("&dn=");
            s.push_str(&urlencoding::encode(n));
        }
        for t in &self.trackers {
            s.push_str("&tr=");
            s.push_str(&urlencoding::encode(t));
        }
        for p in &self.peers {
            s.push_str("&x.pe=");
            s.push_str(&p.to_string());
        }
        s
    }
}

fn parse_btih(s: &str) -> Option<InfoHash> {
    match s.len() {
        40 => hex::decode(s).ok()?.try_into().ok(),
        32 => base32_decode(s)?.try_into().ok(),
        _ => None,
    }
}

/// RFC 4648 base32 without padding (µTorrent-era magnet links).
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut buf, mut bits) = (0u64, 0u32);
    for c in s.bytes() {
        let v = match c.to_ascii_uppercase() {
            c @ b'A'..=b'Z' => c - b'A',
            c @ b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        buf = (buf << 5) | v as u64;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_base32_agree() {
        let hex = Magnet::parse("magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=Foo%20Bar&tr=udp%3A%2F%2Ft%3A1&x.pe=127.0.0.1:6881")
            .unwrap();
        assert_eq!(hex.display_name.as_deref(), Some("Foo Bar"));
        assert_eq!(hex.trackers, vec!["udp://t:1"]);
        assert_eq!(hex.peers.len(), 1);
        let b32 = Magnet::parse("magnet:?xt=urn:btih:YEX6DQDLXISUVHOJ6UM3GNNKPQJWPKEK").unwrap();
        assert_eq!(b32.info_hash, hex.info_hash);
        let back = Magnet::parse(&hex.to_uri()).unwrap();
        assert_eq!(back, hex);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Magnet::parse("http://x").is_err());
        assert!(Magnet::parse("magnet:?dn=x").is_err());
        assert!(Magnet::parse("magnet:?xt=urn:btih:zz").is_err());
    }
}
