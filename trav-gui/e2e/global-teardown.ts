import fs from "node:fs";
import { ENV_FILE, readEnv } from "./env";

export default async function teardown() {
  if (!fs.existsSync(ENV_FILE)) return;
  for (const pid of readEnv().pids) {
    try {
      process.kill(pid, "SIGTERM");
    } catch {
      /* already gone */
    }
  }
}
