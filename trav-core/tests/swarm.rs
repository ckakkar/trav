//! End-to-end: real engines exchanging data over localhost TCP.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trav_core::metainfo::create_torrent;
use trav_core::{AddTorrent, Engine, EngineHandle, Settings, TorrentSource, TorrentStatus};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .with_test_writer()
        .try_init();
}

fn settings(download_dir: &Path) -> Settings {
    Settings {
        download_dir: download_dir.to_path_buf(),
        listen_port: 0,
        enable_dht: false,
        enable_upnp: false,
        ..Settings::default()
    }
}

/// Deterministic pseudo-random bytes so failures are reproducible.
fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// Content: `album/{a.bin, sub/b.bin, empty.txt}` with sizes crossing piece boundaries.
fn make_content(root: &Path) -> PathBuf {
    let dir = root.join("album");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.bin"), payload(300_007, 1)).unwrap();
    std::fs::write(dir.join("sub/b.bin"), payload(1_234_567, 2)).unwrap();
    std::fs::write(dir.join("empty.txt"), b"").unwrap();
    dir
}

async fn wait_for(h: &EngineHandle, hash: &str, want: TorrentStatus, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let snap = h.snapshot();
        let t = snap.torrents.iter().find(|t| t.info_hash == hash);
        if let Some(t) = t {
            if t.status == want {
                return;
            }
            if Instant::now() > deadline {
                panic!(
                    "timed out waiting for {want:?}; status={:?} progress={:.3} peers={} err={:?}",
                    t.status, t.progress, t.peers + t.seeds, t.error
                );
            }
        } else if Instant::now() > deadline {
            panic!("torrent {hash} missing from snapshot");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn same_tree(a: &Path, b: &Path) {
    for rel in ["a.bin", "sub/b.bin", "empty.txt"] {
        let x = std::fs::read(a.join(rel)).unwrap_or_else(|e| panic!("{rel} in {}: {e}", a.display()));
        let y = std::fs::read(b.join(rel)).unwrap_or_else(|e| panic!("{rel} in {}: {e}", b.display()));
        assert!(x == y, "{rel} differs");
    }
}

async fn seeder(tmp: &Path, trackers: &[String]) -> (EngineHandle, String, Vec<u8>, PathBuf) {
    let content_parent = tmp.join("seed-data");
    let content = make_content(&content_parent);
    let torrent = create_torrent(&content, 32 * 1024, trackers, false).unwrap();
    let seed = Engine::start_with(tmp.join("seed-state"), Some(settings(&content_parent))).await.unwrap();
    let hash = seed.add(AddTorrent::new(TorrentSource::Bytes(torrent.clone()))).await.unwrap();
    wait_for(&seed, &hash, TorrentStatus::Seeding, 20).await;
    (seed, hash, torrent, content)
}

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn downloads_from_seed_and_resumes() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let (seed, hash, torrent, content) = seeder(tmp.path(), &[]).await;
    let s = seed.snapshot();
    let st = s.torrents.iter().find(|t| t.info_hash == hash).unwrap();
    assert_eq!(st.progress, 1.0, "seeder should verify existing data");

    let dl_dir = tmp.path().join("dl");
    let leech = Engine::start_with(tmp.path().join("leech-state"), Some(settings(&dl_dir))).await.unwrap();
    let h2 = leech.add(AddTorrent::new(TorrentSource::Bytes(torrent))).await.unwrap();
    assert_eq!(h2, hash);
    leech.add_peers(&hash, vec![local(seed.listen_port())]).unwrap();

    wait_for(&leech, &hash, TorrentStatus::Seeding, 30).await;
    same_tree(&content, &dl_dir.join("album"));

    let d = leech.details(&hash).unwrap();
    assert_eq!(d.files.len(), 3);
    assert!(d.files.iter().all(|f| f.progress == 1.0));
    assert!(seed.snapshot().torrents[0].uploaded >= 1_534_574, "seeder accounts uploads");

    // Restart: resume data must be trusted, no re-download.
    leech.shutdown().await;
    drop(leech);
    let leech = Engine::start_with(tmp.path().join("leech-state"), Some(settings(&dl_dir))).await.unwrap();
    wait_for(&leech, &hash, TorrentStatus::Seeding, 5).await;
    assert_eq!(leech.snapshot().stats.session_downloaded, 0);

    // Remove with data.
    leech.remove(&hash, true).await.unwrap();
    assert!(!dl_dir.join("album").exists());
    assert!(leech.snapshot().torrents.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn magnet_fetches_metadata_from_peer() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let (seed, hash, _torrent, content) = seeder(tmp.path(), &[]).await;

    let dl_dir = tmp.path().join("dl");
    let leech = Engine::start_with(tmp.path().join("leech-state"), Some(settings(&dl_dir))).await.unwrap();
    let magnet = format!("magnet:?xt=urn:btih:{hash}&dn=album&x.pe=127.0.0.1:{}", seed.listen_port());
    let mut events = leech.events();
    let h2 = leech.add(AddTorrent::new(TorrentSource::Magnet(magnet))).await.unwrap();
    assert_eq!(h2, hash);

    wait_for(&leech, &hash, TorrentStatus::Seeding, 30).await;
    same_tree(&content, &dl_dir.join("album"));

    let mut saw = (false, false);
    while let Ok(e) = events.try_recv() {
        match e {
            trav_core::Event::MetadataReceived { .. } => saw.0 = true,
            trav_core::Event::TorrentCompleted { .. } => saw.1 = true,
            _ => {}
        }
    }
    assert!(saw.0 && saw.1, "metadata + completion events: {saw:?}");
    // The fetched metadata is persisted as a .torrent for future sessions.
    assert!(leech.state_dir().join("torrents").join(format!("{hash}.torrent")).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selective_download_and_http_tracker() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();

    // Minimal HTTP tracker: always answers with the seeder's address.
    let tracker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tracker_url = format!("http://{}/announce", tracker.local_addr().unwrap());
    let seed_port = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(0));
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let seed_port = seed_port.clone();
        let hits = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = tracker.accept().await.unwrap();
                let port = seed_port.load(std::sync::atomic::Ordering::SeqCst);
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let mut peers = vec![127, 0, 0, 1];
                    peers.extend_from_slice(&port.to_be_bytes());
                    let mut body = b"d8:intervali60e5:peers6:".to_vec();
                    body.extend_from_slice(&peers);
                    body.push(b'e');
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                });
            }
        });
    }

    let (seed, hash, torrent, content) = seeder(tmp.path(), std::slice::from_ref(&tracker_url)).await;
    seed_port.store(seed.listen_port(), std::sync::atomic::Ordering::SeqCst);

    let dl_dir = tmp.path().join("dl");
    let leech = Engine::start_with(tmp.path().join("leech-state"), Some(settings(&dl_dir))).await.unwrap();
    let preview = leech.inspect(&torrent).unwrap();
    // Files are listed sorted: a.bin, empty.txt, sub/b.bin — skip the big one.
    assert_eq!(preview.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a.bin", "empty.txt", "sub/b.bin"]);
    let mut req = AddTorrent::new(TorrentSource::Bytes(torrent));
    req.file_priorities = Some(vec![1, 1, 0]);
    leech.add(req).await.unwrap();

    wait_for(&leech, &hash, TorrentStatus::Seeding, 30).await;
    assert!(hits.load(std::sync::atomic::Ordering::SeqCst) >= 2, "both engines announced");
    let got = std::fs::read(dl_dir.join("album/a.bin")).unwrap();
    assert_eq!(got, std::fs::read(content.join("a.bin")).unwrap());
    let d = leech.details(&hash).unwrap();
    let b = d.files.iter().find(|f| f.path == "sub/b.bin").unwrap();
    assert!(b.progress < 1.0, "skipped file must not be fully downloaded");
    assert_eq!(d.trackers[0].status, "working");

    // Re-enable the skipped file: the torrent resumes downloading and completes.
    leech.set_file_priorities(&hash, vec![1, 1, 1]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let d = leech.details(&hash).unwrap();
        if d.files.iter().all(|f| f.progress == 1.0) {
            assert_eq!(d.summary.status, TorrentStatus::Seeding);
            break;
        }
        assert!(Instant::now() < deadline, "re-enabled file never completed: {:?}", d.files);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    same_tree(&content, &dl_dir.join("album"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_resume_and_rate_limit() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let (seed, hash, torrent, content) = seeder(tmp.path(), &[]).await;
    let dl_dir = tmp.path().join("dl");
    let mut s = settings(&dl_dir);
    s.download_limit = 400 * 1024;
    let leech = Engine::start_with(tmp.path().join("leech-state"), Some(s)).await.unwrap();
    let mut req = AddTorrent::new(TorrentSource::Bytes(torrent));
    req.paused = true;
    leech.add(req).await.unwrap();
    leech.add_peers(&hash, vec![local(seed.listen_port())]).unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t = &leech.snapshot().torrents[0];
    assert_eq!(t.status, TorrentStatus::Paused);
    assert_eq!(t.done_bytes, 0, "paused torrent must not download");

    let started = Instant::now();
    leech.resume(&hash).unwrap();
    wait_for(&leech, &hash, TorrentStatus::Seeding, 30).await;
    // ~1.5 MB at 400 KiB/s cannot finish in under ~2.5 s.
    assert!(started.elapsed() > Duration::from_millis(2500), "rate limit ignored: {:?}", started.elapsed());
    same_tree(&content, &dl_dir.join("album"));
}

/// `cargo test --release -p trav-core --test swarm throughput -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore]
async fn throughput() {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("seed-data");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join("big.bin");
    std::fs::write(&file, payload(256 << 20, 9)).unwrap();
    let torrent = create_torrent(&file, 1 << 20, &[], false).unwrap();
    let seed = Engine::start_with(tmp.path().join("s"), Some(settings(&src))).await.unwrap();
    let hash = seed.add(AddTorrent::new(TorrentSource::Bytes(torrent.clone()))).await.unwrap();
    wait_for(&seed, &hash, TorrentStatus::Seeding, 60).await;
    let dl = tmp.path().join("dl");
    let leech = Engine::start_with(tmp.path().join("l"), Some(settings(&dl))).await.unwrap();
    leech.add(AddTorrent::new(TorrentSource::Bytes(torrent))).await.unwrap();
    let t = Instant::now();
    leech.add_peers(&hash, vec![local(seed.listen_port())]).unwrap();
    wait_for(&leech, &hash, TorrentStatus::Seeding, 120).await;
    let secs = t.elapsed().as_secs_f64();
    println!("256 MiB in {secs:.2}s = {:.1} MiB/s", 256.0 / secs);
}
