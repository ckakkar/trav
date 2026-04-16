use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::mpsc;
use tracing::error;

use crate::error::Result;

pub const DISK_QUEUE_CAPACITY: usize = 256;

/// Maps a contiguous byte range of the "virtual concatenated file" to a real
/// path on disk. Used for both single-file and multi-file torrents.
#[derive(Debug, Clone)]
pub struct FileRegion {
    /// First byte offset (inclusive) in the virtual file.
    pub start: u64,
    /// Last byte offset (exclusive) in the virtual file.
    pub end: u64,
    /// Absolute path on disk where this region is stored.
    pub path: PathBuf,
}

#[derive(Debug)]
pub enum DiskJob {
    WriteBlock {
        piece_index: u32,
        begin: u32,
        data: Vec<u8>,
    },
    Shutdown,
}

pub struct DiskTask {
    regions: Vec<FileRegion>,
    piece_length: u32,
    /// Open file handles, keyed by path to avoid re-opening on every write.
    handles: HashMap<PathBuf, File>,
    receiver: mpsc::Receiver<DiskJob>,
}

impl DiskTask {
    /// Spawn a single-file disk task (backwards-compat wrapper).
    pub async fn spawn(path: PathBuf, piece_length: u32, total_length: u64) -> Result<mpsc::Sender<DiskJob>> {
        Self::spawn_regions(
            vec![FileRegion { start: 0, end: total_length, path }],
            piece_length,
        )
        .await
    }

    /// Spawn a disk task for a multi-file (or single-file) torrent.
    pub async fn spawn_regions(
        regions: Vec<FileRegion>,
        piece_length: u32,
    ) -> Result<mpsc::Sender<DiskJob>> {
        // Pre-create all parent directories and touch each file
        for r in &regions {
            if let Some(parent) = r.path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    crate::error::BitTorrentError::Engine(format!(
                        "Failed to create dir {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }

        let (sender, receiver) = mpsc::channel(DISK_QUEUE_CAPACITY);
        let task = Self {
            regions,
            piece_length,
            handles: HashMap::new(),
            receiver,
        };
        tokio::spawn(async move { task.run().await });
        Ok(sender)
    }

    async fn run(mut self) {
        while let Some(job) = self.receiver.recv().await {
            match job {
                DiskJob::WriteBlock { piece_index, begin, data } => {
                    let abs_start = piece_index as u64 * self.piece_length as u64 + begin as u64;
                    if let Err(e) = self.write_at(abs_start, &data).await {
                        error!("Disk write error at offset {}: {}", abs_start, e);
                    }
                }
                DiskJob::Shutdown => break,
            }
        }
    }

    /// Writes `data` starting at `abs_offset` in the virtual file, distributing
    /// it across however many FileRegions the range overlaps.
    async fn write_at(&mut self, abs_offset: u64, data: &[u8]) -> std::io::Result<()> {
        let abs_end = abs_offset + data.len() as u64;

        // Collect write targets first to avoid borrow conflicts
        struct WriteTarget {
            path: PathBuf,
            file_offset: u64,
            data_start: usize,
            data_end: usize,
        }
        let targets: Vec<WriteTarget> = self
            .regions
            .iter()
            .filter_map(|region| {
                if region.end <= abs_offset || region.start >= abs_end {
                    return None;
                }
                let data_start = region.start.saturating_sub(abs_offset) as usize;
                let data_end = ((region.end.min(abs_end) - abs_offset) as usize).min(data.len());
                let file_offset =
                    (abs_offset + data_start as u64).saturating_sub(region.start);
                Some(WriteTarget {
                    path: region.path.clone(),
                    file_offset,
                    data_start,
                    data_end,
                })
            })
            .collect();

        for t in targets {
            let chunk = &data[t.data_start..t.data_end];
            let file = self.get_or_open(&t.path).await?;
            file.seek(SeekFrom::Start(t.file_offset)).await?;
            file.write_all(chunk).await?;
            file.flush().await?;
        }
        Ok(())
    }

    async fn get_or_open(&mut self, path: &PathBuf) -> std::io::Result<&mut File> {
        if !self.handles.contains_key(path) {
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .read(true)
                .open(path)
                .await?;
            self.handles.insert(path.clone(), file);
        }
        Ok(self.handles.get_mut(path).unwrap())
    }
}
