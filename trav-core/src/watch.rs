//! The watch folder: `.torrent` files saved into it are added by themselves.
//!
//! A file is taken once its size has held still across two scans (a browser
//! may still be writing it), then renamed to `<name>.torrent.added` (or
//! `.invalid` when it isn't a torrent), so it's never added twice and the user
//! can see what happened to it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What a scan saw last time, by path: size, to know when writing has stopped.
#[derive(Default)]
pub(crate) struct Seen(HashMap<PathBuf, u64>);

/// The `.torrent` files in `dir` that are ready to add: their size is the same
/// as at the previous scan.
pub(crate) fn ready(dir: &Path, seen: &mut Seen) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        seen.0.clear();
        return Vec::new();
    };
    let mut now = HashMap::new();
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let is_torrent = path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("torrent"));
        let hidden = e.file_name().to_string_lossy().starts_with('.');
        let Ok(meta) = e.metadata() else { continue };
        if !is_torrent || hidden || !meta.is_file() {
            continue;
        }
        let len = meta.len();
        if len > 0 && seen.0.get(&path) == Some(&len) {
            out.push(path.clone());
        }
        now.insert(path, len);
    }
    seen.0 = now;
    out.sort();
    out
}

/// Mark a handled file: `x.torrent` → `x.torrent.added` / `.invalid`, never
/// over an existing file (`x.torrent.added (2)` and so on).
pub(crate) fn mark(path: &Path, suffix: &str) {
    let base = path.as_os_str().to_string_lossy().into_owned();
    let mut target = PathBuf::from(format!("{base}.{suffix}"));
    let mut n = 2;
    while target.symlink_metadata().is_ok() {
        target = PathBuf::from(format!("{base}.{suffix} ({n})"));
        n += 1;
    }
    if let Err(e) = std::fs::rename(path, &target) {
        tracing::warn!("watch folder: couldn't rename {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_a_file_once_its_size_holds_still() {
        let dir = tempfile::tempdir().unwrap();
        let mut seen = Seen::default();
        let t = dir.path().join("a.TORRENT");
        std::fs::write(&t, b"d4:").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.path().join(".hidden.torrent"), b"x").unwrap();
        assert!(ready(dir.path(), &mut seen).is_empty(), "first sighting");
        std::fs::write(&t, b"d4:more").unwrap();
        assert!(ready(dir.path(), &mut seen).is_empty(), "still growing");
        assert_eq!(ready(dir.path(), &mut seen), vec![t.clone()]);
    }

    #[test]
    fn marking_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("a.torrent");
        std::fs::write(dir.path().join("a.torrent.added"), b"old").unwrap();
        std::fs::write(&t, b"new").unwrap();
        mark(&t, "added");
        assert!(!t.exists());
        assert_eq!(
            std::fs::read(dir.path().join("a.torrent.added")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(dir.path().join("a.torrent.added (2)")).unwrap(),
            b"new"
        );
    }
}
