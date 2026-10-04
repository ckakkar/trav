import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const LAB = path.join(os.tmpdir(), "trav-e2e");
export const ENV_FILE = path.join(LAB, "env.json");

export interface LabEnv {
  pids: number[];
  seedPort: number;
  torrents: { name: string; path: string; hash: string }[];
  lockedUrl: string;
  token: string;
}

export const readEnv = (): LabEnv => JSON.parse(fs.readFileSync(ENV_FILE, "utf8"));
