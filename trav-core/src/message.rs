use serde::Serialize;

/// Notifications broadcast to every interface (toasts, OS notifications, logs).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    #[serde(rename_all = "camelCase")]
    TorrentAdded { info_hash: String, name: String },
    #[serde(rename_all = "camelCase")]
    MetadataReceived { info_hash: String, name: String },
    #[serde(rename_all = "camelCase")]
    TorrentCompleted { info_hash: String, name: String },
    #[serde(rename_all = "camelCase")]
    TorrentError {
        info_hash: String,
        name: String,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    TorrentRemoved { info_hash: String },
}
