//! A torrent's own speed limit, through real engines on loopback.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use trav_core::metainfo::create_torrent;
use trav_core::{AddTorrent, Engine, EngineHandle, Settings, TorrentSource, TorrentStatus};

fn settings(dl: &Path) -> Settings {
    Settings {
        download_dir: dl.to_path_buf(),
        listen_port: 0,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_utp: false,
        public_trackers: false,
        auto_upload_limit: false,
        ..Settings::default()
    }
}

async fn wait_for(h: &EngineHandle, hash: &str, want: TorrentStatus, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let status = h
            .snapshot()
            .torrents
            .iter()
            .find(|t| t.info_hash == hash)
            .map(|t| t.status);
        if status == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "waiting for {want:?}, got {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_torrents_own_download_limit_holds_and_survives_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let content = tmp.path().join("seed/data.bin");
    std::fs::create_dir_all(content.parent().unwrap()).unwrap();
    std::fs::write(
        &content,
        (0..600_000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
    )
    .unwrap();
    let torrent = create_torrent(&content, 16 * 1024, &[], false).unwrap();

    let seed = Engine::start_with(
        tmp.path().join("seed-state"),
        Some(settings(&tmp.path().join("seed"))),
    )
    .await
    .unwrap();
    let hash = seed
        .add(AddTorrent::new(TorrentSource::Bytes(torrent.clone())))
        .await
        .unwrap();
    wait_for(&seed, &hash, TorrentStatus::Seeding, 20).await;

    let leech_dl = tmp.path().join("leech");
    let leech = Engine::start_with(tmp.path().join("leech-state"), Some(settings(&leech_dl)))
        .await
        .unwrap();
    let mut req = AddTorrent::new(TorrentSource::Bytes(torrent));
    req.paused = true;
    leech.add(req).await.unwrap();
    // 150 KB/s for 600 KB: about four seconds, where unlimited loopback takes a blink.
    leech.set_limits(&hash, 150 * 1024, 0).unwrap();
    assert_eq!(
        leech.details(&hash).unwrap().summary.download_limit,
        150 * 1024
    );
    let started = Instant::now();
    leech.resume(&hash).unwrap();
    leech
        .add_peers(
            &hash,
            vec![SocketAddr::from(([127, 0, 0, 1], seed.listen_port()))],
        )
        .unwrap();
    wait_for(&leech, &hash, TorrentStatus::Seeding, 30).await;
    let took = started.elapsed();
    assert!(
        took > Duration::from_millis(2500),
        "the limit was ignored: {took:?}"
    );
    assert!(
        took < Duration::from_secs(15),
        "far slower than the limit: {took:?}"
    );
    leech.shutdown().await;

    let leech = Engine::start_with(tmp.path().join("leech-state"), Some(settings(&leech_dl)))
        .await
        .unwrap();
    assert_eq!(
        leech.details(&hash).unwrap().summary.download_limit,
        150 * 1024
    );
    leech.shutdown().await;
    seed.shutdown().await;
}
