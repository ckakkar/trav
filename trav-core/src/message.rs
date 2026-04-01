use std::path::PathBuf;

/// Commands sent from the user interface (TUI/GUI) to the headless BitTorrent engine.
#[derive(Debug, Clone)]
pub enum Command {
    /// Add a new .torrent file for download.
    AddTorrent { file_path: PathBuf },
    /// Pause a specified torrent.
    Pause { torrent_hash: [u8; 20] },
    /// Resume a specified torrent.
    Resume { torrent_hash: [u8; 20] },
    /// Gracefully shut down the engine.
    Quit,
}

/// Events broadcasted by the headless engine to all listening interfaces (TUI/GUI).
#[derive(Debug, Clone)]
pub enum Event {
    /// Initialized the engine successfully.
    EngineStarted,
    /// Download progress update for a torrent.
    TorrentProgress { hash: [u8; 20], progress: f32 },
    /// A torrent completed downloading.
    TorrentCompleted { hash: [u8; 20] },
    /// An error occurred during operation.
    Error(String),
}
