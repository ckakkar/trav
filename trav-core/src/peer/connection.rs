use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::net::SocketAddr;
use crate::error::{BitTorrentError, Result};
use tracing::debug;

pub struct PeerConnection {
    pub addr: SocketAddr,
    pub stream: TcpStream,
}

impl PeerConnection {
    /// Attempts to connect to a peer via TCP asynchronously.
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        debug!("Connecting to peer {}...", addr);
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            TcpStream::connect(addr),
        )
        .await
        .map_err(|_| BitTorrentError::Engine(format!("Timeout connecting to {}", addr)))?
        .map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        Ok(Self { addr, stream })
    }

    /// Performs the BitTorrent handshake over the established stream.
    pub async fn handshake(&mut self, info_hash: &[u8; 20], peer_id: &str) -> Result<()> {
        let mut reserved = [0u8; 8];
        reserved[5] |= 0x10; // Extension Protocol support (BEP 10)
        
        let mut handshake = vec![19]; // pstrlen
        handshake.extend_from_slice(b"BitTorrent protocol"); // pstr
        handshake.extend_from_slice(&reserved); // reserved bytes
        handshake.extend_from_slice(info_hash);
        handshake.extend_from_slice(peer_id.as_bytes());

        // Send Handshake
        self.stream.write_all(&handshake).await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;

        // Read Handshake Response
        let mut response = [0u8; 68];
        let bytes_read = self.stream.read_exact(&mut response).await.map_err(|e| BitTorrentError::Engine(e.to_string()))?;
        
        if bytes_read != 68 {
            return Err(BitTorrentError::Engine("Incomplete handshake received".to_string()));
        }

        if &response[1..20] != b"BitTorrent protocol" {
            return Err(BitTorrentError::Engine("Invalid protocol identifier in handshake".to_string()));
        }

        if &response[28..48] != info_hash {
            return Err(BitTorrentError::Engine("Info hash mismatch during handshake".to_string()));
        }

        // We can extract peer_id from &response[48..68] if we want to trace/verify it.
        debug!("Handshake with {} successful.", self.addr);
        Ok(())
    }
}
