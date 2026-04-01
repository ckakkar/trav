use crate::message::{Command, Event};
use crate::error::Result;
use tokio::sync::{mpsc, broadcast};
use tracing::{info, error, debug};

/// The core BitTorrent Engine.
/// Runs as an asynchronous task coordinating networking, disk I/O, and peers.
pub struct Engine {
    command_rx: mpsc::Receiver<Command>,
    event_tx: broadcast::Sender<Event>,
}

impl Engine {
    /// Creates a new Engine and returns the Channels to communicate with it.
    pub fn new() -> (Self, mpsc::Sender<Command>, broadcast::Receiver<Event>) {
        let (command_tx, command_rx) = mpsc::channel(32);
        let (event_tx, event_rx) = broadcast::channel(32);

        let engine = Self {
            command_rx,
            event_tx: event_tx.clone(),
        };

        (engine, command_tx, event_rx)
    }

    /// Spawns the engine's main event loop on the current Tokio runtime.
    pub async fn run(mut self) -> Result<()> {
        info!("Starting BitTorrent Engine...");
        let _ = self.event_tx.send(Event::EngineStarted);

        loop {
            tokio::select! {
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(Command::AddTorrent { file_path }) => {
                            info!("Command received: AddTorrent {:?}", file_path);
                            // TODO: Add torrent logic
                        }
                        Some(Command::Pause { torrent_hash }) => {
                            info!("Command received: Pause {}", hex::encode(torrent_hash));
                        }
                        Some(Command::Resume { torrent_hash }) => {
                            info!("Command received: Resume {}", hex::encode(torrent_hash));
                        }
                        Some(Command::Quit) => {
                            info!("Quit command received. Shutting down engine...");
                            break;
                        }
                        None => {
                            debug!("Command channel closed. Shutting down.");
                            break;
                        }
                    }
                }
                // Later, we add more futures here for network IO, Peer messages, etc.
            }
        }

        info!("BitTorrent Engine fully shutdown.");
        Ok(())
    }
}
