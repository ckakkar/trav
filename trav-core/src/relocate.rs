//! Moving a torrent's files to another folder ("Move Data…").
//!
//! All or nothing: if any destination already exists nothing moves, and a
//! failure part-way puts back what had moved. Files move by rename where they
//! can, and across volumes by copying, then removing the original only once
//! the copy is complete.

use std::io;
use std::path::{Path, PathBuf};

/// Move `files` (absolute paths under `from`) to the same places under `to`.
/// Files that don't exist (skipped, or not downloaded yet) are left alone.
pub fn relocate(files: &[PathBuf], from: &Path, to: &Path) -> Result<(), String> {
    let mut plan = Vec::new();
    for f in files {
        if f.symlink_metadata().is_err() {
            continue;
        }
        let rel = f
            .strip_prefix(from)
            .map_err(|_| format!("{} isn't in {}", f.display(), from.display()))?;
        plan.push((f.clone(), to.join(rel)));
    }
    for (_, dst) in &plan {
        if dst.symlink_metadata().is_ok() {
            return Err(format!("{} already exists", dst.display()));
        }
        refuse_symlinks(dst, to)?;
    }
    let mut moved: Vec<(&Path, &Path)> = Vec::new();
    for (src, dst) in &plan {
        if let Err(e) = move_one(src, dst) {
            for (s, d) in moved.iter().rev() {
                if let Err(back) = move_one(d, s) {
                    tracing::error!(
                        "couldn't put {} back at {}: {back}",
                        d.display(),
                        s.display()
                    );
                }
            }
            for (_, d) in &plan {
                if let Some(parent) = d.parent() {
                    prune_empty(parent, to);
                }
            }
            return Err(format!("couldn't move {}: {e}", src.display()));
        }
        moved.push((src, dst));
    }
    Ok(())
}

fn move_one(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match rename_no_replace(src, dst) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => copy_then_remove(src, dst),
        other => other,
    }
}

/// Another volume: copy into a file that must not exist yet, then remove the
/// original. If that removal fails, the copy (ours: we created it) goes, so
/// the move can be retried.
fn copy_then_remove(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = std::fs::metadata(src)?;
    let copied = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)?;
        let n = io::copy(&mut std::fs::File::open(src)?, &mut out)?;
        if n != meta.len() {
            return Err(io::Error::other(format!(
                "copied {n} of {} bytes",
                meta.len()
            )));
        }
        out.set_permissions(meta.permissions())?;
        out.sync_all()
    })();
    let done = copied.and_then(|()| std::fs::remove_file(src));
    if done.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    done
}

/// `rename` that fails rather than replace a file already at `dst` (two moves
/// into one folder must never overwrite each other's files).
fn rename_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(src.as_os_str().as_bytes())?;
    let to = CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: two NUL-terminated paths that outlive the call.
    #[cfg(target_os = "macos")]
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let rc = {
        let _ = (&from, &to);
        if dst.symlink_metadata().is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        return std::fs::rename(src, dst);
    };
    if rc == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    // A filesystem without exclusive renames: check, then rename.
    if matches!(
        e.raw_os_error(),
        Some(libc::EINVAL) | Some(libc::ENOTSUP) | Some(libc::ENOSYS)
    ) {
        if dst.symlink_metadata().is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        return std::fs::rename(src, dst);
    }
    Err(e)
}

/// Nothing between `root` and `path` may be a symlink: it would send the files
/// somewhere other than the folder that was chosen (as `Storage` refuses too).
fn refuse_symlinks(path: &Path, root: &Path) -> Result<(), String> {
    for a in path.ancestors().skip(1) {
        if a == root || !a.starts_with(root) {
            break;
        }
        if a.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(format!("{} is a symlink", a.display()));
        }
    }
    Ok(())
}

/// Remove the folders under (and including) `dir` that hold nothing but empty
/// folders (and Finder's `.DS_Store`); true if `dir` itself is gone.
pub fn prune_tree(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut empty = true;
    for e in entries.flatten() {
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if !(is_dir && prune_tree(&e.path()) || e.file_name() == ".DS_Store") {
            empty = false;
        }
    }
    if empty {
        let _ = std::fs::remove_file(dir.join(".DS_Store"));
    }
    empty && std::fs::remove_dir(dir).is_ok()
}

