use url::Url;
use crate::error::{BitTorrentError, Result};
use hex::FromHex;

#[derive(Debug, Clone)]
pub struct MagnetUri {
    pub info_hash: [u8; 20],
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}

impl MagnetUri {
    /// Parses a standard Magnet URI string into its constituent BitTorrent parameters.
    pub fn parse(uri_str: &str) -> Result<Self> {
        let url = Url::parse(uri_str)
            .map_err(|e| BitTorrentError::Engine(format!("Invalid Magnet URI format: {}", e)))?;

        if url.scheme() != "magnet" {
            return Err(BitTorrentError::Engine("URI scheme must be 'magnet'".to_string()));
        }

        let mut info_hash = None;
        let mut display_name = None;
        let mut trackers = Vec::new();

        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "xt" => {
                    // Expect format 'urn:btih:<hex_string>'
                    let prefix = "urn:btih:";
                    if value.starts_with(prefix) {
                        let hex_str = &value[prefix.len()..];
                        if hex_str.len() == 40 {
                            let parsed_hash = <[u8; 20]>::from_hex(hex_str).map_err(|e| {
                                BitTorrentError::Engine(format!("Failed to parse info_hash hex: {}", e))
                            })?;
                            info_hash = Some(parsed_hash);
                        } else if hex_str.len() == 32 {
                            // Base32 encoded (often used by uTorrent). For strict Phase 2 we only map hex strings, 
                            // but noting that base32 exists.
                            return Err(BitTorrentError::Engine("Base32 info_hash not yet supported".into()));
                        }
                    }
                }
                "dn" => {
                    display_name = Some(value.into_owned());
                }
                "tr" => {
                    trackers.push(value.into_owned());
                }
                _ => {} // Ignore unknown parameters (e.g., xl)
            }
        }

        let info_hash = info_hash.ok_or_else(|| {
            BitTorrentError::Engine(
                "Magnet URI must contain an exact topic (xt=urn:btih:<hash>)".to_string(),
            )
        })?;

        Ok(Self {
            info_hash,
            display_name,
            trackers,
        })
    }
}
