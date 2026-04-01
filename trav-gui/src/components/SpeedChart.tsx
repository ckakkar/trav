"use client";

import { useState, useEffect } from "react";
import { AreaChart, Area, XAxis, YAxis, Tooltip, ResponsiveContainer } from "recharts";

type SpeedData = {
    time: string;
    download: number;
    upload: number;
};

export default function SpeedChart({ currentDown, currentUp }: { currentDown: number, currentUp: number }) {
    const [data, setData] = useState<SpeedData[]>([]);

    useEffect(() => {
        const now = new Date().toLocaleTimeString();
        // Convert to MB/s
        const dl = currentDown / 1024 / 1024;
        const ul = currentUp / 1024 / 1024;

        setData(prev => {
            const next = [...prev, { time: now, download: dl, upload: ul }];
            if (next.length > 50) next.shift(); // Keep last 50 points
            return next;
        });
    }, [currentDown, currentUp]);

    return (
        <div className="h-48 w-full p-4 bg-slate-900 border border-slate-700 rounded-lg">
            <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={data}>
                    <defs>
                        <linearGradient id="colorDl" x1="0" y1="0" x2="0" y2="1">
                            <stop offset="5%" stopColor="#10b981" stopOpacity={0.8}/>
                            <stop offset="95%" stopColor="#10b981" stopOpacity={0}/>
                        </linearGradient>
                        <linearGradient id="colorUl" x1="0" y1="0" x2="0" y2="1">
                            <stop offset="5%" stopColor="#f43f5e" stopOpacity={0.8}/>
                            <stop offset="95%" stopColor="#f43f5e" stopOpacity={0}/>
                        </linearGradient>
                    </defs>
                    <XAxis dataKey="time" hide />
                    <YAxis unit=" MB/s" stroke="#64748b" fontSize={10} domain={[0, 'dataMax']} />
                    <Tooltip contentStyle={{ backgroundColor: '#0f172a', border: 'none', color: '#f8fafc' }} />
                    <Area type="monotone" dataKey="download" stroke="#10b981" fillOpacity={1} fill="url(#colorDl)" isAnimationActive={false} />
                    <Area type="monotone" dataKey="upload" stroke="#f43f5e" fillOpacity={1} fill="url(#colorUl)" isAnimationActive={false} />
                </AreaChart>
            </ResponsiveContainer>
        </div>
    );
}
