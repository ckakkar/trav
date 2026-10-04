import type { Metadata, Viewport } from "next";
import { GeistSans } from "geist/font/sans";
import { GeistMono } from "geist/font/mono";
import "@fontsource/instrument-serif/400.css";
import "./globals.css";

export const metadata: Metadata = {
  title: "Trav",
  description: "A fast, quiet BitTorrent client.",
};

export const viewport: Viewport = {
  themeColor: "#0c0d10",
  colorScheme: "dark light",
};

// Apply the saved theme before first paint so there is no flash.
const boot = `try{var p=JSON.parse(localStorage.getItem("trav.ui.prefs")||"{}");document.documentElement.dataset.theme=p.theme||"ink";document.documentElement.dataset.scanlines=p.scanlines===false?"off":"on"}catch(e){document.documentElement.dataset.theme="ink"}`;

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en" data-theme="ink" className={`${GeistSans.variable} ${GeistMono.variable}`} suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: boot }} />
      </head>
      <body>{children}</body>
    </html>
  );
}