/// Remove `dir` and its parents up to (not including) `stop` while they are
/// empty (Finder's `.DS_Store` aside).
pub fn prune_empty(dir: &Path, stop: &Path) {
    let mut d = dir;
    while d != stop && d.starts_with(stop) {
        let Ok(entries) = std::fs::read_dir(d) else {
            return;
        };
        let mut only_ds_store = true;
        for e in entries.flatten() {
            if e.file_name() != ".DS_Store" {
                only_ds_store = false;
                break;
            }
        }
        if !only_ds_store {
            return;
        }
        let _ = std::fs::remove_file(d.join(".DS_Store"));
        if std::fs::remove_dir(d).is_err() {
            return;
        }
        let Some(parent) = d.parent() else { return };
        d = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(root: &Path, files: &[(&str, &[u8])]) -> Vec<PathBuf> {
        files
            .iter()
            .map(|(p, data)| {
                let path = root.join(p);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, data).unwrap();
                path
            })
            .collect()
    }

    #[test]
    fn moves_every_file_and_leaves_nothing_behind() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        let mut files = tree(
            from.path(),
            &[("Show/ep1.mkv", b"one"), ("Show/subs/en.srt", b"two")],
        );
        files.push(from.path().join("Show/skipped.mkv"));
        relocate(&files, from.path(), to.path()).unwrap();
        assert!(prune_tree(&from.path().join("Show")));
        assert_eq!(
            std::fs::read(to.path().join("Show/ep1.mkv")).unwrap(),
            b"one"
        );
        assert_eq!(
            std::fs::read(to.path().join("Show/subs/en.srt")).unwrap(),
            b"two"
        );
        assert!(!to.path().join("Show/skipped.mkv").exists());
        assert!(!from.path().join("Show").exists());
    }

    #[test]
    fn overwrites_nothing_and_moves_nothing_when_a_file_is_in_the_way() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        let files = tree(from.path(), &[("Show/a", b"a"), ("Show/b", b"b")]);
        tree(to.path(), &[("Show/b", b"theirs")]);
        let err = relocate(&files, from.path(), to.path()).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(std::fs::read(from.path().join("Show/a")).unwrap(), b"a");
        assert_eq!(std::fs::read(to.path().join("Show/b")).unwrap(), b"theirs");
        assert!(!to.path().join("Show/a").exists());
    }

    #[test]
    fn puts_back_what_moved_when_a_later_file_fails() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        let files = tree(from.path(), &[("Show/a", b"a"), ("Show/z/b", b"b")]);
        // A file where the second one's folder has to go makes its move fail.
        std::fs::create_dir_all(to.path().join("Show")).unwrap();
        std::fs::write(to.path().join("Show/z"), b"blocker").unwrap();
        assert!(relocate(&files, from.path(), to.path()).is_err());
        assert_eq!(std::fs::read(from.path().join("Show/a")).unwrap(), b"a");
        assert_eq!(std::fs::read(from.path().join("Show/z/b")).unwrap(), b"b");
        assert!(!to.path().join("Show/a").exists());
        assert_eq!(std::fs::read(to.path().join("Show/z")).unwrap(), b"blocker");
    }

    #[test]
    fn will_not_move_through_a_symlink() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let files = tree(from.path(), &[("Show/a", b"a")]);
        std::os::unix::fs::symlink(elsewhere.path(), to.path().join("Show")).unwrap();
        assert!(
            relocate(&files, from.path(), to.path())
                .unwrap_err()
                .contains("symlink")
        );
        assert!(from.path().join("Show/a").exists());
        assert!(!elsewhere.path().join("a").exists());
    }

    #[test]
    fn a_rename_never_replaces_a_file_that_appeared_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, b"mine").unwrap();
        std::fs::write(&b, b"theirs").unwrap();
        assert!(move_one(&a, &b).is_err());
        assert_eq!(std::fs::read(&a).unwrap(), b"mine");
        assert_eq!(std::fs::read(&b).unwrap(), b"theirs");
    }

    #[test]
    fn a_copy_whose_original_cant_be_removed_is_taken_back() {
        // QA round 2, F6: across volumes, a source that can't be unlinked
        // must not leave a stray copy that blocks retrying.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let (src, dst) = (locked.join("f"), dir.path().join("copy"));
        std::fs::write(&src, b"two").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let res = copy_then_remove(&src, &dst);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(res.is_err());
        assert_eq!(std::fs::read(&src).unwrap(), b"two");
        assert!(!dst.exists(), "no stray copy");
    }
}
