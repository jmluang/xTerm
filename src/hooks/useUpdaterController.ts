import { useEffect, useMemo, useState } from "react";
import packageJson from "../../package.json";
import type { UpdaterStatus, UpdaterViewState } from "@/types/settings";

const APP_NAME = "xTermius";
const FALLBACK_VERSION = packageJson.version;

type UpdaterModule = {
  check: () => Promise<PendingUpdate | null>;
};

type ProcessModule = {
  relaunch: () => Promise<void>;
};

type PendingUpdate = {
  version: string;
  body?: string | null;
  downloadAndInstall: (onEvent?: (event: DownloadEvent) => void) => Promise<void>;
};

type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength?: number } }
  | { event: "Finished" };

type LoadedUpdate = {
  version: string;
  releaseNotes: string | null;
  downloadAndInstall: (onEvent?: (event: DownloadEvent) => void) => Promise<void>;
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

async function loadUpdaterModule() {
  return Function('return import("@tauri-apps/plugin-updater")')() as Promise<UpdaterModule>;
}

async function loadProcessModule() {
  return Function('return import("@tauri-apps/plugin-process")')() as Promise<ProcessModule>;
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
        const { getVersion } = await import("@tauri-apps/api/app");
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
      const { check } = await loadUpdaterModule();
      const update = await check();
      if (!update) {
        setLoadedUpdate(null);
        setStatus("up-to-date");
        return;
      }

      setLoadedUpdate({
        version: update.version,
        releaseNotes: update.body?.trim() || null,
        downloadAndInstall: update.downloadAndInstall.bind(update),
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
      await loadedUpdate.downloadAndInstall((event) => {
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
      const { relaunch } = await loadProcessModule();
      await relaunch();
    } catch (relaunchError) {
      console.error("[updater] relaunch failed", relaunchError);
      setStatus("restart-required");
      setError("The update was installed, but the app could not restart automatically.");
    }
  }

  return useMemo(
    () => ({
      appName: APP_NAME,
      channel: "stable" as const,
      currentVersion,
      enabled,
      status,
      error,
      availableVersion: loadedUpdate?.version ?? null,
      releaseNotes: loadedUpdate?.releaseNotes ?? null,
      checkForUpdates,
      downloadAndInstall,
    }),
    [currentVersion, enabled, error, loadedUpdate, status]
  );
}
