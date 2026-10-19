//! Piece storage over the torrent's file layout using positioned I/O.
//!
//! All methods here are blocking; async callers go through the `*_async`
//! wrappers which hop onto Tokio's blocking pool. Positioned reads/writes
//! (`pread`/`pwrite`) let concurrent jobs share one handle without seeking.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use sha1::{Digest, Sha1};

use crate::error::Result;
use crate::metainfo::Info;
use crate::path_safety;

const MAX_OPEN_HANDLES: usize = 256;

#[derive(Debug, Clone)]
pub struct StorageFile {
    pub path: PathBuf,
    pub offset: u64,
    pub length: u64,
    pub pad: bool,
}

pub struct Storage {
    /// The save folder: nothing below it may be a symlink.
    jail_root: PathBuf,
    /// `save_path/<name>` — the file (single) or folder (multi) shown to the user.
    content_root: PathBuf,
    files: Vec<StorageFile>,
    total_length: u64,
    handles: Mutex<HashMap<usize, Arc<File>>>,
    /// Set by [`Storage::retire`]; writes hold the read side while they run.
    retired: RwLock<bool>,
}

impl Storage {
    pub fn new(save_path: &Path, info: &Info) -> Result<Self> {
        let content_root = path_safety::jail_join(save_path, [info.name.as_str()])?;
        let files = info
            .files
            .iter()
            .map(|f| {
                let path = if info.multi_file {
                    path_safety::jail_join(
                        save_path,
                        std::iter::once(info.name.as_str())
                            .chain(f.path.iter().map(String::as_str)),
                    )?
                } else {
                    content_root.clone()
                };
                Ok(StorageFile {
                    path,
                    offset: f.offset,
                    length: f.length,
                    pad: f.pad,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut files = files;
        unique_paths(&mut files, save_path);
        Ok(Self {
            jail_root: save_path.to_path_buf(),
            content_root,
            files,
            total_length: info.total_length,
            handles: Mutex::new(HashMap::new()),
            retired: RwLock::new(false),
        })
    }

    pub fn content_root(&self) -> &Path {
        &self.content_root
    }

    pub fn files(&self) -> &[StorageFile] {
        &self.files
    }

    /// Whether any payload file already exists (decides if a hash check is worthwhile).
    pub fn any_file_exists(&self) -> bool {
        self.files.iter().any(|f| !f.pad && f.path.exists())
    }

    fn overlapping(
        &self,
        offset: u64,
        len: usize,
    ) -> impl Iterator<Item = (usize, &StorageFile, u64, usize, usize)> {
        let end = offset + len as u64;
        // Files are sorted by offset; binary search for the first overlap.
        let first = self
            .files
            .partition_point(|f| f.offset + f.length <= offset);
        self.files[first..]
            .iter()
            .enumerate()
            .take_while(move |(_, f)| f.offset < end)
            .filter(|(_, f)| f.length > 0)
            .map(move |(i, f)| {
                let s = offset.max(f.offset);
                let e = end.min(f.offset + f.length);
                (
                    first + i,
                    f,
                    s - f.offset,
                    (s - offset) as usize,
                    (e - offset) as usize,
                )
            })
    }

    fn handle(&self, idx: usize, create: bool) -> io::Result<Arc<File>> {
        if let Some(h) = self.handles.lock().get(&idx) {
            return Ok(h.clone());
        }
        let f = &self.files[idx];
        self.refuse_symlinks(&f.path)?;
        let file = if create {
            if let Some(parent) = f.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&f.path)?;
            // Sparse-allocate to the final size so later reads past EOF behave.
            if file.metadata()?.len() < f.length {
                file.set_len(f.length)?;
            }
            file
        } else {
            let open = |write: bool| {
                OpenOptions::new()
                    .read(true)
                    .write(write)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&f.path)
            };
            match open(true) {
                Ok(file) => file,
                Err(_) => open(false)?,
            }
        };
        let file = Arc::new(file);
        let mut map = self.handles.lock();
        if map.len() >= MAX_OPEN_HANDLES {
            map.clear();
        }
        map.insert(idx, file.clone());
        Ok(file)
    }

    /// Every component between the save folder and `path` must be a real file
    /// or directory. A symlink there, made by anything else, would redirect our
    /// writes outside the folder; the file itself is also opened with O_NOFOLLOW.
    fn refuse_symlinks(&self, path: &Path) -> io::Result<()> {
        for a in path.ancestors() {
            if a == self.jail_root || !a.starts_with(&self.jail_root) {
                break;
            }
            if std::fs::symlink_metadata(a).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("refusing to write through a symlink: {}", a.display()),
                ));
            }
        }
        Ok(())
    }

    pub fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let retired = self.retired.read();
        if *retired {
            return Err(io::Error::other("the torrent was removed"));
        }
        for (idx, f, file_off, a, b) in self.overlapping(offset, data.len()) {
            if f.pad {
                continue;
            }
            let h = self.handle(idx, true)?;
            write_all_at(&h, &data[a..b], file_off)?;
        }
        Ok(())
    }

    pub fn read(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        if offset + len as u64 > self.total_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "read past end of torrent",
            ));
        }
        let mut buf = vec![0u8; len];
        for (idx, f, file_off, a, b) in self.overlapping(offset, len) {
            if f.pad {
                continue; // already zeroed
            }
            let h = self.handle(idx, false)?;
            read_exact_at(&h, &mut buf[a..b], file_off)?;
        }
        Ok(buf)
    }

    /// Read a piece and compare against its expected hash. Missing files read as "not present".
    pub fn verify(&self, offset: u64, len: usize, expected: &[u8; 20]) -> bool {
        match self.read(offset, len) {
            Ok(data) => Sha1::digest(&data).as_slice() == expected,
            Err(_) => false,
        }
    }

    /// True when every file holding a verified piece still exists at full size.
    /// Cheap metadata check used to decide whether resume data can be trusted.
    pub fn consistent_with(&self, have: &crate::bitfield::Bitfield, piece_length: u32) -> bool {
        self.files
            .iter()
            .filter(|f| !f.pad && f.length > 0)
            .all(|f| {
                let first = (f.offset / piece_length as u64) as usize;
                let last = ((f.offset + f.length - 1) / piece_length as u64) as usize;
                let needed = (first..=last).any(|i| have.get(i));
                !needed || std::fs::metadata(&f.path).is_ok_and(|m| m.len() >= f.length)
            })
    }

    /// Zero-length files never receive a write; materialise them explicitly.
    pub fn create_empty_files(&self) {
        let retired = self.retired.read();
        if *retired {
            return;
        }
        for f in self.files.iter().filter(|f| f.length == 0 && !f.pad) {
            if self.refuse_symlinks(&f.path).is_err() {
                continue;
            }
            if let Some(parent) = f.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&f.path);
        }
    }

    pub fn flush(&self) {
        for h in self.handles.lock().values() {
            let _ = h.sync_data();
        }
    }

    pub fn close(&self) {
        self.handles.lock().clear();
    }

    /// Refuse every write from now on, once those in progress finish. A
    /// removed torrent can still have pieces in flight (peers held up by the
    /// rate limiter, hash checks), and they must not recreate its files.
    pub fn retire(&self) {
        *self.retired.write() = true;
    }

    /// Delete payload files and any directories left empty under the content root.
    pub fn delete_files(&self) {
        self.close();
        for f in &self.files {
            let _ = std::fs::remove_file(&f.path);
        }
        if self.content_root.is_dir() {
            remove_empty_dirs(&self.content_root);
        }
    }

    pub async fn read_async(self: &Arc<Self>, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let me = self.clone();
        tokio::task::spawn_blocking(move || me.read(offset, len))
            .await
            .map_err(io::Error::other)?
    }

    pub async fn write_async(self: &Arc<Self>, offset: u64, data: Vec<u8>) -> io::Result<()> {
        let me = self.clone();
        tokio::task::spawn_blocking(move || me.write(offset, &data))
            .await
            .map_err(io::Error::other)?
    }
}

