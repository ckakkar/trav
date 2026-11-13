//! Blocking v1 torrent creation. Content is streamed; metadata is committed by
//! one atomic rename only after hashing, validation, and cancellation checks.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, OsString};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::bencode::{DictBuilder, Value};
use crate::error::{Error, Result};
use crate::metainfo::{Metainfo, normalize_tiers};
use crate::path_safety::{fs_key, sanitize_component};

const MIN_PIECE: u32 = 16 * 1024;
const MAX_PIECE: u32 = 16 * 1024 * 1024;
const BUFFER: usize = 256 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_PIECES: u64 = 4_000_000;
const EMPTY: &str = "Choose a file or folder with something in it";

type Jobs = Mutex<HashMap<String, Arc<AtomicBool>>>;
static JOBS: OnceLock<Jobs> = OnceLock::new();

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CreateOptions {
    pub id: String,
    pub path: String,
    pub out: String,
    pub trackers: Vec<Vec<String>>,
    pub web_seeds: Vec<String>,
    pub private: bool,
    pub comment: Option<String>,
    pub source: Option<String>,
    pub piece_length: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub info_hash: String,
    pub name: String,
    pub total_size: u64,
    pub piece_length: u32,
    pub num_pieces: usize,
    pub out: String,
}

/// Register before dispatching to the blocking pool so an immediate Cancel
/// cannot arrive before the worker has registered. Keep this guard in the worker.
pub fn register(id: &str) -> Result<CreateJob> {
    if id.trim().is_empty() || id.len() > 128 {
        return Err(bad("A creation ID of 1–128 characters is required"));
    }
    let mut jobs = JOBS.get_or_init(Jobs::default).lock();
    if jobs.contains_key(id) {
        return Err(bad("A torrent is already being created with this ID"));
    }
    if jobs.len() >= 8 {
        return Err(bad("Wait for another torrent to finish creating"));
    }
    let flag = Arc::new(AtomicBool::new(false));
    jobs.insert(id.to_owned(), flag.clone());
    Ok(CreateJob {
        id: id.to_owned(),
        flag,
    })
}

pub struct CreateJob {
    id: String,
    flag: Arc<AtomicBool>,
}

impl CreateJob {
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.flag
    }
}

impl Drop for CreateJob {
    fn drop(&mut self) {
        if let Some(jobs) = JOBS.get() {
            let mut jobs = jobs.lock();
            if jobs
                .get(&self.id)
                .is_some_and(|flag| Arc::ptr_eq(flag, &self.flag))
            {
                jobs.remove(&self.id);
            }
        }
    }
}

/// False means that this job has already finished or was never registered.
pub fn cancel(id: &str) -> bool {
    let Some(jobs) = JOBS.get() else { return false };
    let jobs = jobs.lock();
    let Some(flag) = jobs.get(id) else {
        return false;
    };
    flag.store(true, Ordering::Relaxed);
    true
}

/// Smallest allowed power of two giving at most 2,000 pieces, subject to the
/// size limits. Away from those limits this gives approximately 1,000–2,000.
pub fn auto_piece_length(total: u64) -> u32 {
    let mut piece = MIN_PIECE;
    while piece < MAX_PIECE && total.div_ceil(u64::from(piece)) > 2_000 {
        piece *= 2;
    }
    piece
}

fn bad(message: impl Into<String>) -> Error {
    Error::Engine(message.into())
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(bad("Torrent creation cancelled"))
    } else {
        Ok(())
    }
}

fn io_at(action: &str, path: &Path, error: io::Error) -> Error {
    bad(format!("Could not {action} “{}”: {error}", path.display()))
}

fn component(name: &str) -> Result<()> {
    if name.is_empty() || sanitize_component(name) != name {
        return Err(bad(format!("Rename “{name}” so it can be seeded safely")));
    }
    Ok(())
}

