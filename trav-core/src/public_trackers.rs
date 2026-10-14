//! Well-known public trackers, announced to alongside a torrent's own so that
//! magnets and torrents with few or dead trackers still find peers. Never used
//! for private torrents. The list is the community-maintained
//! `trackers_best.txt`, refreshed daily, with a built-in copy for the first
//! start and for when it can't be fetched.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::error::{Error, Result};

const SOURCE: &str =
    "https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt";
const CACHE: &str = "public-trackers.txt";
pub(crate) const REFRESH: Duration = Duration::from_secs(24 * 3600);
const MAX: usize = 30;

const BUILT_IN: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://explodie.org:6969/announce",
    "udp://tracker.qu.ax:6969/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://tracker-udp.gbitt.info:80/announce",
    "udp://tracker.skynetcloud.site:6969/announce",
    "udp://tracker.ducks.party:1984/announce",
    "http://tracker2.dler.org:80/announce",
];

/// Announce URLs from a tracker list: one per line, blank lines between.
pub(crate) fn parse(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        let ok = ["udp://", "http://", "https://"]
            .iter()
            .any(|s| line.starts_with(s))
            && url::Url::parse(line).is_ok_and(|u| u.host_str().is_some());
        if ok && !out.iter().any(|u| u == line) {
            out.push(line.to_string());
        }
        if out.len() == MAX {
            break;
        }
    }
    out
}

/// The cached list, or the built-in one.
pub(crate) fn load(dir: &Path) -> Vec<String> {
    let cached = std::fs::read_to_string(dir.join(CACHE))
        .map(|t| parse(&t))
        .unwrap_or_default();
    if cached.len() >= 3 {
        cached
    } else {
        BUILT_IN.iter().map(|s| s.to_string()).collect()
    }
}

/// Whether the cached copy is missing or older than a day.
pub(crate) fn stale(dir: &Path) -> bool {
    std::fs::metadata(dir.join(CACHE))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_none_or(|age| age >= REFRESH)
}

/// Download the current list and cache it.
pub(crate) async fn refresh(http: &reqwest::Client, dir: &Path) -> Result<Vec<String>> {
    let resp = http
        .get(SOURCE)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| Error::Tracker(format!("tracker list: {e}")))?;
    let body = crate::tracker::read_capped(resp, 64 * 1024).await?;
    let list = parse(&String::from_utf8_lossy(&body));
    if list.len() < 3 {
        return Err(Error::Tracker("tracker list: too few entries".into()));
    }
    std::fs::write(dir.join(CACHE), list.join("\n\n"))?;
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_list_format() {
        let text = "udp://a.example:1337/announce\n\nhttp://b.example/announce\n\n\
                    udp://a.example:1337/announce\nwss://c.example/announce\n\nnot a url\n";
        assert_eq!(
            parse(text),
            ["udp://a.example:1337/announce", "http://b.example/announce"]
        );
    }

    #[test]
    fn falls_back_to_the_built_in_list() {
        let dir = tempfile::tempdir().unwrap();
        assert!(stale(dir.path()));
        assert_eq!(load(dir.path()).len(), BUILT_IN.len());
        std::fs::write(dir.path().join(CACHE), "udp://x.example:1/announce").unwrap();
        // One entry isn't a believable list.
        assert_eq!(load(dir.path()).len(), BUILT_IN.len());
        std::fs::write(
            dir.path().join(CACHE),
            "udp://x.example:1/announce\n\nudp://y.example:1/announce\n\nudp://z.example:1/announce",
        )
        .unwrap();
        assert_eq!(load(dir.path()).len(), 3);
        assert!(!stale(dir.path()));
    }
}
