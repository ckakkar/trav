//! Rendering. Palette mirrors kkrwhofrags.xyz's terminal mode: phosphor green
//! on black with a single signal red.

use base64::Engine as _;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, Tabs, Wrap};
use ratatui::Frame;
use trav_core::snapshot::{TorrentStatus, TorrentSummary};

use crate::app::{Mode, Tab, TuiApp};
use crate::fmt;

const TEXT: Color = Color::Rgb(0x45, 0xcf, 0x6c);
const DIM: Color = Color::Rgb(0x2d, 0x9a, 0x52);
const FAINT: Color = Color::Rgb(0x1e, 0x6c, 0x3a);
const ACCENT: Color = Color::Rgb(0x6a, 0xf5, 0x9a);
const LINE: Color = Color::Rgb(0x12, 0x3a, 0x20);
const SIG: Color = Color::Rgb(0xff, 0x3b, 0x30);
const AMBER: Color = Color::Rgb(0xfe, 0xbc, 0x2e);
const SEL: Color = Color::Rgb(0x0b, 0x1f, 0x12);

fn frame<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(LINE))
        .title(title)
}

fn label(s: &str) -> Span<'_> {
    Span::styled(s, Style::new().fg(FAINT))
}

pub fn draw(f: &mut Frame, app: &mut TuiApp) {
    let [head, graph, list, detail, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(6),
        Constraint::Min(5),
        Constraint::Length(14),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_header(f, app, head);
    draw_graph(f, app, graph);
    draw_table(f, app, list);
    draw_detail(f, app, detail);
    draw_footer(f, app, foot);

    if app.mode == Mode::Help {
        draw_help(f);
    }
}

fn draw_header(f: &mut Frame, app: &TuiApp, area: Rect) {
    let s = &app.snap.stats;
    let mut spans = vec![
        Span::styled(" TRAV ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw("  "),
        Span::styled("▼ ", Style::new().fg(ACCENT)),
        Span::styled(fmt::rate(s.download_rate), Style::new().fg(TEXT).bold()),
        Span::raw("   "),
        Span::styled("▲ ", Style::new().fg(DIM)),
        Span::styled(fmt::rate(s.upload_rate), Style::new().fg(TEXT)),
        Span::raw("   "),
        label("peers "),
        Span::styled(s.connected_peers.to_string(), Style::new().fg(TEXT)),
        Span::raw("   "),
        label("dht "),
        Span::styled(s.dht_nodes.to_string(), Style::new().fg(TEXT)),
        Span::raw("   "),
        label("port "),
        Span::styled(s.listen_port.to_string(), Style::new().fg(TEXT)),
        Span::styled(if s.connectable { " ●" } else { " ○" }, Style::new().fg(if s.connectable { ACCENT } else { FAINT })),
    ];
    if let Some(free) = s.free_space {
        spans.extend([Span::raw("   "), label("free "), Span::styled(fmt::bytes(free), Style::new().fg(TEXT))]);
    }
    if s.download_limit > 0 || s.upload_limit > 0 {
        spans.extend([Span::raw("   "), Span::styled("⧗ limited", Style::new().fg(AMBER))]);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_graph(f: &mut Frame, app: &TuiApp, area: Rect) {
    let [l, r] = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area);
    let w = l.width.saturating_sub(2) as usize;
    let down: Vec<u64> = app.down_hist.iter().rev().take(w).rev().copied().collect();
    let peak = down.iter().copied().max().unwrap_or(0);
    f.render_widget(
        Sparkline::default()
            .block(frame(Line::from(vec![
                Span::styled(" ▼ download ", Style::new().fg(ACCENT)),
                Span::styled(format!("peak {} ", fmt::rate(peak)), Style::new().fg(FAINT)),
            ])))
            .data(down)
            .style(Style::new().fg(ACCENT)),
        l,
    );
    let w = r.width.saturating_sub(2) as usize;
    let up: Vec<u64> = app.up_hist.iter().rev().take(w).rev().copied().collect();
    f.render_widget(
        Sparkline::default()
            .block(frame(Line::from(Span::styled(" ▲ upload ", Style::new().fg(DIM)))))
            .data(up)
            .style(Style::new().fg(DIM)),
        r,
    );
}

fn status_style(s: TorrentStatus) -> (&'static str, Color) {
    match s {
        TorrentStatus::Downloading => ("DOWN", ACCENT),
        TorrentStatus::Seeding => ("SEED", TEXT),
        TorrentStatus::Finished => ("DONE", DIM),
        TorrentStatus::Paused => ("PAUSE", FAINT),
        TorrentStatus::Queued => ("QUEUE", FAINT),
        TorrentStatus::Checking => ("CHECK", AMBER),
        TorrentStatus::Metadata => ("META", AMBER),
        TorrentStatus::Error => ("ERROR", SIG),
    }
}

fn draw_table(f: &mut Frame, app: &mut TuiApp, area: Rect) {
    let header = Row::new(["#", "name", "size", "progress", "state", "▼", "▲", "eta", "s/p", "ratio"])
        .style(Style::new().fg(FAINT).add_modifier(Modifier::BOLD));
    let rows: Vec<Row> = app.snap.torrents.iter().enumerate().map(|(i, t)| torrent_row(i, t)).collect();
    let empty = rows.is_empty();
    let widths = [
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(9),
        Constraint::Length(20),
        Constraint::Length(6),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(5),
    ];
    let title = Line::from(vec![
        Span::styled(" swarm ", Style::new().fg(TEXT)),
        Span::styled(format!("{} ", app.snap.torrents.len()), Style::new().fg(FAINT)),
    ]);
    let table = Table::new(rows, widths)
        .header(header)
        .block(frame(title))
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(SEL).add_modifier(Modifier::BOLD))
        .highlight_symbol(Span::styled("▌", Style::new().fg(ACCENT)));
    f.render_stateful_widget(table, area, &mut app.table);
    if empty {
        let inner = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 2 });
        f.render_widget(
            Paragraph::new(Text::from(vec![
                Line::styled("nothing in the swarm.", Style::new().fg(TEXT)),
                Line::styled("press  a  and paste a magnet link or a path to a .torrent", Style::new().fg(FAINT)),
            ]))
            .alignment(Alignment::Center),
            inner,
        );
    }
}