#[derive(Clone)]
struct Entry {
    parts: Vec<String>,
    meta: Metadata,
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    same_file(a, b)
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

fn c_name(name: &str) -> io::Result<CString> {
    CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Name contains a NUL byte"))
}

/// Every component below the selected directory is opened relative to a held
/// descriptor, with O_NOFOLLOW. A replaced parent symlink cannot escape it.
fn open_child(parent: &File, name: &str) -> io::Result<File> {
    let name = c_name(name)?;
    // SAFETY: parent is live, name is NUL-terminated; ownership of a successful
    // descriptor is transferred exactly once to File.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_relative(root: &File, parts: &[String]) -> io::Result<File> {
    let mut file = root.try_clone()?;
    for part in parts {
        file = open_child(&file, part)?;
    }
    Ok(file)
}

struct Directory(*mut libc::DIR);
impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: fdopendir gave this guard sole ownership of the directory.
        unsafe {
            libc::closedir(self.0);
        }
    }
}

// readdir distinguishes end-of-directory from an error through errno.
fn clear_errno() {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        *libc::__errno_location() = 0;
    }
}

fn children(dir: &File, cancel: &AtomicBool) -> Result<Vec<OsString>> {
    // SAFETY: duplicating a live descriptor; fdopendir owns only the duplicate.
    let fd = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let raw = unsafe { libc::fdopendir(fd) };
    if raw.is_null() {
        let error = io::Error::last_os_error();
        unsafe {
            libc::close(fd);
        }
        return Err(error.into());
    }
    let directory = Directory(raw);
    let mut names = Vec::new();
    loop {
        check_cancel(cancel)?;
        clear_errno();
        // SAFETY: directory is live and not shared; copy the name before the
        // next readdir invalidates the returned entry. POSIX terminates d_name.
        let entry = unsafe { libc::readdir(directory.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error.into());
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name.starts_with(b".") {
            continue;
        }
        if names.len() >= MAX_ENTRIES {
            return Err(bad("This folder contains too many files"));
        }
        names.push(OsString::from_vec(name.to_vec()));
    }
    Ok(names)
}

fn child_kind(dir: &File, name: &str) -> io::Result<libc::mode_t> {
    let name = c_name(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: valid directory/name and writable stat storage; only read on success.
    if unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { stat.assume_init() }.st_mode & libc::S_IFMT)
}

fn collect(root: &File, meta: &Metadata, cancel: &AtomicBool) -> Result<Vec<Entry>> {
    if meta.is_file() {
        return Ok(vec![Entry {
            parts: vec![],
            meta: meta.clone(),
        }]);
    }
    if !meta.is_dir() {
        return Err(bad(EMPTY));
    }
    let mut stack = vec![(Vec::<String>::new(), meta.clone())];
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    while let Some((parts, expected)) = stack.pop() {
        check_cancel(cancel)?;
        let dir = open_relative(root, &parts)?;
        if !same_file(&expected, &dir.metadata()?) {
            return Err(bad("The selected folder changed; try again"));
        }
        for name in children(&dir, cancel)? {
            check_cancel(cancel)?;
            let name = name.to_str().ok_or_else(|| {
                bad("Rename files with non-UTF-8 names before creating a torrent")
            })?;
            let kind = child_kind(&dir, name)?;
            if kind != libc::S_IFREG && kind != libc::S_IFDIR {
                continue;
            }
            component(name)?;
            let mut path = parts.clone();
            path.push(name.to_owned());
            if path.len() > 128 {
                return Err(bad("This folder has too many nested directories"));
            }
            if !seen.insert(fs_key(&path.iter().collect::<PathBuf>())) {
                return Err(bad(
                    "Rename files whose names differ only in case or Unicode spelling",
                ));
            }
            if seen.len() > MAX_ENTRIES {
                return Err(bad("This folder contains too many files"));
            }
            let file = open_child(&dir, name)?;
            let meta = file.metadata()?;
            if meta.is_dir() {
                stack.push((path, meta));
            } else if meta.is_file() {
                entries.push(Entry { parts: path, meta });
            }
        }
    }
    entries.sort_by(|a, b| a.parts.cmp(&b.parts));
    Ok(entries)
}

fn urls(input: &[String], tracker: bool) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for raw in input {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }
        let parsed = url::Url::parse(value).map_err(|_| bad(format!("Invalid URL: {value}")))?;
        let allowed =
            matches!(parsed.scheme(), "http" | "https") || (tracker && parsed.scheme() == "udp");
        if !allowed || parsed.host_str().is_none() {
            return Err(bad(if tracker {
                "Tracker URLs must use http://, https:// or udp://"
            } else {
                "Web seeds must use http:// or https://"
            }));
        }
        let value = parsed.to_string();
        if !out.contains(&value) {
            out.push(value);
        }
    }
    Ok(out)
}

