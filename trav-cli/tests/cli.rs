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
    for flag in ["--daemon", "--create", "--token", "--save-path"] {
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
