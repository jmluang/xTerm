import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { HostEditorDialog } from "@/components/dialogs/HostEditorDialog";
import { SshConfigImportDialog } from "@/components/dialogs/SshConfigImportDialog";
import { HostsSidebar } from "@/components/layout/HostsSidebar";
import { SettingsPanel } from "@/components/settings/SettingsPanel";
import { SettingsWindowApp } from "@/components/settings/SettingsWindowApp";
import { MainPane } from "@/components/layout/MainPane";
import { ToastViewport } from "@/components/ui/ToastViewport";
import { useAppController } from "@/hooks/useAppController";

const SIDEBAR_WIDTH_KEY = "xtermius_hosts_sidebar_width";
const SIDEBAR_WIDTH_DEFAULT = 288;
const SIDEBAR_WIDTH_MIN = 220;

function clampSidebarWidth(width: number) {
  const max = Math.max(SIDEBAR_WIDTH_MIN, Math.floor(window.innerWidth * (2 / 3)));
  return Math.max(SIDEBAR_WIDTH_MIN, Math.min(max, Math.round(width)));
}

function App() {
  const panel = new URLSearchParams(window.location.search).get("panel");
  if (panel === "settings") return <SettingsWindowApp />;

  const ctrl = useAppController();
  const [sidebarWidth, setSidebarWidth] = useState(() => {
    const raw = Number(localStorage.getItem(SIDEBAR_WIDTH_KEY) || SIDEBAR_WIDTH_DEFAULT);
    return Number.isFinite(raw) ? Math.max(SIDEBAR_WIDTH_MIN, Math.round(raw)) : SIDEBAR_WIDTH_DEFAULT;
  });
  const draggingSidebarRef = useRef(false);

  useEffect(() => {
    if (!ctrl.sidebarOpen) return;
    const onResize = () => setSidebarWidth((prev) => clampSidebarWidth(prev));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [ctrl.sidebarOpen]);

  useEffect(() => {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, String(sidebarWidth));
  }, [sidebarWidth]);

  useEffect(() => {
    const onPointerMove = (event: PointerEvent) => {
      if (!draggingSidebarRef.current) return;
      setSidebarWidth(clampSidebarWidth(event.clientX));
    };
    const onPointerUp = () => {
      if (!draggingSidebarRef.current) return;
      draggingSidebarRef.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", onPointerUp);
    return () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerUp);
    };
  }, []);

  useEffect(() => {
    const w = window as any;
    const isTauri = !!(w.__TAURI__ || w.__TAURI_INTERNALS__);
    const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);
    if (!isTauri || !isMac) return;

    let disposed = false;
    const htmlEl = document.documentElement;
    const prevBodyBg = document.body.style.backgroundColor;
    const rootEl = document.getElementById("root");
    const prevRootBg = rootEl?.style.backgroundColor ?? "";
    const prevHtmlBg = htmlEl.style.backgroundColor;

    (async () => {
      try {
        await getCurrentWindow().setBackgroundColor([0, 0, 0, 0]);
        if (disposed) return;
        htmlEl.style.backgroundColor = "transparent";
        document.body.style.backgroundColor = "transparent";
        if (rootEl) rootEl.style.backgroundColor = "transparent";
        document.documentElement.dataset.nativeVibrancy = "1";
      } catch (error) {
        console.debug("[window] transparent window setup failed", error);
      }
    })();

    return () => {
      disposed = true;
      htmlEl.style.backgroundColor = prevHtmlBg;
      document.body.style.backgroundColor = prevBodyBg;
      if (rootEl) rootEl.style.backgroundColor = prevRootBg;
      delete document.documentElement.dataset.nativeVibrancy;
    };
  }, []);

  useEffect(() => {
    const w = window as any;
    const isTauri = !!(w.__TAURI__ || w.__TAURI_INTERNALS__);
    const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);
    if (!isTauri || !isMac) return;

    void getCurrentWindow()
      .setTheme(ctrl.themeMode === "system" ? null : ctrl.themeMode)
      .catch((error) => {
        console.debug("[window] setTheme failed", error);
      });
  }, [ctrl.themeMode]);

  return (
    <div className="h-screen text-foreground overflow-hidden" style={{ background: "transparent" } as any}>
      <div
        className="grid h-full min-h-0 min-w-0 relative"
        style={
          ctrl.sidebarOpen
            ? ({ gridTemplateColumns: `${clampSidebarWidth(sidebarWidth)}px minmax(0,1fr)` } as any)
            : ({ gridTemplateColumns: "minmax(0,1fr)" } as any)
        }
      >
        {ctrl.sidebarOpen ? (
          <HostsSidebar
            hostListRef={ctrl.hostListRef}
            hostListScrollable={ctrl.hostListScrollable}
            hostSearch={ctrl.hostSearch}
            setHostSearch={ctrl.setHostSearch}
            sortedHosts={ctrl.sortedHosts}
            reorderMode={ctrl.reorderMode}
            setReorderMode={ctrl.setReorderMode}
            setActiveDragHostId={ctrl.setActiveDragHostId}
            hosts={ctrl.hosts}
            persistHostOrder={ctrl.persistHostOrder}
            connectingHosts={ctrl.connectingHosts}
            hostStaticById={ctrl.hostStaticById}
            refreshingHostIds={ctrl.refreshingHostIds}
            refreshHostStatic={ctrl.refreshHostStatic}
            openEditDialog={ctrl.openEditDialog}
            deleteHost={ctrl.deleteHost}
            connectToHost={ctrl.connectToHost}
            openAddDialog={ctrl.openAddDialog}
            openSshImportDialog={ctrl.openSshImportDialog}
            sshImportLoading={ctrl.sshImportLoading}
            isInTauri={ctrl.isInTauri}
            setSidebarOpen={ctrl.setSidebarOpen}
          />
        ) : null}
        {ctrl.sidebarOpen ? (
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize hosts sidebar"
            className="cursor-col-resize absolute top-0 bottom-0 z-20"
            style={{ left: `${clampSidebarWidth(sidebarWidth) - 2}px`, width: "4px" } as any}
            onPointerDown={(event) => {
              event.preventDefault();
              draggingSidebarRef.current = true;
              document.body.style.cursor = "col-resize";
              document.body.style.userSelect = "none";
            }}
          />
        ) : null}
        <MainPane
          sidebarOpen={ctrl.sidebarOpen}
          setSidebarOpen={ctrl.setSidebarOpen}
          sessions={ctrl.sessions}
          activeSessionId={ctrl.activeSessionId}
          setActiveSessionId={ctrl.setActiveSessionId}
          sessionIndexById={ctrl.sessionIndexById}
          closeSession={ctrl.closeSession}
          onOpenSyncSettings={() => {
            void ctrl.openSettings("sync");
          }}
          onOpenSettings={() => {
            void ctrl.openSettings("terminal");
          }}
          openAddDialog={ctrl.openAddDialog}
          terminalContainerRef={ctrl.terminalContainerRef}
          terminalRef={ctrl.terminalRef}
          hasSession={ctrl.sessions.length > 0}
          onTerminalMouseDown={() => {
            if (ctrl.activeSessionId) ctrl.terminalInstance.current?.focus();
          }}
          hostHintText={ctrl.sidebarOpen ? "Or click a host from the sidebar" : "Show Hosts from the toolbar"}
          liveHost={ctrl.liveHost}
          liveInfo={ctrl.liveInfo}
          liveError={ctrl.liveError}
          liveLoading={ctrl.liveLoading}
          liveUpdatedAt={ctrl.liveUpdatedAt}
          liveHistory={ctrl.liveHistory}
          metricsDockEnabled={ctrl.metricsDockEnabled}
          themeMode={ctrl.themeMode}
        >
          <HostEditorDialog
            open={ctrl.showDialog}
            onClose={() => ctrl.setShowDialog(false)}
            editingHost={ctrl.editingHost}
            formData={ctrl.formData}
            metricsDockEnabled={ctrl.metricsDockEnabled}
            onOpenHostMetricsDockSettings={() => {
              void ctrl.openSettings("terminal", "host-metrics-dock");
            }}
            setFormData={ctrl.setFormData}
            selectIdentityFile={ctrl.selectIdentityFile}
            onSave={ctrl.handleSave}
          />
          <SshConfigImportDialog
            open={ctrl.showSshImportDialog}
            loading={ctrl.sshImportLoading}
            candidates={ctrl.sshImportCandidates}
            onClose={() => ctrl.setShowSshImportDialog(false)}
            onImport={ctrl.importSshConfigHosts}
          />
        </MainPane>
      </div>

      {!ctrl.isInTauri ? (
        <SettingsPanel
          open={ctrl.showSettings}
          onOpenChange={ctrl.setShowSettings}
          initialSection={ctrl.settingsSection}
          scrollTarget={ctrl.settingsScrollTarget}
          themeMode={ctrl.themeMode}
          setThemeMode={ctrl.setThemeModeState}
          terminalThemeId={ctrl.terminalThemeId}
          setTerminalThemeId={ctrl.setTerminalThemeIdState}
          terminalOptions={ctrl.terminalOptions}
          setTerminalOptions={ctrl.setTerminalOptionsState}
          metricsDockEnabled={ctrl.metricsDockEnabled}
          setMetricsDockEnabled={ctrl.setMetricsDockEnabledState}
          settings={ctrl.settings}
          setSettings={ctrl.setSettings}
          localHostsDbPath={ctrl.localHostsDbPath}
          syncBusy={ctrl.syncBusy}
          syncNotice={ctrl.syncNotice}
          isInTauri={ctrl.isInTauri}
          onSaveSettings={ctrl.saveWebdavSettings}
          onPull={ctrl.doWebdavPull}
          onPush={ctrl.doWebdavPush}
          sshImportBusy={ctrl.sshImportLoading}
        />
      ) : null}
      <ToastViewport />
    </div>
  );
}

export default App;
