import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emitTo } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { getThemeMode, setThemeMode, type ThemeMode } from "@/lib/theme";
import { getTerminalThemeId, setTerminalThemeId, type TerminalThemeId } from "@/lib/terminalTheme";
import { getTerminalOptions, sanitizeTerminalOptions, setTerminalOptions, type TerminalOptionsState } from "@/lib/terminalOptions";
import {
  SETTINGS_NAVIGATE_EVENT,
  SETTINGS_TERMINAL_OPTIONS_EVENT,
  SETTINGS_TERMINAL_THEME_EVENT,
  SETTINGS_THEME_MODE_EVENT,
  type SettingsNavigatePayload,
  type SettingsTerminalOptionsPayload,
  type SettingsTerminalThemePayload,
  type SettingsThemeModePayload,
} from "@/lib/settingsEvents";
import { useWebdavSync } from "@/hooks/useWebdavSync";
import { SettingsPanel } from "@/components/settings/SettingsPanel";
import type { Host } from "@/types/models";
import type { SettingsSection } from "@/types/settings";

function readInitialSection(): SettingsSection {
  const section = new URLSearchParams(window.location.search).get("section");
  if (section === "terminal" || section === "appearance" || section === "sync") return section;
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
  const [settingsSection, setSettingsSection] = useState<SettingsSection>(() => readInitialSection());

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
    loadHosts,
  });

  function emitToMain<T>(event: string, payload: T) {
    if (!isInTauri) return;
    void emitTo<T>("main", event, payload).catch((error) => {
      console.debug(`[settings] emit ${event} failed`, error);
    });
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
    if (!isInTauri) return;
    void loadHosts();
    void webdav.refreshSettingsFromBackend();

    let unlisten: (() => void) | null = null;
    (async () => {
      try {
        const current = getCurrentWebviewWindow();
        unlisten = await current.listen<SettingsNavigatePayload>(SETTINGS_NAVIGATE_EVENT, (event) => {
          const section = event.payload?.section;
          if (section === "terminal" || section === "appearance" || section === "sync") {
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
        settings={webdav.settings}
        setSettings={webdav.setSettings}
        localHostsDbPath={webdav.localHostsDbPath}
        syncBusy={webdav.syncBusy}
        syncNotice={webdav.syncNotice}
        isInTauri={isInTauri}
        onSaveSettings={webdav.saveWebdavSettings}
        onPull={webdav.doWebdavPull}
        onPush={webdav.doWebdavPush}
      />
    </div>
  );
}
