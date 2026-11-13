//! Creation round trips, content verification, cancellation, and real-file timing.
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};
use trav_core::bencode::{self, Value};
use trav_core::create::{CreateOptions, auto_piece_length, cancel, create, register};
use trav_core::metainfo::Metainfo;
use trav_core::storage::Storage;
use trav_core::{AddTorrent, Engine, Settings, TorrentSource, TorrentStatus};

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("trav-create-tests-")
        .tempdir()
        .unwrap()
}

fn options(path: &Path, out: &Path) -> CreateOptions {
    CreateOptions {
        path: path.to_string_lossy().into_owned(),
        out: out.to_string_lossy().into_owned(),
        piece_length: Some(16 * 1024),
        ..Default::default()
    }
}

fn make(opts: &CreateOptions) -> (trav_core::create::Created, Metainfo) {
    let mut progress = Vec::new();
    let created = create(
        opts,
        &mut |hashed, total| progress.push((hashed, total)),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(progress.first(), Some(&(0, created.total_size)));
    assert_eq!(
        progress.last(),
        Some(&(created.total_size, created.total_size))
    );
    assert!(progress.windows(2).all(|p| p[0].0 <= p[1].0));
    let meta = Metainfo::from_bytes(&fs::read(&opts.out).unwrap()).unwrap();
    assert_eq!(created.info_hash, hex::encode(meta.info_hash));
    assert_eq!(created.num_pieces, meta.info.num_pieces());
    assert_eq!(created.total_size, meta.info.total_length);
    let again = Metainfo::from_bytes(&meta.to_torrent_bytes()).unwrap();
    assert_eq!(meta.info_hash, again.info_hash);
    (created, meta)
}

fn verify(meta: &Metainfo, parent: &Path) {
    let storage = Storage::new(parent, &meta.info).unwrap();
    for (index, expected) in meta.info.pieces.iter().enumerate() {
        let (start, end) = meta.info.piece_range(index);
        assert!(
            storage.verify(start, (end - start) as usize, expected),
            "piece {index}"
        );
    }
}

#[test]
fn create_single_file_roundtrips_and_verifies() {
    let tmp = temp();
    let path = tmp.path().join("film.bin");
    let content: Vec<u8> = (0..123_457).map(|i| (i % 251) as u8).collect();
    fs::write(&path, &content).unwrap();
    let opts = options(&path, &tmp.path().join("film.torrent"));
    let (created, meta) = make(&opts);
    assert_eq!(created.name, "film.bin");
    assert!(!meta.info.multi_file);
    assert_eq!(meta.info.files[0].path, ["film.bin"]);
    assert_eq!(meta.info.files[0].length, content.len() as u64);
    assert_eq!(
        meta.info.pieces[0],
        <[u8; 20]>::from(Sha1::digest(&content[..16 * 1024]))
    );
    let info = bencode::decode(&meta.info.raw).unwrap();
    assert!(info.get("private").is_none());
    assert!(info.get("files").is_none());
    assert_eq!(
        meta.created_by.as_deref(),
        Some(concat!("Trav ", env!("CARGO_PKG_VERSION")))
    );
    assert!(meta.creation_date.unwrap() > 0);
    verify(&meta, tmp.path());
    assert_eq!(
        serde_json::to_value(created).unwrap()["pieceLength"],
        16 * 1024
    );
}

#[test]
fn create_nested_folder_skips_hidden_and_symlinks_in_component_order() {
    let tmp = temp();
    let root = tmp.path().join("album");
    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join(".hidden")).unwrap();
    fs::write(root.join("a/inside.bin"), vec![7; 20_001]).unwrap();
    fs::write(root.join("a.bin"), vec![9; 30_007]).unwrap();
    fs::write(root.join("empty.txt"), []).unwrap();
    fs::write(root.join(".DS_Store"), b"skip").unwrap();
    fs::write(root.join(".hidden/secret"), b"skip").unwrap();
    std::os::unix::fs::symlink(root.join("a.bin"), root.join("linked.bin")).unwrap();
    std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
    let (_, meta) = make(&options(&root, &tmp.path().join("album.torrent")));
    assert!(meta.info.multi_file);
    assert_eq!(
        meta.info
            .files
            .iter()
            .map(|f| (f.display_path(), f.length))
            .collect::<Vec<_>>(),
        [
            ("a/inside.bin".to_owned(), 20_001),
            ("a.bin".to_owned(), 30_007),
            ("empty.txt".to_owned(), 0)
        ]
    );
    assert!(meta.info.files.iter().all(|f| !f.pad));
    let mut content = vec![7; 20_001];
    content.extend(vec![9; 30_007]);
    let expected: Vec<[u8; 20]> = content
        .chunks(16 * 1024)
        .map(|p| Sha1::digest(p).into())
        .collect();
    assert_eq!(
        meta.info.pieces, expected,
        "pieces span file boundaries without synthetic padding"
    );
    verify(&meta, tmp.path());
}

