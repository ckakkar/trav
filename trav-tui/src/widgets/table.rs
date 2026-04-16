use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Paragraph, Row, Sparkline, Table,
    },
    Frame,
};
use crate::state::{Status, TorrentState, TuiState};

// ── Colour palette ───────────────────────────────────────────────────────────
const C_ACCENT: Color = Color::Cyan;
const C_DIM: Color = Color::DarkGray;
const C_GOOD: Color = Color::Green;
const C_WARN: Color = Color::Yellow;
const C_BAD: Color = Color::Red;
const C_SEED: Color = Color::Magenta;
const C_TEXT: Color = Color::White;

pub fn draw_ui(f: &mut Frame, state: &mut TuiState) {
    let area = f.size();

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // sparklines / global stats
            Constraint::Min(6),     // torrent table
            Constraint::Length(9),  // logs + peer panel
            Constraint::Length(1),  // status bar
        ])
        .split(area);

    draw_header(f, state, root[0]);
    draw_torrent_table(f, state, root[1]);
    draw_bottom(f, state, root[2]);
    draw_statusbar(f, root[3]);
}

// ── Header: global speed sparklines ─────────────────────────────────────────

fn draw_header(f: &mut Frame, state: &TuiState, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let total_peers: usize = state.torrents.iter().map(|t| t.peers).sum();
    let current_down = state.global_down_history.last().copied().unwrap_or(0);
    let current_up = state.global_up_history.last().copied().unwrap_or(0);

    let down_title = format!(
        " TRAV  ▼ {}  │  Peers: {}  │  Torrents: {} ",
        fmt_speed(current_down),
        total_peers,
        state.torrents.len(),
    );
    let up_title = format!(" ▲ {} ", fmt_speed(current_up));

    let down_spark = Sparkline::default()
        .block(
            Block::default()
                .title(Span::styled(down_title, Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(C_ACCENT)),
        )
        .data(&state.global_down_history)
        .style(Style::default().fg(C_GOOD));

    let up_spark = Sparkline::default()
        .block(
            Block::default()
                .title(Span::styled(up_title, Style::default().fg(C_ACCENT)))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(C_DIM)),
        )
        .data(&state.global_up_history)
        .style(Style::default().fg(C_SEED));

    f.render_widget(down_spark, cols[0]);
    f.render_widget(up_spark, cols[1]);
}

// ── Torrent table ────────────────────────────────────────────────────────────

fn draw_torrent_table(f: &mut Frame, state: &mut TuiState, area: Rect) {
    let header_cells = [
        "Name", "Size", "Progress", "Peers", "▼ Speed", "▲ Speed", "ETA", "Status",
    ]
    .iter()
    .map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(C_ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    });

    let header = Row::new(header_cells)
        .style(Style::default().bg(Color::Black))
        .height(1)
        .bottom_margin(0);

    let rows: Vec<Row> = state
        .torrents
        .iter()
        .map(|t| torrent_row(t))
        .collect();

    let widths = [
        Constraint::Min(20),
        Constraint::Length(10),
        Constraint::Length(20),
        Constraint::Length(6),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(9),
    ];

    let total_peers: usize = state.torrents.iter().map(|t| t.peers).sum();
    let block = Block::default()
        .title(Span::styled(
            format!(" Torrents ({})  ·  Total peers: {} ", state.torrents.len(), total_peers),
            Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_ACCENT));

    let table = Table::new(rows, &widths)
        .header(header)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::Indexed(235))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(table, area, &mut state.table_state);
}

fn torrent_row(t: &TorrentState) -> Row<'_> {
    let name_cell = Cell::from(truncate(&t.name, 30)).style(Style::default().fg(C_TEXT));

    let size_cell = Cell::from(fmt_bytes(t.size_bytes)).style(Style::default().fg(C_DIM));

    let progress_cell = Cell::from(progress_bar(t.progress, 14))
        .style(Style::default().fg(progress_color(t.progress)));

    let peers_cell = Cell::from(t.peers.to_string()).style(Style::default().fg(
        if t.peers > 0 { C_GOOD } else { C_DIM },
    ));

    let down_cell = Cell::from(fmt_speed(t.download_hz)).style(Style::default().fg(
        if t.download_hz > 0 { C_GOOD } else { C_DIM },
    ));

    let up_cell = Cell::from(fmt_speed(t.upload_hz)).style(Style::default().fg(
        if t.upload_hz > 0 { C_SEED } else { C_DIM },
    ));

    let eta_cell = Cell::from(fmt_eta(t)).style(Style::default().fg(C_DIM));

    let (status_label, status_color) = match t.status {
        Status::Downloading => (t.status.label(), C_GOOD),
        Status::Seeding => (t.status.label(), C_SEED),
        Status::Paused => (t.status.label(), C_DIM),
        Status::Connecting => (t.status.label(), C_WARN),
        Status::NoPeers => (t.status.label(), C_BAD),
        Status::Error => (t.status.label(), C_BAD),
    };
    let status_cell = Cell::from(status_label).style(
        Style::default()
            .fg(status_color)
            .add_modifier(Modifier::BOLD),
    );

    Row::new(vec![
        name_cell,
        size_cell,
        progress_cell,
        peers_cell,
        down_cell,
        up_cell,
        eta_cell,
        status_cell,
    ])
    .height(1)
}

