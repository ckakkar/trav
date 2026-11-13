//! "Move Data…" through the real engine: files go to the new folder, the
//! torrent carries on from there, and nothing is lost when a move can't happen.

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
        ..Settings::default()
    }
}

/// `Show/{a.bin, sub/b.bin}` in `parent`, as a torrent seeded from it.
async fn seeding(tmp: &Path) -> (EngineHandle, String, std::path::PathBuf) {
    let parent = tmp.join("dl");
    let show = parent.join("Show");
    std::fs::create_dir_all(show.join("sub")).unwrap();
    std::fs::write(show.join("a.bin"), vec![7u8; 100_000]).unwrap();
    std::fs::write(show.join("sub/b.bin"), vec![9u8; 70_000]).unwrap();
    let bytes = create_torrent(&show, 16 * 1024, &[], false).unwrap();
    let h = Engine::start_with(tmp.join("state"), Some(settings(&parent)))
        .await
        .unwrap();
    let hash = h
        .add(AddTorrent::new(TorrentSource::Bytes(bytes)))
        .await
        .unwrap();
    wait_for(&h, &hash, TorrentStatus::Seeding).await;
    (h, hash, parent)
}

async fn wait_for(h: &EngineHandle, hash: &str, want: TorrentStatus) {
    let deadline = Instant::now() + Duration::from_secs(20);
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
async fn a_seeding_torrent_moves_and_keeps_seeding_from_its_new_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, hash, old) = seeding(tmp.path()).await;
    let new = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&new).unwrap();

    h.move_storage(&hash, new.clone()).await.unwrap();

    assert_eq!(
        std::fs::read(new.join("Show/a.bin")).unwrap(),
        vec![7u8; 100_000]
    );
    assert_eq!(
        std::fs::read(new.join("Show/sub/b.bin")).unwrap(),
        vec![9u8; 70_000]
    );
    assert!(!old.join("Show").exists(), "the old folder is cleaned up");
    let d = h.details(&hash).unwrap();
    assert_eq!(d.summary.save_path, new.display().to_string());
    assert_eq!(
        d.content_path.as_deref(),
        Some(new.join("Show").display().to_string().as_str())
    );
    wait_for(&h, &hash, TorrentStatus::Seeding).await;
    // And a recheck finds every piece where it now is.
    h.recheck(&hash).unwrap();
    wait_for(&h, &hash, TorrentStatus::Seeding).await;
    assert_eq!(h.details(&hash).unwrap().summary.progress, 1.0);
    h.shutdown().await;

    // The new folder survives a restart.
    let h = Engine::start_with(tmp.path().join("state"), Some(settings(&old)))
        .await
        .unwrap();
    wait_for(&h, &hash, TorrentStatus::Seeding).await;
    assert_eq!(
        h.details(&hash).unwrap().summary.save_path,
        new.display().to_string()
    );
    h.shutdown().await;
}

#[tokio::test]
async fn a_move_that_would_overwrite_moves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, hash, old) = seeding(tmp.path()).await;
    let new = tmp.path().join("elsewhere");
    std::fs::create_dir_all(new.join("Show/sub")).unwrap();
    std::fs::write(new.join("Show/sub/b.bin"), b"someone else's").unwrap();

    let err = h.move_storage(&hash, new.clone()).await.unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    assert_eq!(
        std::fs::read(new.join("Show/sub/b.bin")).unwrap(),
        b"someone else's"
    );
    assert!(!new.join("Show/a.bin").exists());
    assert_eq!(
        std::fs::read(old.join("Show/a.bin")).unwrap(),
        vec![7u8; 100_000]
    );
    assert_eq!(
        h.details(&hash).unwrap().summary.save_path,
        old.display().to_string()
    );
    wait_for(&h, &hash, TorrentStatus::Seeding).await;
    h.shutdown().await;
}

#[tokio::test]
async fn removing_with_its_data_during_a_move_deletes_the_moved_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, hash, old) = seeding(tmp.path()).await;
    let new = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&new).unwrap();

    let mover = {
        let h = h.clone();
        let hash = hash.clone();
        let new = new.clone();
        tokio::spawn(async move { h.move_storage(&hash, new).await })
    };
    tokio::task::yield_now().await;
    h.remove(&hash, true).await.unwrap();
    let _ = mover.await.unwrap();
    assert!(!new.join("Show/a.bin").exists(), "moved files deleted");
    assert!(!old.join("Show/a.bin").exists(), "old files deleted");
    h.shutdown().await;
}
