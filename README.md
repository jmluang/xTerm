# xTermius

Tauri (Rust) + React + Vite SSH terminal app using `xterm.js` and a PTY backend.

## Requirements

- Node.js (LTS recommended)
- Rust toolchain
- Tauri prerequisites for macOS

## Dev

Run commands from the repository root.

Frontend only:

```bash
npm install
npm run dev
```

Tauri app (recommended for PTY/SSH):

```bash
npm install
npm run tauri dev
```

## Performance Baseline (P0)

Startup cold benchmark (packaged app, macOS):

```bash
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

## GitHub CI and Release

This repo uses two GitHub Actions workflows:

- `.github/workflows/ci.yml`: runs `npm ci`, `npm run build`, updater regression checks, release workflow regression checks, and Rust tests.
- `.github/workflows/release.yml`: builds and publishes the dual-architecture macOS release.

The release workflow runs on `v*` tag pushes or manual dispatch with a `tag` input. It builds:

- Apple Silicon (`aarch64-apple-darwin`) DMG and updater archive.
- Intel (`x86_64-apple-darwin`) DMG and updater archive.
- `latest.json` plus updater signatures for the Tauri updater.

Before publishing a draft release, the finalizer checks that `latest.json` matches the release version, points platform URLs at the current tag, has non-empty signatures, and references assets attached to the GitHub Release.

Typical release flow:

```bash
git tag v0.3.3
git push origin v0.3.3
```

## Icons

Regenerate Tauri icons from `../icon.png`:

```bash
npm run icons
```

## Notes

- If Tauri dev shows a blank window during startup, wait a few seconds; the WebView can hit transient dev-server timeouts while Vite is coming up.
- Host configuration is generated into the OS config directory under `xtermius/` (see `hosts_save` and `generate_ssh_config` in `src-tauri`).
