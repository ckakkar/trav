//! Best-effort client identification from peer IDs (Azureus/Shadow style).

pub fn identify(peer_id: &[u8; 20]) -> String {
    if peer_id[0] == b'-' && peer_id[7] == b'-' {
        let code = std::str::from_utf8(&peer_id[1..3]).unwrap_or("??");
        let ver = &peer_id[3..7];
        let name = match code {
            "qB" => "qBittorrent",
            "UT" => "µTorrent",
            "UM" => "µTorrent Mac",
            "UW" => "µTorrent Web",
            "BT" => "BitTorrent",
            "TR" => "Transmission",
            "DE" => "Deluge",
            "LT" => "libtorrent",
            "lt" => "rTorrent",
            "AZ" => "Vuze",
            "BI" => "BiglyBT",
            "BC" => "BitComet",
            "FD" => "Free Download Manager",
            "KT" => "KTorrent",
            "TV" => "Trav",
            "WW" => "WebTorrent",
            "XL" => "Xunlei",
            "SD" => "Thunder",
            "AG" | "A~" => "Ares",
            "TX" => "Tixati",
            "PI" => "PicoTorrent",
            "FW" => "FrostWire",
            "BN" => "Baidu Netdisk",
            "SZ" => "Shareaza",
            "MG" => "MediaGet",
            "RT" => "Retriever",
            "IL" => "iLivid",
            "ZT" => "ZipTorrent",
            _ => return format!("{} {}", sanitize(&peer_id[1..3]), version(ver)),
        };
        return format!("{name} {}", version(ver));
    }
    if peer_id[0] == b'M' && peer_id[2] == b'-' {
        return format!(
            "BitTorrent Mainline {}",
            sanitize(&peer_id[1..6]).replace('-', ".")
        );
    }
    if &peer_id[..4] == b"exbc" {
        return "BitComet".into();
    }
    "Unknown".into()
}

fn version(v: &[u8]) -> String {
    let parts: Vec<String> = v
        .iter()
        .map(|&c| match c {
            b'0'..=b'9' => ((c - b'0') as u32).to_string(),
            b'A'..=b'Z' => ((c - b'A') as u32 + 10).to_string(),
            b'a'..=b'z' => ((c - b'a') as u32 + 36).to_string(),
            _ => String::new(),
        })
        .collect();
    // Trim trailing zero components: 4.6.3.0 -> 4.6.3
    let mut end = parts.len();
    while end > 2 && parts[end - 1] == "0" {
        end -= 1;
    }
    parts[..end].join(".")
}

fn sanitize(b: &[u8]) -> String {
    b.iter()
        .map(|&c| if c.is_ascii_graphic() { c as char } else { '?' })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_clients() {
        let id = *b"-qB4630-abcdefghijkl";
        assert_eq!(super::identify(&id), "qBittorrent 4.6.3");
        let id = *b"-TR4050-abcdefghijkl";
        assert_eq!(super::identify(&id), "Transmission 4.0.5");
    }
}
