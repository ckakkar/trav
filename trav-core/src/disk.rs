use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::mpsc;
use tracing::error;

use crate::error::Result;

pub const DISK_QUEUE_CAPACITY: usize = 256;

pub struct DiskTask {
    file: File,
    piece_length: u32,
    receiver: mpsc::Receiver<DiskJob>,
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

impl DiskTask {
    /// Spawns a dedicated Disk Task to avoid concurrent `seek` race conditions.
    pub async fn spawn(path: PathBuf, piece_length: u32) -> Result<mpsc::Sender<DiskJob>> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&path)
            .await
            .map_err(|e| crate::error::BitTorrentError::Engine(format!("Disk I/O Error: {}", e)))?;

        let (sender, receiver) = mpsc::channel(DISK_QUEUE_CAPACITY);

        let mut task = Self { file, piece_length, receiver };

        tokio::spawn(async move {
            task.run().await;
        });

        Ok(sender)
    }

    async fn run(&mut self) {
        while let Some(job) = self.receiver.recv().await {
            match job {
                DiskJob::WriteBlock { piece_index, begin, data } => {
                    let absolute_offset = (piece_index as u64 * self.piece_length as u64) + begin as u64;
                    if let Err(e) = self.write_at(absolute_offset, &data).await {
                        error!("Failed to write block to disk: {}", e);
                    }
                }
                DiskJob::Shutdown => break,
            }
        }
    }

    async fn write_at(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(offset)).await?;
        self.file.write_all(data).await?;
        self.file.flush().await?;
        Ok(())
    }
}
