//! The watch folder: `.torrent` files saved into it are added by themselves.
//!
//! A file is taken once its size has held still across two scans (a browser
//! may still be writing it), then renamed to `<name>.torrent.added` (or
//! `.invalid` when it isn't a torrent), so it's never added twice and the user
//! can see what happened to it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// A file [`mark`] renamed: where it is now, where it was, and its size and
/// modification time when renamed.
pub(crate) struct Marked {
    pub path: PathBuf,
    from: PathBuf,
    stamp: Option<(u64, Option<SystemTime>)>,
}

fn stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let m = std::fs::symlink_metadata(path).ok()?;
    m.is_file().then(|| (m.len(), m.modified().ok()))
}

/// Mark a handled file: `x.torrent` → `x.torrent.added` / `.invalid`, never
/// over an existing file (`x.torrent.added (2)` and so on).
pub(crate) fn mark(path: &Path, suffix: &str) -> Option<Marked> {
    let base = path.as_os_str().to_string_lossy().into_owned();
    let mut target = PathBuf::from(format!("{base}.{suffix}"));
    let mut n = 2;
    while target.symlink_metadata().is_ok() {
        target = PathBuf::from(format!("{base}.{suffix} ({n})"));
        n += 1;
    }
    if let Err(e) = std::fs::rename(path, &target) {
        tracing::warn!("watch folder: couldn't rename {}: {e}", path.display());
        return None;
    }
    let stamp = stamp(&target);
    Some(Marked {
        path: target,
        from: path.to_path_buf(),
        stamp,
    })
}

/// Files marked invalid that have changed since are still being written: put
/// each back under its old name (if that's free) to be read again once it
/// holds still. Returns the ones still waiting.
pub(crate) fn revisit(marks: HashMap<PathBuf, Marked>) -> HashMap<PathBuf, Marked> {
    marks
        .into_iter()
        .filter(|(_, m)| {
            let now = stamp(&m.path);
            if now.is_none() {
                return false; // gone (the user moved or deleted it)
            }
            if now == m.stamp {
                return true;
            }
            if m.from.symlink_metadata().is_ok() {
                return false; // something new is there; leave both alone
            }
            match std::fs::rename(&m.path, &m.from) {
                Ok(()) => false,
                Err(e) => {
                    tracing::warn!("watch folder: couldn't restore {}: {e}", m.path.display());
                    false
                }
            }
        })
        .collect()
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
        assert!(mark(&t, "added").is_some());
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

    #[test]
    fn a_file_marked_invalid_that_grows_again_goes_back() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("slow.torrent");
        std::fs::write(&t, b"d8:").unwrap();
        let m = mark(&t, "invalid").unwrap();
        let mut marks = HashMap::from([(m.path.clone(), m)]);
        marks = revisit(marks);
        assert_eq!(marks.len(), 1, "unchanged: still marked");
        use std::io::Write;
        let marked = dir.path().join("slow.torrent.invalid");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&marked)
            .unwrap()
            .write_all(b"announce")
            .unwrap();
        assert!(revisit(marks).is_empty());
        assert!(t.exists() && !marked.exists(), "back under its own name");
    }
}
