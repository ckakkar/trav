//! BEP 10 extension protocol with ut_metadata (BEP 9) and ut_pex (BEP 11).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use bytes::Bytes;

use crate::bencode::{self, DictBuilder, Value};
use crate::error::{Result, proto};
use crate::tracker::{encode_compact, parse_compact_v4, parse_compact_v6};

/// Our local extension ids (what peers must use when messaging us).
pub const UT_METADATA: u8 = 1;
pub const UT_PEX: u8 = 2;
pub const METADATA_PIECE: usize = 16 * 1024;
/// Refuse absurd metadata sizes advertised by peers (64 MiB covers huge torrents).
pub const MAX_METADATA_SIZE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct ExtHandshake {
    pub m: HashMap<String, u8>,
    pub metadata_size: Option<usize>,
    pub client: Option<String>,
    pub reqq: Option<u32>,
    pub listen_port: Option<u16>,
    pub your_ip: Option<IpAddr>,
}

impl ExtHandshake {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let v = bencode::decode(payload)?;
        let mut m = HashMap::new();
        if let Some(d) = v.get("m").and_then(Value::as_dict) {
            for (k, id) in d {
                if let (Ok(name), Some(id)) = (std::str::from_utf8(k), id.as_int())
                    && (1..=255).contains(&id)
                {
                    m.insert(name.to_string(), id as u8);
                }
            }
        }
        let your_ip = v
            .get("yourip")
            .and_then(Value::as_bytes)
            .and_then(|b| match b.len() {
                4 => Some(IpAddr::from(<[u8; 4]>::try_from(b).ok()?)),
                16 => Some(IpAddr::from(<[u8; 16]>::try_from(b).ok()?)),
                _ => None,
            });
        Ok(Self {
            m,
            metadata_size: v
                .get("metadata_size")
                .and_then(Value::as_int)
                .filter(|&s| s > 0 && s as usize <= MAX_METADATA_SIZE)
                .map(|s| s as usize),
            client: v.get("v").and_then(Value::as_string_lossy),
            reqq: v
                .get("reqq")
                .and_then(Value::as_int)
                .filter(|&r| r > 0)
                .map(|r| r.min(2000) as u32),
            listen_port: v
                .get("p")
                .and_then(Value::as_int)
                .and_then(|p| u16::try_from(p).ok())
                .filter(|&p| p != 0),
            your_ip,
        })
    }

    pub fn ours(
        metadata_size: Option<usize>,
        listen_port: u16,
        peer_ip: IpAddr,
        pex: bool,
    ) -> Bytes {
        let mut m = DictBuilder::new().int("ut_metadata", UT_METADATA as i64);
        if pex {
            m = m.int("ut_pex", UT_PEX as i64);
        }
        let ip = match peer_ip {
            IpAddr::V4(v) => v.octets().to_vec(),
            IpAddr::V6(v) => v.octets().to_vec(),
        };
        let mut d = DictBuilder::new()
            .value("m", m.build())
            .int("p", listen_port as i64)
            .int("reqq", 500)
            .bytes("v", format!("Trav {}", env!("CARGO_PKG_VERSION")))
            .bytes("yourip", ip);
        if let Some(s) = metadata_size {
            d = d.int("metadata_size", s as i64);
        }
        Bytes::from(d.build().encode())
    }
}

#[derive(Debug)]
pub enum MetadataMsg {
    Request {
        piece: u32,
    },
    Data {
        piece: u32,
        total_size: usize,
        data: Bytes,
    },
    Reject {
        piece: u32,
    },
}

