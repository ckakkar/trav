use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::error::Result;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtendedHandshake {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub m: Option<HashMap<String, u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p: Option<u16>, // listen port
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v: Option<String>, // client version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_size: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetadataMsg {
    pub msg_type: u8,
    pub piece: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_size: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PexMsg {
    #[serde(with = "serde_bytes", skip_serializing_if = "Option::is_none")]
    pub added: Option<Vec<u8>>,
    #[serde(with = "serde_bytes", skip_serializing_if = "Option::is_none")]
    pub dropped: Option<Vec<u8>>,
    #[serde(with = "serde_bytes", skip_serializing_if = "Option::is_none")]
    pub added_f: Option<Vec<u8>>,
}

impl ExtendedHandshake {
    pub fn new() -> Self {
        let mut m = HashMap::new();
        m.insert("ut_metadata".to_string(), 1); // We map ut_metadata to ID 1
        m.insert("ut_pex".to_string(), 2);      // We map ut_pex to ID 2

        Self {
            m: Some(m),
            p: None,
            v: Some("TravAsync/0.1.0".to_string()),
            metadata_size: None, // Only populated if we HAVE the metadata
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut bytes = vec![0u8]; // Extended Handshake ID is always 0
        let bencoded = serde_bencode::to_bytes(self)?;
        bytes.extend_from_slice(&bencoded);
        Ok(bytes)
    }
}
