use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a KRPC Message sent over UDP for the Mainline DHT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KrpcMessage {
    /// Transaction ID. Usually 2 random bytes.
    #[serde(with = "serde_bytes")]
    pub t: Vec<u8>,
    
    /// Type of message: "q" for query, "r" for response, "e" for error.
    pub y: String,
    
    /// Query method name (only if y == "q").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    
    /// Query arguments (only if y == "q").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a: Option<QueryArgs>,

    /// Response dictionary (only if y == "r").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<ResponseArgs>,

    /// Error tuple (only if y == "e").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e: Option<(u32, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryArgs {
    #[serde(with = "serde_bytes")]
    pub id: Vec<u8>,
    
    // For find_node or get_peers
    #[serde(with = "serde_bytes", skip_serializing_if = "Option::is_none")]
    pub target: Option<Vec<u8>>,
    #[serde(with = "serde_bytes", skip_serializing_if = "Option::is_none")]
    pub info_hash: Option<Vec<u8>>,

    // For announce_peer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(with = "serde_bytes", skip_serializing_if = "Option::is_none")]
    pub token: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implied_port: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseArgs {
    #[serde(with = "serde_bytes")]
    pub id: Vec<u8>,
    
    #[serde(with = "serde_bytes", skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<u8>>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<serde_bytes::ByteBuf>>,
    
    #[serde(with = "serde_bytes", skip_serializing_if = "Option::is_none")]
    pub token: Option<Vec<u8>>,
}
