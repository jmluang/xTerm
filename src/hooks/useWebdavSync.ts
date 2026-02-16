import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { configDir } from "@tauri-apps/api/path";
import { confirm, message } from "@tauri-apps/plugin-dialog";
import type { RefObject } from "react";
import type { Host, Settings } from "@/types/models";

export function useWebdavSync(params: {
  isInTauri: boolean;
  hostsRef: RefObject<Host[]>;
  loadHosts: () => Promise<void>;
}) {
  const { isInTauri, hostsRef, loadHosts } = params;
  const [settings, setSettings] = useState<Settings>({});
  const [syncBusy, setSyncBusy] = useState<null | "pull" | "push" | "save">(null);
  const [syncNotice, setSyncNotice] = useState<null | { kind: "ok" | "err"; text: string }>(null);
  const [localHostsDbPath, setLocalHostsDbPath] = useState<string>("");

  async function refreshSettingsFromBackend() {
    if (!isInTauri) return;
    try {
      const s = await invoke<Settings>("settings_load");
      setSettings(s ?? {});
      const cd = await configDir().catch(() => "");
      if (cd) setLocalHostsDbPath(`${cd}/xtermius/hosts.db`);
    } catch (e) {
      console.error("[settings] load error", e);
    }
  }

  async function doWebdavPull() {
    if (!isInTauri) return;
    setSyncBusy("pull");
    setSyncNotice(null);
    try {
      await invoke("webdav_pull");
      await loadHosts();
      setSyncNotice({ kind: "ok", text: "Pulled" });
      await message("Pulled from WebDAV.", { title: "WebDAV", kind: "info" });
    } catch (e) {
      const msg = `WebDAV pull failed.\n\n${String(e)}`;
      setSyncNotice({ kind: "err", text: "Pull failed" });
      try {
        await message(msg, { title: "WebDAV", kind: "error" });
      } catch {
        // Ignore.
      }
    } finally {
      setSyncBusy(null);
    }
  }

  async function doWebdavPush() {
    if (!isInTauri) return;
    const hostList = hostsRef.current ?? [];
    const aliveCount = hostList.filter((h) => !h.deleted).length;
    if (aliveCount === 0) {
      const ok = await confirm(
        "No hosts found.\n\nPushing now will overwrite the remote hosts.db with an empty database.\n\nContinue?",
        { title: "WebDAV Push", kind: "warning" }
      );
      if (!ok) return;
    }

    setSyncBusy("push");
    setSyncNotice(null);
    try {
      await invoke("settings_save", { settings });
      await invoke("hosts_save", { hosts: hostList });
      await invoke("webdav_push");
      setSyncNotice({ kind: "ok", text: "Pushed" });
      await message("Pushed to WebDAV.", { title: "WebDAV", kind: "info" });
    } catch (e) {
      const msg = `WebDAV push failed.\n\n${String(e)}`;
      setSyncNotice({ kind: "err", text: "Push failed" });
      try {
        await message(msg, { title: "WebDAV", kind: "error" });
      } catch {
        // Ignore.
      }
    } finally {
      setSyncBusy(null);
    }
  }

  async function saveWebdavSettings() {
    if (!isInTauri) return;
    setSyncBusy("save");
    setSyncNotice(null);
    try {
      await invoke("settings_save", { settings });
      setSyncNotice({ kind: "ok", text: "Saved" });
    } catch (e) {
      const msg = `Failed to save settings.\n\n${String(e)}`;
      setSyncNotice({ kind: "err", text: "Save failed" });
      try {
        await message(msg, { title: "WebDAV", kind: "error" });
      } catch {
        // Ignore.
      }
    } finally {
      setSyncBusy(null);
    }
  }

  return {
    settings,
    setSettings,
    syncBusy,
    syncNotice,
    localHostsDbPath,
    refreshSettingsFromBackend,
    doWebdavPull,
    doWebdavPush,
    saveWebdavSettings,
  };
}
