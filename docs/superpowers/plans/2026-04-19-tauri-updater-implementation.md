# Tauri Updater Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a macOS-only in-app updater flow under Settings -> About, backed by Tauri's official updater and GitHub Releases.

**Architecture:** Keep updater state and platform gating in a dedicated React hook that feeds the shared settings surface used by both the main window and the standalone settings window. Wire Tauri's updater and process plugins on the Rust side, enable the required capabilities/config, and migrate the release workflow to `tauri-apps/tauri-action` so GitHub Releases publishes signed updater artifacts plus `latest.json`.

**Tech Stack:** React 18, TypeScript, Tauri 2.2, Rust, GitHub Actions, `@tauri-apps/plugin-updater`, `@tauri-apps/plugin-process`

---

## File Map

- Modify: `src-tauri/src/app.rs`
  - register updater/process plugins without disturbing the existing vibrancy setup.
- Modify: `src-tauri/Cargo.toml`
  - add desktop updater support and process restart support.
- Modify: `src-tauri/tauri.conf.json`
  - enable updater artifacts, ad-hoc macOS signing, updater endpoints, and the real public key.
- Modify: `src-tauri/capabilities/default.json`
  - expose `process:default` and `updater:default`.
- Modify: `package.json` / `package-lock.json`
  - add updater and process guest bindings.
- Create: `src/hooks/useUpdaterController.ts`
  - own updater status, version lookup, macOS gating, and install/relaunch behavior.
- Modify: `src/types/settings.ts`
  - add the `about` section union member.
- Modify: `src/hooks/useAppController.ts`
  - pass updater state into the shared settings surface outside Tauri.
- Modify: `src/components/settings/SettingsWindowApp.tsx`
  - allow `about` in section parsing/listeners and pass updater state through.
- Modify: `src/components/settings/SettingsPanel.tsx`
  - render the About navigation item and macOS-only updater UI.
- Modify: `.github/workflows/release.yml`
  - switch release publication to `tauri-apps/tauri-action`.

### Task 1: Configure Native Updater and Restart Plugins

**Files:**
- Modify: `src-tauri/src/app.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/capabilities/default.json`
- Modify: `package.json`
- Modify: `package-lock.json`
- Test: `cargo test --manifest-path src-tauri/Cargo.toml`

- [ ] **Step 1: Write the failing native registration**

Add the updater/process plugin calls to [src-tauri/src/app.rs](/Users/luang/Downloads/xTermius/xtermius/src-tauri/src/app.rs) before adding the crates so the Rust build proves the missing dependency surface first:

```rust
pub fn run() {
    tauri::Builder::default()
        .manage(PtyState::default())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            #[cfg(target_os = "macos")]
            {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

                if let Some(window) = app.get_webview_window("main") {
                    let _ = apply_vibrancy(
                        &window,
                        NSVisualEffectMaterial::Sidebar,
                        Some(NSVisualEffectState::Active),
                        None,
                    );
                }
            }

            Ok(())
        })
```

- [ ] **Step 2: Run the Rust smoke test to verify it fails**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: FAIL with unresolved `tauri_plugin_process` / `tauri_plugin_updater` symbols.

- [ ] **Step 3: Add the native crates, JS packages, updater config, and capabilities**

Update [src-tauri/Cargo.toml](/Users/luang/Downloads/xTermius/xtermius/src-tauri/Cargo.toml) to add the restart plugin globally and the updater plugin for desktop targets:

```toml
[dependencies]
tauri = { version = "2", features = ["macos-private-api"] }
tauri-plugin-shell = "2"
tauri-plugin-dialog = "2.6.0"
tauri-plugin-process = "2"

[target.'cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))'.dependencies]
tauri-plugin-updater = "2"
```

Patch [src-tauri/tauri.conf.json](/Users/luang/Downloads/xTermius/xtermius/src-tauri/tauri.conf.json) so Tauri produces updater artifacts and reads the generated public key:

```bash
npm run tauri signer generate -- -w ~/.tauri/xtermius-updater.key
export XT_UPDATER_PUBKEY_FILE="$HOME/.tauri/xtermius-updater.key.pub"
node -e "const fs=require('fs'); const p='src-tauri/tauri.conf.json'; const j=JSON.parse(fs.readFileSync(p,'utf8')); const pubkey=fs.readFileSync(process.env.XT_UPDATER_PUBKEY_FILE,'utf8'); j.bundle={...(j.bundle||{}), active:true, targets:'all', createUpdaterArtifacts:true, macOS:{signingIdentity:'-'}}; j.plugins={...(j.plugins||{}), shell:{open:true}, updater:{pubkey, endpoints:['https://github.com/jmluang/xTerm/releases/latest/download/latest.json']}}; fs.writeFileSync(p, JSON.stringify(j, null, 2)+'\n');"
```

Update [src-tauri/capabilities/default.json](/Users/luang/Downloads/xTermius/xtermius/src-tauri/capabilities/default.json):

```json
{
  "permissions": [
    "core:default",
    "core:webview:allow-create-webview-window",
    "core:window:allow-close",
    "core:window:allow-show",
    "core:window:allow-set-focus",
    "core:window:allow-set-effects",
    "core:window:allow-set-background-color",
    "core:window:allow-set-theme",
    "core:window:allow-start-dragging",
    "shell:allow-open",
    "dialog:default",
    "dialog:allow-open",
    "process:default",
    "updater:default"
  ]
}
```

Install the frontend guest bindings so the TypeScript implementation can use them:

```bash
npm install @tauri-apps/plugin-process @tauri-apps/plugin-updater
```

- [ ] **Step 4: Run the Rust smoke test again**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Commit the native updater wiring**

```bash
git add package.json package-lock.json src-tauri/Cargo.toml src-tauri/src/app.rs src-tauri/tauri.conf.json src-tauri/capabilities/default.json
git commit -m "feat: configure tauri updater runtime"
```

### Task 2: Add the Shared Updater Controller and About Settings Section

**Files:**
- Create: `src/hooks/useUpdaterController.ts`
- Modify: `src/types/settings.ts`
- Modify: `src/hooks/useAppController.ts`
- Modify: `src/components/settings/SettingsWindowApp.tsx`
- Modify: `src/components/settings/SettingsPanel.tsx`
- Test: `npm run build`

- [ ] **Step 1: Write the failing UI contract**

Extend [src/types/settings.ts](/Users/luang/Downloads/xTermius/xtermius/src/types/settings.ts) and [src/components/settings/SettingsPanel.tsx](/Users/luang/Downloads/xTermius/xtermius/src/components/settings/SettingsPanel.tsx) so the shared settings surface requires About/updater data before any caller is updated:

```ts
export type SettingsSection = "terminal" | "sync" | "import" | "about";
```

```ts
type UpdaterPanelState = {
  currentVersion: string | null;
  releaseChannel: "stable";
  supportsUpdaterActions: boolean;
  statusLabel: string;
  errorMessage: string | null;
  updateVersion: string | null;
  updateNotes: string | null;
  isChecking: boolean;
  isInstalling: boolean;
  onCheckForUpdates: () => Promise<void>;
  onDownloadAndInstall: () => Promise<void>;
};
```

Add `updater: UpdaterPanelState` to the existing `SettingsPanel` prop signature, then extend the settings navigation guards in [src/hooks/useAppController.ts](/Users/luang/Downloads/xTermius/xtermius/src/hooks/useAppController.ts) and [src/components/settings/SettingsWindowApp.tsx](/Users/luang/Downloads/xTermius/xtermius/src/components/settings/SettingsWindowApp.tsx) so `about` is accepted anywhere `terminal | sync | import` is currently hard-coded.

- [ ] **Step 2: Run the frontend build to verify it fails**

Run:

```bash
npm run build
```

Expected: FAIL because `SettingsPanel` callers do not yet provide the new `updater` prop and no updater hook exists.

- [ ] **Step 3: Implement the updater hook and render the About section**

Create [src/hooks/useUpdaterController.ts](/Users/luang/Downloads/xTermius/xtermius/src/hooks/useUpdaterController.ts) with shared state, macOS gating, updater calls, restart handling, and concise status/error strings:

```ts
import { useEffect, useMemo, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import packageJson from "../../package.json";

type UpdaterStatus =
  | "idle"
  | "checking"
  | "up-to-date"
  | "update-available"
  | "downloading"
  | "installing"
  | "restart-required"
  | "error";

export function useUpdaterController({ isInTauri }: { isInTauri: boolean }) {
  const supportsUpdaterActions =
    isInTauri && typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);
  const [currentVersion, setCurrentVersion] = useState<string>(packageJson.version);
  const [status, setStatus] = useState<UpdaterStatus>("idle");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);

  useEffect(() => {
    if (!isInTauri) return;
    void getVersion().then(setCurrentVersion).catch(() => setCurrentVersion(packageJson.version));
  }, [isInTauri]);

  async function onCheckForUpdates() {
    if (!supportsUpdaterActions) return;
    setStatus("checking");
    setErrorMessage(null);
    try {
      const update = await check();
      if (!update) {
        setPendingUpdate(null);
        setStatus("up-to-date");
        return;
      }
      setPendingUpdate(update);
      setStatus("update-available");
    } catch (error) {
      setPendingUpdate(null);
      setStatus("error");
      setErrorMessage("Unable to check for updates right now.");
      console.error("[updater] check failed", error);
    }
  }

  async function onDownloadAndInstall() {
    if (!pendingUpdate || !supportsUpdaterActions) return;
    setStatus("downloading");
    setErrorMessage(null);
    try {
      await pendingUpdate.downloadAndInstall((event) => {
        if (event.event === "Started" || event.event === "Progress") setStatus("downloading");
        if (event.event === "Finished") setStatus("installing");
      });
    } catch (error) {
      setStatus("error");
      setErrorMessage("Unable to install the update right now.");
      console.error("[updater] download/install failed", error);
      return;
    }

    try {
      await relaunch();
    } catch (error) {
      setStatus("restart-required");
      setErrorMessage("The update was installed, but the app could not restart automatically.");
      console.error("[updater] relaunch failed", error);
    }
  }

  return useMemo(
    () => ({
      currentVersion,
      releaseChannel: "stable" as const,
      supportsUpdaterActions,
      statusLabel:
        status === "idle"
          ? "Ready to check for updates"
          : status === "checking"
            ? "Checking for updates..."
            : status === "up-to-date"
              ? "Up to date"
              : status === "update-available"
                ? `Update ${pendingUpdate?.version ?? ""} is available`
                : status === "downloading"
                  ? "Downloading update..."
                  : status === "installing"
                    ? "Installing update..."
                    : status === "restart-required"
                      ? "Restart required"
                      : errorMessage ?? "Unable to check for updates right now.",
      errorMessage,
      updateVersion: pendingUpdate?.version ?? null,
      updateNotes: pendingUpdate?.body ?? null,
      isChecking: status === "checking",
      isInstalling: status === "downloading" || status === "installing",
      onCheckForUpdates,
      onDownloadAndInstall,
    }),
    [currentVersion, errorMessage, pendingUpdate, status, supportsUpdaterActions]
  );
}
```

Finish the hook by branching the catch-path copy for signature mismatch and missing platform artifacts so the final UI strings match the design spec instead of collapsing every failure into one generic message.

Wire the hook into both settings entrypoints:

```ts
// src/hooks/useAppController.ts
const updater = useUpdaterController({ isInTauri });

return {
  updater,
};
```

```ts
// src/components/settings/SettingsWindowApp.tsx
const updater = useUpdaterController({ isInTauri });
```

Pass the new hook result into each existing `SettingsPanel` render:

```tsx
updater={updater}
```

Render the About nav item and panel in [src/components/settings/SettingsPanel.tsx](/Users/luang/Downloads/xTermius/xtermius/src/components/settings/SettingsPanel.tsx):

```tsx
{[
  { id: "terminal", label: "Terminal" },
  { id: "sync", label: "Sync" },
  { id: "import", label: "Import SSH Config" },
  { id: "about", label: "About" },
].map((section) => (
```