/// Give every payload file its own path. Sanitizing can map distinct names
/// onto one (`a:b` and `a?b` both become `a_b`), and macOS volumes ignore
/// case and Unicode normalization; two files on one path would overwrite each other while each piece
/// still passes its hash check. Later duplicates become "name (2).ext", ….
fn unique_paths(files: &mut [StorageFile], root: &Path) {
    let key = path_safety::fs_key;
    // Directories files live in are taken too: a file can't share a folder's name.
    let mut taken: HashSet<String> = files
        .iter()
        .filter(|f| !f.pad)
        .flat_map(|f| {
            f.path
                .ancestors()
                .skip(1)
                .take_while(|a| a.starts_with(root) && *a != root)
                .map(key)
                .collect::<Vec<_>>()
        })
        .collect();
    for f in files.iter_mut().filter(|f| !f.pad) {
        if taken.insert(key(&f.path)) {
            continue;
        }
        let stem = f
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = f
            .path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        for n in 2.. {
            let candidate = f.path.with_file_name(format!("{stem} ({n}){ext}"));
            if taken.insert(key(&candidate)) {
                f.path = candidate;
                break;
            }
        }
    }
}

fn remove_empty_dirs(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut empty = true;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if !remove_empty_dirs(&p) {
                empty = false;
            }
        } else {
            empty = false;
        }
    }
    empty && std::fs::remove_dir(dir).is_ok()
}

