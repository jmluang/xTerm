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

## Performance Baseline (P0)

Startup cold benchmark (packaged app, macOS):

```bash
cd /Users/luang/Downloads/xTermius/xtermius
npm run tauri build
npm run bench:startup
```

Runtime metrics snapshot (in devtools console):

```js
window.__xtermiusPerf?.snapshot()
```

This includes:
- app boot time
- first terminal ready
- first session output
- tab switch latency samples
- resize/fitting counters and jitter count
- memory samples by session count

## GitHub Auto DMG Build

This repo includes a GitHub Actions workflow:

- file: `/Users/luang/Downloads/xTermius/xtermius/.github/workflows/build-dmg.yml`
- trigger:
  - push to `main` / `master`: build DMG and upload as workflow artifact
  - push tag `v*` (for example `v0.1.0`): build DMG and publish it to GitHub Releases

Typical release flow:

```bash
git tag v0.1.0
git push origin v0.1.0
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
