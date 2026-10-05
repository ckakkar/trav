//! BEP 3 handshake, with BEP 10 (extension protocol) and BEP 5 (DHT) bits set.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::{Error, Result, proto};
use crate::metainfo::InfoHash;

pub const PSTR: &[u8; 19] = b"BitTorrent protocol";
pub const HANDSHAKE_LEN: usize = 68;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy)]
pub struct Handshake {
    pub reserved: [u8; 8],
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub fn ours(info_hash: InfoHash, peer_id: [u8; 20]) -> Self {
        let mut reserved = [0u8; 8];
        reserved[5] |= 0x10; // BEP 10 extension protocol
        reserved[7] |= 0x01; // BEP 5 DHT
        Self {
            reserved,
            info_hash,
            peer_id,
        }
    }

    pub fn supports_extensions(&self) -> bool {
        self.reserved[5] & 0x10 != 0
    }

    pub fn supports_dht(&self) -> bool {
        self.reserved[7] & 0x01 != 0
    }

    pub fn encode(&self) -> [u8; HANDSHAKE_LEN] {
        let mut b = [0u8; HANDSHAKE_LEN];
        b[0] = 19;
        b[1..20].copy_from_slice(PSTR);
        b[20..28].copy_from_slice(&self.reserved);
        b[28..48].copy_from_slice(&self.info_hash);
        b[48..68].copy_from_slice(&self.peer_id);
        b
    }

    pub fn decode(b: &[u8; HANDSHAKE_LEN]) -> Result<Self> {
        if b[0] != 19 || &b[1..20] != PSTR {
            return Err(proto("not a BitTorrent handshake"));
        }
        Ok(Self {
            reserved: b[20..28].try_into().unwrap(),
            info_hash: b[28..48].try_into().unwrap(),
            peer_id: b[48..68].try_into().unwrap(),
        })
    }
}

pub async fn read_handshake(stream: &mut TcpStream) -> Result<Handshake> {
    let mut buf = [0u8; HANDSHAKE_LEN];
    tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.read_exact(&mut buf))
        .await
        .map_err(|_| Error::Timeout("handshake read"))??;
    Handshake::decode(&buf)
}

pub async fn write_handshake(stream: &mut TcpStream, hs: &Handshake) -> Result<()> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.write_all(&hs.encode()))
        .await
        .map_err(|_| Error::Timeout("handshake write"))??;
    Ok(())
}

/// Dial a peer and complete the handshake, rejecting mismatched swarms and self-connections.
pub async fn connect(addr: SocketAddr, ours: &Handshake) -> Result<(TcpStream, Handshake)> {
    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Timeout("connect"))??;
    stream.set_nodelay(true).ok();
    write_handshake(&mut stream, ours).await?;
    let theirs = read_handshake(&mut stream).await?;
    if theirs.info_hash != ours.info_hash {
        return Err(proto("info-hash mismatch"));
    }
    if theirs.peer_id == ours.peer_id {
        return Err(proto("connected to self"));
    }
    Ok((stream, theirs))
}
