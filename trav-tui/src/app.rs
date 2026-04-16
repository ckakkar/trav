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
use crate::state::{TuiState, Status};
use crate::widgets::table::draw_ui;

pub struct TuiApp {
    command_tx: mpsc::Sender<Command>,
    event_rx: broadcast::Receiver<CoreEvent>,
    state: TuiState,
    snapshot: Arc<RwLock<EngineSnapshot>>,
}

impl TuiApp {
    pub fn new(
        command_tx: mpsc::Sender<Command>,
        event_rx: broadcast::Receiver<CoreEvent>,
        snapshot: Arc<RwLock<EngineSnapshot>>,
    ) -> Self {
        Self {
            command_tx,
            event_rx,
            state: TuiState::new(),
            snapshot,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let mut crossterm_events = EventStream::new();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));

        self.state.log("Engine starting…".to_string());

        loop {
            terminal.draw(|f| draw_ui(f, &mut self.state))?;

            tokio::select! {
                _ = tick.tick() => {
                    if let Ok(snap) = self.snapshot.read() {
                        // Push global speed history
                        self.state.global_down_history.push(snap.total_download_hz);
                        self.state.global_up_history.push(snap.total_upload_hz);
                        if self.state.global_down_history.len() > 120 {
                            self.state.global_down_history.remove(0);
                        }
                        if self.state.global_up_history.len() > 120 {
                            self.state.global_up_history.remove(0);
                        }

                        // Rebuild torrent list from snapshot
                        self.state.torrents.clear();
                        self.state.torrents_map.clear();
                        self.state.peer_health_map.clear();

                        let mut sorted: Vec<_> = snap.active_torrents.values().collect();
                        sorted.sort_by(|a, b| a.name.cmp(&b.name));

                        for (idx, st) in sorted.iter().enumerate() {
                            let mut peer_health: Vec<crate::state::PeerHealthState> = st
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

                            let worst = peer_health.iter().map(|p| p.penalty_score).max().unwrap_or(0);
                            let health_badge = if worst >= 8 { "BAD" } else if worst >= 3 { "WARN" } else { "GOOD" };

                            self.state.torrents_map.insert(st.info_hash, idx);
                            self.state.torrents.push(crate::state::TorrentState {
                                hash: st.info_hash,
                                name: st.name.clone(),
                                size_bytes: st.size_bytes,
                                num_pieces: st.num_pieces,
                                pieces_downloaded: st.pieces_downloaded,
                                progress: st.progress,
                                status: Status::from_str(&st.state),
                                peers: st.peers.len(),
                                download_hz: st.download_hz,
                                upload_hz: st.upload_hz,
                                health_badge: health_badge.to_string(),
                            });
                            self.state.peer_health_map.insert(st.info_hash, peer_health);
                        }

                        // Keep selection valid
                        if self.state.torrents.is_empty() {
                            self.state.table_state.select(None);
                        } else if self.state.table_state.selected().is_none() {
                            self.state.table_state.select(Some(0));
                        }
                    }
                }

                core_event = self.event_rx.recv() => {
                    match core_event {
                        Ok(CoreEvent::EngineStarted) => {
                            self.state.log("Engine started.".to_string());
                        }
                        Ok(CoreEvent::TorrentAdded { name, size_bytes, .. }) => {
                            self.state.log(format!(
                                "Added: {} ({})",
                                name,
                                format_bytes(size_bytes)
                            ));
                        }
                        Ok(CoreEvent::TorrentCompleted { .. }) => {
                            self.state.log("Download complete!".to_string());
                        }
                        Ok(CoreEvent::Error(err)) => {
                            self.state.log(format!("ERROR: {}", err));
                        }
                        _ => {}
                    }
                }

                event = crossterm_events.next() => {
                    if let Some(Ok(CEvent::Key(key))) = event {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                let _ = self.command_tx.send(Command::Quit).await;
                                break;
                            }
                            KeyCode::Down | KeyCode::Char('j') => self.state.next(),
                            KeyCode::Up | KeyCode::Char('k') => self.state.previous(),
                            _ => {}
                        }
                    }
                }
            }
        }

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(())
    }
}

fn format_bytes(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut idx = 0;
    let mut val = bytes as f64;
    while val >= 1024.0 && idx < units.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    format!("{:.1} {}", val, units[idx])
}
