import { useEffect, useMemo, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { Channel, invoke } from "@tauri-apps/api/core";
import packageJson from "../../package.json";
import type { UpdaterStatus, UpdaterViewState } from "@/types/settings";

const APP_NAME = "xTermius";
const FALLBACK_VERSION = packageJson.version;

type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength?: number } }
  | { event: "Finished" };

type UpdateMetadata = {
  rid: number;
  version: string;
  body?: string | null;
};

type LoadedUpdate = {
  rid: number;
  version: string;
  releaseNotes: string | null;
};

function isTauriRuntime() {
  if (typeof window === "undefined") return false;
  const w = window as any;
  return !!(w.__TAURI__ || w.__TAURI_INTERNALS__);
}

function isMacPlatform() {
  if (typeof navigator === "undefined") return false;
  return /Mac/i.test(navigator.userAgent);
}

async function checkForUpdate() {
  const metadata = await invoke<UpdateMetadata | null>("plugin:updater|check");
  return metadata;
}

async function downloadAndInstallUpdate(
  update: LoadedUpdate,
  onEvent?: (event: DownloadEvent) => void
) {
  const channel = new Channel<DownloadEvent>();
  if (onEvent) {
    channel.onmessage = onEvent;
  }

  await invoke("plugin:updater|download_and_install", {
    onEvent: channel,
    rid: update.rid,
  });
}

async function relaunchApp() {
  await invoke("plugin:process|restart");
}

export function useUpdaterController(): UpdaterViewState {
  const [currentVersion, setCurrentVersion] = useState(FALLBACK_VERSION);
  const [status, setStatus] = useState<UpdaterStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [loadedUpdate, setLoadedUpdate] = useState<LoadedUpdate | null>(null);

  const enabled = isTauriRuntime() && isMacPlatform();

  useEffect(() => {
    let cancelled = false;

    (async () => {
      if (!isTauriRuntime()) {
        if (!cancelled) setCurrentVersion(FALLBACK_VERSION);
        return;
      }

      try {
        const version = await getVersion();
        if (!cancelled && version) {
          setCurrentVersion(version);
          return;
        }
      } catch (versionError) {
        console.debug("[updater] getVersion unavailable, falling back to package.json", versionError);
      }

      if (!cancelled) setCurrentVersion(FALLBACK_VERSION);
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  async function checkForUpdates() {
    if (!enabled) return;
    setStatus("checking");
    setError(null);

    try {
      const update = await checkForUpdate();
      if (!update) {
        setLoadedUpdate(null);
        setStatus("up-to-date");
        return;
      }

      setLoadedUpdate({
        rid: update.rid,
        version: update.version,
        releaseNotes: update.body?.trim() || null,
      });
      setStatus("available");
    } catch (checkError) {
      console.error("[updater] check failed", checkError);
      setLoadedUpdate(null);
      setStatus("error");
      setError("Unable to check for updates right now.");
    }
  }

  async function downloadAndInstall() {
    if (!enabled || !loadedUpdate) return;
    setError(null);
    setStatus("downloading");

    try {
      await downloadAndInstallUpdate(loadedUpdate, (event) => {
        if (event.event === "Finished") {
          setStatus("installing");
          return;
        }
        setStatus("downloading");
      });
    } catch (installError) {
      console.error("[updater] install failed", installError);
      setStatus("error");
      setError("Unable to install the update right now.");
      return;
    }

    setLoadedUpdate(null);

    try {
      await relaunchApp();
    } catch (relaunchError) {
      console.error("[updater] relaunch failed", relaunchError);
      setStatus("restart-required");
      setError("The update was installed, but the app could not restart automatically.");
    }
  }

  const statusText = useMemo(() => {
    switch (status) {
      case "checking":
        return "Checking for updates...";
      case "available":
        return loadedUpdate?.version ? `Update available: ${loadedUpdate.version}` : "Update available";
      case "up-to-date":
        return "You're up to date.";
      case "downloading":
        return "Downloading update...";
      case "installing":
        return "Installing update...";
      case "restart-required":
        return "Restart required to finish applying the update.";
      case "error":
        return error ?? "Unable to check for updates right now.";
      case "idle":
      default:
        return "Ready to check for updates.";
    }
  }, [error, loadedUpdate, status]);

  return useMemo(
    () => ({
      appName: APP_NAME,
      channel: "stable" as const,
      currentVersion,
      enabled,
      status,
      statusText,
      error,
      availableVersion: loadedUpdate?.version ?? null,
      releaseNotes: loadedUpdate?.releaseNotes ?? null,
      checkForUpdates,
      downloadAndInstall,
    }),
    [currentVersion, enabled, error, loadedUpdate, status, statusText]
  );
}
