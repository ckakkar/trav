import type { Metadata, Viewport } from "next";
import { GeistSans } from "geist/font/sans";
import { GeistMono } from "geist/font/mono";
// Paper's face: one variable family, from extra-condensed poster type to text.
import "@fontsource-variable/archivo/wdth.css";
// The newer themes' faces, Latin only: Textmode, Teletext, Instrument and
// Receipt (Doto), Comic (Bangers, Comic Neue).
import "@fontsource/vt323/latin-400.css";
import "@fontsource/tiny5/latin-400.css";
import "@fontsource-variable/doto/wght.css";
import "@fontsource/bangers/latin-400.css";
import "@fontsource/comic-neue/latin-400.css";
import "@fontsource/comic-neue/latin-700.css";
import "./globals.css";
// One stylesheet per newer theme, after the shared one.
import "./themes/textmode.css";
import "./themes/teletext.css";
import "./themes/instrument.css";
import "./themes/swarm.css";
import "./themes/departures.css";
import "./themes/receipt.css";
import "./themes/comic.css";

export const metadata: Metadata = {
  title: "Trav",
  description: "A fast, quiet BitTorrent client.",
};

export const viewport: Viewport = {
  themeColor: "#04070d",
  colorScheme: "dark light",
};

// Apply the saved theme before first paint so there is no flash.
const boot = `try{var p=JSON.parse(localStorage.getItem("trav.ui.prefs")||"{}");var t=p.theme||"ink";if(p.autoTheme)t=matchMedia("(prefers-color-scheme: dark)").matches?p.darkTheme||"ink":p.lightTheme||"paper";document.documentElement.dataset.theme=t;document.documentElement.dataset.scanlines=p.scanlines===false?"off":"on"}catch(e){document.documentElement.dataset.theme="ink"}`;

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
