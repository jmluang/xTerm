import { useEffect, useState } from "react";
import { listen, emitTo } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { getThemeMode, setThemeMode, type ThemeMode } from "@/lib/theme";
import { TERMINAL_THEME_OPTIONS, getTerminalThemeId, setTerminalThemeId, type TerminalThemeId } from "@/lib/terminalTheme";
import {
  getTerminalOptions,
  sanitizeTerminalOptions,
  setTerminalOptions,
  type TerminalOptionsState,
} from "@/lib/terminalOptions";
import {
  SETTINGS_HOSTS_RELOAD_EVENT,
  SETTINGS_NAVIGATE_EVENT,
  SETTINGS_METRICS_DOCK_EVENT,
  SETTINGS_TERMINAL_OPTIONS_EVENT,
  SETTINGS_TERMINAL_THEME_EVENT,
  SETTINGS_THEME_MODE_EVENT,
  type SettingsMetricsDockPayload,
  type SettingsNavigatePayload,
  type SettingsHostsReloadPayload,
  type SettingsTerminalOptionsPayload,
  type SettingsTerminalThemePayload,
  type SettingsThemeModePayload,
} from "@/lib/settingsEvents";
import { getMetricsDockEnabled, setMetricsDockEnabled } from "@/lib/metricsDock";
import { useHostsManager } from "@/hooks/useHostsManager";
import { useTerminalSessions } from "@/hooks/useTerminalSessions";
import { useWebdavSync } from "@/hooks/useWebdavSync";
import { useHostInsights } from "@/hooks/useHostInsights";
import type { SettingsSection } from "@/types/settings";

const TERMINAL_THEME_IDS = new Set<string>(TERMINAL_THEME_OPTIONS.map((option) => option.id));

