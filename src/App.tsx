import "@xterm/xterm/css/xterm.css";
import { HostEditorDialog } from "@/components/dialogs/HostEditorDialog";
import { SshConfigImportDialog } from "@/components/dialogs/SshConfigImportDialog";
import { HostsSidebar } from "@/components/layout/HostsSidebar";
import { SettingsPanel } from "@/components/settings/SettingsPanel";
import { SettingsWindowApp } from "@/components/settings/SettingsWindowApp";
import { MainPane } from "@/components/layout/MainPane";
import { useAppController } from "@/hooks/useAppController";

function App() {
  const panel = new URLSearchParams(window.location.search).get("panel");
  if (panel === "settings") return <SettingsWindowApp />;

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
            openSshImportDialog={ctrl.openSshImportDialog}
            sshImportLoading={ctrl.sshImportLoading}
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
          themeMode={ctrl.themeMode}
          setThemeMode={ctrl.setThemeModeState}
          terminalThemeId={ctrl.terminalThemeId}
          setTerminalThemeId={ctrl.setTerminalThemeIdState}
          terminalOptions={ctrl.terminalOptions}
          setTerminalOptions={ctrl.setTerminalOptionsState}
          settings={ctrl.settings}
          setSettings={ctrl.setSettings}
          localHostsDbPath={ctrl.localHostsDbPath}
          syncBusy={ctrl.syncBusy}
          syncNotice={ctrl.syncNotice}
          isInTauri={ctrl.isInTauri}
          onSaveSettings={ctrl.saveWebdavSettings}
          onPull={ctrl.doWebdavPull}
          onPush={ctrl.doWebdavPush}
        />
      ) : null}
    </div>
  );
}

export default App;
