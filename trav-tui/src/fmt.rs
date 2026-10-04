pub fn bytes(b: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub fn rate(b: u64) -> String {
    if b == 0 {
        "—".into()
    } else {
        format!("{}/s", bytes(b))
    }
}

pub fn eta(secs: Option<u64>) -> String {
    match secs {
        None => "∞".into(),
        Some(s) if s >= 86_400 * 365 => "∞".into(),
        Some(s) if s >= 86_400 => format!("{}d{}h", s / 86_400, (s % 86_400) / 3600),
        Some(s) if s >= 3600 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        Some(s) if s >= 60 => format!("{}m{:02}s", s / 60, s % 60),
        Some(s) => format!("{s}s"),
    }
}

/// Fixed-width bar using eighth-blocks for sub-cell precision.
pub fn bar(p: f64, width: usize) -> String {
    const PARTS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let p = p.clamp(0.0, 1.0);
    let total = p * width as f64;
    let full = total.floor() as usize;
    let mut s: String = "█".repeat(full);
    if full < width {
        let frac = ((total - full as f64) * 8.0).floor() as usize;
        s.push(if frac == 0 { '·' } else { PARTS[frac] });
        s.push_str(&"·".repeat(width - full - 1));
    }
    s
}

pub fn ago(unix: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = (now - unix).max(0) as u64;
    if d < 60 {
        "just now".into()
    } else {
        format!("{} ago", eta(Some(d)))
    }
}
