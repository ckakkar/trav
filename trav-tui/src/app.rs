use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event as CEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::widgets::TableState;
use trav_core::snapshot::{EngineSnapshot, TorrentDetails, TorrentStatus};
use trav_core::{AddTorrent, EngineHandle, Event, TorrentSource};

use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    General,
    Files,
    Peers,
    Trackers,
    Log,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::General, Tab::Files, Tab::Peers, Tab::Trackers, Tab::Log];

    pub fn title(self) -> &'static str {
        match self {
            Tab::General => "general",
            Tab::Files => "files",
            Tab::Peers => "peers",
            Tab::Trackers => "trackers",
            Tab::Log => "log",
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) enum Mode {
    Normal,
    Add,
    Confirm { delete: bool },
    Help,
}

pub struct TuiApp {
    pub(crate) h: EngineHandle,
    pub(crate) snap: Arc<EngineSnapshot>,
    pub(crate) details: Option<TorrentDetails>,
    pub(crate) table: TableState,
    pub(crate) tab: Tab,
    pub(crate) mode: Mode,
    pub(crate) input: String,
    pub(crate) log: VecDeque<String>,
    pub(crate) down_hist: VecDeque<u64>,
    pub(crate) up_hist: VecDeque<u64>,
    pub(crate) flash: Option<(String, bool, Instant)>,
    pub(crate) detail_scroll: usize,
}

const HISTORY: usize = 240;

impl TuiApp {
    pub fn new(h: EngineHandle) -> Self {
        let snap = h.snapshot();
        Self {
            h,
            snap,
            details: None,
            table: TableState::default(),
            tab: Tab::General,
            mode: Mode::Normal,
            input: String::new(),
            log: VecDeque::with_capacity(200),
            down_hist: VecDeque::from(vec![0; HISTORY]),
            up_hist: VecDeque::from(vec![0; HISTORY]),
            flash: None,
            detail_scroll: 0,
        }
    }

    pub(crate) fn selected_hash(&self) -> Option<String> {
        self.table.selected().and_then(|i| self.snap.torrents.get(i)).map(|t| t.info_hash.clone())
    }

    fn note(&mut self, msg: impl Into<String>, error: bool) {
        let msg = msg.into();
        let ts = chrono_like_now();
        if self.log.len() >= 200 {
            self.log.pop_back();
        }
        self.log.push_front(format!("{ts}  {msg}"));
        self.flash = Some((msg, error, Instant::now()));
    }

