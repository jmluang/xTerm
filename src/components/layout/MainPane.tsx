import type { Dispatch, ReactNode, RefObject, SetStateAction } from "react";
import { Cloud, PanelLeftOpen, Plus, Settings2 } from "lucide-react";
import type { Session } from "@/types/models";

export function MainPane(props: {
  sidebarOpen: boolean;
  setSidebarOpen: Dispatch<SetStateAction<boolean>>;
  sessions: Session[];
  activeSessionId: string | null;
  setActiveSessionId: Dispatch<SetStateAction<string | null>>;
  sessionIndexById: Map<string, number>;
  closeSession: (sessionId: string, reason?: "user" | "timeout" | "unknown") => Promise<void>;
  onOpenSyncSettings: () => void;
  onOpenSettings: () => void;
  openAddDialog: () => void;
  terminalContainerRef: RefObject<HTMLDivElement>;
  terminalRef: RefObject<HTMLDivElement>;
  hasSession: boolean;
  onTerminalMouseDown: () => void;
  hostHintText: string;
  children?: ReactNode;
}) {
  const {
    sidebarOpen,
    setSidebarOpen,
    sessions,
    activeSessionId,
    setActiveSessionId,
    sessionIndexById,
    closeSession,
    onOpenSyncSettings,
    onOpenSettings,
    openAddDialog,
    terminalContainerRef,
    terminalRef,
    hasSession,
    onTerminalMouseDown,
    hostHintText,
    children,
  } = props;

  return (
    <div className="min-w-0 min-h-0 flex flex-col" style={{ background: "var(--app-bg)" } as any}>
      <div
        data-tauri-drag-region
        className={[
          "h-[44px] pt-[4px] pb-0 flex items-center gap-2 pr-3 select-none",
          sidebarOpen ? "pl-3" : "pl-[88px]",
        ].join(" ")}
        style={{ WebkitAppRegion: "drag" } as any}
      >
        {!sidebarOpen ? (
          <button
            type="button"
            title="Show Hosts"
            aria-label="Show Hosts"
            onClick={() => setSidebarOpen(true)}
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
            data-tauri-drag-region="false"
            style={{ WebkitAppRegion: "no-drag" } as any}
          >
            <PanelLeftOpen size={18} />
          </button>
        ) : null}

        <div className="min-w-0 flex items-center gap-2 flex-1">
          {sessions.length > 0 ? (
            <div className="flex items-center gap-1 overflow-x-auto h-full max-w-[min(720px,60vw)]">
              {sessions.map((session) => {
                const active = session.id === activeSessionId;
                const exited = session.status === "exited";
                const idx = sessionIndexById.get(session.id) ?? 1;
                return (
                  <div
                    key={session.id}
                    className={[
                      "group shrink-0 inline-flex items-center gap-2 h-7 px-2 rounded-lg cursor-pointer",
                      exited
                        ? "bg-[var(--app-chip-failed)] hover:bg-[var(--app-chip-failed-hover)] text-[var(--app-chip-failed-text)] ring-1 ring-red-500/10"
                        : active
                          ? "bg-[var(--app-chip-active)] text-foreground"
                          : "bg-[var(--app-chip)] hover:bg-[var(--app-chip-hover)] text-foreground",
                    ].join(" ")}
                    onClick={() => setActiveSessionId(session.id)}
                    title={
                      exited
                        ? `${session.hostAlias} #${idx} (exited${typeof session.exitCode === "number" ? `, code ${session.exitCode}` : ""})`
                        : `${session.hostAlias} #${idx}`
                    }
                    aria-label={`Switch to ${session.hostAlias}`}
                    data-tauri-drag-region
                    style={{ WebkitAppRegion: "drag" } as any}
                  >
                    {exited ? <span className="h-2 w-2 rounded-full bg-red-500/70" aria-hidden="true" /> : null}
                    {idx > 1 ? <span className="text-[11px] font-semibold opacity-70">#{idx}</span> : null}
                    <span className="text-sm font-semibold leading-none whitespace-nowrap">{session.hostAlias}</span>
                    <button
                      type="button"
                      className="h-6 w-6 rounded-md text-muted-foreground hover:text-foreground hover:bg-background/60 inline-flex items-center justify-center"
                      onClick={(e) => {
                        e.stopPropagation();
                        void closeSession(session.id, "user");
                      }}
                      aria-label="Close session"
                      title="Close"
                      data-tauri-drag-region="false"
                      style={{ WebkitAppRegion: "no-drag" } as any}
                    >
                      x
                    </button>
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="h-full flex items-center px-2 text-sm font-semibold leading-none text-muted-foreground/80">
              xTermius
            </div>
          )}

          <div className="flex-1" />
        </div>

        <div
          className="ml-auto flex items-center gap-1"
          data-tauri-drag-region="false"
          style={{ WebkitAppRegion: "no-drag" } as any}
        >
          <button
            type="button"
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
            onClick={onOpenSyncSettings}
            title="WebDAV Sync"
            aria-label="WebDAV Sync"
          >
            <Cloud size={18} />
          </button>
          <button
            type="button"
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
            onClick={onOpenSettings}
            title="Settings"
            aria-label="Settings"
          >
            <Settings2 size={18} />
          </button>
          <button
            type="button"
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
            onClick={openAddDialog}
            title="Add Host"
            aria-label="Add Host"
          >
            <Plus size={18} />
          </button>
        </div>
      </div>

      <main className="flex-1 min-h-0 min-w-0 flex flex-col overflow-hidden">
        <div className="flex-1 min-h-0 pt-1 px-3 pb-3 relative overflow-hidden">
          <div
            ref={terminalContainerRef}
            className="h-full w-full min-h-0 rounded-xl p-2 overflow-hidden"
            data-has-session={hasSession ? "1" : "0"}
            style={{ background: "var(--app-term-bg)" } as any}
            onMouseDown={onTerminalMouseDown}
          >
            <div
              ref={terminalRef}
              className="h-full w-full"
              style={!hasSession ? ({ visibility: "hidden", pointerEvents: "none" } as any) : undefined}
            />
          </div>
          {!hasSession ? (
            <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
              <div className="text-center text-slate-300">
                <p className="text-lg mb-2">Select a host to connect</p>
                <p className="text-sm opacity-80">{hostHintText}</p>
              </div>
            </div>
          ) : null}

          {children}
        </div>
      </main>
    </div>
  );
}
