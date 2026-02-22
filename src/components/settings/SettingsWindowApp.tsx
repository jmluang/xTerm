import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emitTo } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { message } from "@tauri-apps/plugin-dialog";
import { getThemeMode, setThemeMode, type ThemeMode } from "@/lib/theme";
import { getTerminalThemeId, setTerminalThemeId, type TerminalThemeId } from "@/lib/terminalTheme";
import { getTerminalOptions, sanitizeTerminalOptions, setTerminalOptions, type TerminalOptionsState } from "@/lib/terminalOptions";
import {
  SETTINGS_HOSTS_RELOAD_EVENT,
  SETTINGS_METRICS_DOCK_EVENT,
  SETTINGS_NAVIGATE_EVENT,
  SETTINGS_TERMINAL_OPTIONS_EVENT,
  SETTINGS_TERMINAL_THEME_EVENT,
  SETTINGS_THEME_MODE_EVENT,
  type SettingsMetricsDockPayload,
  type SettingsHostsReloadPayload,
  type SettingsNavigatePayload,
  type SettingsTerminalOptionsPayload,
  type SettingsTerminalThemePayload,
  type SettingsThemeModePayload,
} from "@/lib/settingsEvents";
import { getMetricsDockEnabled, setMetricsDockEnabled } from "@/lib/metricsDock";
import { useWebdavSync } from "@/hooks/useWebdavSync";
import { SettingsPanel } from "@/components/settings/SettingsPanel";
import type { Host, SshConfigImportCandidate } from "@/types/models";
import type { SettingsSection } from "@/types/settings";

function readInitialSection(): SettingsSection {
  const section = new URLSearchParams(window.location.search).get("section");
  if (section === "terminal" || section === "sync" || section === "import") return section;
  return "terminal";
}

