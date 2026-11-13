//! The watch folder, through the real engine.

use std::time::{Duration, Instant};

use trav_core::metainfo::create_torrent;
use trav_core::{Engine, Settings};

#[tokio::test]
async fn torrents_saved_to_the_watch_folder_are_added_once() {
    let tmp = tempfile::tempdir().unwrap();
    let watch = tmp.path().join("watch");
    std::fs::create_dir_all(&watch).unwrap();
    let content = tmp.path().join("content.bin");
    std::fs::write(&content, vec![1u8; 50_000]).unwrap();
    std::fs::write(
        watch.join("good.torrent"),
        create_torrent(&content, 16 * 1024, &[], false).unwrap(),
    )
    .unwrap();
    std::fs::write(watch.join("junk.torrent"), b"not a torrent").unwrap();
    std::fs::write(watch.join("readme.txt"), b"left alone").unwrap();

    let settings = Settings {
        download_dir: tmp.path().join("dl"),
        listen_port: 0,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_utp: false,
        public_trackers: false,
        watch_dir: Some(watch.clone()),
        ..Settings::default()
    };
    let h = Engine::start_with(tmp.path().join("state"), Some(settings))
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !(watch.join("good.torrent.added").exists()
        && watch.join("junk.torrent.invalid").exists())
    {
        assert!(
            Instant::now() < deadline,
            "watch folder not handled: {:?}",
            std::fs::read_dir(&watch)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(h.snapshot().torrents.len(), 1);
    assert!(watch.join("readme.txt").exists());
    // Saving the same torrent again doesn't add a second copy.
    std::fs::write(
        watch.join("again.torrent"),
        create_torrent(&content, 16 * 1024, &[], false).unwrap(),
    )
    .unwrap();
    while !watch.join("again.torrent.added").exists() {
        assert!(
            Instant::now() < deadline + Duration::from_secs(10),
            "second file not handled"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(h.snapshot().torrents.len(), 1);
    h.shutdown().await;
}
