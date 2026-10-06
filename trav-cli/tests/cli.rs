//! Black-box tests of the `trav` binary.

use std::process::Command;

fn trav() -> Command {
    Command::new(env!("CARGO_BIN_EXE_trav"))
}

#[test]
fn help_and_version() {
    let out = trav().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for flag in ["--daemon", "--create", "--token", "--save-path", "get"] {
        assert!(text.contains(flag), "--help is missing {flag}");
    }
    let out = trav().arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn create_makes_a_valid_torrent() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("album");
    std::fs::create_dir_all(dir.join("cd2")).unwrap();
    std::fs::write(dir.join("01.flac"), vec![7u8; 300_000]).unwrap();
    std::fs::write(dir.join("cd2/02.flac"), vec![9u8; 123_457]).unwrap();
    let out_path = tmp.path().join("album.torrent");
    let out = trav()
        .args(["--create"])
        .arg(&dir)
        .arg("-o")
        .arg(&out_path)
        .args([
            "--tracker",
            "udp://tracker.example:1337/announce",
            "--private",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let m = trav_core::metainfo::Metainfo::from_bytes(&std::fs::read(&out_path).unwrap()).unwrap();
    assert_eq!(m.info.name, "album");
    assert!(m.info.private);
    assert_eq!(m.info.total_length, 423_457);
    assert_eq!(m.info.files.len(), 2);
    assert_eq!(m.trackers[0][0], "udp://tracker.example:1337/announce");
    // stdout leads with the info-hash.
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.starts_with(&hex(&m.info_hash)));
}

#[test]
fn daemon_refuses_public_bind_without_token() {
    let tmp = tempfile::tempdir().unwrap();
    let out = trav()
        .args([
            "--daemon",
            "--web-bind",
            "0.0.0.0:0",
            "--port",
            "0",
            "--state-dir",
        ])
        .arg(tmp.path())
        .env_remove("TRAV_TOKEN")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("without --token"));
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn health_check_fails_when_nothing_listens() {
    let out = trav()
        .args(["--health-check", "--web-bind", "127.0.0.1:9"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

/// `trav get` against an in-process seeder: fetches metadata from a magnet's
/// peer hint, downloads, prints where the files went, exits 0 and cleans up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_downloads_a_magnet_and_exits() {
    use trav_core::{AddTorrent, Engine, Settings, TorrentSource, TorrentStatus};
    let tmp = tempfile::tempdir().unwrap();
    let quiet = |dir: std::path::PathBuf| Settings {
        download_dir: dir,
        listen_port: 0,
        enable_dht: false,
        enable_upnp: false,
        ..Settings::default()
    };

    let content = tmp.path().join("seed/pack");
    std::fs::create_dir_all(&content).unwrap();
    let data: Vec<u8> = (0..400_000u32).map(|i| (i * 7 % 251) as u8).collect();
    std::fs::write(content.join("a.bin"), &data).unwrap();
    std::fs::write(content.join("b.txt"), b"hello").unwrap();
    let torrent = trav_core::metainfo::create_torrent(&content, 32 * 1024, &[], false).unwrap();
    let seed = Engine::start_with(
        tmp.path().join("seed-state"),
        Some(quiet(tmp.path().join("seed"))),
    )
    .await
    .unwrap();
    let hash = seed
        .add(AddTorrent::new(TorrentSource::Bytes(torrent)))
        .await
        .unwrap();
    for _ in 0..100 {
        if seed.snapshot().torrents.first().map(|t| t.status) == Some(TorrentStatus::Seeding) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // The library only supplies settings here: keep the child off the real network.
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(
        lib.join("settings.json"),
        r#"{"enableDht":false,"enableUpnp":false,"listenPort":0}"#,
    )
    .unwrap();
    let scratch = tmp.path().join("tmp");
    std::fs::create_dir_all(&scratch).unwrap();
    let dl = tmp.path().join("dl");
    let magnet = format!(
        "magnet:?xt=urn:btih:{hash}&x.pe=127.0.0.1:{}",
        seed.listen_port()
    );

    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_trav"))
        .arg("--state-dir")
        .arg(&lib)
        .args(["get", &magnet, "-o"])
        .arg(&dl)
        .env("TMPDIR", &scratch)
        .env("TMP", &scratch)
        .env("TEMP", &scratch)
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(60), child)
        .await
        .expect("trav get finished")
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("✓ pack"), "{stdout}");
    assert_eq!(std::fs::read(dl.join("pack/a.bin")).unwrap(), data);
    assert_eq!(std::fs::read(dl.join("pack/b.txt")).unwrap(), b"hello");
    assert!(
        std::fs::read_dir(&scratch).unwrap().next().is_none(),
        "throwaway state directory was removed"
    );
}
