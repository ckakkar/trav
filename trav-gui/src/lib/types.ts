// Mirrors trav-core/src/snapshot.rs (serde camelCase).

export type TorrentStatus =
  | "paused"
  | "queued"
  | "checking"
  | "metadata"
  | "downloading"
  | "seeding"
  | "finished"
  | "error";

export interface GlobalStats {
  downloadRate: number;
  uploadRate: number;
  sessionDownloaded: number;
  sessionUploaded: number;
  downloadLimit: number;
  uploadLimit: number;
  connectedPeers: number;
  dhtNodes: number;
  listenPort: number;
  connectable: boolean;
  freeSpace: number | null;
  upnp: string | null;
}

export interface TorrentSummary {
  infoHash: string;
  name: string;
  status: TorrentStatus;
  progress: number;
  size: number;
  totalSize: number;
  doneBytes: number;
  downloaded: number;
  uploaded: number;
  downloadRate: number;
  uploadRate: number;
  eta: number | null;
  ratio: number;
  peers: number;
  seeds: number;
  swarmPeers: number | null;
  swarmSeeds: number | null;
  addedAt: number;
  completedAt: number | null;
  savePath: string;
  error: string | null;
  queuePosition: number;
  hasMetadata: boolean;
  sequential: boolean;
  private: boolean;
  availability: number;
}

export interface EngineSnapshot {
  seq: number;
  torrents: TorrentSummary[];
  stats: GlobalStats;
}

export interface FileInfo {
  index: number;
  path: string;
  size: number;
  done: number;
  progress: number;
  priority: number;
}

export interface PeerInfo {
  addr: string;
  client: string;
  flags: string;
  progress: number;
  downloadRate: number;
  uploadRate: number;
  downloaded: number;
  uploaded: number;
  source: string;
}

export interface TrackerInfo {
  url: string;
  tier: number;
  status: "idle" | "announcing" | "working" | "error";
  message: string | null;
  peers: number;
  seeds: number | null;
  leechers: number | null;
  nextAnnounce: number | null;
}

export interface TorrentDetails {
  summary: TorrentSummary;
  comment: string | null;
  createdBy: string | null;
  creationDate: number | null;
  pieceLength: number;
  numPieces: number;
  pieces: string;
  piecesInProgress: string;
  files: FileInfo[];
  peers: PeerInfo[];
  trackers: TrackerInfo[];
  magnet: string;
  contentPath: string | null;
  wasted: number;
  hashFails: number;
  dhtEnabled: boolean;
}

export interface TorrentPreview {
  infoHash: string;
  name: string;
  totalSize: number;
  pieceLength: number;
  numPieces: number;
  files: { path: string; size: number }[];
  trackers: string[];
  comment: string | null;
  createdBy: string | null;
  private: boolean;
  alreadyAdded: boolean;
}

export interface Settings {
  downloadDir: string;
  listenPort: number;
  maxPeersPerTorrent: number;
  maxPeersGlobal: number;
  maxActiveDownloads: number;
  downloadLimit: number;
  uploadLimit: number;
  uploadSlots: number;
  enableDht: boolean;
  enablePex: boolean;
  enableUpnp: boolean;
  seedRatioLimit: number;
}

export type EngineEvent =
  | { type: "torrentAdded"; infoHash: string; name: string }
  | { type: "metadataReceived"; infoHash: string; name: string }
  | { type: "torrentCompleted"; infoHash: string; name: string }
  | { type: "torrentError"; infoHash: string; name: string; message: string }
  | { type: "torrentRemoved"; infoHash: string };