fn torrent_row(i: usize, t: &TorrentSummary) -> Row<'static> {
    let (st, color) = status_style(t.status);
    let pct = format!(" {:>5.1}%", t.progress * 100.0);
    let bar_color = match t.status {
        TorrentStatus::Downloading => ACCENT,
        TorrentStatus::Error => SIG,
        TorrentStatus::Checking | TorrentStatus::Metadata => AMBER,
        _ => DIM,
    };
    Row::new(vec![
        Cell::from(Span::styled(format!("{:02}", i + 1), Style::new().fg(FAINT))),
        Cell::from(Span::styled(t.name.clone(), Style::new().fg(TEXT))),
        Cell::from(Span::styled(if t.size > 0 { fmt::bytes(t.size) } else { "?".into() }, Style::new().fg(DIM))),
        Cell::from(Line::from(vec![
            Span::styled(fmt::bar(t.progress, 12), Style::new().fg(bar_color)),
            Span::styled(pct, Style::new().fg(TEXT)),
        ])),
        Cell::from(Span::styled(st, Style::new().fg(color).bold())),
        Cell::from(Span::styled(fmt::rate(t.download_rate), Style::new().fg(if t.download_rate > 0 { ACCENT } else { FAINT }))),
        Cell::from(Span::styled(fmt::rate(t.upload_rate), Style::new().fg(if t.upload_rate > 0 { TEXT } else { FAINT }))),
        Cell::from(Span::styled(
            if t.status == TorrentStatus::Downloading { fmt::eta(t.eta) } else { "".into() },
            Style::new().fg(DIM),
        )),
        Cell::from(Span::styled(format!("{}/{}", t.seeds, t.peers), Style::new().fg(DIM))),
        Cell::from(Span::styled(format!("{:.2}", t.ratio), Style::new().fg(DIM))),
    ])
}