fn progress_bar(pct: f32, width: usize) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{} {:5.1}%", "█".repeat(filled), "░".repeat(empty), pct)
}

fn progress_color(pct: f32) -> Color {
    if pct >= 100.0 {
        C_SEED
    } else if pct >= 50.0 {
        C_GOOD
    } else if pct >= 10.0 {
        C_WARN
    } else {
        C_DIM
    }
}

fn fmt_eta(t: &TorrentState) -> String {
    match t.eta_secs() {
        None if t.progress >= 100.0 => "done".to_string(),
        None => "—".to_string(),
        Some(s) if s < 60 => format!("{}s", s),
        Some(s) if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        Some(s) => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}

// ── Bottom panel: logs + peer health ─────────────────────────────────────────

fn draw_bottom(f: &mut Frame, state: &TuiState, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    draw_logs(f, state, cols[0]);
    draw_peer_health(f, state, cols[1]);
}

fn draw_logs(f: &mut Frame, state: &TuiState, area: Rect) {
    let visible = (area.height as usize).saturating_sub(2);
    let lines: Vec<Line> = state
        .logs
        .iter()
        .take(visible)
        .enumerate()
        .map(|(i, msg)| {
            let color = if i == 0 { C_TEXT } else { C_DIM };
            Line::from(Span::styled(msg.as_str(), Style::default().fg(color)))
        })
        .collect();

    let block = Block::default()
        .title(Span::styled(" Engine Logs ", Style::default().fg(C_ACCENT)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_DIM));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_peer_health(f: &mut Frame, state: &TuiState, area: Rect) {
    let lines = peer_health_lines(state, (area.height as usize).saturating_sub(2));

    let block = Block::default()
        .title(Span::styled(" Peer Health ", Style::default().fg(C_ACCENT)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_DIM));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn peer_health_lines(state: &TuiState, limit: usize) -> Vec<Line<'static>> {
    let Some(idx) = state.table_state.selected() else {
        return vec![dim_line("No torrent selected")];
    };
    let Some(t) = state.torrents.get(idx) else {
        return vec![dim_line("No torrent selected")];
    };
    let Some(peers) = state.peer_health_map.get(&t.hash) else {
        return vec![dim_line("No peer data yet")];
    };
    if peers.is_empty() {
        return vec![dim_line("No peers connected")];
    }

    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{} peer(s) connected", peers.len()), Style::default().fg(C_ACCENT)),
    ])];

    for p in peers.iter().take(limit.saturating_sub(1)) {
        let (badge, color) = if p.penalty_score >= 8 {
            ("BAD ", C_BAD)
        } else if p.penalty_score >= 3 {
            ("WARN", C_WARN)
        } else {
            ("GOOD", C_GOOD)
        };
        lines.push(Line::from(vec![
            Span::styled(badge, Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "  {}  sc={} t={} bd={} hf={}",
                truncate(&p.addr, 22),
                p.penalty_score,
                p.timeout_count,
                p.bad_data_count,
                p.hash_fail_count,
            )),
        ]));
    }
    lines
}

// ── Status bar ───────────────────────────────────────────────────────────────

fn draw_statusbar(f: &mut Frame, area: Rect) {
    let text = Paragraph::new(Line::from(vec![
        Span::styled(" [j/k] ", Style::default().fg(C_ACCENT)),
        Span::raw("navigate  "),
        Span::styled("[q/Esc] ", Style::default().fg(C_ACCENT)),
        Span::raw("quit  "),
        Span::styled("[p] ", Style::default().fg(C_ACCENT)),
        Span::raw("pause/resume  "),
        Span::styled("  Add torrent: ", Style::default().fg(C_DIM)),
        Span::styled("trav-cli /path/file.torrent", Style::default().fg(C_DIM)),
    ]))
    .alignment(Alignment::Left)
    .style(Style::default().bg(Color::Indexed(235)));

    f.render_widget(text, area);
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn fmt_bytes(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut idx = 0;
    let mut val = bytes as f64;
    while val >= 1024.0 && idx < units.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    format!("{:.1} {}", val, units[idx])
}

fn fmt_speed(hz: u64) -> String {
    format!("{}/s", fmt_bytes(hz))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

fn dim_line(s: &'static str) -> Line<'static> {
    Line::from(Span::styled(s, Style::default().fg(C_DIM)))
}
