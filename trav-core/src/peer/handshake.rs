//! BEP 3 handshake, with BEP 10 (extension protocol) and BEP 5 (DHT) bits set,
//! in the clear or inside MSE/PE encryption.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use super::mse::{self, CRYPTO_PLAIN, CRYPTO_RC4};
use super::stream::PeerStream;
use crate::error::{Error, Result, proto};
use crate::metainfo::InfoHash;
use crate::settings::Encryption;

pub const PSTR: &[u8; 19] = b"BitTorrent protocol";
pub const HANDSHAKE_LEN: usize = 68;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
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

pub async fn read_handshake<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Handshake> {
    let mut buf = [0u8; HANDSHAKE_LEN];
    tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.read_exact(&mut buf))
        .await
        .map_err(|_| Error::Timeout("handshake read"))??;
    Handshake::decode(&buf)
}

pub async fn write_handshake<S: AsyncWrite + Unpin>(stream: &mut S, hs: &Handshake) -> Result<()> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.write_all(&hs.encode()))
        .await
        .map_err(|_| Error::Timeout("handshake write"))??;
    Ok(())
}

async fn dial(addr: SocketAddr) -> Result<TcpStream> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Timeout("connect"))??;
    stream.set_nodelay(true).ok();
    Ok(stream)
}

fn check(ours: &Handshake, theirs: Handshake) -> Result<Handshake> {
    if theirs.info_hash != ours.info_hash {
        return Err(proto("info-hash mismatch"));
    }
    if theirs.peer_id == ours.peer_id {
        return Err(proto("connected to self"));
    }
    Ok(theirs)
}

async fn plain(tcp: TcpStream, ours: &Handshake) -> Result<(PeerStream, Handshake)> {
    let mut s = PeerStream::plain(tcp);
    write_handshake(&mut s, ours).await?;
    let theirs = read_handshake(&mut s).await?;
    Ok((s, check(ours, theirs)?))
}

async fn encrypted(tcp: TcpStream, ours: &Handshake) -> Result<(PeerStream, Handshake)> {
    let mut s = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        mse::initiate(tcp, &ours.info_hash, &ours.encode(), CRYPTO_RC4),
    )
    .await
    .map_err(|_| Error::Timeout("encrypted handshake"))??;
    let theirs = read_handshake(&mut s).await?;
    Ok((s, check(ours, theirs)?))
}

/// Dial a peer and complete the handshake, rejecting mismatched swarms and self-connections.
pub async fn connect(
    addr: SocketAddr,
    ours: &Handshake,
    enc: Encryption,
) -> Result<(PeerStream, Handshake)> {
    match enc {
        Encryption::Off => plain(dial(addr).await?, ours).await,
        Encryption::Require => encrypted(dial(addr).await?, ours).await,
        Encryption::Prefer => match encrypted(dial(addr).await?, ours).await {
            Ok(done) => Ok(done),
            // Not every client speaks MSE: dial again in the clear.
            Err(_) => plain(dial(addr).await?, ours).await,
        },
    }
}

/// Answer an incoming connection, plain or encrypted. `torrents` lists the
/// info-hashes we serve (an encrypted handshake names its torrent by hash).
pub async fn accept(
    mut tcp: TcpStream,
    enc: Encryption,
    torrents: impl FnOnce() -> Vec<InfoHash>,
) -> Result<(PeerStream, Handshake)> {
    tcp.set_nodelay(true).ok();
    let mut first = [0u8; 20];
    tokio::time::timeout(HANDSHAKE_TIMEOUT, tcp.read_exact(&mut first))
        .await
        .map_err(|_| Error::Timeout("handshake read"))??;
    if first[0] == 19 && &first[1..] == PSTR {
        if enc == Encryption::Require {
            return Err(proto("plain connection refused"));
        }
        let mut s = PeerStream::with_prefix(tcp, first.to_vec());
        let theirs = read_handshake(&mut s).await?;
        return Ok((s, theirs));
    }
    if enc == Encryption::Off {
        return Err(proto("encrypted connection refused"));
    }
    let allow = match enc {
        Encryption::Require => CRYPTO_RC4,
        _ => CRYPTO_RC4 | CRYPTO_PLAIN,
    };
    let (mut s, info_hash) = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        mse::respond(tcp, first.to_vec(), &torrents(), allow),
    )
    .await
    .map_err(|_| Error::Timeout("encrypted handshake"))??;
    let theirs = read_handshake(&mut s).await?;
    if theirs.info_hash != info_hash {
        return Err(proto("info-hash mismatch"));
    }
    Ok((s, theirs))
}