#[cfg(unix)]
fn write_all_at(f: &File, buf: &[u8], off: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, off)
}

#[cfg(unix)]
fn read_exact_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(f, buf, off)
}

#[cfg(windows)]
fn write_all_at(f: &File, mut buf: &[u8], mut off: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_write(buf, off)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        off += n as u64;
    }
    Ok(())
}

#[cfg(windows)]
fn read_exact_at(f: &File, mut buf: &mut [u8], mut off: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_read(buf, off)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[n..];
        off += n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metainfo::FileEntry;

    fn info(files: &[(&str, u64)]) -> Info {
        let mut off = 0;
        let files: Vec<FileEntry> = files
            .iter()
            .map(|(n, l)| {
                let f = FileEntry {
                    path: vec![n.to_string()],
                    length: *l,
                    offset: off,
                    pad: false,
                };
                off += l;
                f
            })
            .collect();
        let pieces = vec![[0u8; 20]; off.div_ceil(4) as usize];
        Info {
            name: "t".into(),
            piece_length: 4,
            pieces,
            files,
            multi_file: true,
            total_length: off,
            private: false,
            raw: vec![],
        }
    }

    #[test]
    fn spans_file_boundaries() {
        let dir = std::env::temp_dir().join(format!("trav-storage-{}", std::process::id()));
        let s = Storage::new(&dir, &info(&[("a", 3), ("empty", 0), ("b", 5)])).unwrap();
        s.write(0, b"abcdefgh").unwrap();
        assert_eq!(std::fs::read(dir.join("t/a")).unwrap(), b"abc");
        assert_eq!(std::fs::read(dir.join("t/b")).unwrap(), b"defgh");
        assert_eq!(s.read(2, 4).unwrap(), b"cdef");
        s.delete_files();
        assert!(!dir.join("t").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn colliding_names_get_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(
            dir.path(),
            &info(&[("a:b.txt", 4), ("a?b.txt", 4), ("README", 4), ("readme", 4)]),
        )
        .unwrap();
        let names: Vec<String> = s
            .files()
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a_b.txt", "a_b (2).txt", "README", "readme (2)"]);
        s.write(0, b"1111222233334444").unwrap();
        assert_eq!(s.read(0, 16).unwrap(), b"1111222233334444");
    }

    #[test]
    fn unicode_equivalent_names_get_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path(), &info(&[("caf\u{e9}", 4), ("cafe\u{301}", 4)])).unwrap();
        assert_eq!(s.files()[1].path.file_name().unwrap(), "cafe\u{301} (2)");
        s.write(0, b"aaaabbbb").unwrap();
        assert_eq!(s.read(0, 8).unwrap(), b"aaaabbbb");
    }

    #[test]
    fn a_retired_storage_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path(), &info(&[("a", 4)])).unwrap();
        s.retire();
        assert!(s.write(0, b"late").is_err());
        assert!(!dir.path().join("t").exists());
    }

    #[test]
    fn a_file_cannot_take_a_folders_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut i = info(&[("x", 4), ("y", 4)]);
        i.files[1].path = vec!["x".into(), "inner".into()];
        let s = Storage::new(dir.path(), &i).unwrap();
        assert_eq!(s.files()[0].path.file_name().unwrap(), "x (2)");
        s.write(0, b"aaaabbbb").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("t/x/inner")).unwrap(),
            b"bbbb"
        );
    }

    #[test]
    fn will_not_write_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // Something else left a symlink where the torrent's folder goes.
        std::os::unix::fs::symlink(outside.path(), dir.path().join("t")).unwrap();
        let s = Storage::new(dir.path(), &info(&[("a", 4)])).unwrap();
        let err = s.write(0, b"evil").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(!outside.path().join("a").exists());

        // A symlinked file is refused too.
        std::fs::remove_file(dir.path().join("t")).unwrap();
        std::fs::create_dir(dir.path().join("t")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("target"), dir.path().join("t/a")).unwrap();
        assert!(s.write(0, b"evil").is_err());
        assert!(!outside.path().join("target").exists());
    }

    #[test]
    fn the_save_folder_itself_may_be_a_symlink() {
        let real = tempfile::tempdir().unwrap();
        let links = tempfile::tempdir().unwrap();
        let save = links.path().join("Downloads");
        std::os::unix::fs::symlink(real.path(), &save).unwrap();
        let s = Storage::new(&save, &info(&[("a", 4)])).unwrap();
        s.write(0, b"good").unwrap();
        assert_eq!(std::fs::read(real.path().join("t/a")).unwrap(), b"good");
    }
}
