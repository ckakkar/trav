use std::path::{Component, Path, PathBuf};

use crate::error::{BitTorrentError, Result};

pub fn build_jailed_single_file_path(
    download_dir: &Path,
    info_hash: &[u8; 20],
    torrent_name: &str,
) -> Result<PathBuf> {
    let jail_root = download_dir.join(hex::encode(info_hash));
    let safe_name = sanitize_filename_segment(torrent_name)?;
    let target = jail_root.join(safe_name);
    ensure_within_jail(&jail_root, &target)?;
    Ok(target)
}

pub fn validate_multi_file_paths(paths: &[Vec<String>]) -> Result<()> {
    for parts in paths {
        if parts.is_empty() {
            return Err(BitTorrentError::Engine("Torrent file path cannot be empty".into()));
        }
        for segment in parts {
            let _ = sanitize_filename_segment(segment)?;
        }
    }
    Ok(())
}

fn sanitize_filename_segment(segment: &str) -> Result<String> {
    if segment.trim().is_empty() {
        return Err(BitTorrentError::Engine("Empty path segment in torrent metadata".into()));
    }
    if segment.contains('\0') {
        return Err(BitTorrentError::Engine("NUL byte in torrent path segment".into()));
    }
    if segment == "." || segment == ".." {
        return Err(BitTorrentError::Engine("Relative traversal segment in torrent metadata".into()));
    }
    if segment.contains('/') || segment.contains('\\') {
        return Err(BitTorrentError::Engine("Path separator found inside torrent path segment".into()));
    }
    if Path::new(segment).is_absolute() {
        return Err(BitTorrentError::Engine("Absolute path in torrent metadata".into()));
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return Err(BitTorrentError::Engine("Unsafe trailing path characters in torrent metadata".into()));
    }
    Ok(segment.to_string())
}

fn ensure_within_jail(jail_root: &Path, target: &Path) -> Result<()> {
    for component in target.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(BitTorrentError::Engine("Path escaped download jail".into()));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }

    if !target.starts_with(jail_root) {
        return Err(BitTorrentError::Engine("Path escaped download jail".into()));
    }
    Ok(())
}