fn draw_detail(f: &mut Frame, app: &TuiApp, area: Rect) {
    let idx = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    let titles: Vec<Line> = Tab::ALL.iter().enumerate().map(|(i, t)| Line::from(format!("{} {}", i + 1, t.title()))).collect();
    let block = frame(Line::default());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [tabs_area, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    f.render_widget(
        Tabs::new(titles)
            .select(idx)
            .style(Style::new().fg(FAINT))
            .highlight_style(Style::new().fg(ACCENT).bold().underlined())
            .divider(Span::styled("·", Style::new().fg(LINE))),
        tabs_area,
    );

    let lines: Vec<Line> = match (&app.details, app.tab) {
        (_, Tab::Log) => app.log.iter().map(|l| Line::styled(l.clone(), Style::new().fg(DIM))).collect(),
        (None, _) => vec![Line::styled("select a torrent", Style::new().fg(FAINT))],
        (Some(d), Tab::General) => general(d, body.width),
        (Some(d), Tab::Files) => {
            if d.files.is_empty() {
                vec![Line::styled("waiting for metadata…", Style::new().fg(FAINT))]
            } else {
                d.files
                    .iter()
                    .map(|fi| {
                        let prio = match fi.priority {
                            0 => Span::styled("skip ", Style::new().fg(FAINT)),
                            1 => Span::styled("     ", Style::new()),
                            _ => Span::styled("high ", Style::new().fg(AMBER)),
                        };
                        Line::from(vec![
                            Span::styled(fmt::bar(fi.progress, 10), Style::new().fg(if fi.progress >= 1.0 { DIM } else { ACCENT })),
                            Span::styled(format!(" {:>5.1}% ", fi.progress * 100.0), Style::new().fg(TEXT)),
                            prio,
                            Span::styled(format!("{:>9}  ", fmt::bytes(fi.size)), Style::new().fg(DIM)),
                            Span::styled(fi.path.clone(), Style::new().fg(TEXT)),
                        ])
                    })
                    .collect()
            }
        }
        (Some(d), Tab::Peers) => {
            let mut v = vec![Line::styled(
                format!("{:<24} {:<22} {:<6} {:>6} {:>11} {:>11}", "address", "client", "flags", "have", "▼", "▲"),
                Style::new().fg(FAINT),
            )];
            let mut peers = d.peers.clone();
            peers.sort_by(|a, b| (b.download_rate + b.upload_rate).cmp(&(a.download_rate + a.upload_rate)));
            v.extend(peers.iter().map(|p| {
                Line::styled(
                    format!(
                        "{:<24} {:<22} {:<6} {:>5.0}% {:>11} {:>11}",
                        p.addr,
                        p.client.chars().take(22).collect::<String>(),
                        p.flags,
                        p.progress * 100.0,
                        fmt::rate(p.download_rate),
                        fmt::rate(p.upload_rate)
                    ),
                    Style::new().fg(if p.download_rate > 0 { ACCENT } else { TEXT }),
                )
            }));
            if d.peers.is_empty() {
                v.push(Line::styled("no peers connected", Style::new().fg(FAINT)));
            }
            v
        }
        (Some(d), Tab::Trackers) => {
            let mut v: Vec<Line> = d
                .trackers
                .iter()
                .map(|t| {
                    let c = match t.status.as_str() {
                        "working" => ACCENT,
                        "error" => SIG,
                        "announcing" => AMBER,
                        _ => FAINT,
                    };
                    Line::from(vec![
                        Span::styled(format!("{:<10} ", t.status), Style::new().fg(c)),
                        Span::styled(format!("t{} ", t.tier), Style::new().fg(FAINT)),
                        Span::styled(t.url.clone(), Style::new().fg(TEXT)),
                        Span::styled(
                            format!(
                                "  peers {}  seeds {}  leech {}{}",
                                t.peers,
                                t.seeds.map_or("–".into(), |s| s.to_string()),
                                t.leechers.map_or("–".into(), |s| s.to_string()),
                                t.message.as_ref().map(|m| format!("  · {m}")).unwrap_or_default()
                            ),
                            Style::new().fg(DIM),
                        ),
                    ])
                })
                .collect();
            v.insert(
                0,
                Line::from(vec![
                    Span::styled(format!("{:<10} ", if d.dht_enabled { "enabled" } else { "off" }), Style::new().fg(if d.dht_enabled { ACCENT } else { FAINT })),
                    Span::styled("DHT · PEX", Style::new().fg(TEXT)),
                ]),
            );
            v
        }
    };
    let scroll = app.detail_scroll.min(lines.len().saturating_sub(1)) as u16;
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)), body);
}

