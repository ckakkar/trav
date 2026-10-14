# trav-gui

The Nova UI (Next.js, static export) and `src-tauri/`, the shell that makes it
Trav for macOS.

```bash
npm run tauri dev    # the app with hot reload (or `make run` from the root)
npm run typecheck && npm test && npm run build
```

- `src/lib/rpc.ts`: the bridge to the app's Rust side: engine calls over IPC
  (`rpc`, routed to trav-core's `rpc::dispatch`) and the native integrations
  (dialogs, Finder, menu bar, updates, login item, keep-awake).
- `src/components/`: the UI. `App.tsx` owns state, keyboard and menu actions.
- `src-tauri/src/lib.rs`: engine hosting, Dock, menu bar icon, Trash,
  keep-awake; `menu.rs` the menu bar; `update.rs` updates from GitHub releases.

See the root [README](../README.md) and [USAGE](../USAGE.md).