#[test]
fn create_private_tracker_tiers_web_seeds_comment_and_source() {
    let tmp = temp();
    let path = tmp.path().join("payload");
    fs::write(&path, b"hello world").unwrap();
    let mut opts = options(&path, &tmp.path().join("private.torrent"));
    opts.private = true;
    opts.trackers = vec![
        vec![
            "https://tracker.example/announce".into(),
            "udp://backup.example:1337/announce".into(),
        ],
        vec!["https://last.example/announce".into()],
    ];
    opts.web_seeds = vec!["https://mirror.example/files/".into()];
    opts.comment = Some("Shared with Trav".into());
    opts.source = Some("my-private-tracker".into());
    let (_, meta) = make(&opts);
    assert!(meta.info.private);
    assert_eq!(meta.trackers, opts.trackers);
    assert_eq!(meta.web_seeds, opts.web_seeds);
    assert_eq!(meta.comment, opts.comment);
    let top = bencode::decode(&fs::read(&opts.out).unwrap()).unwrap();
    assert_eq!(
        top.get("announce").and_then(Value::as_str),
        Some("https://tracker.example/announce")
    );
    assert_eq!(
        top.get("info")
            .unwrap()
            .get("source")
            .and_then(Value::as_str),
        Some("my-private-tracker")
    );
    assert_eq!(
        top.get("info")
            .unwrap()
            .get("private")
            .and_then(Value::as_int),
        Some(1)
    );
}

#[test]
fn create_cancel_after_first_progress_leaves_no_torrent_or_temporary_file() {
    let tmp = temp();
    let path = tmp.path().join("payload");
    fs::write(&path, vec![3; 1 << 20]).unwrap();
    let out = tmp.path().join("cancelled.torrent");
    let flag = AtomicBool::new(false);
    let result = create(
        &options(&path, &out),
        &mut |_, _| flag.store(true, Ordering::Relaxed),
        &flag,
    );
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert!(!out.exists());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
}

#[test]
fn create_cancel_at_final_progress_preserves_existing_output() {
    let tmp = temp();
    let path = tmp.path().join("payload");
    fs::write(&path, vec![3; 1 << 20]).unwrap();
    let out = tmp.path().join("existing.torrent");
    fs::write(&out, b"previous torrent").unwrap();
    let flag = AtomicBool::new(false);
    let result = create(
        &options(&path, &out),
        &mut |hashed, total| {
            if hashed == total {
                flag.store(true, Ordering::Relaxed);
            }
        },
        &flag,
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&out).unwrap(), b"previous torrent");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 2);
}

#[test]
fn create_cancel_registry_is_registered_before_the_worker_and_cleans_up() {
    let id = format!("create-test-{:016x}", rand::random::<u64>());
    let job = register(&id).unwrap();
    assert!(register(&id).is_err());
    assert!(cancel(&id));
    assert!(job.cancel_flag().load(Ordering::Relaxed));
    let result = create(&CreateOptions::default(), &mut |_, _| {}, job.cancel_flag());
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    drop(job);
    assert!(!cancel(&id));
    assert!(register("").is_err());
    assert!(register(&"x".repeat(129)).is_err());
}

