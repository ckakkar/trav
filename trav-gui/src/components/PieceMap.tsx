"use client";

import { useEffect, useRef } from "react";

export default function PieceMap({ base64Data }: { base64Data: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    if (!canvasRef.current) return;
    const ctx = canvasRef.current.getContext("2d");
    if (!ctx) return;

    // Decode base64 to binary string
    const binaryStr = atob(base64Data);
    const bytes = new Uint8Array(binaryStr.length);
    for (let i = 0; i < binaryStr.length; i++) {
      bytes[i] = binaryStr.charCodeAt(i);
    }

    // Typical piece map visual settings
    const bits = bytes.length * 8;
    const cols = 200;
    const rows = Math.ceil(bits / cols);
    const cellSize = 3;

    canvasRef.current.width = cols * cellSize;
    canvasRef.current.height = rows * cellSize;

    ctx.fillStyle = "#1e293b"; // Tailwind slate-800
    ctx.fillRect(0, 0, canvasRef.current.width, canvasRef.current.height);

    ctx.fillStyle = "#10b981"; // Tailwind emerald-500 (completed piece)

    let bitIndex = 0;
    for (let i = 0; i < bytes.length; i++) {
        for (let j = 7; j >= 0; j--) {
            if (bitIndex >= bits) break;
            
            const isSet = (bytes[i] & (1 << j)) !== 0;
            if (isSet) {
                const x = (bitIndex % cols) * cellSize;
                const y = Math.floor(bitIndex / cols) * cellSize;
                ctx.fillRect(x, y, cellSize - 1, cellSize - 1);
            }
            bitIndex++;
        }
    }
  }, [base64Data]);

  return (
    <div className="border border-slate-700 p-2 rounded-lg bg-slate-900 inline-block">
      <h3 className="text-xs text-slate-400 mb-2 uppercase tracking-wider font-semibold">Piece Map</h3>
      <canvas ref={canvasRef} className="block shadow-inner"></canvas>
    </div>
  );
}