```tsx
{activeSection === "about" ? (
  <div className="mx-auto max-w-4xl rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
    <div>
      <div className="text-lg font-semibold">About xTermius</div>
      <div className="text-xs text-muted-foreground mt-1">Desktop app metadata and software updates.</div>
    </div>
    <div className="grid gap-2 text-sm">
      <div>Version: <span className="font-medium">{updater.currentVersion ?? "Unknown"}</span></div>
      <div>Channel: <span className="font-medium">{updater.releaseChannel}</span></div>
      <div>Status: <span className="font-medium">{updater.statusLabel}</span></div>
    </div>
    {updater.errorMessage ? (
      <div className="text-sm text-destructive">{updater.errorMessage}</div>
    ) : null}
    {updater.supportsUpdaterActions ? (
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="outline" disabled={updater.isChecking || updater.isInstalling} onClick={() => void updater.onCheckForUpdates()}>
          {updater.isChecking ? "Checking..." : "Check for Updates"}
        </Button>
        <Button
          variant="default"
          disabled={!updater.updateVersion || updater.isChecking || updater.isInstalling}
          onClick={() => void updater.onDownloadAndInstall()}
        >
          {updater.isInstalling ? "Installing..." : "Download and Install"}
        </Button>
      </div>
    ) : null}
    {updater.updateVersion ? (
      <div className="rounded-lg border border-border bg-muted/25 px-3 py-2 text-sm">
        <div>New version: <span className="font-medium">{updater.updateVersion}</span></div>
        {updater.updateNotes ? <div className="mt-2 whitespace-pre-wrap text-muted-foreground">{updater.updateNotes}</div> : null}
      </div>
    ) : null}
  </div>
) : null}
```

- [ ] **Step 4: Run the frontend build again**

Run:

```bash
npm run build
```

Expected: PASS.

- [ ] **Step 5: Commit the About/updater UI**

```bash
git add src/types/settings.ts src/hooks/useUpdaterController.ts src/hooks/useAppController.ts src/components/settings/SettingsWindowApp.tsx src/components/settings/SettingsPanel.tsx
git commit -m "feat: add settings about updater flow"
```

### Task 3: Migrate the Release Workflow to `tauri-action`

**Files:**
- Modify: `.github/workflows/release.yml`
- Test: `rg -n "tauri-apps/tauri-action@action-v0.6.2|TAURI_SIGNING_PRIVATE_KEY|includeUpdaterJson|Sync app versions from tag|aarch64-apple-darwin|x86_64-apple-darwin" .github/workflows/release.yml`

- [ ] **Step 1: Write the failing workflow assertion**

Confirm the current workflow is still missing the official updater release path:

```bash
rg -n "tauri-apps/tauri-action@action-v0.6.2|TAURI_SIGNING_PRIVATE_KEY|includeUpdaterJson" .github/workflows/release.yml
```

Expected: FAIL with no matches.

- [ ] **Step 2: Replace the two-job DMG upload flow with a single updater-aware publish job**

Rewrite [`.github/workflows/release.yml`](/Users/luang/Downloads/xTermius/xtermius/.github/workflows/release.yml) around `tauri-apps/tauri-action@action-v0.6.2`, while preserving the current tag trigger, version-sync step, and dual macOS targets:

