//! Web seeds (BEP 19): HTTP servers holding the torrent's files, used like a
//! peer that has every piece. Each one downloads runs of whole pieces with
//! range requests, starting where the swarm is thinnest. Everything it sends
//! is checked against the piece hashes like any peer's data, and a server
//! that serves the wrong bytes (or is gone) is dropped.

use std::collections::VecDeque;

use bytes::BytesMut;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};

use super::*;
use crate::metainfo::Info;

/// Downloads running against one server at once.
const WORKERS: usize = 4;
/// Bytes per batch of range requests (whole pieces, at least one).
const RUN_BYTES: u64 = 4 << 20;
/// A server that stops sending for this long is treated as failed.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WEB_SEEDS: usize = 8;

pub(super) struct WebSeed {
    pub url: String,
    pub key: PeerKey,
    pub down: RateMeter,
    workers: Vec<tokio::task::JoinHandle<()>>,
    fails: u32,
    next_try: Instant,
    /// Why it's no longer used (gone, ignores ranges, bad data).
    disabled: Option<String>,
    hash_fails: u32,
}

impl WebSeed {
    /// The torrent's `url-list` plus a magnet's `ws=`, each with its own key.
    pub(super) fn list(
        from_torrent: &[String],
        from_magnet: &[String],
        next_key: &mut PeerKey,
    ) -> Vec<Self> {
        let mut out: Vec<Self> = Vec::new();
        for url in from_torrent.iter().chain(from_magnet) {
            if out.len() >= MAX_WEB_SEEDS || out.iter().any(|w| &w.url == url) {
                continue;
            }
            out.push(Self {
                url: url.clone(),
                key: *next_key,
                down: RateMeter::default(),
                workers: Vec::new(),
                fails: 0,
                next_try: Instant::now(),
                disabled: None,
                hash_fails: 0,
            });
            *next_key += 1;
        }
        out
    }

    /// Where it is, for the peer list.
    pub(super) fn host(&self) -> String {
        url::Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| self.url.clone())
    }

    pub(super) fn active(&self) -> bool {
        self.workers.iter().any(|w| !w.is_finished())
    }

    pub(super) fn usable(&self) -> bool {
        self.disabled.is_none()
    }
}

/// What went wrong with a request: worth retrying later, or not at all.
enum Failure {
    Retry(String),
    Fatal(String),
}

impl From<reqwest::Error> for Failure {
    fn from(e: reqwest::Error) -> Self {
        Self::Retry(e.to_string())
    }
}

impl Torrent {
    /// Every second while downloading: keep each usable web seed busy.
    pub(super) fn run_web_seeds(self: &Arc<Self>, st: &mut State) {
        if st.phase != Phase::Active
            || st.picker.as_ref().is_none_or(Picker::wanted_complete)
            || st.web_seeds.is_empty()
        {
            return;
        }
        let now = Instant::now();
        for w in st.web_seeds.iter_mut() {
            w.workers.retain(|h| !h.is_finished());
            if !w.usable() || w.next_try > now {
                continue;
            }
            while w.workers.len() < WORKERS {
                w.workers.push(tokio::spawn(worker(
                    self.clone(),
                    w.key,
                    w.url.clone(),
                    self.ctx.web.clone(),
                )));
            }
        }
    }

    /// Stop downloading from web seeds (pause, completion, shutdown).
    pub(super) fn stop_web_seeds(st: &mut State) {
        for w in st.web_seeds.iter_mut() {
            for h in w.workers.drain(..) {
                h.abort();
            }
        }
    }

    /// A piece `key` helped with failed its hash check: a mirror with the
    /// wrong data is dropped (at once if the whole piece was its).
    pub(super) fn web_seed_bad_piece(st: &mut State, key: PeerKey, sole: bool) {
        let name = st.name.clone();
        if let Some(w) = st.web_seeds.iter_mut().find(|w| w.key == key) {
            w.hash_fails += 1;
            if sole || w.hash_fails >= 3 {
                warn!("{name}: web seed {} sent bad data; not using it", w.url);
                w.disabled = Some("sent data that failed verification".into());
                for h in w.workers.drain(..) {
                    h.abort();
                }
            }
        }
    }

