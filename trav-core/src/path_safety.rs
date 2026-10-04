//! Turning untrusted torrent metadata into filesystem paths.
//!
//! Every component is *sanitised* rather than rejected: a torrent with a
//! trailing-space folder name is still a valid torrent, and refusing it would be
//! a worse user experience than writing `name_`. Traversal is impossible by
//! construction because each component is reduced to a single `Normal` segment.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

const RESERVED_WIN: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Reduce an arbitrary metadata string to one portable path component.
pub fn sanitize_component(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();

    // Windows silently strips trailing dots/spaces, which can alias two entries.
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let trimmed = s.trim_start().to_string();
    let mut s = if trimmed.is_empty() { "_".to_string() } else { trimmed };

    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED_WIN.contains(&stem.as_str()) {
        s.insert(0, '_');
    }
    // Most filesystems cap a component at 255 bytes.
    if s.len() > 240 {
        let mut cut = 240;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// Join sanitised components under `root`, asserting the result stays inside it.
pub fn jail_join<'a>(root: &Path, components: impl IntoIterator<Item = &'a str>) -> Result<PathBuf> {
    let mut out = root.to_path_buf();
    let mut pushed = 0usize;
    for c in components {
        out.push(sanitize_component(c));
        pushed += 1;
    }
    if pushed == 0 {
        return Err(Error::Metainfo("empty path".into()));
    }
    ensure_within(root, &out)?;
    Ok(out)
}

/// Verify `target` is lexically inside `root` with no `..`/root/prefix components after it.
pub fn ensure_within(root: &Path, target: &Path) -> Result<()> {
    let rel = target
        .strip_prefix(root)
        .map_err(|_| Error::Metainfo("path escaped download directory".into()))?;
    for c in rel.components() {
        if !matches!(c, Component::Normal(_)) {
            return Err(Error::Metainfo("path escaped download directory".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_hostile_segments() {
        assert_eq!(sanitize_component(".."), "_");
        assert_eq!(sanitize_component("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitize_component("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_component("name. "), "name");
        assert_eq!(sanitize_component("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_component(""), "_");
        assert_eq!(sanitize_component("nul\0byte"), "nul_byte");
    }

    #[test]
    fn jail_holds() {
        let root = Path::new("/downloads");
        let p = jail_join(root, ["..", "..", "x"]).unwrap();
        assert!(p.starts_with(root));
        assert_eq!(p, Path::new("/downloads/_/_/x"));
        assert!(ensure_within(root, Path::new("/downloads/../x")).is_err());
    }
}
