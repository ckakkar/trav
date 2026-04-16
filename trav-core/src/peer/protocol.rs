use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};
use crate::error::BitTorrentError;

#[derive(Debug, Clone, PartialEq)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have { piece_index: u32 },
    Bitfield { payload: Vec<u8> },
    Request { index: u32, begin: u32, length: u32 },
    Piece { index: u32, begin: u32, block: Vec<u8> },
    Cancel { index: u32, begin: u32, length: u32 },
    Port { listen_port: u16 },
    Extended { extended_id: u8, payload: Vec<u8> },
    /// Gracefully absorbs any message ID we don't handle.
    Unknown { id: u8, payload: Vec<u8> },
}

pub struct PeerCodec;
const MAX_PEER_MESSAGE_LEN: usize = 2 * 1024 * 1024;

impl Decoder for PeerCodec {
    type Item = PeerMessage;
    type Error = BitTorrentError;

    fn decode(&mut self, src: &mut BytesMut) -> std::result::Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }

        let length = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;

        if length > MAX_PEER_MESSAGE_LEN {
            return Err(BitTorrentError::Engine(format!(
                "Peer message too large: {} bytes", length
            )));
        }

        if length == 0 {
            src.advance(4);
            return Ok(Some(PeerMessage::KeepAlive));
        }

        if src.len() < 4 + length {
            src.reserve(4 + length - src.len());
            return Ok(None);
        }

        src.advance(4);
        let id = src.get_u8();
        let payload_len = length - 1;

        let msg = match id {
            0 => PeerMessage::Choke,
            1 => PeerMessage::Unchoke,
            2 => PeerMessage::Interested,
            3 => PeerMessage::NotInterested,
            4 => {
                if payload_len != 4 {
                    return Err(BitTorrentError::Engine(format!("Bad Have length: {}", payload_len)));
                }
                PeerMessage::Have { piece_index: src.get_u32() }
            }
            5 => {
                let mut bf = vec![0u8; payload_len];
                src.copy_to_slice(&mut bf);
                PeerMessage::Bitfield { payload: bf }
            }
            6 => {
                if payload_len != 12 {
                    return Err(BitTorrentError::Engine(format!("Bad Request length: {}", payload_len)));
                }
                PeerMessage::Request {
                    index: src.get_u32(),
                    begin: src.get_u32(),
                    length: src.get_u32(),
                }
            }
            7 => {
                if payload_len < 8 {
                    return Err(BitTorrentError::Engine(format!("Bad Piece length: {}", payload_len)));
                }
                let index = src.get_u32();
                let begin = src.get_u32();
                let mut block = vec![0u8; payload_len - 8];
                src.copy_to_slice(&mut block);
                PeerMessage::Piece { index, begin, block }
            }
            8 => {
                if payload_len != 12 {
                    return Err(BitTorrentError::Engine(format!("Bad Cancel length: {}", payload_len)));
                }
                PeerMessage::Cancel {
                    index: src.get_u32(),
                    begin: src.get_u32(),
                    length: src.get_u32(),
                }
            }
            9 => {
                if payload_len != 2 {
                    return Err(BitTorrentError::Engine(format!("Bad Port length: {}", payload_len)));
                }
                PeerMessage::Port { listen_port: src.get_u16() }
            }
            20 => {
                if payload_len < 1 {
                    return Err(BitTorrentError::Engine("Bad Extended length".into()));
                }
                let extended_id = src.get_u8();
                let mut payload = vec![0u8; payload_len - 1];
                src.copy_to_slice(&mut payload);
                PeerMessage::Extended { extended_id, payload }
            }
            unknown_id => {
                // Consume the payload and continue rather than killing the connection.
                let mut payload = vec![0u8; payload_len];
                if payload_len > 0 {
                    src.copy_to_slice(&mut payload);
                }
                PeerMessage::Unknown { id: unknown_id, payload }
            }
        };

        Ok(Some(msg))
    }
}

impl Encoder<PeerMessage> for PeerCodec {
    type Error = BitTorrentError;

    fn encode(&mut self, item: PeerMessage, dst: &mut BytesMut) -> std::result::Result<(), Self::Error> {
        match item {
            PeerMessage::KeepAlive => dst.put_u32(0),
            PeerMessage::Choke => { dst.put_u32(1); dst.put_u8(0); }
            PeerMessage::Unchoke => { dst.put_u32(1); dst.put_u8(1); }
            PeerMessage::Interested => { dst.put_u32(1); dst.put_u8(2); }
            PeerMessage::NotInterested => { dst.put_u32(1); dst.put_u8(3); }
            PeerMessage::Have { piece_index } => {
                dst.put_u32(5); dst.put_u8(4); dst.put_u32(piece_index);
            }
            PeerMessage::Bitfield { payload } => {
                dst.put_u32(1 + payload.len() as u32); dst.put_u8(5); dst.put_slice(&payload);
            }
            PeerMessage::Request { index, begin, length } => {
                dst.put_u32(13); dst.put_u8(6);
                dst.put_u32(index); dst.put_u32(begin); dst.put_u32(length);
            }
            PeerMessage::Piece { index, begin, block } => {
                dst.put_u32(9 + block.len() as u32); dst.put_u8(7);
                dst.put_u32(index); dst.put_u32(begin); dst.put_slice(&block);
            }
            PeerMessage::Cancel { index, begin, length } => {
                dst.put_u32(13); dst.put_u8(8);
                dst.put_u32(index); dst.put_u32(begin); dst.put_u32(length);
            }
            PeerMessage::Port { listen_port } => {
                dst.put_u32(3); dst.put_u8(9); dst.put_u16(listen_port);
            }
            PeerMessage::Extended { extended_id, payload } => {
                dst.put_u32(2 + payload.len() as u32); dst.put_u8(20);
                dst.put_u8(extended_id); dst.put_slice(&payload);
            }
            PeerMessage::Unknown { .. } => {} // never sent
        }
        Ok(())
    }
}
