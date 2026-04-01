use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use crossterm::{
    event::{self, Event as CEvent, KeyCode, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tokio::sync::{mpsc, broadcast};
use tokio_stream::StreamExt;
use std::io;

use trav_core::message::{Command, Event as CoreEvent};
use anyhow::Result;

use crate::state::TuiState;
use crate::widgets::table::draw_ui;

pub struct TuiApp {
    command_tx: mpsc::Sender<Command>,
    event_rx: broadcast::Receiver<CoreEvent>,
    state: TuiState,
}

impl TuiApp {
    pub fn new(command_tx: mpsc::Sender<Command>, event_rx: broadcast::Receiver<CoreEvent>) -> Self {
        Self {
            command_tx,
            event_rx,
            state: TuiState::new(),
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

        self.state.log("Starting...".to_string());

        loop {
            // Render UI
            terminal.draw(|f| draw_ui(f, &mut self.state))?;

            // Handle Events Multi-plexing
            tokio::select! {
                // Handle core engine events
                core_event_result = self.event_rx.recv() => {
                    match core_event_result {
                        Ok(CoreEvent::EngineStarted) => {
                            self.state.log("=> Engine Started Successfully".to_string());
                        }
                        Ok(CoreEvent::Error(err)) => {
                            self.state.log(format!("=> ERROR: {}", err));
                        }
                        Ok(CoreEvent::TorrentAdded { hash, name, size_bytes }) => {
                            self.state.add_torrent(hash, name.clone(), size_bytes);
                            self.state.log(format!("=> Added Torrent: {}", name));
                        }
                        Ok(CoreEvent::TorrentProgress { hash, progress }) => {
                            if let Some(&idx) = self.state.torrents_map.get(&hash) {
                                self.state.torrents[idx].progress = progress;
                            }
                        }
                        Ok(CoreEvent::PeerCountUpdated { hash, count }) => {
                            if let Some(&idx) = self.state.torrents_map.get(&hash) {
                                self.state.torrents[idx].peers = count;
                            }
                        }
                        Ok(CoreEvent::SpeedUpdated { hash, download_hz, upload_hz }) => {
                            if let Some(&idx) = self.state.torrents_map.get(&hash) {
                                self.state.torrents[idx].download_hz = download_hz;
                                self.state.torrents[idx].upload_hz = upload_hz;
                            }
                            // Calculate global naive speeds
                            let mut d_hz = 0;
                            let mut u_hz = 0;
                            for t in &self.state.torrents {
                                d_hz += t.download_hz;
                                u_hz += t.upload_hz;
                            }
                            self.state.global_down_hz = d_hz;
                            self.state.global_up_hz = u_hz;
                        }
                        Ok(CoreEvent::TorrentCompleted { hash }) => {
                            if let Some(&idx) = self.state.torrents_map.get(&hash) {
                                self.state.torrents[idx].progress = 100.0;
                                self.state.torrents[idx].status = crate::state::Status::Seeding;
                                self.state.log(format!("=> Completed: {}", self.state.torrents[idx].name));
                            }
                        }
                        Err(_) => {
                            // Engine dropped the channel. Engine died?
                            self.state.log("=> Engine Disconnected!".to_string());
                        }
                    }
                }
                
                // Handle terminal input events
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
                            // Future: Enter key to view detailed stats of a torrent
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