impl MetadataMsg {
    pub fn decode(payload: &Bytes) -> Result<Self> {
        let (v, used) = bencode::decode_prefix(payload)?;
        let ty = v
            .get("msg_type")
            .and_then(Value::as_int)
            .ok_or_else(|| proto("ut_metadata without msg_type"))?;
        let piece = v
            .get("piece")
            .and_then(Value::as_int)
            .and_then(|p| u32::try_from(p).ok())
            .ok_or_else(|| proto("ut_metadata without piece"))?;
        Ok(match ty {
            0 => Self::Request { piece },
            1 => Self::Data {
                piece,
                total_size: v
                    .get("total_size")
                    .and_then(Value::as_int)
                    .unwrap_or(0)
                    .max(0) as usize,
                data: payload.slice(used..),
            },
            _ => Self::Reject { piece },
        })
    }

    pub fn encode(&self) -> Bytes {
        let mut out = match self {
            Self::Request { piece } => DictBuilder::new()
                .int("msg_type", 0)
                .int("piece", *piece as i64),
            Self::Data {
                piece, total_size, ..
            } => DictBuilder::new()
                .int("msg_type", 1)
                .int("piece", *piece as i64)
                .int("total_size", *total_size as i64),
            Self::Reject { piece } => DictBuilder::new()
                .int("msg_type", 2)
                .int("piece", *piece as i64),
        }
        .build()
        .encode();
        if let Self::Data { data, .. } = self {
            out.extend_from_slice(data);
        }
        Bytes::from(out)
    }
}

#[derive(Debug, Default)]
pub struct PexMsg {
    pub added: Vec<SocketAddr>,
    pub dropped: Vec<SocketAddr>,
}

impl PexMsg {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let v = bencode::decode(payload)?;
        let mut added = Vec::new();
        let mut dropped = Vec::new();
        if let Some(b) = v.get("added").and_then(Value::as_bytes) {
            added.extend(parse_compact_v4(b));
        }
        if let Some(b) = v.get("added6").and_then(Value::as_bytes) {
            added.extend(parse_compact_v6(b));
        }
        if let Some(b) = v.get("dropped").and_then(Value::as_bytes) {
            dropped.extend(parse_compact_v4(b));
        }
        added.truncate(200);
        Ok(Self { added, dropped })
    }

    pub fn encode(&self) -> Bytes {
        let (mut a4, mut a6, mut d4, mut d6) = (vec![], vec![], vec![], vec![]);
        for a in &self.added {
            encode_compact(a, if a.is_ipv4() { &mut a4 } else { &mut a6 });
        }
        for a in &self.dropped {
            encode_compact(a, if a.is_ipv4() { &mut d4 } else { &mut d6 });
        }
        let flags4 = vec![0u8; a4.len() / 6];
        let flags6 = vec![0u8; a6.len() / 18];
        Bytes::from(
            DictBuilder::new()
                .bytes("added", a4)
                .bytes("added.f", flags4)
                .bytes("added6", a6)
                .bytes("added6.f", flags6)
                .bytes("dropped", d4)
                .bytes("dropped6", d6)
                .build()
                .encode(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_roundtrip() {
        let raw = ExtHandshake::ours(Some(1234), 6881, "1.2.3.4".parse().unwrap(), true);
        let hs = ExtHandshake::decode(&raw).unwrap();
        assert_eq!(hs.m.get("ut_metadata"), Some(&UT_METADATA));
        assert_eq!(hs.metadata_size, Some(1234));
        assert_eq!(hs.your_ip, Some("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn metadata_roundtrip() {
        let m = MetadataMsg::Data {
            piece: 2,
            total_size: 40000,
            data: Bytes::from_static(b"xyz"),
        };
        match MetadataMsg::decode(&m.encode()).unwrap() {
            MetadataMsg::Data {
                piece,
                total_size,
                data,
            } => {
                assert_eq!((piece, total_size, &data[..]), (2, 40000, &b"xyz"[..]));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pex_roundtrip() {
        let p = PexMsg {
            added: vec!["1.2.3.4:5".parse().unwrap(), "[::1]:7".parse().unwrap()],
            dropped: vec![],
        };
        let d = PexMsg::decode(&p.encode()).unwrap();
        assert_eq!(d.added, p.added);
    }
}
