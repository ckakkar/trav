//! Message Stream Encryption (MSE/PE), the obfuscated handshake most clients
//! speak: Diffie-Hellman over a 768-bit prime, then RC4 both ways. It reaches
//! peers that refuse plain BitTorrent and gets past networks that throttle it.

use num_bigint::BigUint;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::stream::{PeerStream, Rc4};
use crate::error::{Result, proto};
use crate::metainfo::InfoHash;

const PRIME: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A63A36210000000000090563";
const KEY_LEN: usize = 96;
const MAX_PAD: usize = 512;
const VC: [u8; 8] = [0; 8];

pub const CRYPTO_PLAIN: u32 = 0x01;
pub const CRYPTO_RC4: u32 = 0x02;

struct Dh {
    private: BigUint,
    public: [u8; KEY_LEN],
}

fn prime() -> BigUint {
    BigUint::parse_bytes(PRIME.as_bytes(), 16).expect("valid prime")
}

fn to_key(n: &BigUint) -> [u8; KEY_LEN] {
    let b = n.to_bytes_be();
    let mut out = [0u8; KEY_LEN];
    out[KEY_LEN - b.len()..].copy_from_slice(&b);
    out
}

impl Dh {
    fn new() -> Self {
        let private = BigUint::from_bytes_be(&rand::random::<[u8; 20]>());
        let public = to_key(&BigUint::from(2u32).modpow(&private, &prime()));
        Self { private, public }
    }

    fn secret(&self, theirs: &[u8; KEY_LEN]) -> Result<[u8; KEY_LEN]> {
        let p = prime();
        let y = BigUint::from_bytes_be(theirs);
        if y <= BigUint::from(1u32) || y >= &p - 1u32 {
            return Err(proto("MSE: bad public key"));
        }
        Ok(to_key(&y.modpow(&self.private, &p)))
    }
}