fn general(d: &trav_core::TorrentDetails, width: u16) -> Vec<Line<'static>> {
    let s = &d.summary;
    let kv = |k: &'static str, v: String| -> Vec<Span<'static>> {
        vec![Span::styled(format!("{k:<11}"), Style::new().fg(FAINT)), Span::styled(format!("{v:<26}"), Style::new().fg(TEXT))]
    };
    let mut lines = vec![
        Line::from(vec![Span::styled(s.name.clone(), Style::new().fg(ACCENT).bold())]),
        Line::styled(format!("{}  ·  {}", s.info_hash, d.content_path.clone().unwrap_or_else(|| s.save_path.clone())), Style::new().fg(FAINT)),
        Line::default(),
        Line::from([kv("done", fmt::bytes(s.done_bytes)), kv("of", fmt::bytes(s.size)), kv("pieces", format!("{} × {}", d.num_pieces, fmt::bytes(d.piece_length as u64)))].concat()),
        Line::from([kv("downloaded", fmt::bytes(s.downloaded)), kv("uploaded", fmt::bytes(s.uploaded)), kv("ratio", format!("{:.3}", s.ratio))].concat()),
        Line::from([kv("swarm", format!("{} seeds · {} peers", s.swarm_seeds.map_or("?".into(), |x| x.to_string()), s.swarm_peers.map_or("?".into(), |x| x.to_string()))), kv("avail", format!("{:.2}", s.availability)), kv("added", fmt::ago(s.added_at))].concat()),
        Line::from([kv("wasted", fmt::bytes(d.wasted)), kv("hash fails", d.hash_fails.to_string()), kv("mode", if s.sequential { "sequential".into() } else { "rarest-first".into() })].concat()),
    ];
    if let Some(e) = &s.error {
        lines.push(Line::styled(format!("error: {e}"), Style::new().fg(SIG)));
    }
    // Piece map: one cell per bucket of pieces, shaded by completion.
    if d.num_pieces > 0 {
        let have = base64::engine::general_purpose::STANDARD.decode(&d.pieces).unwrap_or_default();
        let cells = (width as usize).saturating_sub(2).max(10);
        let mut map = String::with_capacity(cells * 3);
        for c in 0..cells {
            let a = c * d.num_pieces / cells;
            let b = ((c + 1) * d.num_pieces / cells).max(a + 1);
            let got = (a..b).filter(|&i| have.get(i / 8).is_some_and(|byte| byte & (0x80 >> (i % 8)) != 0)).count();
            let frac = got as f64 / (b - a) as f64;
            map.push(match frac {
                x if x >= 1.0 => '█',
                x if x >= 0.66 => '▓',
                x if x >= 0.33 => '▒',
                x if x > 0.0 => '░',
                _ => '·',
            });
        }
        lines.push(Line::default());
        lines.push(Line::styled(map, Style::new().fg(DIM)));
    }
    lines
}

fn draw_footer(f: &mut Frame, app: &TuiApp, area: Rect) {
    let line = match &app.mode {
        Mode::Add => Line::from(vec![
            Span::styled(" add › ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
            Span::styled(format!(" {}", app.input), Style::new().fg(TEXT)),
            Span::styled("█", Style::new().fg(ACCENT).add_modifier(Modifier::SLOW_BLINK)),
            Span::styled("   magnet link, info-hash or path · enter to add · esc to cancel", Style::new().fg(FAINT)),
        ]),
        Mode::Confirm { delete } => Line::from(vec![
            Span::styled(" confirm ", Style::new().fg(Color::Black).bg(SIG).bold()),
            Span::styled(
                if *delete { "  remove torrent AND delete its data?  y / n" } else { "  remove torrent (keep data)?  y / n" },
                Style::new().fg(SIG),
            ),
        ]),
        _ => {
            if let Some((msg, err, at)) = &app.flash {
                if at.elapsed().as_secs() < 4 {
                    f.render_widget(
                        Paragraph::new(Line::styled(format!(" {msg}"), Style::new().fg(if *err { SIG } else { ACCENT }))),
                        area,
                    );
                    return;
                }
            }
            let keys = [("a", "add"), ("␣", "pause"), ("d", "remove"), ("r", "recheck"), ("s", "seq"), ("tab", "panel"), ("?", "help"), ("q", "quit")];
            let mut spans = vec![Span::raw(" ")];
            for (k, v) in keys {
                spans.push(Span::styled(k, Style::new().fg(ACCENT)));
                spans.push(Span::styled(format!(" {v}   "), Style::new().fg(FAINT)));
            }
            Line::from(spans)
        }
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_help(f: &mut Frame) {
    let area = f.area();
    let w = 58.min(area.width);
    let h = 20.min(area.height);
    let r = Rect::new((area.width - w) / 2, (area.height - h) / 2, w, h);
    let rows = [
        ("j k ↑ ↓", "move selection"),
        ("g G", "first / last"),
        ("a  o", "add magnet / .torrent path"),
        ("space p", "pause / resume"),
        ("P  U", "pause all / resume all"),
        ("d", "remove torrent (keep data)"),
        ("D", "remove torrent and delete data"),
        ("r", "force recheck"),
        ("R", "reannounce to trackers + DHT"),
        ("s", "toggle sequential download"),
        ("tab 1-5", "detail panel"),
        ("J K", "scroll detail panel"),
        ("q esc", "quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| Line::from(vec![Span::styled(format!("  {k:<12}"), Style::new().fg(ACCENT)), Span::styled(*v, Style::new().fg(TEXT))]))
        .collect();
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(frame(Span::styled(" keys ", Style::new().fg(ACCENT))).style(Style::new().bg(Color::Black))), r);
}
