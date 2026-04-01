use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Row, Table, Paragraph, Gauge},
    Frame,
};
use crate::state::{TuiState, Status};

pub fn draw_ui(f: &mut Frame, state: &mut TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),  // Header / Stats
            Constraint::Min(10),    // Torrents Table
            Constraint::Length(7),  // Logs
        ].as_ref())
        .split(f.size());

    // 1. Render Header/Stats
    let total_down = format_speed(state.global_down_hz);
    let total_up = format_speed(state.global_up_hz);
    let total_peers: usize = state.torrents.iter().map(|t| t.peers).sum();
    
    let stats_text = format!("Total Speeds: ↓ {} / ↑ {}  |  Total Peers: {}", total_down, total_up, total_peers);
    let header = Paragraph::new(stats_text)
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::ALL).title("Trav BitTorrent Client - Global Stats"));
    f.render_widget(header, chunks[0]);

    // 2. Render Torrents Table
    let selected_style = Style::default().add_modifier(Modifier::REVERSED);
    let normal_style = Style::default().bg(Color::DarkGray);

    let header_cells = ["Name", "Size", "Progress", "Peers", "Down Speed", "Up Speed", "Status"]
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

        // We can't render a fully interactive Gauge widget directly inside a Cell efficiently in ratatui,
        // so we format the progress as a string text bar instead, or just text %
        let progress_str = format!("{:.1}%", torrent.progress);

        Row::new(vec![
            Cell::from(torrent.name.clone()),
            Cell::from(size_str),
            Cell::from(progress_str), // Using text instead of Gauge widget for table cells
            Cell::from(torrent.peers.to_string()),
            Cell::from(down_str),
            Cell::from(up_str),
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
        Constraint::Min(10),
    ])
    .header(header_row)
    .block(Block::default().borders(Borders::ALL).title("Active Torrents"))
    .highlight_style(selected_style)
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, chunks[1], &mut state.table_state);

    // 3. Render Logs
    let logs_text = state.logs.iter().map(|msg| Line::from(msg.as_str())).collect::<Vec<_>>();
    let logs_widget = Paragraph::new(logs_text)
        .block(Block::default().borders(Borders::ALL).title("Engine Event Logs"));
    f.render_widget(logs_widget, chunks[2]);
}

fn format_bytes(mut bytes: u64) -> String {
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
