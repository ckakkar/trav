//! BEP 3 peer-wire framing.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::error::{proto, Error};

#[derive(Debug, Clone, PartialEq)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have { piece_index: u32 },
    Bitfield { payload: Bytes },
    Request { index: u32, begin: u32, length: u32 },
    Piece { index: u32, begin: u32, block: Bytes },
    Cancel { index: u32, begin: u32, length: u32 },
    Port { listen_port: u16 },
    Extended { extended_id: u8, payload: Bytes },
    /// Any message id we do not implement; payload is discarded.
    Unknown { id: u8 },
}

/// Largest frame accepted: a 16 KiB block plus generous room for bitfields of huge torrents.
pub const MAX_PEER_MESSAGE_LEN: usize = 2 * 1024 * 1024;
/// We never serve or accept blocks larger than this (BEP 3 de-facto limit).
pub const MAX_BLOCK_LEN: u32 = 128 * 1024;

#[derive(Default)]
pub struct PeerCodec;

impl Decoder for PeerCodec {
    type Item = PeerMessage;
    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }
        let length = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
        if length > MAX_PEER_MESSAGE_LEN {
            return Err(proto(format!("message too large: {length} bytes")));
        }
        if src.len() < 4 + length {
            src.reserve(4 + length - src.len());
            return Ok(None);
        }
        src.advance(4);
        if length == 0 {
            return Ok(Some(PeerMessage::KeepAlive));
        }
        let mut body = src.split_to(length).freeze();
        let id = body.get_u8();
        let n = body.len();
        let need = |want: usize| if n == want { Ok(()) } else { Err(proto(format!("bad length {n} for message {id}"))) };

        let msg = match id {
            0 => PeerMessage::Choke,
            1 => PeerMessage::Unchoke,
            2 => PeerMessage::Interested,
            3 => PeerMessage::NotInterested,
            4 => {
                need(4)?;
                PeerMessage::Have { piece_index: body.get_u32() }
            }
            5 => PeerMessage::Bitfield { payload: body },
            6 | 8 => {
                need(12)?;
                let (index, begin, length) = (body.get_u32(), body.get_u32(), body.get_u32());
                if id == 6 {
                    PeerMessage::Request { index, begin, length }
                } else {
                    PeerMessage::Cancel { index, begin, length }
                }
            }
            7 => {
                if n < 8 {
                    return Err(proto("short piece message"));
                }
                let index = body.get_u32();
                let begin = body.get_u32();
                PeerMessage::Piece { index, begin, block: body }
            }
            9 => {
                need(2)?;
                PeerMessage::Port { listen_port: body.get_u16() }
            }
            20 => {
                if n < 1 {
                    return Err(proto("empty extended message"));
                }
                let extended_id = body.get_u8();
                PeerMessage::Extended { extended_id, payload: body }
            }
            id => PeerMessage::Unknown { id },
        };
        Ok(Some(msg))
    }
}

impl Encoder<PeerMessage> for PeerCodec {
    type Error = Error;

    fn encode(&mut self, item: PeerMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let simple = |dst: &mut BytesMut, id: u8| {
            dst.put_u32(1);
            dst.put_u8(id);
        };
        match item {
            PeerMessage::KeepAlive => dst.put_u32(0),
            PeerMessage::Choke => simple(dst, 0),
            PeerMessage::Unchoke => simple(dst, 1),
            PeerMessage::Interested => simple(dst, 2),
            PeerMessage::NotInterested => simple(dst, 3),
            PeerMessage::Have { piece_index } => {
                dst.put_u32(5);
                dst.put_u8(4);
                dst.put_u32(piece_index);
            }
            PeerMessage::Bitfield { payload } => {
                dst.reserve(5 + payload.len());
                dst.put_u32(1 + payload.len() as u32);
                dst.put_u8(5);
                dst.put_slice(&payload);
            }
            PeerMessage::Request { index, begin, length } | PeerMessage::Cancel { index, begin, length } => {
                let id = if matches!(item, PeerMessage::Request { .. }) { 6 } else { 8 };
                dst.put_u32(13);
                dst.put_u8(id);
                dst.put_u32(index);
                dst.put_u32(begin);
                dst.put_u32(length);
            }
            PeerMessage::Piece { index, begin, block } => {
                dst.reserve(13 + block.len());
                dst.put_u32(9 + block.len() as u32);
                dst.put_u8(7);
                dst.put_u32(index);
                dst.put_u32(begin);
                dst.put_slice(&block);
            }
            PeerMessage::Port { listen_port } => {
                dst.put_u32(3);
                dst.put_u8(9);
                dst.put_u16(listen_port);
            }
            PeerMessage::Extended { extended_id, payload } => {
                dst.reserve(6 + payload.len());
                dst.put_u32(2 + payload.len() as u32);
                dst.put_u8(20);
                dst.put_u8(extended_id);
                dst.put_slice(&payload);
            }
            PeerMessage::Unknown { .. } => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all() {
        let msgs = vec![
            PeerMessage::KeepAlive,
            PeerMessage::Unchoke,
            PeerMessage::Have { piece_index: 7 },
            PeerMessage::Bitfield { payload: Bytes::from_static(&[0xf0]) },
            PeerMessage::Request { index: 1, begin: 16384, length: 16384 },
            PeerMessage::Piece { index: 1, begin: 0, block: Bytes::from_static(b"data") },
            PeerMessage::Cancel { index: 1, begin: 0, length: 4 },
            PeerMessage::Extended { extended_id: 0, payload: Bytes::from_static(b"de") },
        ];
        let mut buf = BytesMut::new();
        for m in &msgs {
            PeerCodec.encode(m.clone(), &mut buf).unwrap();
        }
        for m in msgs {
            assert_eq!(PeerCodec.decode(&mut buf).unwrap().unwrap(), m);
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn rejects_oversize_and_partial_waits() {
        let mut buf = BytesMut::from(&[0xff, 0xff, 0xff, 0xff][..]);
        assert!(PeerCodec.decode(&mut buf).is_err());
        let mut buf = BytesMut::from(&[0, 0, 0, 5, 4, 0][..]);
        assert!(PeerCodec.decode(&mut buf).unwrap().is_none());
    }
}
