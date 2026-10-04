use std::path::Path;
use std::sync::Arc;

use sha1::{Digest, Sha1};

use crate::bencode::{self, DictBuilder, Value};
use crate::error::{Error, Result};

pub type InfoHash = [u8; 20];

/// Upper bound accepted for `piece length`; anything bigger is hostile or broken.
const MAX_PIECE_LENGTH: u64 = 128 * 1024 * 1024;
const MAX_FILES: usize = 1_000_000;

#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Path components relative to the torrent root (single-file: `[name]`).
    pub path: Vec<String>,
    pub length: u64,
    /// Offset of the file within the concatenated torrent payload.
    pub offset: u64,
    /// BEP 47 padding file — never written to disk.
    pub pad: bool,
}

impl FileEntry {
    pub fn display_path(&self) -> String {
        self.path.join("/")
    }
}

#[derive(Debug, Clone)]
pub struct Info {
    pub name: String,
    pub piece_length: u32,
    pub pieces: Vec<[u8; 20]>,
    pub files: Vec<FileEntry>,
    /// True if the torrent declares a `files` list (payload lives in a directory).
    pub multi_file: bool,
    pub total_length: u64,
    pub private: bool,
    /// Exact bencoded info dictionary; hashing these bytes yields the info-hash.
    pub raw: Vec<u8>,
}

impl Info {
    pub fn from_bytes(raw: &[u8]) -> Result<Self> {
        let v = bencode::decode(raw)?;
        Self::from_value(&v, raw.to_vec())
    }

    fn from_value(v: &Value, raw: Vec<u8>) -> Result<Self> {
        let bad = |m: &str| Error::Metainfo(m.to_string());
        if v.as_dict().is_none() {
            return Err(bad("info is not a dictionary"));
        }

        let name = v
            .get("name.utf-8")
            .or_else(|| v.get("name"))
            .and_then(Value::as_string_lossy)
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "untitled".to_string());

        let piece_length =
            v.get("piece length")
                .and_then(Value::as_int)
                .filter(|&p| p > 0 && (p as u64) <= MAX_PIECE_LENGTH)
                .ok_or_else(|| bad("missing or invalid piece length"))? as u32;

        let pieces_raw = v
            .get("pieces")
            .and_then(Value::as_bytes)
            .ok_or_else(|| bad("missing pieces"))?;
        if pieces_raw.is_empty() || pieces_raw.len() % 20 != 0 {
            return Err(bad("pieces length is not a multiple of 20"));
        }
        let pieces: Vec<[u8; 20]> = pieces_raw
            .chunks_exact(20)
            .map(|c| c.try_into().expect("chunk is 20 bytes"))
            .collect();

        let private = v.get("private").and_then(Value::as_int) == Some(1);

        let mut files = Vec::new();
        let mut offset = 0u64;
        let multi_file;
        if let Some(list) = v.get("files").and_then(Value::as_list) {
            multi_file = true;
            if list.is_empty() || list.len() > MAX_FILES {
                return Err(bad("invalid file list"));
            }
            for f in list {
                let length = f
                    .get("length")
                    .and_then(Value::as_int)
                    .filter(|&l| l >= 0)
                    .ok_or_else(|| bad("file without valid length"))?
                    as u64;
                let path: Vec<String> = f
                    .get("path.utf-8")
                    .or_else(|| f.get("path"))
                    .and_then(Value::as_list)
                    .ok_or_else(|| bad("file without path"))?
                    .iter()
                    .map(|c| c.as_string_lossy().unwrap_or_default())
                    .filter(|c| !c.is_empty())
                    .collect();
                if path.is_empty() {
                    return Err(bad("file with empty path"));
                }
                let pad = f
                    .get("attr")
                    .and_then(Value::as_bytes)
                    .is_some_and(|a| a.contains(&b'p'));
                files.push(FileEntry {
                    path,
                    length,
                    offset,
                    pad,
                });
                offset = offset
                    .checked_add(length)
                    .ok_or_else(|| bad("total length overflow"))?;
            }
        } else {
            multi_file = false;
            let length = v
                .get("length")
                .and_then(Value::as_int)
                .filter(|&l| l > 0)
                .ok_or_else(|| bad("missing length"))? as u64;
            files.push(FileEntry {
                path: vec![name.clone()],
                length,
                offset: 0,
                pad: false,
            });
            offset = length;
        }

        let total_length = offset;
        if total_length == 0 {
            return Err(bad("torrent is empty"));
        }
        let expected = total_length.div_ceil(piece_length as u64);
        if expected != pieces.len() as u64 {
            return Err(bad(&format!(
                "piece count mismatch: {} hashes for {} pieces",
                pieces.len(),
                expected
            )));
        }