/// Blocking worker. Progress is emitted initially, at most every 100 ms while
/// hashing, and finally. No payload-sized allocation or piece buffer is needed.
pub fn create(
    opts: &CreateOptions,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<Created> {
    check_cancel(cancel)?;
    if opts.path.trim().is_empty() {
        return Err(bad(EMPTY));
    }
    if opts.out.trim().is_empty() {
        return Err(bad("Choose where to save the .torrent file"));
    }
    let input = Path::new(&opts.path);
    let original = fs::symlink_metadata(input).map_err(|e| io_at("read", input, e))?;
    if original.file_type().is_symlink() {
        return Err(bad("Choose the original file or folder, not a symlink"));
    }
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(input)
        .map_err(|e| io_at("open", input, e))?;
    let meta = root.metadata()?;
    if !same_file(&original, &meta) {
        return Err(bad("The selection changed; try again"));
    }
    let input = fs::canonicalize(input).map_err(|e| io_at("read", input, e))?;
    let name = input
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| bad(EMPTY))?
        .to_owned();
    component(&name)?;
    if name.starts_with('.') {
        return Err(bad(EMPTY));
    }
    let entries = collect(&root, &meta, cancel)?;
    let total = entries
        .iter()
        .try_fold(0u64, |sum, entry| sum.checked_add(entry.meta.len()))
        .filter(|total| *total > 0 && *total <= i64::MAX as u64)
        .ok_or_else(|| bad(EMPTY))?;
    let piece_length = opts
        .piece_length
        .unwrap_or_else(|| auto_piece_length(total));
    if !(MIN_PIECE..=MAX_PIECE).contains(&piece_length) || !piece_length.is_power_of_two() {
        return Err(bad(
            "Choose a power-of-two piece size between 16 KiB and 16 MiB",
        ));
    }
    let count = total.div_ceil(u64::from(piece_length));
    if count > MAX_PIECES {
        return Err(bad(
            "This selection needs too many pieces; choose a larger piece size",
        ));
    }
    let tiers = opts
        .trackers
        .iter()
        .map(|tier| urls(tier, true))
        .collect::<Result<Vec<_>>>()?;
    let tiers = normalize_tiers(tiers);
    let web_seeds = urls(&opts.web_seeds, false)?;
    if web_seeds.len() > 16 {
        return Err(bad("Use at most 16 web seeds"));
    }
    let requested_out = Path::new(&opts.out);
    let parent = requested_out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent =
        fs::canonicalize(parent).map_err(|e| io_at("open the destination folder", parent, e))?;
    // Held open from here on: the .torrent is written and put in place
    // relative to this folder, so a path swapped for a symlink while hashing
    // can't send it anywhere else.
    let out_dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&parent)
        .map_err(|e| io_at("open the destination folder", &parent, e))?;
    let out_name = requested_out
        .file_name()
        .ok_or_else(|| bad("Choose a .torrent filename"))?;
    let out_c =
        CString::new(out_name.as_encoded_bytes()).map_err(|_| bad("Choose a .torrent filename"))?;
    let out = parent.join(out_name);
    refuse_input_output(&out, &input, &entries, &meta)?;
    progress(0, total);
    check_cancel(cancel)?;
    let mut last_progress = Instant::now();
    let mut hashed = 0u64;
    let mut in_piece = 0u32;
    let mut hash = Sha1::new();
    let mut pieces = Vec::with_capacity(count as usize * 20);
    let mut buffer = vec![0u8; BUFFER];
    for entry in &entries {
        check_cancel(cancel)?;
        let mut file = if meta.is_dir() {
            open_relative(&root, &entry.parts)?
        } else {
            root.try_clone()?
        };
        if !unchanged(&entry.meta, &file.metadata()?) {
            return Err(bad("A selected file changed; try again"));
        }
        let mut remaining = entry.meta.len();
        while remaining > 0 {
            check_cancel(cancel)?;
            let size = remaining.min(BUFFER as u64) as usize;
            let n = file
                .read(&mut buffer[..size])
                .map_err(|e| io_at("read content", &input, e))?;
            if n == 0 {
                return Err(bad("A selected file became shorter; try again"));
            }
            let mut data = &buffer[..n];
            while !data.is_empty() {
                let take = data.len().min((piece_length - in_piece) as usize);
                hash.update(&data[..take]);
                in_piece += take as u32;
                data = &data[take..];
                if in_piece == piece_length {
                    pieces.extend_from_slice(&hash.finalize_reset());
                    in_piece = 0;
                }
            }
            hashed += n as u64;
            remaining -= n as u64;
            if last_progress.elapsed() >= Duration::from_millis(100) && hashed != total {
                progress(hashed, total);
                last_progress = Instant::now();
            }
        }
        if !unchanged(&entry.meta, &file.metadata()?) {
            return Err(bad("A selected file changed while hashing; try again"));
        }
    }
    if in_piece > 0 {
        pieces.extend_from_slice(&hash.finalize());
    }
    check_cancel(cancel)?;
    progress(total, total);
    check_cancel(cancel)?;
    let mut info = DictBuilder::new()
        .bytes("name", name.as_bytes())
        .int("piece length", i64::from(piece_length))
        .bytes("pieces", pieces);
    if meta.is_dir() {
        info = info.value(
            "files",
            Value::List(
                entries
                    .iter()
                    .map(|entry| {
                        DictBuilder::new()
                            .int("length", entry.meta.len() as i64)
                            .value(
                                "path",
                                Value::List(
                                    entry
                                        .parts
                                        .iter()
                                        .map(|part| Value::Bytes(part.as_bytes().to_vec()))
                                        .collect(),
                                ),
                            )
                            .build()
                    })
                    .collect(),
            ),
        );
    } else {
        info = info.int("length", total as i64);
    }
    if opts.private {
        info = info.int("private", 1);
    }
    if let Some(source) = opts.source.as_deref().filter(|s| !s.is_empty()) {
        info = info.bytes("source", source);
    }
    let mut top = DictBuilder::new()
        .value("info", info.build())
        .bytes("created by", format!("Trav {}", env!("CARGO_PKG_VERSION")))
        .int(
            "creation date",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .min(i64::MAX as u64) as i64,
        );
    if let Some(first) = tiers.first().and_then(|tier| tier.first()) {
        top = top.bytes("announce", first.as_bytes());
    }
    if !tiers.is_empty() {
        top = top.value(
            "announce-list",
            Value::List(
                tiers
                    .iter()
                    .map(|tier| {
                        Value::List(
                            tier.iter()
                                .map(|url| Value::Bytes(url.as_bytes().to_vec()))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
        );
    }
    if !web_seeds.is_empty() {
        top = top.value(
            "url-list",
            Value::List(
                web_seeds
                    .iter()
                    .map(|url| Value::Bytes(url.as_bytes().to_vec()))
                    .collect(),
            ),
        );
    }
    if let Some(comment) = &opts.comment {
        top = top.bytes("comment", comment.as_bytes());
    }
    let bytes = top.build().encode();
    let parsed = Metainfo::from_bytes(&bytes)?;
    check_cancel(cancel)?;
    let mut output = AtomicOutput::new(&out_dir, &out)?;
    for chunk in bytes.chunks(BUFFER) {
        check_cancel(cancel)?;
        output.file.write_all(chunk)?;
    }
    output.file.sync_all()?;
    check_cancel(cancel)?;
    // A descriptor remains valid after a rename. Check that the selected paths
    // still name the files we hashed before committing a torrent for them.
    let current = fs::symlink_metadata(&input)
        .map_err(|error| io_at("find the selected content", &input, error))?;
    if !same_file(&meta, &current) || current.file_type().is_symlink() {
        return Err(bad("The selected content moved or changed; try again"));
    }
    for entry in &entries {
        check_cancel(cancel)?;
        let file = if meta.is_dir() {
            open_relative(&root, &entry.parts)?
        } else {
            root.try_clone()?
        };
        if !unchanged(&entry.meta, &file.metadata()?) {
            return Err(bad("A selected file changed; try again"));
        }
    }
    // The checks below go by path: the path must still be the folder held open.
    let held = out_dir.metadata()?;
    let still = fs::symlink_metadata(&parent).ok();
    if !still.is_some_and(|m| m.is_dir() && same_file(&m, &held)) {
        return Err(bad("The destination folder moved or changed; try again"));
    }
    refuse_input_output(&out, &input, &entries, &meta)?;
    check_cancel(cancel)?;
    output
        .commit(&out_c)
        .map_err(|e| io_at("save the torrent", &out, e))?;
    Ok(Created {
        info_hash: hex::encode(parsed.info_hash),
        name,
        total_size: total,
        piece_length,
        num_pieces: count as usize,
        out: out.to_string_lossy().into_owned(),
    })
}

fn refuse_input_output(out: &Path, input: &Path, entries: &[Entry], root: &Metadata) -> Result<()> {
    let existing = match fs::symlink_metadata(out) {
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(bad(
                    "Choose a destination that is a regular file, not a folder or symlink",
                ));
            }
            Some(meta)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_at("check the destination", out, error)),
    };
    let key = fs_key(out);
    if key == fs_key(input)
        || existing.as_ref().is_some_and(|out| same_file(root, out))
        || entries.iter().any(|entry| {
            let path = if root.is_dir() {
                input.join(entry.parts.iter().collect::<PathBuf>())
            } else {
                input.to_owned()
            };
            key == fs_key(&path)
                || existing
                    .as_ref()
                    .is_some_and(|out| same_file(&entry.meta, out))
        })
    {
        return Err(bad(
            "Save the .torrent somewhere other than the selected content",
        ));
    }
    Ok(())
}

/// The .torrent being written: a 0600 temporary file beside the chosen name,
/// in the folder held open, renamed over it only once complete.
struct AtomicOutput {
    file: File,
    dir: File,
    temp: CString,
    committed: bool,
}

impl AtomicOutput {
    fn new(dir: &File, out: &Path) -> Result<Self> {
        let dir = dir.try_clone()?;
        for _ in 0..32 {
            let temp = c_name(&format!(".trav-create-{:016x}.tmp", rand::random::<u64>()))?;
            // SAFETY: dir is a live directory descriptor and temp is
            // NUL-terminated; a successful descriptor is owned once, by File.
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    temp.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600 as libc::c_uint,
                )
            };
            if fd >= 0 {
                let file = unsafe { File::from_raw_fd(fd) };
                return Ok(Self {
                    file,
                    dir,
                    temp,
                    committed: false,
                });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(io_at("write the torrent", out, error));
            }
        }
        Err(bad("Could not create a temporary torrent file; try again"))
    }

    /// Put the finished file in place under `name`, in the held folder.
    fn commit(&mut self, name: &CStr) -> io::Result<()> {
        let dir = self.dir.as_raw_fd();
        // SAFETY: a live directory descriptor and two NUL-terminated names.
        if unsafe { libc::renameat(dir, self.temp.as_ptr(), dir, name.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        self.committed = true;
        Ok(())
    }
}

impl Drop for AtomicOutput {
    fn drop(&mut self) {
        if !self.committed {
            // SAFETY: a live directory descriptor and a NUL-terminated name.
            unsafe { libc::unlinkat(self.dir.as_raw_fd(), self.temp.as_ptr(), 0) };
        }
    }
}
