/** @type {import('next').NextConfig} */
const nextConfig = {
  // Static export, bundled into the macOS app by Tauri.
  output: "export",
  images: { unoptimized: true },
  devIndicators: false,
};

export default nextConfig;