        Ok(Self {
            name,
            piece_length,
            pieces,
            files,
            multi_file,
            total_length,
            private,
            raw,
        })
    }

    pub fn info_hash(&self) -> InfoHash {
        Sha1::digest(&self.raw).into()
    }

    #[inline]
    pub fn num_pieces(&self) -> usize {
        self.pieces.len()
    }

    pub fn piece_size(&self, index: usize) -> u32 {
        if index + 1 == self.pieces.len() {
            let rem = self.total_length - self.piece_length as u64 * index as u64;
            rem as u32
        } else {
            self.piece_length
        }
    }

    /// Byte range `[start, end)` of a piece within the payload.
    pub fn piece_range(&self, index: usize) -> (u64, u64) {
        let start = self.piece_length as u64 * index as u64;
        (start, start + self.piece_size(index) as u64)
    }
}

#[derive(Debug, Clone)]
pub struct Metainfo {
    pub info_hash: InfoHash,
    pub info: Arc<Info>,
    /// Tracker tiers (BEP 12). A plain `announce` becomes a single tier.
    pub trackers: Vec<Vec<String>>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<i64>,
}

impl Metainfo {
    pub async fn read_file(path: impl AsRef<Path>) -> Result<Self> {
        let data = tokio::fs::read(path).await?;
        Self::from_bytes(&data)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let root = bencode::decode(data)?;
        let info_raw = bencode::raw_value_span(data, b"info")?;
        let info_val = root
            .get("info")
            .ok_or_else(|| Error::Metainfo("missing info".into()))?;
        let info = Info::from_value(info_val, info_raw.to_vec())?;
        let info_hash = info.info_hash();

        let mut trackers: Vec<Vec<String>> = root
            .get("announce-list")
            .and_then(Value::as_list)
            .map(|tiers| {
                tiers
                    .iter()
                    .filter_map(Value::as_list)
                    .map(|tier| {
                        tier.iter()
                            .filter_map(Value::as_string_lossy)
                            .collect::<Vec<_>>()
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(a) = root.get("announce").and_then(Value::as_string_lossy)
            && !trackers.iter().flatten().any(|t| t == &a)
        {
            trackers.insert(0, vec![a]);
        }

        Ok(Self {
            info_hash,
            info: Arc::new(info),
            trackers: normalize_tiers(trackers),
            comment: root.get("comment").and_then(Value::as_string_lossy),
            created_by: root.get("created by").and_then(Value::as_string_lossy),
            creation_date: root.get("creation date").and_then(Value::as_int),
        })
    }

    /// Assemble a metainfo from an info dict fetched over ut_metadata.
    pub fn from_info(info: Info, trackers: Vec<Vec<String>>) -> Self {
        Self {
            info_hash: info.info_hash(),
            info: Arc::new(info),
            trackers: normalize_tiers(trackers),
            comment: None,
            created_by: None,
            creation_date: None,
        }
    }

    /// Serialise as a `.torrent` file, splicing in the original info bytes verbatim.
    pub fn to_torrent_bytes(&self) -> Vec<u8> {
        let mut d = DictBuilder::new();
        if let Some(first) = self.trackers.first().and_then(|t| t.first()) {
            d = d.bytes("announce", first.as_bytes());
        }
        if !self.trackers.is_empty() {
            let tiers = self
                .trackers
                .iter()
                .map(|t| {
                    Value::List(
                        t.iter()
                            .map(|u| Value::Bytes(u.as_bytes().to_vec()))
                            .collect(),
                    )
                })
                .collect();
            d = d.value("announce-list", Value::List(tiers));
        }
        if let Some(c) = &self.comment {
            d = d.bytes("comment", c.as_bytes());
        }
        if let Some(c) = &self.created_by {
            d = d.bytes("created by", c.as_bytes());
        }
        if let Some(c) = self.creation_date {
            d = d.int("creation date", c);
        }
        // Encode with a placeholder then splice the raw info dict in sorted position.
        let Value::Dict(mut map) = d.build() else {
            unreachable!()
        };
        map.insert(b"info".to_vec(), Value::Bytes(Vec::new()));
        let mut out = vec![b'd'];
        for (k, v) in &map {
            bencode::encode_bytes(k, &mut out);
            if k == b"info" {
                out.extend_from_slice(&self.info.raw);
            } else {
                v.encode_into(&mut out);
            }
        }
        out.push(b'e');
        out
    }

    pub fn all_trackers(&self) -> impl Iterator<Item = &String> {
        self.trackers.iter().flatten()
    }
}

/// Drop unsupported schemes and duplicates while preserving tier order.
pub fn normalize_tiers(tiers: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let mut seen = std::collections::HashSet::new();
    tiers
        .into_iter()
        .map(|tier| {
            tier.into_iter()
                .map(|u| u.trim().to_string())
                .filter(|u| {
                    (u.starts_with("http://")
                        || u.starts_with("https://")
                        || u.starts_with("udp://"))
                        && seen.insert(u.clone())
                })
                .collect::<Vec<_>>()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// Build a `.torrent` from a file or directory (used by tests and for seeding new content).
pub fn create_torrent(
    root: &Path,
    piece_length: u32,
    trackers: &[String],
    private: bool,
) -> Result<Vec<u8>> {
    use std::io::Read;

    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::Metainfo("invalid root name".into()))?
        .to_string();

    let mut entries: Vec<(Vec<String>, std::path::PathBuf, u64)> = Vec::new();
    if root.is_dir() {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir)? {
                let e = e?;
                let p = e.path();
                if e.file_type()?.is_dir() {
                    stack.push(p);
                } else {
                    let rel: Vec<String> = p
                        .strip_prefix(root)
                        .expect("walked under root")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect();
                    let len = e.metadata()?.len();
                    entries.push((rel, p, len));
                }
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
    } else {
        let len = std::fs::metadata(root)?.len();
        entries.push((vec![name.clone()], root.to_path_buf(), len));
    }

    let mut pieces = Vec::new();
    let mut buf = Vec::with_capacity(piece_length as usize);
    let mut chunk = vec![0u8; 1 << 16];
    for (_, path, _) in &entries {
        let mut f = std::fs::File::open(path)?;
        loop {
            let n = f.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            let mut rest = &chunk[..n];
            while !rest.is_empty() {
                let take = (piece_length as usize - buf.len()).min(rest.len());
                buf.extend_from_slice(&rest[..take]);
                rest = &rest[take..];
                if buf.len() == piece_length as usize {
                    pieces.extend_from_slice(&Sha1::digest(&buf));
                    buf.clear();
                }
            }
        }
    }
    if !buf.is_empty() {
        pieces.extend_from_slice(&Sha1::digest(&buf));
    }

    let mut info = DictBuilder::new()
        .bytes("name", name.as_bytes())
        .int("piece length", piece_length as i64)
        .bytes("pieces", pieces);
    if root.is_dir() {
        let files = entries
            .iter()
            .map(|(rel, _, len)| {
                DictBuilder::new()
                    .int("length", *len as i64)
                    .value(
                        "path",
                        Value::List(
                            rel.iter()
                                .map(|c| Value::Bytes(c.as_bytes().to_vec()))
                                .collect(),
                        ),
                    )
                    .build()
            })
            .collect();
        info = info.value("files", Value::List(files));
    } else {
        info = info.int("length", entries[0].2 as i64);
    }
    if private {
        info = info.int("private", 1);
    }

    let mut top = DictBuilder::new()
        .bytes("created by", "Trav/0.2")
        .value("info", info.build());
    if let Some(first) = trackers.first() {
        top = top.bytes("announce", first.as_bytes());
        top = top.value(
            "announce-list",
            Value::List(
                trackers
                    .iter()
                    .map(|t| Value::List(vec![Value::Bytes(t.as_bytes().to_vec())]))
                    .collect(),
            ),
        );
    }
    Ok(top.build().encode())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let info = DictBuilder::new()
            .bytes("name", "hello.txt")
            .int("piece length", 4)
            .int("length", 10)
            .bytes("pieces", vec![0u8; 60])
            .build();
        DictBuilder::new()
            .bytes("announce", "udp://tracker.example:80")
            .value("info", info)
            .build()
            .encode()
    }

    #[test]
    fn parses_and_roundtrips() {
        let raw = sample();
        let m = Metainfo::from_bytes(&raw).unwrap();
        assert_eq!(m.info.name, "hello.txt");
        assert_eq!(m.info.num_pieces(), 3);
        assert_eq!(m.info.piece_size(2), 2);
        assert_eq!(
            m.trackers,
            vec![vec!["udp://tracker.example:80".to_string()]]
        );
        let again = Metainfo::from_bytes(&m.to_torrent_bytes()).unwrap();
        assert_eq!(again.info_hash, m.info_hash);
    }

    #[test]
    fn rejects_piece_mismatch() {
        let info = DictBuilder::new()
            .bytes("name", "x")
            .int("piece length", 4)
            .int("length", 100)
            .bytes("pieces", vec![0u8; 20])
            .build();
        assert!(Info::from_bytes(&info.encode()).is_err());
    }
}
