# xTermius

Tauri (Rust) + React + Vite SSH terminal app using `xterm.js` and a PTY backend.

## Requirements

- Node.js (LTS recommended)
- Rust toolchain
- Tauri prerequisites for macOS

## Dev

Frontend only:

```bash
cd /Users/luang/Downloads/xTermius/xtermius
npm install
npm run dev
```

Tauri app (recommended for PTY/SSH):

```bash
cd /Users/luang/Downloads/xTermius/xtermius
npm install
npm run tauri dev
```

## Icons

Regenerate Tauri icons from `../icon.png`:

```bash
cd /Users/luang/Downloads/xTermius/xtermius
npm run icons
```

## Notes

- If Tauri dev shows a blank window during startup, wait a few seconds; the WebView can hit transient dev-server timeouts while Vite is coming up.
- Host configuration is generated into the OS config directory under `xtermius/` (see `hosts_save` and `generate_ssh_config` in `src-tauri`).