export function SettingsWindowApp() {
  const [isInTauri] = useState(() => {
    const w = window as any;
    return !!(w.__TAURI__ || w.__TAURI_INTERNALS__);
  });
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() => getThemeMode());
  const [terminalThemeId, setTerminalThemeIdState] = useState<TerminalThemeId>(() => getTerminalThemeId());
  const [terminalOptions, setTerminalOptionsState] = useState<TerminalOptionsState>(() => getTerminalOptions());
  const [metricsDockEnabled, setMetricsDockEnabledState] = useState<boolean>(() => getMetricsDockEnabled());
  const [settingsSection, setSettingsSection] = useState<SettingsSection>(() => readInitialSection());
  const [sshImportLoading, setSshImportLoading] = useState(false);
  const [sshImportBusy, setSshImportBusy] = useState(false);
  const [sshImportCandidates, setSshImportCandidates] = useState<SshConfigImportCandidate[]>([]);

  const hostsRef = useRef<Host[]>([]);
  async function loadHosts() {
    if (!isInTauri) return;
    try {
      const hosts = await invoke<Host[]>("hosts_load");
      hostsRef.current = hosts ?? [];
    } catch (error) {
      console.error("[hosts] load error in settings window", error);
    }
  }

  const webdav = useWebdavSync({
    isInTauri,
    hostsRef,
    loadHosts: async () => {
      await loadHosts();
      emitToMain<SettingsHostsReloadPayload>(SETTINGS_HOSTS_RELOAD_EVENT, { reason: "webdav-pull" });
    },
  });

  function emitToMain<T>(event: string, payload: T) {
    if (!isInTauri) return;
    void emitTo<T>("main", event, payload).catch((error) => {
      console.debug(`[settings] emit ${event} failed`, error);
    });
  }

  async function refreshSshImportCandidates() {
    if (!isInTauri) return;
    setSshImportLoading(true);
    try {
      const candidates = await invoke<SshConfigImportCandidate[]>("ssh_config_scan_importable_hosts");
      setSshImportCandidates(candidates ?? []);
    } catch (error) {
      setSshImportCandidates([]);
      await message(`Failed to scan ~/.ssh config.\n\n${String(error)}`, {
        title: "SSH Config Import",
        kind: "error",
      });
    } finally {
      setSshImportLoading(false);
    }
  }

  async function importSshConfigHosts(selectedAliases: string[]) {
    if (!isInTauri || selectedAliases.length === 0) return;
    setSshImportBusy(true);
    try {
      const selected = sshImportCandidates.filter((item) => selectedAliases.includes(item.alias));
      if (selected.length === 0) return;

      await loadHosts();
      const hosts = hostsRef.current ?? [];
      const now = new Date().toISOString();
      const aliveHosts = hosts.filter((h) => !h.deleted);
      const maxSortOrder = aliveHosts.reduce((max, host, index) => {
        const value = typeof host.sortOrder === "number" ? host.sortOrder : index;
        return Math.max(max, value);
      }, -1);

      const existingAlias = new Set(aliveHosts.map((h) => (h.alias || "").trim().toLowerCase()).filter(Boolean));
      const existingEndpoint = new Set(
        aliveHosts
          .map((h) => `${(h.user || "").trim().toLowerCase()}@${(h.hostname || "").trim().toLowerCase()}:${h.port || 22}`)
          .filter(Boolean)
      );

      let imported = 0;
      let skipped = 0;
      const append: Host[] = [];
      for (const [index, item] of selected.entries()) {
        const alias = (item.alias || "").trim();
        const hostname = (item.hostname || alias).trim();
        const user = (item.user || "").trim();
        const port = item.port || 22;
        const aliasKey = alias.toLowerCase();
        const endpointKey = `${user.toLowerCase()}@${hostname.toLowerCase()}:${port}`;

        if (!alias || !hostname) {
          skipped += 1;
          continue;
        }
        if (existingAlias.has(aliasKey) || existingEndpoint.has(endpointKey)) {
          skipped += 1;
          continue;
        }

        append.push({
          id: `${Date.now()}-${index}-${Math.random().toString(36).slice(2, 8)}`,
          name: hostname,
          alias,
          hostname,
          user,
          port,
          hasPassword: false,
          hostInsightsEnabled: true,
          hostLiveMetricsEnabled: true,
          identityFile: item.identityFile,
          proxyJump: item.proxyJump,
          envVars: "",
          encoding: "utf-8",
          sortOrder: maxSortOrder + imported + 1,
          tags: [],
          notes: `Imported from ${item.sourcePath}`,
          updatedAt: now,
          deleted: false,
        });
        existingAlias.add(aliasKey);
        existingEndpoint.add(endpointKey);
        imported += 1;
      }

      if (append.length === 0) {
        await message("No new hosts were imported. Selected hosts may already exist.", {
          title: "SSH Config Import",
          kind: "info",
        });
        return;
      }

      const nextHosts = [...hosts, ...append];
      await invoke("hosts_save", { hosts: nextHosts });
      hostsRef.current = nextHosts;
      emitToMain<SettingsHostsReloadPayload>(SETTINGS_HOSTS_RELOAD_EVENT, { reason: "manual" });
      await message(`Imported ${imported} host(s).${skipped > 0 ? ` Skipped ${skipped} duplicate(s).` : ""}`, {
        title: "SSH Config Import",
        kind: "info",
      });
      await refreshSshImportCandidates();
    } catch (error) {
      await message(`Failed to import hosts.\n\n${String(error)}`, {
        title: "SSH Config Import",
        kind: "error",
      });
    } finally {
      setSshImportBusy(false);
    }
  }

  useEffect(() => {
    setThemeMode(themeMode);
    emitToMain<SettingsThemeModePayload>(SETTINGS_THEME_MODE_EVENT, { mode: themeMode });
  }, [themeMode, isInTauri]);

  useEffect(() => {
    setTerminalThemeId(terminalThemeId);
    emitToMain<SettingsTerminalThemePayload>(SETTINGS_TERMINAL_THEME_EVENT, { themeId: terminalThemeId });
  }, [terminalThemeId, isInTauri]);

  useEffect(() => {
    const safe = sanitizeTerminalOptions(terminalOptions);
    setTerminalOptions(safe);
    emitToMain<SettingsTerminalOptionsPayload>(SETTINGS_TERMINAL_OPTIONS_EVENT, { options: safe });
  }, [terminalOptions, isInTauri]);

  useEffect(() => {
    setMetricsDockEnabled(metricsDockEnabled);
    emitToMain<SettingsMetricsDockPayload>(SETTINGS_METRICS_DOCK_EVENT, { enabled: metricsDockEnabled });
  }, [metricsDockEnabled, isInTauri]);

  useEffect(() => {
    if (!isInTauri) return;
    void loadHosts();
    void webdav.refreshSettingsFromBackend();

    let unlisten: (() => void) | null = null;
    (async () => {
      try {
        const current = getCurrentWebviewWindow();
        unlisten = await current.listen<SettingsNavigatePayload>(SETTINGS_NAVIGATE_EVENT, (event) => {
          const section = event.payload?.section;
          if (section === "terminal" || section === "sync" || section === "import") {
            setSettingsSection(section);
          }
        });
      } catch (error) {
        console.debug("[settings] listen navigate event failed", error);
      }
    })();

    return () => {
      try {
        unlisten?.();
      } catch {
        // ignore
      }
    };
  }, [isInTauri]);

  return (
    <div className="h-screen overflow-hidden" style={{ background: "var(--app-bg)" } as any}>
      <SettingsPanel
        open
        onOpenChange={(open) => {
          if (open) return;
          if (isInTauri) {
            void getCurrentWebviewWindow().close();
          }
        }}
        initialSection={settingsSection}
        themeMode={themeMode}
        setThemeMode={setThemeModeState}
        terminalThemeId={terminalThemeId}
        setTerminalThemeId={setTerminalThemeIdState}
        terminalOptions={terminalOptions}
        setTerminalOptions={setTerminalOptionsState}
        metricsDockEnabled={metricsDockEnabled}
        setMetricsDockEnabled={setMetricsDockEnabledState}
        settings={webdav.settings}
        setSettings={webdav.setSettings}
        localHostsDbPath={webdav.localHostsDbPath}
        syncBusy={webdav.syncBusy}
        syncNotice={webdav.syncNotice}
        isInTauri={isInTauri}
        onSaveSettings={webdav.saveWebdavSettings}
        onPull={webdav.doWebdavPull}
        onPush={webdav.doWebdavPush}
        sshImportBusy={sshImportBusy}
        sshImportLoading={sshImportLoading}
        sshImportCandidates={sshImportCandidates}
        onRefreshSshImport={refreshSshImportCandidates}
        onImportSshConfigSelected={importSshConfigHosts}
      />
    </div>
  );
}