fn hash(parts: &[&[u8]]) -> [u8; 20] {
    let mut h = Sha1::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn xor20(a: [u8; 20], b: [u8; 20]) -> [u8; 20] {
    std::array::from_fn(|i| a[i] ^ b[i])
}

fn padding() -> Vec<u8> {
    let n = rand::random_range(0..=MAX_PAD / 4);
    (0..n).map(|_| rand::random()).collect()
}

/// Bytes read past what the handshake has consumed so far.
struct Reader {
    tcp: TcpStream,
    buf: Vec<u8>,
}

impl Reader {
    async fn take(&mut self, n: usize) -> Result<Vec<u8>> {
        while self.buf.len() < n {
            let mut chunk = [0u8; 1024];
            let got = self.tcp.read(&mut chunk).await?;
            if got == 0 {
                return Err(proto("MSE: connection closed"));
            }
            self.buf.extend_from_slice(&chunk[..got]);
        }
        Ok(self.buf.drain(..n).collect())
    }

    /// Skip up to `MAX_PAD` bytes of padding until `pattern`, consuming it.
    async fn sync(&mut self, pattern: &[u8]) -> Result<()> {
        loop {
            if let Some(at) = self.buf.windows(pattern.len()).position(|w| w == pattern) {
                self.buf.drain(..at + pattern.len());
                return Ok(());
            }
            if self.buf.len() >= MAX_PAD + pattern.len() {
                return Err(proto("MSE: no sync marker"));
            }
            let mut chunk = [0u8; 1024];
            let got = self.tcp.read(&mut chunk).await?;
            if got == 0 {
                return Err(proto("MSE: connection closed"));
            }
            self.buf.extend_from_slice(&chunk[..got]);
        }
    }

    fn finish(self, mut rx: Rc4, tx: Rc4, rc4: bool, mut prefix: Vec<u8>) -> PeerStream {
        let mut rest = self.buf;
        if rc4 {
            rx.apply(&mut rest);
            prefix.extend(rest);
            PeerStream::new(self.tcp, prefix, Some((rx, tx)))
        } else {
            prefix.extend(rest);
            PeerStream::new(self.tcp, prefix, None)
        }
    }
}

/// Dialling side. `ia` (our BitTorrent handshake) rides along in the third
/// message, saving a round trip; read the peer's handshake from the result.
pub async fn initiate(
    mut tcp: TcpStream,
    info_hash: &InfoHash,
    ia: &[u8],
    provide: u32,
) -> Result<PeerStream> {
    let dh = Dh::new();
    let mut hello = dh.public.to_vec();
    hello.extend(padding());
    tcp.write_all(&hello).await?;

    let mut r = Reader {
        tcp,
        buf: Vec::new(),
    };
    let yb: [u8; KEY_LEN] = r.take(KEY_LEN).await?.try_into().unwrap();
    let s = dh.secret(&yb)?;
    let mut tx = Rc4::new(&hash(&[b"keyA", &s, info_hash]));
    let mut rx = Rc4::new(&hash(&[b"keyB", &s, info_hash]));

    let mut msg = hash(&[b"req1", &s]).to_vec();
    msg.extend(xor20(hash(&[b"req2", info_hash]), hash(&[b"req3", &s])));
    let mut sealed = VC.to_vec();
    sealed.extend(provide.to_be_bytes());
    sealed.extend(0u16.to_be_bytes()); // no PadC
    sealed.extend((ia.len() as u16).to_be_bytes());
    sealed.extend(ia);
    tx.apply(&mut sealed);
    msg.extend(sealed);
    r.tcp.write_all(&msg).await?;

    // Their reply starts with VC under their key, after up to 512 bytes of padding.
    let mut vc = VC;
    rx.apply(&mut vc);
    r.sync(&vc).await?;
    let mut head = r.take(6).await?;
    rx.apply(&mut head);
    let select = u32::from_be_bytes(head[..4].try_into().unwrap());
    let pad = u16::from_be_bytes([head[4], head[5]]) as usize;
    if pad > MAX_PAD {
        return Err(proto("MSE: padding too long"));
    }
    let mut skip = r.take(pad).await?;
    rx.apply(&mut skip);
    let rc4 = match select {
        CRYPTO_RC4 if provide & CRYPTO_RC4 != 0 => true,
        CRYPTO_PLAIN if provide & CRYPTO_PLAIN != 0 => false,
        _ => return Err(proto("MSE: peer chose a method we did not offer")),
    };
    Ok(r.finish(rx, tx, rc4, Vec::new()))
}

/// Accepting side, once the first bytes showed this isn't a plain handshake.
/// `first` holds what was read so far; `torrents` are the info-hashes we serve.
/// Returns the stream (their handshake is in it, ready to read) and the torrent.
pub async fn respond(
    tcp: TcpStream,
    first: Vec<u8>,
    torrents: &[InfoHash],
    allow: u32,
) -> Result<(PeerStream, InfoHash)> {
    let mut r = Reader { tcp, buf: first };
    let ya: [u8; KEY_LEN] = r.take(KEY_LEN).await?.try_into().unwrap();
    let dh = Dh::new();
    let mut hello = dh.public.to_vec();
    hello.extend(padding());
    r.tcp.write_all(&hello).await?;
    let s = dh.secret(&ya)?;

    r.sync(&hash(&[b"req1", &s])).await?;
    let skey: [u8; 20] = r.take(20).await?.try_into().unwrap();
    let wanted = xor20(skey, hash(&[b"req3", &s]));
    let info_hash = *torrents
        .iter()
        .find(|ih| hash(&[b"req2", ih.as_slice()]) == wanted)
        .ok_or_else(|| proto("MSE: unknown torrent"))?;
    let mut rx = Rc4::new(&hash(&[b"keyA", &s, &info_hash]));
    let mut tx = Rc4::new(&hash(&[b"keyB", &s, &info_hash]));

    let mut head = r.take(14).await?;
    rx.apply(&mut head);
    if head[..8] != VC {
        return Err(proto("MSE: bad verification constant"));
    }
    let provide = u32::from_be_bytes(head[8..12].try_into().unwrap());
    let pad = u16::from_be_bytes([head[12], head[13]]) as usize;
    if pad > MAX_PAD {
        return Err(proto("MSE: padding too long"));
    }
    let mut skip = r.take(pad).await?;
    rx.apply(&mut skip);
    let mut len = r.take(2).await?;
    rx.apply(&mut len);
    let ia_len = u16::from_be_bytes([len[0], len[1]]) as usize;
    if ia_len > 4096 {
        return Err(proto("MSE: initial payload too long"));
    }
    let mut ia = r.take(ia_len).await?;
    rx.apply(&mut ia);

    let both = provide & allow;
    let select = if both & CRYPTO_RC4 != 0 {
        CRYPTO_RC4
    } else if both & CRYPTO_PLAIN != 0 {
        CRYPTO_PLAIN
    } else {
        return Err(proto("MSE: no common method"));
    };
    let mut reply = VC.to_vec();
    reply.extend(select.to_be_bytes());
    reply.extend(0u16.to_be_bytes()); // no PadD
    tx.apply(&mut reply);
    r.tcp.write_all(&reply).await?;
    Ok((r.finish(rx, tx, select == CRYPTO_RC4, ia), info_hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (a, b) = tokio::join!(TcpStream::connect(addr), l.accept());
        (a.unwrap(), b.unwrap().0)
    }

    async fn roundtrip(provide: u32, allow: u32) -> (bool, bool) {
        let (a, b) = pair().await;
        let ih: InfoHash = [7; 20];
        let other: InfoHash = [9; 20];
        let ia = b"hello from A".to_vec();
        let responder = tokio::spawn(async move {
            let mut first = vec![0u8; 20];
            let mut b = b;
            b.read_exact(&mut first).await.unwrap();
            let (mut s, found) = respond(b, first, &[other, ih], allow).await.unwrap();
            assert_eq!(found, ih);
            let mut got = vec![0u8; 12];
            s.read_exact(&mut got).await.unwrap();
            assert_eq!(got, b"hello from A");
            s.write_all(b"and B replies").await.unwrap();
            s.flush().await.unwrap();
            let mut more = vec![0u8; 100_000];
            s.read_exact(&mut more).await.unwrap();
            assert!(more.iter().enumerate().all(|(i, &b)| b == i as u8));
            s.encrypted()
        });
        let mut s = initiate(a, &ih, &ia, provide).await.unwrap();
        let mut got = vec![0u8; 13];
        s.read_exact(&mut got).await.unwrap();
        assert_eq!(got, b"and B replies");
        let big: Vec<u8> = (0..100_000).map(|i| i as u8).collect();
        s.write_all(&big).await.unwrap();
        s.flush().await.unwrap();
        (s.encrypted(), responder.await.unwrap())
    }

    #[tokio::test]
    async fn rc4_both_ways() {
        assert_eq!(
            roundtrip(CRYPTO_RC4, CRYPTO_RC4 | CRYPTO_PLAIN).await,
            (true, true)
        );
    }

    #[tokio::test]
    async fn plaintext_after_obfuscated_handshake() {
        assert_eq!(
            roundtrip(CRYPTO_PLAIN | CRYPTO_RC4, CRYPTO_PLAIN).await,
            (false, false)
        );
    }

    #[tokio::test]
    async fn unknown_torrent_is_refused() {
        let (a, b) = pair().await;
        let responder =
            tokio::spawn(async move { respond(b, Vec::new(), &[[1; 20]], CRYPTO_RC4).await.err() });
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            initiate(a, &[2; 20], b"x", CRYPTO_RC4),
        )
        .await
        .unwrap();
        assert!(res.is_err());
        assert!(responder.await.unwrap().is_some());
    }
}
