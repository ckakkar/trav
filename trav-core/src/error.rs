use thiserror::Error;

#[derive(Error, Debug)]
pub enum BitTorrentError {
    #[error("Network error: {0}")]
    Network(#[from] std::io::Error),
    #[error("Bencode decoding failed: {0}")]
    Bencode(#[from] serde_bencode::Error),
    #[error("Internal Engine Error: {0}")]
    Engine(String),
}

pub type Result<T> = std::result::Result<T, BitTorrentError>;
