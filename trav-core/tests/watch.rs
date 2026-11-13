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

fn watching(dl: &std::path::Path, watch: &std::path::Path) -> Settings {
    Settings {
        download_dir: dl.to_path_buf(),
        listen_port: 0,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_utp: false,
        public_trackers: false,
        watch_dir: Some(watch.to_path_buf()),
        ..Settings::default()
    }
}

#[tokio::test]
async fn a_writer_that_pauses_still_gets_its_torrent_added() {
    // QA round 2, F7: half a file is "invalid" for a while; once the writer
    // finishes, the whole file is read again and added.
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let watch = tmp.path().join("watch");
    std::fs::create_dir_all(&watch).unwrap();
    let content = tmp.path().join("content.bin");
    std::fs::write(&content, b"payload").unwrap();
    let bytes = create_torrent(&content, 16 * 1024, &[], false).unwrap();
    let mut f = std::fs::File::create(watch.join("slow.torrent")).unwrap();
    f.write_all(&bytes[..4]).unwrap();
    let h = Engine::start_with(
        tmp.path().join("state"),
        Some(watching(&tmp.path().join("dl"), &watch)),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    f.write_all(&bytes[4..]).unwrap();
    drop(f);
    let deadline = Instant::now() + Duration::from_secs(15);
    while h.snapshot().torrents.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the finished torrent was never added"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.shutdown().await;
}

#[tokio::test]
async fn a_watch_folder_never_touches_a_torrents_own_files() {
    // QA round 2, F8: watching the download folder must leave a payload that
    // happens to be named .torrent where its torrent keeps it.
    let tmp = tempfile::tempdir().unwrap();
    let dl = tmp.path().join("dl");
    std::fs::create_dir_all(&dl).unwrap();
    let payload = dl.join("archive.torrent");
    std::fs::write(&payload, b"not metainfo: someone's payload").unwrap();
    let bytes = create_torrent(&payload, 16 * 1024, &[], false).unwrap();
    let h = Engine::start_with(tmp.path().join("state"), Some(watching(&dl, &dl)))
        .await
        .unwrap();
    h.add(trav_core::AddTorrent::new(trav_core::TorrentSource::Bytes(
        bytes,
    )))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(7)).await;
    let still = payload.exists();
    h.shutdown().await;
    assert!(still, "the payload was renamed");
}