    fn web_seed_result(&self, key: PeerKey, res: std::result::Result<(), Failure>) {
        let mut st = self.st.lock();
        let name = st.name.clone();
        let Some(w) = st.web_seeds.iter_mut().find(|w| w.key == key) else {
            return;
        };
        match res {
            Ok(()) => w.fails = 0,
            Err(Failure::Fatal(m)) => {
                info!("{name}: not using web seed {}: {m}", w.url);
                w.disabled = Some(m);
            }
            Err(Failure::Retry(m)) => {
                w.fails += 1;
                let wait =
                    Duration::from_secs(15u64 << w.fails.min(7)).min(Duration::from_secs(1800));
                debug!(
                    "{name}: web seed {} failed ({m}); retrying in {wait:?}",
                    w.url
                );
                w.next_try = Instant::now() + wait;
            }
        }
    }
}

/// Blocks reserved for a worker and not delivered yet; handed back to the
/// picker if the worker stops early (error, pause, abort).
struct Held {
    t: Arc<Torrent>,
    key: PeerKey,
    reqs: VecDeque<BlockReq>,
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.reqs.is_empty() {
            return;
        }
        let reqs = std::mem::take(&mut self.reqs);
        let (t, key) = (self.t.clone(), self.key);
        // Never lock here: the drop may run inside a locked section (abort).
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if let Some(pk) = t.st.lock().picker.as_mut() {
                    for r in &reqs {
                        pk.release(key, r);
                    }
                }
            });
        }
    }
}

async fn worker(t: Arc<Torrent>, key: PeerKey, url: String, http: reqwest::Client) {
    loop {
        let (meta, mut reqs) = {
            let mut st = t.st.lock();
            if st.phase != Phase::Active {
                return;
            }
            let Some(meta) = st.meta.clone() else { return };
            let Some(pk) = st.picker.as_mut() else { return };
            if pk.wanted_complete() {
                return;
            }
            let mut reqs = Vec::new();
            pk.pick_run(key, RUN_BYTES.max(meta.info.piece_length as u64), &mut reqs);
            (meta, reqs)
        };
        if reqs.is_empty() {
            // Everything is spoken for; the next tick tries again.
            return;
        }
        reqs.sort_unstable_by_key(|r| (r.piece, r.begin));
        let mut held = Held {
            t: t.clone(),
            key,
            reqs: reqs.into(),
        };
        let res = fetch(&t, key, &url, &http, &meta.info, &mut held).await;
        let failed = res.is_err();
        drop(held);
        t.web_seed_result(key, res);
        if failed {
            return;
        }
    }
}

/// Where a block sits in the torrent's concatenated payload.
fn offset(info: &Info, r: &BlockReq) -> u64 {
    r.piece as u64 * info.piece_length as u64 + r.begin as u64
}

enum Segment {
    /// BEP 47 padding: zeros that aren't on the server.
    Zeros(usize),
    File {
        index: usize,
        from: u64,
        to: u64,
    },
}

/// The file pieces making up `start..end` of the payload, in order.
fn segments(info: &Info, start: u64, end: u64) -> Vec<Segment> {
    info.files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.length > 0 && f.offset < end && f.offset + f.length > start)
        .map(|(i, f)| {
            let from = start.max(f.offset) - f.offset;
            let to = end.min(f.offset + f.length) - f.offset;
            if f.pad {
                Segment::Zeros((to - from) as usize)
            } else {
                Segment::File { index: i, from, to }
            }
        })
        .collect()
}

/// BEP 19: a single-file torrent's URL is the file (or a directory to add
/// the name to); a multi-file torrent's is the directory holding `name/`.
fn file_url(base: &str, info: &Info, index: usize) -> String {
    let enc = |s: &str| urlencoding::encode(s).into_owned();
    if !info.multi_file {
        return if base.ends_with('/') {
            format!("{base}{}", enc(&info.name))
        } else {
            base.to_string()
        };
    }
    let mut u = base.to_string();
    if !u.ends_with('/') {
        u.push('/');
    }
    u.push_str(&enc(&info.name));
    for c in &info.files[index].path {
        u.push('/');
        u.push_str(&enc(c));
    }
    u
}

