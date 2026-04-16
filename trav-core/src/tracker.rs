use serde::Deserialize;
use std::net::Ipv4Addr;
use std::time::Duration;
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
    pub fn parse_peers(&self) -> Vec<PeerInfo> {
        self.peers
            .chunks_exact(6)
            .map(|chunk| PeerInfo {
                ip: Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]),
                port: u16::from_be_bytes([chunk[4], chunk[5]]),
            })
            .collect()
    }
}

pub struct TrackerClient {
    client: Client,
    peer_id: String,
    port: u16,
}

impl TrackerClient {
    pub fn new(peer_id: &str, port: u16) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            client,
            peer_id: peer_id.to_string(),
            port,
        }
    }

    fn urlencode_bytes(bytes: &[u8]) -> String {
        let mut encoded = String::with_capacity(bytes.len() * 3);
        for &b in bytes {
            match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(b as char);
                }
                _ => encoded.push_str(&format!("%{:02x}", b)),
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
        let url = format!(
            "{}?info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1",
            announce_url,
            Self::urlencode_bytes(info_hash),
            Self::urlencode_bytes(self.peer_id.as_bytes()),
            self.port,
            uploaded,
            downloaded,
            left,
        );

        let bytes = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BitTorrentError::Engine(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let resp: TrackerResponse = serde_bencode::from_bytes(&bytes)?;
        Ok(resp)
    }
}

use tokio::net::UdpSocket;
use bytes::{Buf, BufMut, BytesMut};
use url::Url;

const UDP_TIMEOUT: Duration = Duration::from_secs(8);

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
        let parsed = Url::parse(announce_url)
            .map_err(|e| BitTorrentError::Engine(format!("Invalid UDP tracker URL: {}", e)))?;

        let port = parsed.port().unwrap_or(6969); // UDP trackers default to 6969
        let host = parsed.host_str().unwrap_or("");
        let addr = format!("{}:{}", host, port);

        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| BitTorrentError::Engine(e.to_string()))?;
        socket
            .connect(&addr)
            .await
            .map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let transaction_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();

        // 1. Connect request
        let mut connect_req = BytesMut::with_capacity(16);
        connect_req.put_u64(0x41727101980);
        connect_req.put_u32(0);
        connect_req.put_u32(transaction_id);

        socket
            .send(&connect_req)
            .await
            .map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let mut buf = vec![0u8; 1024];
        let n = tokio::time::timeout(UDP_TIMEOUT, socket.recv(&mut buf))
            .await
            .map_err(|_| BitTorrentError::Engine("UDP connect response timeout".into()))?
            .map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let mut response = &buf[..n];
        if response.len() < 16 {
            return Err(BitTorrentError::Engine("UDP connect response too short".into()));
        }

        let action = response.get_u32();
        let res_tid = response.get_u32();
        if action != 0 || res_tid != transaction_id {
            return Err(BitTorrentError::Engine("Invalid UDP connect response".into()));
        }
        let connection_id = response.get_u64();

        // 2. Announce request
        let ann_tid = transaction_id.wrapping_add(1);
        let mut ann_req = BytesMut::with_capacity(98);
        ann_req.put_u64(connection_id);
        ann_req.put_u32(1);
        ann_req.put_u32(ann_tid);
        ann_req.put_slice(info_hash);

        let mut peer_id_bytes = [0u8; 20];
        let pid = self.peer_id.as_bytes();
        peer_id_bytes[..pid.len().min(20)].copy_from_slice(&pid[..pid.len().min(20)]);
        ann_req.put_slice(&peer_id_bytes);

        ann_req.put_u64(downloaded);
        ann_req.put_u64(left);
        ann_req.put_u64(uploaded);
        ann_req.put_u32(2); // event: started
        ann_req.put_u32(0);
        ann_req.put_u32(0);
        ann_req.put_i32(-1);
        ann_req.put_u16(self.port);

        socket
            .send(&ann_req)
            .await
            .map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let mut buf = vec![0u8; 2048];
        let n = tokio::time::timeout(UDP_TIMEOUT, socket.recv(&mut buf))
            .await
            .map_err(|_| BitTorrentError::Engine("UDP announce response timeout".into()))?
            .map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        let mut response = &buf[..n];
        if response.len() < 20 {
            return Err(BitTorrentError::Engine("UDP announce response too short".into()));
        }

        let action = response.get_u32();
        let res_tid = response.get_u32();
        if action != 1 || res_tid != ann_tid {
            return Err(BitTorrentError::Engine("Invalid UDP announce response".into()));
        }

        let interval = response.get_u32();
        let _leechers = response.get_u32();
        let _seeders = response.get_u32();
        let peers_bytes = response.to_vec();

        Ok(TrackerResponse {
            interval: interval as u64,
            peers: peers_bytes,
        })
    }
}
