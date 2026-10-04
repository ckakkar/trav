//! Piece storage over the torrent's file layout using positioned I/O.
//!
//! All methods here are blocking; async callers go through the `*_async`
//! wrappers which hop onto Tokio's blocking pool. Positioned reads/writes
//! (`pread`/`pwrite`) let concurrent jobs share one handle without seeking.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
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
    /// `save_path/<name>` — the file (single) or folder (multi) shown to the user.
    content_root: PathBuf,
    files: Vec<StorageFile>,
    total_length: u64,
    handles: Mutex<HashMap<usize, Arc<File>>>,
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
                        std::iter::once(info.name.as_str()).chain(f.path.iter().map(String::as_str)),
                    )?
                } else {
                    content_root.clone()
                };
                Ok(StorageFile { path, offset: f.offset, length: f.length, pad: f.pad })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { content_root, files, total_length: info.total_length, handles: Mutex::new(HashMap::new()) })
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

    fn overlapping(&self, offset: u64, len: usize) -> impl Iterator<Item = (usize, &StorageFile, u64, usize, usize)> {
        let end = offset + len as u64;
        // Files are sorted by offset; binary search for the first overlap.
        let first = self.files.partition_point(|f| f.offset + f.length <= offset);
        self.files[first..]
            .iter()
            .enumerate()
            .take_while(move |(_, f)| f.offset < end)
            .filter(|(_, f)| f.length > 0)
            .map(move |(i, f)| {
                let s = offset.max(f.offset);
                let e = end.min(f.offset + f.length);
                (first + i, f, s - f.offset, (s - offset) as usize, (e - offset) as usize)
            })
    }

    fn handle(&self, idx: usize, create: bool) -> io::Result<Arc<File>> {
        if let Some(h) = self.handles.lock().get(&idx) {
            return Ok(h.clone());
        }
        let f = &self.files[idx];
        let file = if create {
            if let Some(parent) = f.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&f.path)?;
            // Sparse-allocate to the final size so later reads past EOF behave.
            if file.metadata()?.len() < f.length {
                file.set_len(f.length)?;
            }
            file
        } else {
            match OpenOptions::new().read(true).write(true).open(&f.path) {
                Ok(file) => file,
                Err(_) => File::open(&f.path)?,
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

    pub fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
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
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "read past end of torrent"));
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

    /// Zero-length files never receive a write; materialise them explicitly.
    pub fn create_empty_files(&self) {
        for f in self.files.iter().filter(|f| f.length == 0 && !f.pad) {
            if let Some(parent) = f.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = OpenOptions::new().write(true).create(true).truncate(false).open(&f.path);
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

fn remove_empty_dirs(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
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
                let f = FileEntry { path: vec![n.to_string()], length: *l, offset: off, pad: false };
                off += l;
                f
            })
            .collect();
        let pieces = vec![[0u8; 20]; off.div_ceil(4) as usize];
        Info { name: "t".into(), piece_length: 4, pieces, files, multi_file: true, total_length: off, private: false, raw: vec![] }
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
}
