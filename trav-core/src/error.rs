use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bencode: {0}")]
    Bencode(String),
    #[error("invalid torrent: {0}")]
    Metainfo(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("tracker: {0}")]
    Tracker(String),
    #[error("timed out: {0}")]
    Timeout(&'static str),
    #[error("{0}")]
    Engine(String),
}

/// Back-compat alias for the pre-0.2 name.
pub type BitTorrentError = Error;

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn proto(msg: impl Into<String>) -> Error {
    Error::Protocol(msg.into())
}
