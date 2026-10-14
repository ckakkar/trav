# trav-gui

The Nova UI (Next.js, static export) and `src-tauri/`, the shell that makes it
Trav for macOS. The same export is embedded in `trav --daemon` as the web UI.

```bash
npm run dev          # UI only, against a daemon: NEXT_PUBLIC_TRAV_API=http://127.0.0.1:9696
npm run tauri dev    # the Mac app with hot reload (or `make desktop` from the root)
npm run typecheck && npm test && npm run build
npm run e2e          # Playwright against real daemons (`make e2e` builds them first)
```

- `src/lib/rpc.ts`: the transport (Tauri IPC in the app, HTTP + SSE against a
  daemon) and the app-only integrations (menu bar, updates, login item).
- `src/components/`: the UI. `App.tsx` owns state, keyboard and menu actions.
- `src-tauri/src/lib.rs`: engine hosting, Dock, menu bar icon, Trash, keep-awake;
  `menu.rs` the menu bar; `update.rs` updates from GitHub releases.

See the root [README](../README.md) and [USAGE](../USAGE.md).