```yaml
name: Release

on:
  push:
    tags:
      - "v*"
  workflow_dispatch:

jobs:
  publish-tauri:
    permissions:
      contents: write
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: macos-14
            target: aarch64-apple-darwin
          - os: macos-15-intel
            target: x86_64-apple-darwin
    runs-on: ${{ matrix.os }}

    steps:
      - name: Checkout
        uses: actions/checkout@v4
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
          fetch-depth: 0

      - name: Derive release version from tag
        if: startsWith(github.ref, 'refs/tags/')
        run: |
          TAG="${GITHUB_REF_NAME}"
          VERSION="${TAG#v}"
          echo "RELEASE_TAG=${TAG}" >> "$GITHUB_ENV"
          echo "RELEASE_VERSION=${VERSION}" >> "$GITHUB_ENV"

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: "20"
          cache: npm

      - name: Sync app versions from tag
        if: env.RELEASE_VERSION != ''
        run: |
          node -e "const fs=require('fs'); const p='package.json'; const j=JSON.parse(fs.readFileSync(p,'utf8')); j.version=process.env.RELEASE_VERSION; fs.writeFileSync(p, JSON.stringify(j, null, 2)+'\n');"
          node -e "const fs=require('fs'); const p='src-tauri/tauri.conf.json'; const j=JSON.parse(fs.readFileSync(p,'utf8')); j.version=process.env.RELEASE_VERSION; fs.writeFileSync(p, JSON.stringify(j, null, 2)+'\n');"
          node -e 'const fs=require("fs"); const p="src-tauri/Cargo.toml"; let s=fs.readFileSync(p,"utf8"); s=s.replace(/(^version\\s*=\\s*")[^"]+("\\s*$)/m, "$1"+process.env.RELEASE_VERSION+"$2"); fs.writeFileSync(p, s);'

      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install frontend deps
        run: npm ci

      - uses: tauri-apps/tauri-action@action-v0.6.2
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
        with:
          tagName: ${{ env.RELEASE_TAG }}
          releaseName: "xTermius ${{ env.RELEASE_TAG }}"
          releaseDraft: true
          prerelease: false
          generateReleaseNotes: true
          tauriScript: npm run tauri
          args: --target ${{ matrix.target }}
          includeUpdaterJson: true

  finalize_release:
    needs: build
    if: startsWith(github.ref, 'refs/tags/')
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - name: Publish draft release
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          gh release edit "${GITHUB_REF_NAME}" --draft=false
```

This removes the old `upload-artifact` + `ncipollo/release-action` path entirely. `tauri-action` still owns build/upload publication, but the release only becomes public after the finalizer job confirms both macOS targets succeeded.

- [ ] **Step 3: Run the workflow structure assertion again**

Run:

```bash
rg -n "tauri-apps/tauri-action@action-v0.6.2|TAURI_SIGNING_PRIVATE_KEY|includeUpdaterJson|Sync app versions from tag|aarch64-apple-darwin|x86_64-apple-darwin" .github/workflows/release.yml
```

Expected: PASS with hits for the action, signing secrets, updater JSON inclusion, version-sync step, and both macOS targets.

- [ ] **Step 4: Commit the release migration**

```bash
git add .github/workflows/release.yml
git commit -m "ci: publish tauri updater releases"
```

### Task 4: Verify the Signed Artifacts and In-App Update Flow

**Files:**
- Verify only: working tree changes from Tasks 1-3

- [ ] **Step 1: Run the local build checks**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
```

Expected: both PASS.

- [ ] **Step 2: Build a signed macOS bundle locally**

Run:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat "$HOME/.tauri/xtermius-updater.key")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$HOME/.tauri/xtermius-updater.key.password")"
npm run tauri build -- --target aarch64-apple-darwin
```

Expected: PASS, with updater artifacts emitted alongside the normal bundle.

- [ ] **Step 3: Confirm the local artifacts exist**

Run:

```bash
rg --files src-tauri/target/aarch64-apple-darwin/release/bundle | rg '(\.app\.tar\.gz|\.sig|\.dmg)$'
```

Expected: PASS with at least one `.dmg`, one `.app.tar.gz`, and one matching `.sig`.

- [ ] **Step 4: Trigger a disposable GitHub release and inspect the assets**

Run:

```bash
git tag v0.1.1-updater-test
git push origin v0.1.1-updater-test
```

Expected on GitHub Actions / Releases:

```text
- a release for v0.1.1-updater-test exists
- latest.json is attached
- both darwin targets publish updater bundles
- each updater bundle has a matching .sig
- both DMGs are still attached for direct download
```

- [ ] **Step 5: Exercise the About flow on macOS and clean up the disposable tag**

Manual validation:

```text
1. Install an older macOS build.
2. Publish or keep the newer test release from Step 4.
3. Open Settings -> About.
4. Click "Check for Updates" and confirm the status changes to "Update 0.1.1-updater-test is available".
5. Click "Download and Install" and confirm the app relaunches into the new version.
```

Cleanup:

```bash
git push origin --delete v0.1.1-updater-test
git tag -d v0.1.1-updater-test
```
