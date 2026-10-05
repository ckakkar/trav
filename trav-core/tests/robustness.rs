//! Hostile-input robustness: every parser that sees network or file input must
//! return an error, never panic, on arbitrary bytes. Deterministic so failures
//! reproduce; covers structured mutations of valid inputs, not just noise.

use bytes::{Bytes, BytesMut};
use tokio_util::codec::Decoder;
use trav_core::bencode::{self, DictBuilder, Value};
use trav_core::magnet::Magnet;
use trav_core::metainfo::{Info, Metainfo};
use trav_core::peer::extension::{ExtHandshake, MetadataMsg, PexMsg};
use trav_core::peer::protocol::PeerCodec;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

fn valid_torrent() -> Vec<u8> {
    let files = Value::List(vec![
        DictBuilder::new()
            .int("length", 40_000)
            .value("path", Value::List(vec![Value::Bytes(b"a.bin".to_vec())]))
            .build(),
        DictBuilder::new()
            .int("length", 10)
            .value(
                "path",
                Value::List(vec![
                    Value::Bytes(b"dir".to_vec()),
                    Value::Bytes(b"b".to_vec()),
                ]),
            )
            .build(),
    ]);
    let info = DictBuilder::new()
        .bytes("name", "x")
        .int("piece length", 16384)
        .bytes("pieces", vec![0u8; 60])
        .value("files", files)
        .build();
    DictBuilder::new()
        .bytes("announce", "udp://t:1")
        .value("info", info)
        .build()
        .encode()
}

/// Random byte flips, truncations, insertions and duplications of `seed`.
fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut v = seed.to_vec();
    for _ in 0..=rng.below(4) {
        match rng.below(5) {
            0 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = rng.next() as u8;
            }
            1 => v.truncate(rng.below(v.len() + 1)),
            2 => {
                let i = rng.below(v.len() + 1);
                let n = rng.below(8) + 1;
                let junk = rng.bytes(n);
                v.splice(i..i, junk);
            }
            3 if !v.is_empty() => {
                let i = rng.below(v.len());
                let digit = b"0123456789"[rng.below(10)];
                v[i] = digit;
            }
            _ => {
                let i = rng.below(v.len() + 1);
                let tail = v[i..].to_vec();
                v.extend_from_slice(&tail);
            }
        }
    }
    v
}

#[test]
fn bencode_and_metainfo_never_panic() {
    let seed = valid_torrent();
    assert!(Metainfo::from_bytes(&seed).is_ok());
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for _ in 0..20_000 {
        let input = if rng.below(4) == 0 {
            let n = rng.below(256);
            rng.bytes(n)
        } else {
            mutate(&mut rng, &seed)
        };
        let _ = bencode::decode(&input);
        let _ = bencode::decode_prefix(&input);
        let _ = bencode::raw_value_span(&input, b"info");
        let _ = Metainfo::from_bytes(&input);
        let _ = Info::from_bytes(&input);
    }
}

#[test]
fn peer_wire_decoder_never_panics() {
    let mut rng = Rng(42);
    for _ in 0..20_000 {
        let mut buf = BytesMut::new();
        // Plausible length prefix + random ids/payload to reach every arm.
        let len = rng.below(40) as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        let n = rng.below(48);
        buf.extend_from_slice(&rng.bytes(n));
        let mut codec = PeerCodec;
        for _ in 0..8 {
            match codec.decode(&mut buf) {
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
    }
}

#[test]
fn extension_messages_never_panic() {
    let seeds = [
        ExtHandshake::ours(Some(1234), 6881, "1.2.3.4".parse().unwrap(), true).to_vec(),
        MetadataMsg::Data {
            piece: 1,
            total_size: 30000,
            data: Bytes::from_static(b"abc"),
        }
        .encode()
        .to_vec(),
        PexMsg {
            added: vec!["1.2.3.4:5".parse().unwrap()],
            dropped: vec![],
        }
        .encode()
        .to_vec(),
    ];
    let mut rng = Rng(7);
    for _ in 0..20_000 {
        let seed = &seeds[rng.below(seeds.len())];
        let input = mutate(&mut rng, seed);
        let _ = ExtHandshake::decode(&input);
        let _ = MetadataMsg::decode(&Bytes::from(input.clone()));
        let _ = PexMsg::decode(&input);
    }
}

#[test]
fn magnet_parser_never_panics() {
    let seed = b"magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=x&tr=udp%3A%2F%2Ft%3A1&x.pe=1.2.3.4:5";
    let mut rng = Rng(99);
    for _ in 0..20_000 {
        let input = mutate(&mut rng, seed);
        let _ = Magnet::parse(&String::from_utf8_lossy(&input));
    }
}

#[test]
fn hostile_metadata_is_jailed() {
    // A torrent whose file paths try to escape the download directory.
    let files = Value::List(vec![
        DictBuilder::new()
            .int("length", 5)
            .value(
                "path",
                Value::List(vec![
                    Value::Bytes(b"..".to_vec()),
                    Value::Bytes(b"..".to_vec()),
                    Value::Bytes(b"etc".to_vec()),
                ]),
            )
            .build(),
    ]);
    let info = DictBuilder::new()
        .bytes("name", "../../escape")
        .int("piece length", 16384)
        .bytes("pieces", vec![0u8; 20])
        .value("files", files)
        .build();
    let info = Info::from_bytes(&info.encode()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let storage = trav_core::storage::Storage::new(root.path(), &info).unwrap();
    for f in storage.files() {
        assert!(
            f.path.starts_with(root.path()),
            "{} escaped",
            f.path.display()
        );
        assert!(
            f.path
                .components()
                .all(|c| !matches!(c, std::path::Component::ParentDir))
        );
    }
}
