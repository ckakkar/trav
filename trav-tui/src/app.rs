use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use crossterm::{
    event::{Event as CEvent, KeyCode, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tokio::sync::{mpsc, broadcast};
use tokio_stream::StreamExt;
use std::io;

use trav_core::message::{Command, Event as CoreEvent};
use anyhow::Result;

use std::sync::{Arc, RwLock};
use trav_core::snapshot::EngineSnapshot;
use crate::state::TuiState;
use crate::widgets::table::draw_ui;

pub struct TuiApp {
    command_tx: mpsc::Sender<Command>,
    event_rx: broadcast::Receiver<CoreEvent>,
    state: TuiState,
    snapshot: Arc<RwLock<EngineSnapshot>>,
}

impl TuiApp {
    pub fn new(command_tx: mpsc::Sender<Command>, event_rx: broadcast::Receiver<CoreEvent>, snapshot: Arc<RwLock<EngineSnapshot>>) -> Self {
        Self {
            command_tx,
            event_rx,
            state: TuiState::new(),
            snapshot,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let mut crossterm_events = EventStream::new();
        let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(100)); // 10 FPS refresh

        self.state.log("Starting...".to_string());

        loop {
            // Render UI
            terminal.draw(|f| draw_ui(f, &mut self.state))?;

            // Handle Events Multi-plexing
            tokio::select! {
                _ = tick_interval.tick() => {
                    // Lock the snapshot, copy exactly what we need for rendering fast
                    if let Ok(snap) = self.snapshot.read() {
                        self.state.global_down_history.push(snap.total_download_hz);
                        self.state.global_up_history.push(snap.total_upload_hz);
                        if self.state.global_down_history.len() > 100 {
                            self.state.global_down_history.remove(0);
                        }
                        if self.state.global_up_history.len() > 100 {
                            self.state.global_up_history.remove(0);
                        }

                        // Simply sync active torrents
                        self.state.torrents.clear();
                        self.state.torrents_map.clear();
                        self.state.peer_health_map.clear();
                        for (idx, (hash, snapshot_torrent)) in snap.active_torrents.iter().enumerate() {
                            let mut peer_health: Vec<crate::state::PeerHealthState> = snapshot_torrent
                                .peers
                                .iter()
                                .map(|p| crate::state::PeerHealthState {
                                    addr: p.addr.to_string(),
                                    penalty_score: p.penalty_score,
                                    network_penalty: p.network_penalty,
                                    data_penalty: p.data_penalty,
                                    timeout_count: p.timeout_count,
                                    bad_data_count: p.bad_data_count,
                                    hash_fail_count: p.hash_fail_count,
                                })
                                .collect();
                            peer_health.sort_by_key(|p| p.penalty_score);

                            let worst_penalty = peer_health.iter().map(|p| p.penalty_score).max().unwrap_or(0);
                            let health_badge = if worst_penalty >= 8 {
                                "BAD"
                            } else if worst_penalty >= 3 {
                                "WARN"
                            } else {
                                "GOOD"
                            };

                            self.state.torrents_map.insert(*hash, idx);
                            self.state.torrents.push(crate::state::TorrentState {
                                hash: snapshot_torrent.info_hash,
                                name: snapshot_torrent.name.clone(),
                                size_bytes: snapshot_torrent.size_bytes,
                                progress: snapshot_torrent.progress,
                                status: crate::state::Status::Downloading, // Simplifying for Phase 4 mock
                                peers: snapshot_torrent.peers.len(),
                                download_hz: snapshot_torrent.download_hz,
                                upload_hz: snapshot_torrent.upload_hz,
                                health_badge: health_badge.to_string(),
                            });
                            self.state.peer_health_map.insert(*hash, peer_health);
                        }
                    }
                }

                core_event_result = self.event_rx.recv() => {
                    match core_event_result {
                        Ok(CoreEvent::EngineStarted) => {
                            self.state.log("=> Engine Started Successfully".to_string());
                        }
                        Ok(CoreEvent::Error(err)) => {
                            self.state.log(format!("=> ERROR: {}", err));
                        }
                        Ok(CoreEvent::TorrentAdded { name, .. }) => {
                            self.state.log(format!("=> Added Torrent: {}", name));
                        }
                        Ok(CoreEvent::TorrentCompleted { .. }) => {
                            self.state.log("=> A torrent completed downloading!".to_string());
                        }
                        Err(_) | _ => {}
                    }
                }
                
                crossterm_event = crossterm_events.next() => {
                    if let Some(Ok(CEvent::Key(key))) = crossterm_event {
                        match key.code {
                            KeyCode::Char('q') => {
                                let _ = self.command_tx.send(Command::Quit).await;
                                break;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                self.state.next();
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                self.state.previous();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Restore terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        Ok(())
    }
}
