import "@xterm/xterm/css/xterm.css";
import { HostEditorDialog } from "@/components/dialogs/HostEditorDialog";
import { SettingsDialog } from "@/components/dialogs/SettingsDialog";
import { WebdavSyncDialog } from "@/components/dialogs/WebdavSyncDialog";
import { HostsSidebar } from "@/components/layout/HostsSidebar";
import { MainPane } from "@/components/layout/MainPane";
import { useAppController } from "@/hooks/useAppController";

function App() {
  const ctrl = useAppController();

  return (
    <div className="h-screen text-foreground overflow-hidden" style={{ background: "var(--app-bg)" } as any}>
      <div className={["grid h-full min-h-0 min-w-0", ctrl.sidebarOpen ? "grid-cols-[288px_1fr]" : "grid-cols-1"].join(" ")}>
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
            openEditDialog={ctrl.openEditDialog}
            deleteHost={ctrl.deleteHost}
            connectToHost={ctrl.connectToHost}
            openAddDialog={ctrl.openAddDialog}
            isInTauri={ctrl.isInTauri}
            setSidebarOpen={ctrl.setSidebarOpen}
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
          setShowWebdavSync={ctrl.setShowWebdavSync}
          setShowSettings={ctrl.setShowSettings}
          openAddDialog={ctrl.openAddDialog}
          terminalContainerRef={ctrl.terminalContainerRef}
          terminalRef={ctrl.terminalRef}
          hasSession={ctrl.sessions.length > 0}
          onTerminalMouseDown={() => {
            if (ctrl.activeSessionId) ctrl.terminalInstance.current?.focus();
          }}
          hostHintText={ctrl.sidebarOpen ? "Or click a host from the sidebar" : "Show Hosts from the toolbar"}
        >
          <HostEditorDialog
            open={ctrl.showDialog}
            onClose={() => ctrl.setShowDialog(false)}
            editingHost={ctrl.editingHost}
            formData={ctrl.formData}
            setFormData={ctrl.setFormData}
            isInTauri={ctrl.isInTauri}
            selectIdentityFile={ctrl.selectIdentityFile}
            onSave={ctrl.handleSave}
          />
        </MainPane>
      </div>

      <SettingsDialog
        open={ctrl.showSettings}
        onOpenChange={ctrl.setShowSettings}
        themeMode={ctrl.themeMode}
        setThemeMode={ctrl.setThemeModeState}
        onOpenWebdav={() => {
          ctrl.setShowSettings(false);
          ctrl.setShowWebdavSync(true);
        }}
      />

      <WebdavSyncDialog
        open={ctrl.showWebdavSync}
        onOpenChange={ctrl.setShowWebdavSync}
        settings={ctrl.settings}
        setSettings={ctrl.setSettings}
        localHostsDbPath={ctrl.localHostsDbPath}
        syncBusy={ctrl.syncBusy}
        syncNotice={ctrl.syncNotice}
        isInTauri={ctrl.isInTauri}
        onOpenSettings={() => ctrl.setShowSettings(true)}
        onSaveSettings={ctrl.saveWebdavSettings}
        onPull={ctrl.doWebdavPull}
        onPush={ctrl.doWebdavPush}
      />
    </div>
  );
}

export default App;