    fn refresh(&mut self) {
        self.snap = self.h.snapshot();
        let n = self.snap.torrents.len();
        match self.table.selected() {
            _ if n == 0 => self.table.select(None),
            None => self.table.select(Some(0)),
            Some(i) if i >= n => self.table.select(Some(n - 1)),
            _ => {}
        }
        self.details = self.selected_hash().and_then(|h| self.h.details(&h));
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut terminal = ratatui::init();
        let mut keys = EventStream::new();
        let mut events = self.h.events();
        let mut redraw = tokio::time::interval(Duration::from_millis(250));
        let mut sample = tokio::time::interval(Duration::from_secs(1));
        self.note(format!("engine up · listening on {}", self.h.listen_port()), false);

        let res: Result<()> = async {
            loop {
                self.refresh();
                terminal.draw(|f| ui::draw(f, self))?;
                tokio::select! {
                    _ = redraw.tick() => {}
                    _ = sample.tick() => {
                        push(&mut self.down_hist, self.snap.stats.download_rate);
                        push(&mut self.up_hist, self.snap.stats.upload_rate);
                    }
                    ev = events.recv() => if let Ok(ev) = ev { self.on_event(ev) },
                    key = keys.next() => match key {
                        Some(Ok(CEvent::Key(k))) if k.kind != KeyEventKind::Release => {
                            if self.on_key(k).await {
                                break;
                            }
                        }
                        Some(Err(_)) | None => break,
                        _ => {}
                    },
                }
            }
            Ok(())
        }
        .await;
        ratatui::restore();
        res
    }

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::TorrentAdded { name, .. } => self.note(format!("added  {name}"), false),
            Event::MetadataReceived { name, .. } => self.note(format!("metadata  {name}"), false),
            Event::TorrentCompleted { name, .. } => self.note(format!("complete  {name}"), false),
            Event::TorrentError { name, message, .. } => self.note(format!("error  {name}: {message}"), true),
            Event::TorrentRemoved { .. } => self.note("removed", false),
        }
    }

    /// Returns true to quit.
    async fn on_key(&mut self, k: KeyEvent) -> bool {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            return true;
        }
        match self.mode.clone() {
            Mode::Help => self.mode = Mode::Normal,
            Mode::Add => match k.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.input.clear();
                }
                KeyCode::Enter => {
                    let input = std::mem::take(&mut self.input);
                    self.mode = Mode::Normal;
                    let path = input.trim().trim_matches(['"', '\'']).to_string();
                    if !path.is_empty() {
                        let src = TorrentSource::from_input(&expand_home(&path));
                        if let Err(e) = self.h.add(AddTorrent::new(src)).await {
                            self.note(format!("add failed: {e}"), true);
                        }
                    }
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => self.input.clear(),
                KeyCode::Char(c) => self.input.push(c),
                _ => {}
            },
            Mode::Confirm { delete } => {
                self.mode = Mode::Normal;
                if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    if let Some(h) = self.selected_hash() {
                        match self.h.remove(&h, delete).await {
                            Ok(()) if delete => self.note("removed torrent and data", false),
                            Ok(()) => {}
                            Err(e) => self.note(format!("remove failed: {e}"), true),
                        }
                    }
                }
            }
            Mode::Normal => return self.on_normal_key(k).await,
        }
        false
    }

    async fn on_normal_key(&mut self, k: KeyEvent) -> bool {
        let n = self.snap.torrents.len();
        let sel = self.selected_hash();
        let sel_status = self.table.selected().and_then(|i| self.snap.torrents.get(i)).map(|t| t.status);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                let i = self.table.selected().map_or(0, |i| (i + 1).min(n - 1));
                self.table.select(Some(i));
                self.detail_scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                let i = self.table.selected().map_or(0, |i| i.saturating_sub(1));
                self.table.select(Some(i));
                self.detail_scroll = 0;
            }
            KeyCode::Char('g') | KeyCode::Home if n > 0 => self.table.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End if n > 0 => self.table.select(Some(n - 1)),
            KeyCode::PageDown | KeyCode::Char('J') => self.detail_scroll += 5,
            KeyCode::PageUp | KeyCode::Char('K') => self.detail_scroll = self.detail_scroll.saturating_sub(5),
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => self.cycle_tab(1),
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => self.cycle_tab(-1),
            KeyCode::Char(c @ '1'..='5') => {
                self.tab = Tab::ALL[(c as u8 - b'1') as usize];
                self.detail_scroll = 0;
            }
            KeyCode::Char('a') | KeyCode::Char('o') => {
                self.mode = Mode::Add;
                self.input.clear();
            }
            KeyCode::Char(' ') | KeyCode::Char('p') => {
                if let (Some(h), Some(s)) = (sel, sel_status) {
                    let r = if matches!(s, TorrentStatus::Paused | TorrentStatus::Finished | TorrentStatus::Error) {
                        self.h.resume(&h)
                    } else {
                        self.h.pause(&h)
                    };
                    if let Err(e) = r {
                        self.note(e.to_string(), true);
                    }
                }
            }
            KeyCode::Char('P') => {
                self.h.pause_all();
                self.note("paused all", false);
            }
            KeyCode::Char('U') => {
                self.h.resume_all();
                self.note("resumed all", false);
            }
            KeyCode::Char('r') => {
                if let Some(h) = sel {
                    let _ = self.h.recheck(&h);
                    self.note("rechecking", false);
                }
            }
            KeyCode::Char('R') => {
                if let Some(h) = sel {
                    let _ = self.h.reannounce(&h);
                    self.note("reannouncing", false);
                }
            }
            KeyCode::Char('s') => {
                if let (Some(h), Some(t)) = (sel, self.table.selected().and_then(|i| self.snap.torrents.get(i))) {
                    let on = !t.sequential;
                    let _ = self.h.set_sequential(&h, on);
                    self.note(if on { "sequential on" } else { "sequential off" }, false);
                }
            }
            KeyCode::Char('d') if sel.is_some() => self.mode = Mode::Confirm { delete: false },
            KeyCode::Char('D') if sel.is_some() => self.mode = Mode::Confirm { delete: true },
            _ => {}
        }
        false
    }

    fn cycle_tab(&mut self, d: i32) {
        let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0) as i32;
        self.tab = Tab::ALL[(i + d).rem_euclid(Tab::ALL.len() as i32) as usize];
        self.detail_scroll = 0;
    }
}

fn push(q: &mut VecDeque<u64>, v: u64) {
    if q.len() >= HISTORY {
        q.pop_front();
    }
    q.push_back(v);
}

fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => p.to_string(),
    }
}

/// HH:MM:SS (UTC) without pulling in a date crate.
fn chrono_like_now() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
}
