//! BEP 3 handshake, with BEP 10 (extension protocol) and BEP 5 (DHT) bits set,
//! in the clear or inside MSE/PE encryption, over TCP or µTP.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use super::mse::{self, CRYPTO_PLAIN, CRYPTO_RC4};
use super::stream::{PeerStream, Transport};
use crate::error::{Error, Result, proto};
use crate::metainfo::InfoHash;
use crate::settings::Encryption;
use crate::utp::{Dial, UtpSocket};

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

/// How to reach a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Tcp,
    /// TCP first, with µTP joining in if TCP hasn't connected shortly after;
    /// whichever connects first is used.
    Auto,
    /// µTP only (hole punching needs it).
    Utp(Dial),
}

/// How long TCP runs alone before µTP joins in. TCP goes first because it
/// downloads faster: µTP's congestion control yields to other traffic by
/// design and backs off hard on loss, so it is for peers TCP can't reach.
pub const TCP_HEAD_START: Duration = Duration::from_secs(1);

async fn dial_tcp(addr: SocketAddr) -> Result<Transport> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Timeout("connect"))??;
    stream.set_nodelay(true).ok();
    Ok(Transport::Tcp(stream))
}

async fn dial_utp(utp: &UtpSocket, addr: SocketAddr, dial: Dial) -> Result<Transport> {
    // The socket gives up on its own (after ~4 s, or ~10 s when hole punching).
    Ok(Transport::Utp(utp.connect(addr, dial).await?))
}

async fn dial(addr: SocketAddr, route: Route, utp: Option<&UtpSocket>) -> Result<Transport> {
    let Some(utp) = utp else {
        return match route {
            Route::Utp(_) => Err(Error::Protocol("µTP is off".into())),
            _ => dial_tcp(addr).await,
        };
    };
    match route {
        Route::Tcp => dial_tcp(addr).await,
        Route::Utp(d) => dial_utp(utp, addr, d).await,
        Route::Auto => {
            // A quick refusal starts µTP at once rather than after the head start.
            let tcp_failed = tokio::sync::Notify::new();
            let t = async {
                let r = dial_tcp(addr).await;
                if r.is_err() {
                    tcp_failed.notify_one();
                }
                r
            };
            let u = async {
                tokio::select! {
                    _ = tokio::time::sleep(TCP_HEAD_START) => {}
                    _ = tcp_failed.notified() => {}
                }
                dial_utp(utp, addr, Dial::Normal).await
            };
            tokio::pin!(u, t);
            let (mut u_err, mut t_err) = (None, None);
            loop {
                tokio::select! {
                    r = &mut t, if t_err.is_none() => match r {
                        Ok(s) => return Ok(s),
                        Err(e) => t_err = Some(e),
                    },
                    r = &mut u, if u_err.is_none() => match r {
                        Ok(s) => return Ok(s),
                        Err(e) => u_err = Some(e),
                    },
                }
                if let (Some(_), Some(_)) = (&u_err, &t_err) {
                    // TCP's error says more (refused, unreachable) than a µTP timeout.
                    return Err(t_err.or(u_err).expect("both set"));
                }
            }
        }
    }
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

async fn plain(io: Transport, ours: &Handshake) -> Result<(PeerStream, Handshake)> {
    let mut s = PeerStream::plain(io);
    write_handshake(&mut s, ours).await?;
    let theirs = read_handshake(&mut s).await?;
    Ok((s, check(ours, theirs)?))
}

async fn encrypted(io: Transport, ours: &Handshake) -> Result<(PeerStream, Handshake)> {
    let mut s = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        mse::initiate(io, &ours.info_hash, &ours.encode(), CRYPTO_RC4),
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
    route: Route,
    utp: Option<&UtpSocket>,
) -> Result<(PeerStream, Handshake)> {
    match enc {
        Encryption::Off => plain(dial(addr, route, utp).await?, ours).await,
        Encryption::Require => encrypted(dial(addr, route, utp).await?, ours).await,
        Encryption::Prefer => {
            let io = dial(addr, route, utp).await?;
            // Redial the way that worked.
            let again = if io.is_utp() {
                Route::Utp(Dial::Normal)
            } else {
                Route::Tcp
            };
            match encrypted(io, ours).await {
                Ok(done) => Ok(done),
                // Not every client speaks MSE: dial again in the clear.
                Err(_) => plain(dial(addr, again, utp).await?, ours).await,
            }
        }
    }
}

/// Answer an incoming connection, plain or encrypted. `torrents` lists the
/// info-hashes we serve (an encrypted handshake names its torrent by hash).
pub async fn accept(
    mut io: Transport,
    enc: Encryption,
    torrents: impl FnOnce() -> Vec<InfoHash>,
) -> Result<(PeerStream, Handshake)> {
    if let Transport::Tcp(t) = &io {
        t.set_nodelay(true).ok();
    }
    let mut first = [0u8; 20];
    tokio::time::timeout(HANDSHAKE_TIMEOUT, io.read_exact(&mut first))
        .await
        .map_err(|_| Error::Timeout("handshake read"))??;
    if first[0] == 19 && &first[1..] == PSTR {
        if enc == Encryption::Require {
            return Err(proto("plain connection refused"));
        }
        let mut s = PeerStream::with_prefix(io, first.to_vec());
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
        mse::respond(io, first.to_vec(), &torrents(), allow),
    )
    .await
    .map_err(|_| Error::Timeout("encrypted handshake"))??;
    let theirs = read_handshake(&mut s).await?;
    if theirs.info_hash != info_hash {
        return Err(proto("info-hash mismatch"));
    }
    Ok((s, theirs))
}
