//! Persistence, settings and the RPC surface, exercised through the public API.

use serde_json::{Value, json};
use trav_core::persist::{ResumeData, Store};
use trav_core::{Engine, Settings, rpc};

fn settings(dir: &std::path::Path) -> Settings {
    Settings {
        download_dir: dir.join("dl"),
        listen_port: 0,
        enable_dht: false,
        enable_upnp: false,
        ..Settings::default()
    }
}

#[test]
fn store_roundtrip_and_exclusive_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().to_path_buf()).unwrap();
    assert!(
        Store::new(tmp.path().to_path_buf()).is_err(),
        "second engine must not share the state dir"
    );

    let ih = [3u8; 20];
    let r = ResumeData {
        name: "n".into(),
        queue_position: 2,
        uploaded: 77,
        ..Default::default()
    };
    store.save_resume(&ih, &r).unwrap();
    store.save_torrent(&ih, b"d4:infode").unwrap();
    let all = store.load_all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, ih);
    assert_eq!(all[0].1.uploaded, 77);
    assert!(store.load_torrent(&ih).is_some());
    // No temp files are left behind by atomic writes.
    let stray = std::fs::read_dir(tmp.path().join("torrents"))
        .unwrap()
        .flatten()
        .any(|e| e.path().to_string_lossy().ends_with(".tmp"));
    assert!(!stray);
    store.remove(&ih);
    assert!(store.load_all().is_empty());

    store.unlock();
    assert!(
        Store::new(tmp.path().to_path_buf()).is_ok(),
        "lock released on unlock"
    );
}

#[test]
fn corrupt_state_files_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().to_path_buf()).unwrap();
    std::fs::write(
        tmp.path()
            .join("torrents")
            .join(format!("{}.json", "ab".repeat(20))),
        b"{not json",
    )
    .unwrap();
    std::fs::write(tmp.path().join("torrents").join("garbage.json"), b"{}").unwrap();
    assert!(store.load_all().is_empty());
}

#[test]
fn settings_are_sanitized() {
    let s = Settings {
        max_peers_per_torrent: 0,
        upload_slots: 100_000,
        seed_ratio_limit: f64::NAN,
        ..Settings::default()
    }
    .sanitized();
    assert_eq!(s.max_peers_per_torrent, 1);
    assert_eq!(s.upload_slots, 200);
    assert_eq!(s.seed_ratio_limit, 0.0);
    // Unknown and missing fields are tolerated when loading older configs.
    let partial: Settings =
        serde_json::from_value(json!({ "listenPort": 1234, "bogus": true })).unwrap();
    assert_eq!(partial.listen_port, 1234);
    assert!(partial.enable_dht);
}

#[tokio::test]
async fn rpc_surface() {
    let tmp = tempfile::tempdir().unwrap();
    let h = Engine::start_with(tmp.path().join("state"), Some(settings(tmp.path())))
        .await
        .unwrap();
    let call = |m: &'static str, p: Value| {
        let h = h.clone();
        async move { rpc::dispatch(&h, m, p).await }
    };

    let hash = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
    let added = call("add", json!({ "magnet": hash, "paused": true }))
        .await
        .unwrap();
    assert_eq!(
        added["infoHash"], hash,
        "bare info-hashes are accepted as magnets"
    );
    assert!(
        call("add", json!({ "magnet": hash }))
            .await
            .unwrap_err()
            .contains("already")
    );
    assert!(call("add", json!({})).await.is_err());
    assert!(
        call("add", json!({ "torrent": "%%%not-base64" }))
            .await
            .is_err()
    );
    assert!(call("inspect", json!({ "torrent": "AAAA" })).await.is_err());

    call("resume", json!({ "hashes": [hash] })).await.unwrap();
    call("pause", json!({ "hash": hash })).await.unwrap();
    assert!(call("pause", json!({ "hash": "zz" })).await.is_err());
    assert!(
        call("pause", json!({ "hash": "00".repeat(20) }))
            .await
            .is_err()
    );
    assert!(call("details", Value::Null).await.is_err());
    let d = call("details", json!({ "hash": hash })).await.unwrap();
    assert_eq!(d["summary"]["hasMetadata"], false);
    assert!(
        d["magnet"]
            .as_str()
            .unwrap()
            .starts_with("magnet:?xt=urn:btih:")
    );

    let mut s = call("getSettings", Value::Null).await.unwrap();
    s["downloadLimit"] = json!(123_456);
    let saved = call("setSettings", s).await.unwrap();
    assert_eq!(saved["downloadLimit"], 123_456);
    assert!(
        call("setSettings", json!({ "listenPort": "nope" }))
            .await
            .is_err()
    );

    call("remove", json!({ "hash": hash, "deleteFiles": true }))
        .await
        .unwrap();
    let snap = call("snapshot", Value::Null).await.unwrap();
    assert!(snap["torrents"].as_array().unwrap().is_empty());
    assert!(
        call("frobnicate", Value::Null)
            .await
            .unwrap_err()
            .contains("unknown method")
    );
    h.shutdown().await;

    // Settings persist across restarts.
    let h = Engine::start(tmp.path().join("state")).await.unwrap();
    assert_eq!(h.settings().download_limit, 123_456);
    h.shutdown().await;
}
