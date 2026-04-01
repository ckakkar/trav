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

use tokio::net::UdpSocket;
use bytes::{Buf, BufMut, BytesMut};
use url::Url;

pub struct UdpTrackerClient {
    port: u16,
    peer_id: String,
}

impl UdpTrackerClient {
    pub fn new(peer_id: &str, port: u16) -> Self {
        Self {
            peer_id: peer_id.to_string(),
            port,
        }
    }

    pub async fn announce(
        &self,
        announce_url: &str,
        info_hash: &[u8; 20],
        downloaded: u64,
        uploaded: u64,
        left: u64,
    ) -> Result<TrackerResponse> {
        let parsed_url = Url::parse(announce_url)
            .map_err(|e| BitTorrentError::Engine(format!("Invalid UDP tracker URL: {}", e)))?;
        
        // Ensure default port is handled
        let port = parsed_url.port().unwrap_or(80);
        let host = parsed_url.host_str().unwrap_or("");
        let addr = format!("{}:{}", host, port);

        let socket = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;
        socket.connect(&addr).await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let transaction_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();

        // 1. Connect Request
        let mut connect_req = BytesMut::with_capacity(16);
        connect_req.put_u64(0x41727101980); // magic protocol ID
        connect_req.put_u32(0); // action: connect
        connect_req.put_u32(transaction_id);

        socket.send(&connect_req).await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let mut buf = vec![0u8; 1024];
        let n = socket.recv(&mut buf).await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;
        let mut response = &buf[..n];

        if response.len() < 16 {
            return Err(BitTorrentError::Engine("UDP Connect response too short".into()));
        }

        let action = response.get_u32();
        let res_transaction_id = response.get_u32();
        
        if action != 0 || res_transaction_id != transaction_id {
            return Err(BitTorrentError::Engine("Invalid UDP Connect response".into()));
        }

        let connection_id = response.get_u64();

        // 2. Announce Request
        let announce_transaction_id = transaction_id.wrapping_add(1);
        let mut announce_req = BytesMut::with_capacity(98);
        announce_req.put_u64(connection_id);
        announce_req.put_u32(1); // action: announce
        announce_req.put_u32(announce_transaction_id);
        announce_req.put_slice(info_hash);
        
        let mut peer_id_bytes = [0u8; 20];
        let pid_bytes = self.peer_id.as_bytes();
        let copy_len = std::cmp::min(20, pid_bytes.len());
        peer_id_bytes[..copy_len].copy_from_slice(&pid_bytes[..copy_len]);
        announce_req.put_slice(&peer_id_bytes);
        
        announce_req.put_u64(downloaded);
        announce_req.put_u64(left);
        announce_req.put_u64(uploaded);
        announce_req.put_u32(2); // event: started
        announce_req.put_u32(0); // ip address (default)
        announce_req.put_u32(0); // key
        announce_req.put_i32(-1); // num_want (default)
        announce_req.put_u16(self.port);

        socket.send(&announce_req).await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let mut buf = vec![0u8; 2048];
        let n = socket.recv(&mut buf).await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;
        let mut response = &buf[..n];

        if response.len() < 20 {
            return Err(BitTorrentError::Engine("UDP Announce response too short".into()));
        }

        let action = response.get_u32();
        let res_transaction_id = response.get_u32();
        
        if action != 1 || res_transaction_id != announce_transaction_id {
            return Err(BitTorrentError::Engine("Invalid UDP Announce response".into()));
        }

        let interval = response.get_u32();
        let _leechers = response.get_u32();
        let _seeders = response.get_u32();

        let peers_bytes = response.to_vec(); // The rest is pairs of 4-byte IP and 2-byte Port

        Ok(TrackerResponse {
            interval: interval as u64,
            peers: peers_bytes,
        })
    }
}
