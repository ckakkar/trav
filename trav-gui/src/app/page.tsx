"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import SpeedChart from "@/components/SpeedChart";
import PieceMap from "@/components/PieceMap";
import { Activity, Download, Upload, Settings } from "lucide-react";

type PeerSnapshot = {
  addr: string;
  is_choked: boolean;
  is_interested: boolean;
  peer_choking: boolean;
  peer_interested: boolean;
  download_hz: number;
  upload_hz: number;
  penalty_score: number;
  network_penalty: number;
  data_penalty: number;
  timeout_count: number;
  bad_data_count: number;
  hash_fail_count: number;
};

type TorrentSnapshot = {
  name: string;
  info_hash_hex: string;
  size_bytes: number;
  progress: number;
  download_hz: number;
  upload_hz: number;
  state: string;
  peers: PeerSnapshot[];
  piece_map_base64: string;
};

type EngineSnapshot = {
  total_download_hz: number;
  total_upload_hz: number;
  active_torrents: Record<string, TorrentSnapshot>;
  is_running: boolean;
};

function formatBytes(bytes: number) {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

export default function Home() {
  const [snapshot, setSnapshot] = useState<EngineSnapshot | null>(null);
  const [selectedHash, setSelectedHash] = useState<string | null>(null);

  useEffect(() => {
    let interval: NodeJS.Timeout;
    
    // Tauri specific, check window directly to only invoke backend inside Tauri context
    if (typeof window !== 'undefined' && (window as any).__TAURI_IPC__) {
        interval = setInterval(async () => {
            try {
                const snap = await invoke<EngineSnapshot>("get_snapshot");
                setSnapshot(snap);
            } catch (err) {
                console.error("IPC failed:", err);
            }
        }, 300); // 300ms refersh
    }

    return () => clearInterval(interval);
  }, []);

  if (!snapshot) {
    return (
        <div className="min-h-screen flex items-center justify-center bg-slate-950 text-slate-400">
            Connecting to Trav-Core Engine...
        </div>
    );
  }

  const torrentsList = Object.values(snapshot.active_torrents);
  const selectedTorrent = selectedHash ? snapshot.active_torrents[selectedHash] : torrentsList[0];

  return (
    <main className="min-h-screen bg-slate-950 text-slate-200 p-6 font-sans">
      <div className="max-w-6xl mx-auto space-y-6">
        
        {/* Header / Global Stats */}
        <header className="flex justify-between items-center pb-4 border-b border-slate-800">
            <div className="flex items-center gap-3">
                <Activity className="text-emerald-500 w-8 h-8" />
                <h1 className="text-2xl font-bold tracking-tight">Trav <span className="text-slate-500 font-light">Nova</span></h1>
            </div>
            <div className="flex gap-6">
                <div className="flex flex-col items-end">
                    <span className="text-xs text-slate-500 uppercase font-semibold">Global Down</span>
                    <span className="text-lg text-emerald-400 flex items-center gap-1"><Download className="w-4 h-4" /> {formatBytes(snapshot.total_download_hz)}/s</span>
                </div>
                <div className="flex flex-col items-end">
                    <span className="text-xs text-slate-500 uppercase font-semibold">Global Up</span>
                    <span className="text-lg text-rose-400 flex items-center gap-1"><Upload className="w-4 h-4" /> {formatBytes(snapshot.total_upload_hz)}/s</span>
                </div>
                <button className="p-2 hover:bg-slate-800 rounded-full transition-colors"><Settings className="w-5 h-5 text-slate-400" /></button>
            </div>
        </header>

        <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
            
            {/* Active Torrents List */}
            <div className="col-span-2 space-y-4">
                <h2 className="text-lg font-semibold text-slate-100">Swarming Transfers</h2>
                
                <div className="bg-slate-900 border border-slate-800 rounded-xl overflow-hidden">
                    <table className="w-full text-left">
                        <thead className="bg-slate-950 text-slate-400 text-xs uppercase tracking-wider">
                            <tr>
                                <th className="p-4 font-medium">Name</th>
                                <th className="p-4 font-medium">Size</th>
                                <th className="p-4 font-medium">Done</th>
                                <th className="p-4 font-medium">Status</th>
                                <th className="p-4 font-medium">Speed</th>
                            </tr>
                        </thead>
                        <tbody className="divide-y divide-slate-800/50">
                            {torrentsList.map(t => (
                                <tr 
                                    key={t.info_hash_hex} 
                                    onClick={() => setSelectedHash(t.info_hash_hex)}
                                    className={`cursor-pointer transition hover:bg-slate-800/80 ${selectedHash === t.info_hash_hex ? 'bg-slate-800' : ''}`}
                                >
                                    <td className="p-4 font-medium text-slate-100 truncate max-w-[200px]">{t.name}</td>
                                    <td className="p-4 text-sm text-slate-400">{formatBytes(t.size_bytes)}</td>
                                    <td className="p-4">
                                        <div className="flex items-center gap-3">
                                            <div className="w-full bg-slate-800 rounded-full h-1.5 overflow-hidden">
                                                <div className="bg-emerald-500 h-1.5 rounded-full" style={{ width: `${t.progress}%` }}></div>
                                            </div>
                                            <span className="text-xs font-semibold w-10 text-right">{t.progress.toFixed(1)}%</span>
                                        </div>
                                    </td>
                                    <td className="p-4 text-sm text-blue-400">{t.state}</td>
                                    <td className="p-4 text-sm text-slate-300">↓ {formatBytes(t.download_hz)}/s</td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>

                <div className="pt-4">
                    <SpeedChart currentDown={snapshot.total_download_hz} currentUp={snapshot.total_upload_hz} />
                </div>
            </div>

            {/* Selected Torrent Details Sidebar */}
            <div className="col-span-1 border-l border-slate-800 pl-6 space-y-6">
                {selectedTorrent ? (
                    <>
                        <div>
                            <h3 className="text-lg font-bold text-slate-100 break-all leading-tight">{selectedTorrent.name}</h3>
                            <p className="text-xs text-slate-500 font-mono mt-2 break-all">{selectedTorrent.info_hash_hex}</p>
                        </div>
                        
                        <PieceMap base64Data={selectedTorrent.piece_map_base64} />
                        
                        <div>
                            <h4 className="text-sm font-semibold text-slate-400 uppercase tracking-wider mb-3">Live Peer Connections ({selectedTorrent.peers.length})</h4>
                            <div className="space-y-2 max-h-64 overflow-y-auto pr-2 custom-scrollbar">
                                {selectedTorrent.peers.map((peer, i) => (
                                    <div key={i} className="bg-slate-900 border border-slate-800 p-3 rounded-lg flex items-center justify-between">
                                        <div>
                                            <p className="text-sm font-mono text-slate-300">{peer.addr}</p>
                                            <div className="flex gap-2 mt-1">
                                                <span className={`text-[10px] px-1.5 py-0.5 rounded uppercase font-bold ${peer.is_choked ? 'bg-rose-950 text-rose-400' : 'bg-emerald-950 text-emerald-400'}`}>
                                                    {peer.is_choked ? 'CHOKED' : 'UNCHOKED'}
                                                </span>
                                                <span className={`text-[10px] px-1.5 py-0.5 rounded uppercase font-bold ${peer.peer_interested ? 'bg-blue-950 text-blue-400' : 'bg-slate-800 text-slate-500'}`}>
                                                    {peer.peer_interested ? 'INTERESTED' : 'IDLE'}
                                                </span>
                                            </div>
                                        </div>
                                        <div className="text-right text-xs text-slate-400 font-mono space-y-1">
                                            <div className="text-emerald-400">↓ {formatBytes(peer.download_hz)}/s</div>
                                            <div className="text-rose-400">↑ {formatBytes(peer.upload_hz)}/s</div>
                                            <div className="text-amber-400">penalty {peer.penalty_score}</div>
                                        </div>
                                    </div>
                                ))}
                            </div>
                        </div>
                    </>
                ) : (
                    <div className="text-slate-500 text-center mt-20">Select a torrent to view details</div>
                )}
            </div>
            
        </div>
      </div>
    </main>
  );
}
