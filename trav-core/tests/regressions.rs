//! Regressions from review findings, driven through the public API with
//! synthetic torrents in temporary folders.

use std::path::Path;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trav_core::bencode::{DictBuilder, Value};
use trav_core::limiter::RateLimiter;
use trav_core::metainfo::Metainfo;
use trav_core::peer::handshake::{self, Handshake, Route};
use trav_core::peer::protocol::{PeerCodec, PeerMessage};
use trav_core::picker::BLOCK_SIZE;
use trav_core::{AddTorrent, Encryption, Engine, Settings, TorrentSource};

/// A single-file info dict holding `data` in one piece.
fn single(name: &str, data: &[u8]) -> Value {
    DictBuilder::new()
        .bytes("name", name)
        .int("length", data.len() as i64)
        .int("piece length", data.len() as i64)
        .bytes("pieces", Sha1::digest(data).to_vec())
        .build()
}

/// A multi-file info dict `bundle/<name>` with one byte (and one piece) per file.
fn multi(names: &[&str], bytes: &[u8]) -> Value {
    let files = names
        .iter()
        .map(|name| {
            DictBuilder::new()
                .int("length", 1)
                .value(
                    "path",
                    Value::List(vec![Value::Bytes(name.as_bytes().to_vec())]),
                )
                .build()
        })
        .collect();
    let pieces: Vec<u8> = bytes
        .iter()
        .flat_map(|b| Sha1::digest([*b]).to_vec())
        .collect();
    DictBuilder::new()
        .bytes("name", "bundle")
        .value("files", Value::List(files))
        .int("piece length", 1)
        .bytes("pieces", pieces)
        .build()
}

fn torrent(info: Value) -> Vec<u8> {
    DictBuilder::new().value("info", info).build().encode()
}

fn quiet(dl: &Path) -> Settings {
    Settings {
        download_dir: dl.into(),
        listen_port: 0,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        public_trackers: false,
        enable_utp: false,
        ..Settings::default()
    }
}

fn paused(bytes: Vec<u8>) -> AddTorrent {
    let mut req = AddTorrent::new(TorrentSource::Bytes(bytes));
    req.paused = true;
    req
}

#[test]
fn a_second_info_dict_is_refused() {
    let mut bytes = b"d4:info".to_vec();
    bytes.extend_from_slice(&single("first", b"A").encode());
    bytes.extend_from_slice(b"4:info");
    bytes.extend_from_slice(&single("second", b"B").encode());
    bytes.push(b'e');
    assert!(Metainfo::from_bytes(&bytes).is_err());
}

#[tokio::test]
async fn two_torrents_cannot_share_their_files() {
    let tmp = tempfile::tempdir().unwrap();
    let h = Engine::start_with(
        tmp.path().join("state"),
        Some(quiet(&tmp.path().join("dl"))),
    )
    .await
    .unwrap();
    let first = h
        .add(paused(torrent(single("Same.txt", b"A"))))
        .await
        .unwrap();
    // Different content, same name (here only by case): one file on a Mac.
    let err = h
        .add(paused(torrent(single("same.txt", b"B"))))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Same.txt"), "{err}");
    // Once the first is gone, the place is free.
    h.remove(&first, false).await.unwrap();
    h.add(paused(torrent(single("same.txt", b"B"))))
        .await
        .unwrap();
    h.shutdown().await;
}

#[tokio::test]
async fn a_removed_torrent_writes_nothing_more() {
    let tmp = tempfile::tempdir().unwrap();
    let dl = tmp.path().join("dl");
    let mut settings = quiet(&dl);
    settings.encryption = Encryption::Off;
    // Hold the block in the rate limiter until after the removal.
    settings.download_limit = 1;
    let h = Engine::start_with(tmp.path().join("state"), Some(settings))
        .await
        .unwrap();
    let data = vec![b'A'; BLOCK_SIZE as usize];
    let bytes = torrent(single("removed.bin", &data));
    let ih = Metainfo::from_bytes(&bytes).unwrap().info_hash;
    let hash = h
        .add(AddTorrent::new(TorrentSource::Bytes(bytes)))
        .await
        .unwrap();

    let ours = Handshake::ours(ih, [42; 20]);
    let addr = ([127, 0, 0, 1], h.listen_port()).into();
    let (stream, _) = handshake::connect(addr, &ours, Encryption::Off, Route::Tcp, None)
        .await
        .unwrap();
    let mut peer = tokio_util::codec::Framed::new(stream, PeerCodec);
    peer.send(PeerMessage::HaveAll).await.unwrap();
    peer.send(PeerMessage::Unchoke).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(msg) = peer.next().await {
            if let PeerMessage::Request { index, begin, .. } = msg.unwrap() {
                let block = bytes::Bytes::copy_from_slice(&data);
                peer.send(PeerMessage::Piece {
                    index,
                    begin,
                    block,
                })
                .await
                .unwrap();
                break;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    h.remove(&hash, true).await.unwrap();
    let mut settings = h.settings();
    settings.download_limit = 0;
    h.update_settings(settings).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!dl.join("removed.bin").exists());
    h.shutdown().await;
}

#[tokio::test]
async fn trackers_hear_what_is_missing_even_if_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let dl = tmp.path().join("dl");
    std::fs::create_dir_all(dl.join("bundle")).unwrap();
    std::fs::write(dl.join("bundle/a"), b"A").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/announce", listener.local_addr().unwrap());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tracker = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let body = b"d8:intervali60e5:peers0:e";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        }
    });
    let h = Engine::start_with(tmp.path().join("state"), Some(quiet(&dl)))
        .await
        .unwrap();
    let bytes = DictBuilder::new()
        .bytes("announce", url)
        .value("info", multi(&["a", "b"], b"AB"))
        .build()
        .encode();
    let mut req = AddTorrent::new(TorrentSource::Bytes(bytes));
    // `a` is on disk, `b` is skipped: done for the user, not a seed.
    req.file_priorities = Some(vec![1, 0]);
    h.add(req).await.unwrap();
    let announce = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(announce.contains("&left=1&"), "{announce}");
    h.shutdown().await;
    tracker.abort();
}

#[tokio::test]
async fn raising_a_limit_shortens_a_wait_once() {
    let limiter = std::sync::Arc::new(RateLimiter::new(1000));
    let start = Instant::now();
    let worker = tokio::spawn({
        let limiter = limiter.clone();
        async move { limiter.acquire(4000).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    limiter.set_rate(2000);
    worker.await.unwrap();
    // ~0.2 s at 1000 B/s, then the remaining ~3.8 s of debt at twice the rate.
    let took = start.elapsed();
    assert!(took > Duration::from_millis(1700), "{took:?}");
    assert!(took < Duration::from_secs(3), "{took:?}");
}
