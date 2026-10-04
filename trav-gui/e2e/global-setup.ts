import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { ENV_FILE, LAB, type LabEnv } from "./env";

const BIN = process.env.TRAV_BIN ?? path.resolve(__dirname, "../../target/debug/trav");

async function waitFor(url: string, ms = 30_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      const r = await fetch(url);
      if (r.ok) return;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`timed out waiting for ${url}`);
}

function daemon(name: string, args: string[]): number {
  const log = fs.openSync(path.join(LAB, `${name}.log`), "w");
  const p = spawn(BIN, ["--daemon", "--state-dir", path.join(LAB, `${name}-state`), ...args], {
    stdio: ["ignore", log, log],
    detached: true,
    env: { ...process.env, RUST_LOG: "warn" },
  });
  p.unref();
  return p.pid!;
}

export default async function setup() {
  if (!fs.existsSync(BIN)) throw new Error(`trav binary not found at ${BIN}; run cargo build -p trav-cli`);
  fs.rmSync(LAB, { recursive: true, force: true });
  const content = path.join(LAB, "content");
  fs.mkdirSync(path.join(content, "Album", "Bonus"), { recursive: true });
  fs.writeFileSync(path.join(content, "Album", "01 - Opening.flac"), crypto.randomBytes(3_200_000));
  fs.writeFileSync(path.join(content, "Album", "Bonus", "02 - Demo.flac"), crypto.randomBytes(1_100_000));
  fs.writeFileSync(path.join(content, "Album", "cover.jpg"), crypto.randomBytes(40_000));
  fs.writeFileSync(path.join(content, "notes.iso"), crypto.randomBytes(2_400_000));

  const torrents: LabEnv["torrents"] = [];
  for (const name of ["Album", "notes.iso"]) {
    const out = path.join(LAB, `${name}.torrent`);
    const line = execFileSync(BIN, ["--create", path.join(content, name), "-o", out]).toString();
    torrents.push({ name, path: out, hash: line.split(/\s+/)[0] });
  }

  const seedPort = 47_301;
  const pids = [
    daemon("seed", ["-s", content, "-p", String(seedPort), "--web-bind", "127.0.0.1:9798", ...torrents.map((t) => t.path)]),
    daemon("leech", ["-s", path.join(LAB, "dl"), "-p", "47302", "--web-bind", "127.0.0.1:9797"]),
    daemon("locked", ["-p", "47303", "--web-bind", "127.0.0.1:9799", "--token", "e2e-token"]),
  ];
  await Promise.all([9797, 9798, 9799].map((p) => waitFor(`http://127.0.0.1:${p}/api/health`)));

  // Wait for the seeder to finish verifying its data.
  const end = Date.now() + 30_000;
  for (;;) {
    const r = await fetch("http://127.0.0.1:9798/api/rpc", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ method: "snapshot" }),
    }).then((r) => r.json());
    if (r.result.torrents.length === torrents.length && r.result.torrents.every((t: { status: string }) => t.status === "seeding")) break;
    if (Date.now() > end) throw new Error("seeder never finished checking");
    await new Promise((r) => setTimeout(r, 250));
  }

  const env: LabEnv = { pids, seedPort, torrents, lockedUrl: "http://127.0.0.1:9799/", token: "e2e-token" };
  fs.writeFileSync(ENV_FILE, JSON.stringify(env));
}