export function useAppController() {
  const [isInTauri] = useState(() => {
    const w = window as any;
    return !!(w.__TAURI__ || w.__TAURI_INTERNALS__);
  });
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() => getThemeMode());
  const [terminalThemeId, setTerminalThemeIdState] = useState<TerminalThemeId>(() => getTerminalThemeId());
  const [terminalOptions, setTerminalOptionsState] = useState<TerminalOptionsState>(() => getTerminalOptions());
  const [metricsDockEnabled, setMetricsDockEnabledState] = useState<boolean>(() => getMetricsDockEnabled());
  const [sidebarOpen, setSidebarOpen] = useState(() => localStorage.getItem("xtermius_sidebar_open") !== "0");
  const [, setActiveDragHostId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("terminal");

  const hostsMgr = useHostsManager({ isInTauri, sidebarOpen });
  const terminal = useTerminalSessions({
    isInTauri,
    hosts: hostsMgr.hosts,
    sidebarOpen,
    themeMode,
    terminalThemeId,
    terminalOptions,
  });
  const webdav = useWebdavSync({
    isInTauri,
    hostsRef: hostsMgr.hostsRef,
    loadHosts: hostsMgr.loadHosts,
  });
  const hostInsights = useHostInsights({
    isInTauri,
    hosts: hostsMgr.hosts,
    sessions: terminal.sessions,
    activeSessionId: terminal.activeSessionId,
  });

  useEffect(() => {
    void hostsMgr.loadHosts();
  }, [isInTauri]);

  useEffect(() => {
    setThemeMode(themeMode);
  }, [themeMode]);

  useEffect(() => {
    setTerminalThemeId(terminalThemeId);
  }, [terminalThemeId]);

  useEffect(() => {
    setTerminalOptions(terminalOptions);
  }, [terminalOptions]);

  useEffect(() => {
    setMetricsDockEnabled(metricsDockEnabled);
  }, [metricsDockEnabled]);

  useEffect(() => {
    localStorage.setItem("xtermius_sidebar_open", sidebarOpen ? "1" : "0");
  }, [sidebarOpen]);

  useEffect(() => {
    if (!showSettings) return;
    if (!isInTauri) return;
    void webdav.refreshSettingsFromBackend();
  }, [showSettings, isInTauri]);

  useEffect(() => {
    if (!isInTauri) return;
    let unlistenThemeMode: (() => void) | null = null;
    let unlistenTerminalTheme: (() => void) | null = null;
    let unlistenTerminalOptions: (() => void) | null = null;
    let unlistenMetricsDock: (() => void) | null = null;
    let unlistenHostsReload: (() => void) | null = null;

    (async () => {
      try {
        unlistenThemeMode = await listen<SettingsThemeModePayload>(SETTINGS_THEME_MODE_EVENT, (event) => {
          const mode = event.payload?.mode;
          if (mode === "system" || mode === "light" || mode === "dark") {
            setThemeModeState(mode);
          }
        });

        unlistenTerminalTheme = await listen<SettingsTerminalThemePayload>(SETTINGS_TERMINAL_THEME_EVENT, (event) => {
          const themeId = event.payload?.themeId;
          if (typeof themeId === "string" && TERMINAL_THEME_IDS.has(themeId)) {
            setTerminalThemeIdState(themeId as TerminalThemeId);
          }
        });

        unlistenTerminalOptions = await listen<SettingsTerminalOptionsPayload>(SETTINGS_TERMINAL_OPTIONS_EVENT, (event) => {
          const options = event.payload?.options;
          if (!options) return;
          setTerminalOptionsState(sanitizeTerminalOptions(options));
        });

        unlistenMetricsDock = await listen<SettingsMetricsDockPayload>(SETTINGS_METRICS_DOCK_EVENT, (event) => {
          const enabled = event.payload?.enabled;
          if (typeof enabled === "boolean") setMetricsDockEnabledState(enabled);
        });

        unlistenHostsReload = await listen<SettingsHostsReloadPayload>(SETTINGS_HOSTS_RELOAD_EVENT, () => {
          void hostsMgr.loadHosts();
        });

      } catch (error) {
        console.debug("[settings-window] event listeners unavailable", error);
      }
    })();

    return () => {
      try {
        unlistenThemeMode?.();
      } catch {
        // ignore
      }
      try {
        unlistenTerminalTheme?.();
      } catch {
        // ignore
      }
      try {
        unlistenTerminalOptions?.();
      } catch {
        // ignore
      }
      try {
        unlistenMetricsDock?.();
      } catch {
        // ignore
      }
      try {
        unlistenHostsReload?.();
      } catch {
        // ignore
      }
    };
  }, [isInTauri]);

  async function openSettings(section: SettingsSection = "terminal") {
    setSettingsSection(section);
    if (!isInTauri) {
      setShowSettings(true);
      return;
    }
    try {
      const existing = await WebviewWindow.getByLabel("settings");
      if (existing) {
        await existing.show();
        await existing.setFocus();
        await emitTo<SettingsNavigatePayload>("settings", SETTINGS_NAVIGATE_EVENT, { section });
        return;
      }

      const url = `/?panel=settings&section=${section}`;
      const settingsWindow = new WebviewWindow("settings", {
        title: "Settings",
        url,
        width: 1000,
        height: 760,
        minWidth: 860,
        minHeight: 620,
        center: true,
        resizable: true,
        decorations: true,
        hiddenTitle: true,
        titleBarStyle: "overlay",
        trafficLightPosition: new LogicalPosition(12, 18),
        focus: true,
      });

      settingsWindow.once("tauri://created", () => {
        void emitTo<SettingsNavigatePayload>("settings", SETTINGS_NAVIGATE_EVENT, { section });
      });
      settingsWindow.once("tauri://error", (error) => {
        console.error("[settings-window] failed to create window", error);
      });
    } catch (error) {
      console.error("[settings-window] open failed", error);
    }
  }

  return {
    hosts: hostsMgr.hosts,
    sessions: terminal.sessions,
    activeSessionId: terminal.activeSessionId,
    setActiveSessionId: terminal.setActiveSessionId,
    connectingHosts: terminal.connectingHosts,
    hostSearch: hostsMgr.hostSearch,
    setHostSearch: hostsMgr.setHostSearch,
    hostListRef: hostsMgr.hostListRef,
    hostListScrollable: hostsMgr.hostListScrollable,
    reorderMode: hostsMgr.reorderMode,
    setReorderMode: hostsMgr.setReorderMode,
    setActiveDragHostId,
    showDialog: hostsMgr.showDialog,
    setShowDialog: hostsMgr.setShowDialog,
    showSettings,
    setShowSettings,
    settingsSection,
    setSettingsSection,
    openSettings,
    settings: webdav.settings,
    setSettings: webdav.setSettings,
    syncBusy: webdav.syncBusy,
    syncNotice: webdav.syncNotice,
    localHostsDbPath: webdav.localHostsDbPath,
    editingHost: hostsMgr.editingHost,
    formData: hostsMgr.formData,
    setFormData: hostsMgr.setFormData,
    isInTauri,
    terminalContainerRef: terminal.terminalContainerRef,
    terminalRef: terminal.terminalRef,
    terminalInstance: terminal.terminalInstance,
    themeMode,
    setThemeModeState,
    terminalThemeId,
    setTerminalThemeIdState,
    terminalOptions,
    setTerminalOptionsState,
    metricsDockEnabled,
    setMetricsDockEnabledState,
    sidebarOpen,
    setSidebarOpen,
    sortedHosts: hostsMgr.sortedHosts,
    sessionIndexById: terminal.sessionIndexById,
    persistHostOrder: hostsMgr.persistHostOrder,
    openEditDialog: hostsMgr.openEditDialog,
    deleteHost: hostsMgr.deleteHost,
    connectToHost: terminal.connectToHost,
    openAddDialog: hostsMgr.openAddDialog,
    openSshImportDialog: () => openSettings("import"),
    importSshConfigHosts: hostsMgr.importSshConfigHosts,
    showSshImportDialog: hostsMgr.showSshImportDialog,
    setShowSshImportDialog: hostsMgr.setShowSshImportDialog,
    sshImportCandidates: hostsMgr.sshImportCandidates,
    sshImportLoading: hostsMgr.sshImportLoading,
    hostStaticById: hostInsights.hostStaticById,
    refreshingHostIds: hostInsights.refreshingHostIds,
    refreshHostStatic: hostInsights.refreshHostStatic,
    liveHost: hostInsights.liveHost,
    liveInfo: hostInsights.liveInfo,
    liveError: hostInsights.liveError,
    liveLoading: hostInsights.liveLoading,
    liveUpdatedAt: hostInsights.liveUpdatedAt,
    liveHistory: hostInsights.liveHistory,
    selectIdentityFile: hostsMgr.selectIdentityFile,
    handleSave: hostsMgr.handleSave,
    closeSession: terminal.closeSession,
    saveWebdavSettings: webdav.saveWebdavSettings,
    doWebdavPull: webdav.doWebdavPull,
    doWebdavPush: webdav.doWebdavPush,
  };
}
