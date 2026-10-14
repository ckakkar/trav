import { readFileSync } from "node:fs";

const { version } = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8"));

/** @type {import('next').NextConfig} */
const nextConfig = {
  // Static export: served by Tauri (the macOS app) and embedded in `trav --daemon` (web).
  output: "export",
  images: { unoptimized: true },
  devIndicators: false,
  env: { NEXT_PUBLIC_TRAV_VERSION: version },
};

export default nextConfig;
