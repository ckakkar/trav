use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use reqwest::Client;

use crate::error::{BitTorrentError, Result};

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub ip: Ipv4Addr,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackerResponse {
    pub interval: u64,
    #[serde(with = "serde_bytes")]
    pub peers: Vec<u8>,
}

impl TrackerResponse {
    /// Parses the compact peers representation (6 bytes per peer: 4 for IPv4, 2 for Port)
    pub fn parse_peers(&self) -> Vec<PeerInfo> {
        let mut peers = Vec::new();
        let chunks = self.peers.chunks_exact(6);
        for chunk in chunks {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            peers.push(PeerInfo { ip, port });
        }
        peers
    }
}

pub struct TrackerClient {
    client: Client,
    peer_id: String,
    port: u16,
}

impl TrackerClient {
    pub fn new(peer_id: &str, port: u16) -> Self {
        Self {
            client: Client::new(),
            peer_id: peer_id.to_string(),
            port,
        }
    }

    /// URL encodes arbitrary bytes (like the SHA-1 info_hash) to be used in HTTP queries.
    fn urlencode_bytes(bytes: &[u8]) -> String {
        let mut encoded = String::with_capacity(bytes.len() * 3);
        for &b in bytes {
            match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(b as char);
                }
                _ => {
                    encoded.push_str(&format!("%{:02x}", b));
                }
            }
        }
        encoded
    }

    pub async fn announce(
        &self,
        announce_url: &str,
        info_hash: &[u8; 20],
        downloaded: u64,
        uploaded: u64,
        left: u64,
    ) -> Result<TrackerResponse> {
        let info_hash_encoded = Self::urlencode_bytes(info_hash);
        let peer_id_encoded = Self::urlencode_bytes(self.peer_id.as_bytes());

        let url = format!(
            "{}?info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1",
            announce_url,
            info_hash_encoded,
            peer_id_encoded,
            self.port,
            uploaded,
            downloaded,
            left
        );

        let response = self.client.get(&url).send().await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;
        let bytes = response.bytes().await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;
        
        // Parse bencoded response
        let tracker_response: TrackerResponse = serde_bencode::from_bytes(&bytes)?;
        Ok(tracker_response)
    }
}
