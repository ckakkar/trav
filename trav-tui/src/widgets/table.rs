use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Row, Table, Paragraph},
    Frame,
};
use crate::state::{TuiState, Status};

pub fn draw_ui(f: &mut Frame, state: &mut TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(7),   // Sparklines Header
            Constraint::Min(10),     // Torrents Table
            Constraint::Length(10),  // Logs + Peer Health
        ].as_ref())
        .split(f.size());

    // 1. Render Header/Stats (incorporating Ratatui Sparkline)
    let current_down = state.global_down_history.last().copied().unwrap_or(0);
    let current_up = state.global_up_history.last().copied().unwrap_or(0);
    let total_peers: usize = state.torrents.iter().map(|t| t.peers).sum();
    
    let spark_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(chunks[0]);

    let down_sparkline = ratatui::widgets::Sparkline::default()
        .block(Block::default().borders(Borders::ALL).title(format!("Download {}/s", format_bytes(current_down))))
        .data(&state.global_down_history)
        .style(Style::default().fg(Color::Green));

    let up_sparkline = ratatui::widgets::Sparkline::default()
        .block(Block::default().borders(Borders::ALL).title(format!("Upload {}/s", format_bytes(current_up))))
        .data(&state.global_up_history)
        .style(Style::default().fg(Color::Red));

    f.render_widget(down_sparkline, spark_layout[0]);
    f.render_widget(up_sparkline, spark_layout[1]);

    // 2. Render Torrents Table
    let selected_style = Style::default().add_modifier(Modifier::REVERSED);
    let normal_style = Style::default().bg(Color::DarkGray);

    let header_cells = ["Name", "Size", "Progress", "Peers", "Down Speed", "Up Speed", "Health", "Status"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let header_row = Row::new(header_cells)
        .style(normal_style)
        .height(1)
        .bottom_margin(1);

    let rows: Vec<Row> = state.torrents.iter().map(|torrent| {
        let size_str = format_bytes(torrent.size_bytes);
        let down_str = format_speed(torrent.download_hz);
        let up_str = format_speed(torrent.upload_hz);
        
        let status_color = match torrent.status {
            Status::Downloading => Color::Blue,
            Status::Seeding => Color::Green,
            Status::Paused => Color::DarkGray,
            Status::Error => Color::Red,
        };
        let status_str = format!("{:?}", torrent.status);

        let progress_str = format!("{:.1}%", torrent.progress);

        let health_color = match torrent.health_badge.as_str() {
            "BAD" => Color::Red,
            "WARN" => Color::Yellow,
            _ => Color::Green,
        };

        Row::new(vec![
            Cell::from(torrent.name.clone()),
            Cell::from(size_str),
            Cell::from(progress_str), 
            Cell::from(torrent.peers.to_string()),
            Cell::from(down_str),
            Cell::from(up_str),
            Cell::from(torrent.health_badge.clone()).style(Style::default().fg(health_color)),
            Cell::from(status_str).style(Style::default().fg(status_color)),
        ])
        .height(1)
        .bottom_margin(0)
    }).collect();

    let table = Table::new(rows, &[
        Constraint::Percentage(30),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(15),
        Constraint::Length(15),
        Constraint::Length(8),
        Constraint::Min(8),
    ])
    .header(header_row)
    .block(Block::default().borders(Borders::ALL).title(format!("Active Torrents | Total Peers: {}", total_peers)))
    .highlight_style(selected_style)
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, chunks[1], &mut state.table_state);

    // 3. Render Logs + Peer Health details
    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)].as_ref())
        .split(chunks[2]);

    let logs_text = state.logs.iter().map(|msg| Line::from(msg.as_str())).collect::<Vec<_>>();
    let logs_widget = Paragraph::new(logs_text)
        .block(Block::default().borders(Borders::ALL).title("Engine Event Logs"));
    f.render_widget(logs_widget, bottom_chunks[0]);

    let peer_health_lines = selected_peer_lines(state);
    let peers_widget = Paragraph::new(peer_health_lines)
        .block(Block::default().borders(Borders::ALL).title("Selected Torrent Peer Health"));
    f.render_widget(peers_widget, bottom_chunks[1]);
}

fn format_bytes(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut unit_idx = 0;
    let mut val = bytes as f64;
    while val >= 1024.0 && unit_idx < units.len() - 1 {
        val /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.1} {}", val, units[unit_idx])
}

fn format_speed(hz: u64) -> String {
    format!("{}/s", format_bytes(hz))
}

fn selected_peer_lines(state: &TuiState) -> Vec<Line<'static>> {
    let Some(selected_idx) = state.table_state.selected() else {
        return vec![Line::from("No torrent selected")];
    };
    let Some(torrent) = state.torrents.get(selected_idx) else {
        return vec![Line::from("No torrent selected")];
    };
    let Some(peers) = state.peer_health_map.get(&torrent.hash) else {
        return vec![Line::from("No peer telemetry yet")];
    };
    if peers.is_empty() {
        return vec![Line::from("No peers connected")];
    }

    peers
        .iter()
        .take(5)
        .map(|p| {
            let badge = if p.penalty_score >= 8 {
                "BAD"
            } else if p.penalty_score >= 3 {
                "WARN"
            } else {
                "GOOD"
            };
            Line::from(format!(
                "{} [{}] score={} net={} data={} t/o={} bad={} hash={}",
                p.addr, badge, p.penalty_score, p.network_penalty, p.data_penalty, p.timeout_count, p.bad_data_count, p.hash_fail_count
            ))
        })
        .collect()
}
