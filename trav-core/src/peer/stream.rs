//! A peer connection's byte stream: TCP or µTP underneath, in the clear or
//! RC4-encrypted after an MSE/PE handshake (see `mse`).

use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use crate::utp::{UtpProbe, UtpStream};

/// The connection under the BitTorrent stream.
#[derive(Debug)]
pub enum Transport {
    Tcp(TcpStream),
    Utp(UtpStream),
}

impl Transport {
    pub fn is_utp(&self) -> bool {
        matches!(self, Self::Utp(_))
    }

    pub fn probe(&self) -> RttProbe {
        match self {
            Self::Tcp(t) => RttProbe::Tcp(t.as_raw_fd()),
            Self::Utp(u) => RttProbe::Utp(u.probe()),
        }
    }
}

impl AsyncRead for Transport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(t) => Pin::new(t).poll_read(cx, buf),
            Self::Utp(u) => Pin::new(u).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Transport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(t) => Pin::new(t).poll_write(cx, data),
            Self::Utp(u) => Pin::new(u).poll_write(cx, data),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(t) => Pin::new(t).poll_flush(cx),
            Self::Utp(u) => Pin::new(u).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(t) => Pin::new(t).poll_shutdown(cx),
            Self::Utp(u) => Pin::new(u).poll_shutdown(cx),
        }
    }
}

/// Reads a connection's smoothed round-trip time while the stream itself is
/// busy inside a codec.
pub enum RttProbe {
    /// The kernel's estimate for this TCP socket (valid while it is open).
    Tcp(RawFd),
    Utp(UtpProbe),
}

impl RttProbe {
    pub fn srtt_ms(&self) -> Option<u32> {
        match self {
            Self::Tcp(fd) => crate::rtt::srtt_ms(*fd),
            Self::Utp(p) => p.srtt().map(|d| d.as_millis().max(1) as u32),
        }
    }
}

/// RC4 with the first 1024 keystream bytes dropped, as MSE specifies.
pub(crate) struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    pub fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for (i, b) in s.iter_mut().enumerate() {
            *b = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        let mut rc4 = Self { s, i: 0, j: 0 };
        rc4.apply(&mut [0u8; 1024]);
        rc4
    }

    pub fn apply(&mut self, data: &mut [u8]) {
        for b in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k = self.s[self.s[self.i as usize].wrapping_add(self.s[self.j as usize]) as usize];
            *b ^= k;
        }
    }
}

pub struct PeerStream {
    io: Transport,
    /// Bytes the handshake already read off the socket, ready for the reader.
    prefix: Vec<u8>,
    prefix_pos: usize,
    rx: Option<Rc4>,
    tx: Option<Rc4>,
    /// Encrypted bytes accepted by `poll_write` but not yet on the socket.
    wbuf: Vec<u8>,
    wpos: usize,
}

impl PeerStream {
    pub fn plain(io: Transport) -> Self {
        Self::new(io, Vec::new(), None)
    }

    pub(crate) fn new(io: Transport, prefix: Vec<u8>, ciphers: Option<(Rc4, Rc4)>) -> Self {
        let (rx, tx) = ciphers.map_or((None, None), |(rx, tx)| (Some(rx), Some(tx)));
        Self {
            io,
            prefix,
            prefix_pos: 0,
            rx,
            tx,
            wbuf: Vec::new(),
            wpos: 0,
        }
    }

    /// In the clear, with bytes already read from it (e.g. while sniffing the protocol).
    pub(crate) fn with_prefix(io: Transport, prefix: Vec<u8>) -> Self {
        Self::new(io, prefix, None)
    }

    pub fn encrypted(&self) -> bool {
        self.tx.is_some()
    }

    pub fn is_utp(&self) -> bool {
        self.io.is_utp()
    }

    pub fn rtt_probe(&self) -> RttProbe {
        self.io.probe()
    }

    fn drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.wpos < self.wbuf.len() {
            let n = ready!(Pin::new(&mut self.io).poll_write(cx, &self.wbuf[self.wpos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.wpos += n;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for PeerStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.prefix_pos < me.prefix.len() {
            let n = buf.remaining().min(me.prefix.len() - me.prefix_pos);
            buf.put_slice(&me.prefix[me.prefix_pos..me.prefix_pos + n]);
            me.prefix_pos += n;
            if me.prefix_pos == me.prefix.len() {
                me.prefix = Vec::new();
                me.prefix_pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        ready!(Pin::new(&mut me.io).poll_read(cx, buf))?;
        if let Some(rx) = me.rx.as_mut() {
            rx.apply(&mut buf.filled_mut()[before..]);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for PeerStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        if me.tx.is_none() {
            return Pin::new(&mut me.io).poll_write(cx, data);
        }
        // The keystream has advanced over earlier bytes: send those first.
        ready!(me.drain(cx))?;
        let n = data.len().min(64 * 1024);
        me.wbuf.clear();
        me.wbuf.extend_from_slice(&data[..n]);
        if let Some(tx) = me.tx.as_mut() {
            tx.apply(&mut me.wbuf);
        }
        me.wpos = 0;
        // Push out what the socket takes now; the rest goes with the next write or flush.
        if let Poll::Ready(Err(e)) = me.drain(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        ready!(me.drain(cx))?;
        Pin::new(&mut me.io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        ready!(me.drain(cx))?;
        Pin::new(&mut me.io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_matches_reference_vectors() {
        // RFC 6229, key 0x0102030405, keystream at offset 1024 (what MSE uses).
        let mut rc4 = Rc4::new(&[1, 2, 3, 4, 5]);
        let mut ks = [0u8; 16];
        rc4.apply(&mut ks);
        assert_eq!(
            ks,
            [
                0x30, 0xab, 0xbc, 0xc7, 0xc2, 0x0b, 0x01, 0x60, 0x9f, 0x23, 0xee, 0x2d, 0x5f, 0x6b,
                0xb7, 0xdf
            ]
        );
    }
}
