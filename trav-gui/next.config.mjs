/** @type {import('next').NextConfig} */
const nextConfig = {
  // Static export: served by Tauri (desktop) and embedded in `trav --daemon` (web).
  output: "export",
  images: { unoptimized: true },
  devIndicators: false,
};

export default nextConfig;