/// Download the held blocks: each run of consecutive blocks is one span of
/// the payload, fetched file by file.
async fn fetch(
    t: &Arc<Torrent>,
    key: PeerKey,
    url: &str,
    http: &reqwest::Client,
    info: &Info,
    held: &mut Held,
) -> std::result::Result<(), Failure> {
    while let Some(first) = held.reqs.front() {
        let start = offset(info, first);
        let mut end = start;
        for r in &held.reqs {
            if offset(info, r) != end {
                break;
            }
            end += r.len as u64;
        }
        let mut buf = BytesMut::new();
        for seg in segments(info, start, end) {
            match seg {
                Segment::Zeros(n) => {
                    buf.resize(buf.len() + n, 0);
                    deliver(t, key, held, &mut buf).await;
                }
                Segment::File { index, from, to } => {
                    let mut resp = http
                        .get(file_url(url, info, index))
                        .header(RANGE, format!("bytes={from}-{}", to - 1))
                        .send()
                        .await?;
                    match resp.status() {
                        StatusCode::PARTIAL_CONTENT => {
                            let ok = resp
                                .headers()
                                .get(CONTENT_RANGE)
                                .and_then(|v| v.to_str().ok())
                                .is_none_or(|v| v.starts_with(&format!("bytes {from}-")));
                            if !ok {
                                return Err(Failure::Fatal("answered a different range".into()));
                            }
                        }
                        // The whole file: fine if we wanted it from the start.
                        StatusCode::OK if from == 0 => {}
                        StatusCode::OK => {
                            return Err(Failure::Fatal("doesn't support range requests".into()));
                        }
                        s if s.is_server_error() || s == StatusCode::TOO_MANY_REQUESTS => {
                            return Err(Failure::Retry(format!("HTTP {s}")));
                        }
                        s => return Err(Failure::Fatal(format!("HTTP {s}"))),
                    }
                    let mut left = (to - from) as usize;
                    while left > 0 {
                        let chunk = tokio::time::timeout(READ_TIMEOUT, resp.chunk())
                            .await
                            .map_err(|_| Failure::Retry("stalled".into()))??
                            .ok_or_else(|| Failure::Retry("response ended early".into()))?;
                        let take = chunk.len().min(left);
                        buf.extend_from_slice(&chunk[..take]);
                        left -= take;
                        deliver(t, key, held, &mut buf).await;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Hand every complete block at the front of `buf` to the picker.
async fn deliver(t: &Arc<Torrent>, key: PeerKey, held: &mut Held, buf: &mut BytesMut) {
    while let Some(r) = held.reqs.front().copied() {
        if buf.len() < r.len as usize {
            return;
        }
        let data = buf.split_to(r.len as usize).freeze();
        t.ctx.down.acquire(data.len()).await;
        held.reqs.pop_front();
        t.ctx
            .session_down
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        let complete = {
            let mut st = t.st.lock();
            if let Some(w) = st.web_seeds.iter_mut().find(|w| w.key == key) {
                w.down.add(data.len() as u64);
            }
            take_block(&mut st, key, r.piece, r.begin, &data)
        };
        if let Some((piece, data, contributors)) = complete {
            t.spawn_verify(piece, data, contributors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metainfo::FileEntry;

    fn info(multi: bool, files: &[(&[&str], u64, bool)]) -> Info {
        let mut offset = 0;
        let files = files
            .iter()
            .map(|(path, len, pad)| {
                let f = FileEntry {
                    path: path.iter().map(|s| s.to_string()).collect(),
                    length: *len,
                    offset,
                    pad: *pad,
                };
                offset += len;
                f
            })
            .collect();
        Info {
            name: "My Album".into(),
            piece_length: 16384,
            pieces: vec![],
            files,
            multi_file: multi,
            total_length: offset,
            private: false,
            raw: vec![],
        }
    }

    #[test]
    fn urls_follow_bep19() {
        let single = info(false, &[(&["My Album"], 10, false)]);
        assert_eq!(file_url("http://a/x.iso", &single, 0), "http://a/x.iso");
        assert_eq!(file_url("http://a/d/", &single, 0), "http://a/d/My%20Album");
        let multi = info(true, &[(&["CD 1", "01 #1.flac"], 10, false)]);
        assert_eq!(
            file_url("http://a/d", &multi, 0),
            "http://a/d/My%20Album/CD%201/01%20%231.flac"
        );
    }

    #[test]
    fn spans_split_across_files_and_padding() {
        let i = info(
            true,
            &[
                (&["a"], 100, false),
                (&[".pad", "28"], 28, true),
                (&["b"], 50, false),
            ],
        );
        let segs = segments(&i, 90, 140);
        let desc: Vec<String> = segs
            .iter()
            .map(|s| match s {
                Segment::Zeros(n) => format!("zeros {n}"),
                Segment::File { index, from, to } => format!("{index}:{from}-{to}"),
            })
            .collect();
        assert_eq!(desc, ["0:90-100", "zeros 28", "2:0-12"]);
    }
}