#[test]
fn create_rejects_input_as_output_including_hard_links_and_symlinks() {
    let tmp = temp();
    let path = tmp.path().join("payload");
    fs::write(&path, b"keep this content").unwrap();
    let hard = tmp.path().join("hard.torrent");
    fs::hard_link(&path, &hard).unwrap();
    let sym = tmp.path().join("link.torrent");
    std::os::unix::fs::symlink(&path, &sym).unwrap();
    for out in [&path, &hard, &sym] {
        assert!(
            create(
                &options(&path, out),
                &mut |_, _| {},
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    assert_eq!(fs::read(path).unwrap(), b"keep this content");
}

#[test]
fn create_rejects_empty_odd_input_and_changed_content_without_partial_output() {
    let tmp = temp();
    let root = tmp.path().join("empty");
    fs::create_dir(&root).unwrap();
    let out = tmp.path().join("output.torrent");
    for path in [&root, &tmp.path().join("missing")] {
        assert!(
            create(
                &options(path, &out),
                &mut |_, _| {},
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    fs::write(root.join(".hidden"), b"hidden").unwrap();
    fs::write(root.join("empty.txt"), []).unwrap();
    assert!(
        create(
            &options(&root, &out),
            &mut |_, _| {},
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .to_string()
        .contains("something in it")
    );
    let path = tmp.path().join("payload");
    fs::write(&path, b"data").unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(
        create(
            &options(&link, &out),
            &mut |_, _| {},
            &AtomicBool::new(false)
        )
        .is_err()
    );
    for size in [0, 1, 16_385, 32 << 20, u32::MAX] {
        let mut opts = options(&path, &out);
        opts.piece_length = Some(size);
        assert!(create(&opts, &mut |_, _| {}, &AtomicBool::new(false)).is_err());
    }
    let mut opts = options(&path, &out);
    opts.web_seeds = vec!["ftp://example/file".into()];
    assert!(create(&opts, &mut |_, _| {}, &AtomicBool::new(false)).is_err());
    let result = create(
        &options(&path, &out),
        &mut |hashed, _| {
            if hashed == 0 {
                fs::write(&path, b"changed").unwrap();
            }
        },
        &AtomicBool::new(false),
    );
    assert!(result.unwrap_err().to_string().contains("changed"));
    assert!(!out.exists());
    let weird = tmp.path().join("a:b");
    fs::write(&weird, b"data").unwrap();
    assert!(
        create(
            &options(&weird, &out),
            &mut |_, _| {},
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .to_string()
        .contains("Rename")
    );
}

#[test]
fn auto_piece_length_table() {
    for (total, expected) in [
        (0, 16 << 10),
        (1, 16 << 10),
        (2_000 * (16 << 10), 16 << 10),
        (2_000 * (16 << 10) + 1, 32 << 10),
        (1 << 30, 1 << 20),
        (10 << 30, 8 << 20),
        (32 << 30, 16 << 20),
        (u64::MAX, 16 << 20),
    ] {
        assert_eq!(auto_piece_length(total), expected, "total={total}");
        assert!(auto_piece_length(total).is_power_of_two());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_roundtrip_engine_verifies_existing_folder_and_seeds() {
    let tmp = temp();
    let root = tmp.path().join("album");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.bin"), vec![4; 300_007]).unwrap();
    fs::write(root.join("sub/b.bin"), vec![8; 1_234_567]).unwrap();
    let opts = options(&root, &tmp.path().join("album.torrent"));
    let (created, meta) = make(&opts);
    verify(&meta, tmp.path());
    let settings = Settings {
        download_dir: tmp.path().to_owned(),
        listen_port: 0,
        enable_dht: false,
        enable_upnp: false,
        enable_lsd: false,
        public_trackers: false,
        ..Default::default()
    };
    let engine = Engine::start_with(tmp.path().join("state"), Some(settings))
        .await
        .unwrap();
    let hash = engine
        .add(AddTorrent::new(TorrentSource::File(opts.out.into())))
        .await
        .unwrap();
    assert_eq!(hash, created.info_hash);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = engine.snapshot();
        if snapshot
            .torrents
            .iter()
            .any(|torrent| torrent.info_hash == hash && torrent.status == TorrentStatus::Seeding)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "did not verify and seed: {:?}",
            snapshot.torrents
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let details = engine.details(&hash).unwrap();
    assert_eq!(details.hash_fails, 0);
    assert_eq!(details.summary.done_bytes, created.total_size);
    assert_eq!(engine.snapshot().stats.session_downloaded, 0);
    engine.shutdown().await;
}

/// Debug-profile, fixed-buffer, generated-content benchmark; removed on completion.
/// cargo test --offline -p trav-core --test create create_benchmark_gib -- --ignored --nocapture
#[test]
#[ignore = "generates and hashes 1 GiB; run explicitly for throughput verification"]
fn create_benchmark_gib() {
    let tmp = tempfile::Builder::new()
        .prefix("trav-create-benchmark-")
        .tempdir()
        .unwrap();
    let root = tmp.path().join("share");
    fs::create_dir_all(root.join("notes")).unwrap();
    let mut file = File::create(root.join("large.bin")).unwrap();
    let chunk: Vec<u8> = (0..1 << 20).map(|i| (i % 251) as u8).collect();
    for _ in 0..1024 {
        file.write_all(&chunk).unwrap();
    }
    file.sync_all().unwrap();
    drop(file);
    fs::write(
        root.join("notes/readme.txt"),
        b"Generated Trav creator verification folder.\n",
    )
    .unwrap();
    fs::write(root.join("empty.txt"), []).unwrap();
    fs::write(root.join(".DS_Store"), b"must not appear").unwrap();
    let mut opts = options(&root, &tmp.path().join("share.torrent"));
    opts.piece_length = None;
    let started = Instant::now();
    let created = create(&opts, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
    let elapsed = started.elapsed();
    let meta = Metainfo::from_bytes(&fs::read(&opts.out).unwrap()).unwrap();
    println!("Generated folder: {}", root.display());
    println!("Info hash: {}", created.info_hash);
    for file in &meta.info.files {
        println!("  {}: {} bytes", file.display_path(), file.length);
    }
    println!(
        "Hashed {} bytes in {:.6} s = {:.2} MB/s ({:.2} MiB/s); piece length {}, {} pieces",
        created.total_size,
        elapsed.as_secs_f64(),
        created.total_size as f64 / elapsed.as_secs_f64() / 1e6,
        created.total_size as f64 / elapsed.as_secs_f64() / 1048576.0,
        created.piece_length,
        created.num_pieces
    );
    assert_eq!(created.info_hash, hex::encode(meta.info_hash));
    tmp.close().unwrap();
    println!("Generated folder and torrent removed");
}

#[test]
fn create_cancel_while_streaming_keeps_the_previous_output() {
    let tmp = temp();
    let path = tmp.path().join("large.bin");
    File::create(&path).unwrap().set_len(256 << 20).unwrap();
    let out = tmp.path().join("previous.torrent");
    fs::write(&out, b"keep previous torrent").unwrap();
    let flag = AtomicBool::new(false);
    let (begin, wait) = std::sync::mpsc::channel();
    let (result, latency) = std::thread::scope(|scope| {
        let flag_ref = &flag;
        let cancel = scope.spawn(move || {
            wait.recv().unwrap();
            std::thread::sleep(Duration::from_millis(10));
            let at = Instant::now();
            flag_ref.store(true, Ordering::Relaxed);
            at
        });
        let mut first = true;
        let result = create(
            &options(&path, &out),
            &mut |_, _| {
                if first {
                    first = false;
                    begin.send(()).unwrap();
                }
            },
            &flag,
        );
        let finished = Instant::now();
        let cancelled_at = cancel.join().unwrap();
        (result, finished.saturating_duration_since(cancelled_at))
    });
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(fs::read(out).unwrap(), b"keep previous torrent");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 2);
    println!(
        "Mid-stream cancellation observed in {:.3} ms",
        latency.as_secs_f64() * 1_000.0
    );
    assert!(
        latency < Duration::from_secs(1),
        "cancellation stalled: {latency:?}"
    );
}

#[test]
fn create_failure_after_writing_temp_preserves_existing_output_and_cleans_up() {
    let tmp = temp();
    let path = tmp.path().join("payload");
    let moved = tmp.path().join("moved");
    let out = tmp.path().join("previous.torrent");
    fs::write(&path, b"content").unwrap();
    fs::write(&out, b"previous torrent").unwrap();
    let result = create(
        &options(&path, &out),
        &mut |hashed, total| {
            if hashed == total {
                fs::rename(&path, &moved).unwrap();
            }
        },
        &AtomicBool::new(false),
    );
    assert!(result.is_err());
    assert_eq!(fs::read(moved).unwrap(), b"content");
    assert_eq!(fs::read(out).unwrap(), b"previous torrent");
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        2,
        "temporary output was removed after failure"
    );
}

#[test]
fn a_destination_folder_swapped_for_a_symlink_while_hashing_is_never_written_through() {
    // QA round 3, F11: the .torrent goes into the folder chosen, or nowhere;
    // never into wherever a symlink put in its place points.
    let tmp = temp();
    let source = tmp.path().join("payload.bin");
    fs::write(&source, b"selected payload").unwrap();
    let (chosen, outside) = (tmp.path().join("chosen"), tmp.path().join("outside"));
    fs::create_dir(&chosen).unwrap();
    fs::create_dir(&outside).unwrap();
    let victim = outside.join("output.torrent");
    fs::write(&victim, b"UNSELECTED USER DATA").unwrap();
    let mut swapped = false;
    let result = create(
        &options(&source, &chosen.join("output.torrent")),
        &mut |hashed, _| {
            if hashed == 0 && !swapped {
                fs::rename(&chosen, tmp.path().join("chosen-moved")).unwrap();
                std::os::unix::fs::symlink(&outside, &chosen).unwrap();
                swapped = true;
            }
        },
        &AtomicBool::new(false),
    );
    assert!(swapped);
    assert!(result.is_err(), "a moved destination folder is refused");
    assert_eq!(fs::read(&victim).unwrap(), b"UNSELECTED USER DATA");
    let left: Vec<_> = fs::read_dir(tmp.path().join("chosen-moved"))
        .unwrap()
        .collect();
    assert!(left.is_empty(), "no temporary file left behind");
}
