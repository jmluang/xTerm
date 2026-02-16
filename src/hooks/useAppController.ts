import { useEffect, useState } from "react";
import { getThemeMode, setThemeMode, type ThemeMode } from "@/lib/theme";
import { useHostsManager } from "@/hooks/useHostsManager";
import { useTerminalSessions } from "@/hooks/useTerminalSessions";
import { useWebdavSync } from "@/hooks/useWebdavSync";

export function useAppController() {
  const [isInTauri] = useState(() => {
    const w = window as any;
    return !!(w.__TAURI__ || w.__TAURI_INTERNALS__);
  });
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() => getThemeMode());
  const [sidebarOpen, setSidebarOpen] = useState(() => localStorage.getItem("xtermius_sidebar_open") !== "0");
  const [, setActiveDragHostId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showWebdavSync, setShowWebdavSync] = useState(false);

  const hostsMgr = useHostsManager({ isInTauri, sidebarOpen });
  const terminal = useTerminalSessions({
    isInTauri,
    hosts: hostsMgr.hosts,
    sidebarOpen,
    themeMode,
  });
  const webdav = useWebdavSync({
    isInTauri,
    hostsRef: hostsMgr.hostsRef,
    loadHosts: hostsMgr.loadHosts,
  });

  useEffect(() => {
    void hostsMgr.loadHosts();
  }, [isInTauri]);

  useEffect(() => {
    setThemeMode(themeMode);
  }, [themeMode]);

  useEffect(() => {
    localStorage.setItem("xtermius_sidebar_open", sidebarOpen ? "1" : "0");
  }, [sidebarOpen]);

  useEffect(() => {
    if (!showSettings) return;
    if (!isInTauri) return;
    void webdav.refreshSettingsFromBackend();
  }, [showSettings, isInTauri]);

  useEffect(() => {
    if (!showWebdavSync) return;
    if (!isInTauri) return;
    void webdav.refreshSettingsFromBackend();
  }, [showWebdavSync, isInTauri]);

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
    showWebdavSync,
    setShowWebdavSync,
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
    sidebarOpen,
    setSidebarOpen,
    sortedHosts: hostsMgr.sortedHosts,
    sessionIndexById: terminal.sessionIndexById,
    persistHostOrder: hostsMgr.persistHostOrder,
    openEditDialog: hostsMgr.openEditDialog,
    deleteHost: hostsMgr.deleteHost,
    connectToHost: terminal.connectToHost,
    openAddDialog: hostsMgr.openAddDialog,
    selectIdentityFile: hostsMgr.selectIdentityFile,
    handleSave: hostsMgr.handleSave,
    closeSession: terminal.closeSession,
    saveWebdavSettings: webdav.saveWebdavSettings,
    doWebdavPull: webdav.doWebdavPull,
    doWebdavPush: webdav.doWebdavPush,
  };
}
