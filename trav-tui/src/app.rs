use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
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

pub struct TuiApp {
    command_tx: mpsc::Sender<Command>,
    event_rx: broadcast::Receiver<CoreEvent>,
    status: String,
    logs: Vec<String>,
}

impl TuiApp {
    pub fn new(command_tx: mpsc::Sender<Command>, event_rx: broadcast::Receiver<CoreEvent>) -> Self {
        Self {
            command_tx,
            event_rx,
            status: "Starting...".to_string(),
            logs: vec![],
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

        loop {
            // Render UI
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
                    .split(f.size());

                let header = Paragraph::new(format!("Trav Async BitTorrent Client - Status: {}", self.status))
                    .block(Block::default().borders(Borders::ALL).title("Core"));
                f.render_widget(header, chunks[0]);

                let logs_text = self.logs.join("\n");
                let logs_widget = Paragraph::new(logs_text)
                    .block(Block::default().borders(Borders::ALL).title("Events Logs"));
                f.render_widget(logs_widget, chunks[1]);
            })?;

            // Handle Events Multi-plexing
            tokio::select! {
                // Handle core engine events
                core_event_result = self.event_rx.recv() => {
                    match core_event_result {
                        Ok(CoreEvent::EngineStarted) => {
                            self.status = "Engine Running".to_string();
                            self.logs.push("=> Engine Started Successfully".to_string());
                        }
                        Ok(CoreEvent::Error(err)) => {
                            self.logs.push(format!("=> ERROR: {}", err));
                        }
                        Ok(CoreEvent::TorrentProgress { hash, progress }) => {
                            self.logs.push(format!("=> Progress {}: {:.2}%", hex::encode(hash), progress));
                        }
                        Ok(CoreEvent::TorrentCompleted { hash }) => {
                            self.logs.push(format!("=> Completed: {}", hex::encode(hash)));
                        }
                        Err(_) => {
                            // Engine dropped the channel. Engine died?
                            self.status = "Engine Disconnected".to_string();
                        }
                    }
                }
                
                // Handle terminal input events
                crossterm_event = crossterm_events.next() => {
                    if let Some(Ok(CEvent::Key(key))) = crossterm_event {
                        if key.code == KeyCode::Char('q') {
                            // Let the engine know we are quitting
                            let _ = self.command_tx.send(Command::Quit).await;
                            break;
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
