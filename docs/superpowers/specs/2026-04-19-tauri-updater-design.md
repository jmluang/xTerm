# xTermius Tauri Updater Design

Date: 2026-04-19

## Goal

Add a user-visible update flow for desktop builds so users can open an About surface, check for updates, and install a newer release from GitHub Releases.

This phase intentionally avoids requiring a paid Apple Developer account. The update pipeline must still be valid, signed for Tauri updater verification, and operable on macOS direct-download builds.

## Scope

In scope:

- Register and configure Tauri v2 official updater support.
- Add an About entry point in the existing app UI.
- Automatically check for updates on macOS builds when the updater controller starts.
- Keep a manual "Check for Updates" action on macOS builds as an explicit retry path.
- Support updater states: idle, checking, up to date, update available, downloading, installing, error.
- Publish updater artifacts and metadata to GitHub Releases.
- Keep the implementation compatible with future Apple code signing / notarization work.

Out of scope for this phase:

- Apple Developer ID signing.
- Apple notarization.
- Silent background auto-install.
- Release channels such as beta/nightly.
- Windows/Linux-specific UX polish beyond keeping the updater pipeline cross-platform compatible.

## Existing Context

- The app already uses Tauri 2.x in [src-tauri/Cargo.toml](../../../src-tauri/Cargo.toml) and [`@tauri-apps/api`](https://www.npmjs.com/package/@tauri-apps/api) in [package.json](../../../package.json).
- The app does not currently include `tauri-plugin-updater` or `@tauri-apps/plugin-updater`.
- Tauri capabilities currently grant `core`, `shell`, and `dialog` permissions only in [src-tauri/capabilities/default.json](../../../src-tauri/capabilities/default.json).
- The current GitHub workflow in [release.yml](../../../.github/workflows/release.yml) builds DMGs and uploads them to a GitHub Release, but it does not produce updater metadata or signed updater artifacts.
- Settings navigation currently lives in [src/components/settings/SettingsPanel.tsx](../../../src/components/settings/SettingsPanel.tsx), [src/components/settings/SettingsWindowApp.tsx](../../../src/components/settings/SettingsWindowApp.tsx), and [src/hooks/useAppController.ts](../../../src/hooks/useAppController.ts). `SettingsSection` in [src/types/settings.ts](../../../src/types/settings.ts) only supports `terminal | sync | import`, so About/update work must extend that shared navigation contract instead of introducing a parallel route.

## Options Considered

### Option A: Tauri official updater + GitHub Releases static JSON

Use `tauri-plugin-updater`, generate updater artifacts during CI, upload `latest.json` plus signed bundles to GitHub Releases, and consume them from the frontend via `check()` / `downloadAndInstall()`.

Pros:

- Official Tauri path.
- Works with the current stack.
- Cross-platform architecture even if macOS is the immediate target.
- Lowest maintenance cost.

Cons:

- Requires updater signing key management.
- macOS user experience remains less polished until Apple signing / notarization is added.

### Option B: Sparkle for macOS

Use the native Sparkle framework for macOS-only updates.

Pros:

- Strong macOS-native updater UX.
- Mature ecosystem.

Cons:

- Splits update logic by platform.
- Adds native integration complexity to a Tauri app.
- Worse long-term fit than Tauri's built-in updater.

### Option C: Tauri updater + hosted update service

Use Tauri updater with CrabNebula Cloud or another managed release backend.

Pros:

- Better release operations.
- Easier future support for channels and staged rollouts.

Cons:

- Adds service dependency and operating cost.
- Not necessary for the current repo size and release model.

## Decision

Use **Option A**.

The app will integrate Tauri's official updater plugin, publish static updater metadata to GitHub Releases, and expose an automatic check plus manual retry flow through a new About surface. The implementation must preserve a clean boundary so Apple signing and notarization can be layered on later without rewriting app logic.

## User Experience

Add an About action in the existing settings UI rather than inventing a separate top-level window in this phase. This keeps navigation simple and reuses existing settings patterns.

In code terms, phase 1 extends the existing settings section model with an `about` section and routes both the main-window settings launcher and the standalone settings window through the same section value.

The About section should show:

- app name
- current version
- release channel label: `stable`
- update status text on macOS builds
- automatic update-check status on macOS builds
- `Check for Updates` button on macOS builds for retry/manual refresh
- when an update is available on macOS builds:
  - target version
  - optional release notes snippet
  - `Download and Install` button

On non-macOS builds in phase 1, the About section shows version and channel information only. It does not expose updater actions.

Suggested state progression:

```text
Idle -> Checking -> UpToDate
                 -> UpdateAvailable -> Downloading -> Installing -> RestartRequired
                 -> Error
```

ASCII wireframe (version numbers are illustrative only; actual values come from `package.json` / `tauri.conf.json` at build time):

```text
+--------------------------------------------------+
| Settings                                          |
|                                                  |
|  [Terminal] [Sync] [Import] [About]              |
|                                                  |
|  About xTermius                                  |
|  Version: <current>                              |
|  Channel: stable                                 |
|                                                  |
|  Status: Checking... -> Up to date               |
|  [Check for Updates]                             |
|                                                  |
|  If update exists:                               |
|  New version: <target>                           |
|  Notes: Fix terminal session restore...          |
|  [Download and Install]                          |
+--------------------------------------------------+
```

## Architecture

### Frontend

Add a dedicated updater controller hook that owns:

- current version
- check status
- discovered update metadata
- progress state
- last error

This hook should isolate all updater calls from presentational components. UI components should receive plain state and event handlers, not updater API objects.

The hook should sit above [SettingsPanel.tsx](../../../src/components/settings/SettingsPanel.tsx) and be consumed by both [useAppController.ts](../../../src/hooks/useAppController.ts) and [SettingsWindowApp.tsx](../../../src/components/settings/SettingsWindowApp.tsx), because those are the two places that currently feed state into the shared settings surface.

Primary frontend responsibilities:

- call updater `check()`
- trigger one automatic updater check when updater actions are supported
- store update metadata if present
- call `downloadAndInstall()` when the user confirms
- call `relaunch()` after a successful install so the new version is activated immediately
- map raw updater errors into concise UI copy
- gate updater UI and updater API usage behind a shared desktop-macOS capability check in phase 1

Platform gating belongs in the updater controller layer, not inside the About card markup. The UI should receive a simple `supportsUpdaterActions` boolean so non-macOS builds render the same About section without exposing dead buttons.

### Rust / Tauri

Register `tauri_plugin_updater` in the builder setup alongside existing plugins.

Register `tauri_plugin_process` as the companion restart path for a completed install.

Most update actions can stay in frontend guest bindings. Rust should only hold configuration, plugin registration, and capability exposure unless a platform-specific limitation later forces backend orchestration.

### CI / Release Pipeline

The release workflow must stop being "DMG upload only" and become "updater-aware release publishing".

Required outputs:

- GitHub Release asset for direct install
- updater bundle artifact for each supported target
- `.sig` for each updater artifact
- `latest.json` static manifest consumable by Tauri updater

This is the root-cause fix. Without these artifacts, any About-page button is fake UI.

## Configuration Changes

### Rust dependencies

Add `tauri-plugin-updater` to desktop targets in `src-tauri/Cargo.toml`.

Add `tauri-plugin-process` so the installed update can relaunch into the new version without a manual restart prompt.

### Frontend dependencies

Add `@tauri-apps/plugin-updater`.

Add `@tauri-apps/plugin-process` for the post-install relaunch step.

### Tauri config

Update [src-tauri/tauri.conf.json](../../../src-tauri/tauri.conf.json):

- enable `bundle.createUpdaterArtifacts`
- add `plugins.updater.pubkey`
- add `plugins.updater.endpoints`
- add macOS ad-hoc signing identity `-` for this phase

The endpoint should point at the GitHub Releases static JSON asset, not the DMG.

Expected shape:

```json
{
  "bundle": {
    "createUpdaterArtifacts": true,
    "macOS": {
      "signingIdentity": "-"
    }
  },
  "plugins": {
    "updater": {
      "pubkey": "the exact public key emitted by `tauri signer generate`",
      "endpoints": [
        "https://github.com/jmluang/xTerm/releases/latest/download/latest.json"
      ]
    }
  }
}
```

### Capabilities

Add `updater:default` and `process:default` to the main capability.

### `latest.json` manifest shape

The updater pipeline must publish a static `latest.json` asset to GitHub Releases using the official Tauri GitHub release flow. The app consumes that manifest as a release artifact and verifies its presence during release validation rather than defining a custom manifest-generation format in phase 1.

The manifest must resolve to updater artifacts, not DMGs. During release validation, confirm that:

- `latest.json` exists on the GitHub Release
- it contains macOS target entries for both `darwin-aarch64` and `darwin-x86_64`
- each entry points at the updater bundle for that target
- each entry references the matching updater signature

## Release Pipeline Design

The GitHub workflow must inject updater signing secrets during release builds:

- `TAURI_SIGNING_PRIVATE_KEY`, containing the exact one-line base64 private key file generated by `tauri signer generate`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if used

Release build requirements:

1. Sync app version from the pushed tag as it does now.
2. Build with updater artifact generation enabled.
3. Collect updater bundles and `.sig`.
4. Publish `latest.json` and target artifacts to the GitHub Release.

The current workflow already contains the invariants that must survive the migration:

- tag-triggered releases (`v*`)
- version synchronization across [package.json](../../../package.json), [src-tauri/tauri.conf.json](../../../src-tauri/tauri.conf.json), and [src-tauri/Cargo.toml](../../../src-tauri/Cargo.toml)
- separate macOS matrix entries for `aarch64-apple-darwin` and `x86_64-apple-darwin`
- Node/Rust setup before `tauri build`

Implementation path:

- Migrate the release workflow to `tauri-apps/tauri-action`, keeping the updater pipeline aligned with the official Tauri GitHub release flow.
- Keep the existing version-sync step before the action runs so tag-driven versioning still works.
- Preserve the current macOS Intel and Apple Silicon matrix targets in the migrated workflow.
- Treat `latest.json`, updater bundles, and `.sig` files as required outputs of the official updater release pipeline, and add workflow/release verification that those assets are present on the final GitHub Release.
- Keep matrix build jobs uploading into a draft release first, then finalize the release only after all macOS targets succeed. Do not expose a public release from a single matrix leg.
- Serialize same-tag runs with workflow concurrency and serialize same-run target uploads with matrix `max-parallel: 1`.
- Make reruns state-aware: published releases must no-op, draft releases may resume upload/finalize, and only missing releases may be created from scratch.

The workflow must continue to publish macOS Intel and Apple Silicon artifacts because updater resolution is target-specific.

## Data Flow

### Check for update

1. App starts, or a settings webview with updater actions starts.
2. Frontend hook automatically calls `check()`.
3. Tauri updater fetches `latest.json`.
4. If no update is available, UI enters `UpToDate`.
5. If an update is available, UI stores:
   - version
   - notes
   - publication date if present
6. User can click `Check for Updates` to retry the same check path manually.

### Install update

1. User clicks `Download and Install`.
2. Frontend hook calls `downloadAndInstall()`.
3. UI shows progress state when possible.
4. On success, frontend calls `relaunch()`.
5. The new app instance starts on the updated version.

## Error Handling

Handle these failure classes explicitly:

- updater not configured
- endpoint unavailable
- malformed `latest.json`
- signature mismatch
- no matching platform artifact
- download failure
- install failure
- relaunch failure after a successful install

User-facing copy should be short and actionable:

- `Unable to check for updates right now.`
- `This build does not have a compatible update package.`
- `Update package verification failed.`
- `The update was installed, but the app could not restart automatically.`

Do not surface raw stack traces in the primary About UI. Log detailed errors to the console for diagnostics.

## Security Requirements

- Never commit the updater private key.
- Public key may live in config or be injected at build time.
- GitHub workflow must read signing material from secrets only.
- The updater endpoint must stay HTTPS in production.

## Testing Strategy

### Local verification

- Build a tagged release locally with updater signing env vars present.
- Confirm updater artifacts are generated for macOS targets.
- Inspect generated `.sig` files.

### Workflow verification

- Trigger a release build from a test version tag.
- Confirm GitHub Release contains:
  - direct installer assets
  - updater bundles
  - `.sig`
  - `latest.json`

### App verification

- Install older build manually.
- Publish newer release.
- Open About and verify:
  - automatic check completes successfully
  - manual check can retry the same path
  - update metadata is shown
  - install path completes

### Failure-path verification

- Break endpoint URL and verify error state.
- Remove matching platform entry from test JSON and verify graceful fallback.
- Validate that unsigned or mismatched signatures are rejected.

## Rollout Notes

This phase is intentionally acceptable-but-not-perfect for macOS distribution. Users may still have to explicitly allow direct-download builds because ad-hoc signing is not notarization.

That trade-off is acceptable because the immediate goal is to establish a working updater pipeline and in-app update UX without requiring paid Apple infrastructure. Future work can replace ad-hoc signing with Developer ID + notarization without changing the app-side updater architecture.

Platform coverage: phase 1 enables in-app updater actions on macOS builds only. The release matrix only builds macOS `aarch64` and `x86_64` targets, so the shipped updater manifest is validated against macOS entries only. Windows/Linux builds keep the About surface but do not expose updater actions until their artifacts are added to the release matrix. A visible `no matching platform artifact` error on non-macOS builds is treated as a configuration regression, not expected behavior.

## Implementation Outline

1. Add updater dependencies and register the plugin.
2. Extend capabilities and config.
3. Build an updater controller hook.
4. Add About section UI to Settings.
5. Wire check/install interactions.
6. Rework release workflow to publish updater assets.
7. Validate end-to-end with a test release tag.

## Open Questions Resolved

- Do we need Sparkle first? No. Tauri official updater is the preferred fit.
- Do we need a paid Apple account for phase 1? No.
- Do we need updater signing anyway? Yes, absolutely.
