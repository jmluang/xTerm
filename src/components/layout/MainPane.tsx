import { useEffect, useState, type Dispatch, type ReactNode, type RefObject, type SetStateAction } from "react";
import { Cloud, PanelLeftOpen, Plus, Settings2 } from "lucide-react";
import type { Host, HostLiveInfo, Session } from "@/types/models";

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
  liveHost: Host | null;
  liveInfo: HostLiveInfo | null;
  liveError: string | null;
  liveLoading: boolean;
  liveUpdatedAt: number | null;
  liveHistory: { cpu: number[]; mem: number[]; load: number[] };
  metricsDockEnabled: boolean;
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
    liveInfo,
    liveError,
    liveLoading,
    liveUpdatedAt,
    liveHistory,
    metricsDockEnabled,
    children,
  } = props;

  const [metricsMode, setMetricsMode] = useState<"minimal" | "full">("minimal");

  useEffect(() => {
    setMetricsMode("minimal");
  }, [activeSessionId]);

  useEffect(() => {
    if (!metricsDockEnabled) {
      setMetricsMode("minimal");
    }
  }, [metricsDockEnabled]);

  function resolvedAppearance(): "light" | "dark" {
    const datasetTheme = document.documentElement.dataset.theme;
    if (datasetTheme === "light" || datasetTheme === "dark") return datasetTheme;
    return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }

  const appearance = resolvedAppearance();
  const chartPalette =
    appearance === "dark"
      ? {
          cpu: "rgb(34 211 238)",
          mem: "rgb(52 211 153)",
          load: "rgb(251 191 36)",
          track: "rgba(255,255,255,0.12)",
          ringTrack: "rgba(255,255,255,0.12)",
        }
      : {
          cpu: "rgb(8 145 178)",
          mem: "rgb(5 150 105)",
          load: "rgb(202 138 4)",
          track: "rgba(15,23,42,0.14)",
          ringTrack: "rgba(15,23,42,0.12)",
        };

  function formatMem(kb?: number) {
    if (!kb || kb <= 0) return "--";
    const gb = kb / 1024 / 1024;
    if (gb >= 1) return `${gb.toFixed(1)}G`;
    const mb = kb / 1024;
    return `${mb.toFixed(0)}M`;
  }

  function formatPercent(v?: number) {
    if (typeof v !== "number" || Number.isNaN(v)) return "--";
    return `${v.toFixed(1)}%`;
  }

  function formatLoad(v?: number) {
    if (typeof v !== "number" || Number.isNaN(v)) return "--";
    return v.toFixed(1);
  }

  function formatUptime(seconds?: number) {
    if (typeof seconds !== "number" || Number.isNaN(seconds) || seconds <= 0) return "--";
    const d = Math.floor(seconds / 86400);
    if (d > 0) return `${d}D`;
    const h = Math.floor((seconds % 86400) / 3600);
    if (h > 0) return `${h}H`;
    const m = Math.floor((seconds % 3600) / 60);
    return `${m}M`;
  }

  function memUsedPercent() {
    if (!liveInfo?.memTotalKb || !liveInfo?.memUsedKb || liveInfo.memTotalKb <= 0) return 0;
    return Math.max(0, Math.min(100, (liveInfo.memUsedKb / liveInfo.memTotalKb) * 100));
  }

  function memCachePercent() {
    if (!liveInfo?.memTotalKb || !liveInfo?.memPageCacheKb || liveInfo.memTotalKb <= 0) return 0;
    return Math.max(0, Math.min(100, (liveInfo.memPageCacheKb / liveInfo.memTotalKb) * 100));
  }

  function memFreePercent() {
    if (!liveInfo?.memTotalKb || !liveInfo?.memFreeKb || liveInfo.memTotalKb <= 0) return 0;
    return Math.max(0, Math.min(100, (liveInfo.memFreeKb / liveInfo.memTotalKb) * 100));
  }

  function diskUsedPercent() {
    if (!liveInfo?.diskRootTotalKb || !liveInfo?.diskRootUsedKb || liveInfo.diskRootTotalKb <= 0) return 0;
    return Math.max(0, Math.min(100, (liveInfo.diskRootUsedKb / liveInfo.diskRootTotalKb) * 100));
  }

  function renderCpuGrid(value?: number) {
    const safe = typeof value === "number" && !Number.isNaN(value) ? Math.max(0, Math.min(100, value)) : 0;
    const active = Math.round((safe / 100) * 40);
    const cells = Array.from({ length: 40 }, (_, i) => i < active);
    return (
      <div className="grid grid-cols-20 gap-[3px] mt-2">
        {cells.map((on, idx) => (
          <span
            key={`${idx}-${on ? 1 : 0}`}
            className={["h-[7px] rounded-[3px]", on ? "bg-emerald-400/90" : "bg-white/10"].join(" ")}
          />
        ))}
      </div>
    );
  }

  function renderLoadRadar(load1?: number, cores?: number) {
    const c = Math.max(1, cores || 1);
    const ratio = typeof load1 === "number" ? Math.max(0, Math.min(1.5, load1 / c)) : 0;
    const deg = Math.round(ratio * 270);
    return (
      <div className="relative h-9 w-9">
        <div className="absolute inset-0 rounded-full border border-white/10" />
        <div className="absolute inset-[4px] rounded-full border border-white/10" />
        <div className="absolute inset-[8px] rounded-full border border-white/10" />
        <div
          className="absolute inset-0 rounded-full"
          style={{ background: `conic-gradient(${chartPalette.mem} 0 ${deg}deg, ${chartPalette.track} ${deg}deg 360deg)` }}
        />
        <div className="absolute inset-[10px] rounded-full" style={{ background: "var(--app-term-bg)" } as any} />
      </div>
    );
  }

  function renderCombinedTrend() {
    const maxLen = Math.max(liveHistory.cpu.length, liveHistory.mem.length, liveHistory.load.length);
    if (maxLen < 2) return <div className="h-14 rounded-md bg-muted/25" />;
    const width = 100;
    const height = 48;
    const px = (idx: number) => (idx / (maxLen - 1)) * (width - 1);
    const py = (v: number) => height - 4 - (Math.max(0, Math.min(100, v)) / 100) * (height - 8);
    const points = (series: number[]) => series.map((v, idx) => `${px(idx)},${py(v)}`).join(" ");
    return (
      <div className="rounded-md bg-muted/25 p-2">
        <div className="mb-1 flex items-center gap-3 text-[10px] text-muted-foreground">
          <span><i className="inline-block h-2 w-2 rounded-full mr-1" style={{ background: chartPalette.cpu }} />CPU</span>
          <span><i className="inline-block h-2 w-2 rounded-full mr-1" style={{ background: chartPalette.mem }} />MEM</span>
          <span><i className="inline-block h-2 w-2 rounded-full mr-1" style={{ background: chartPalette.load }} />LOAD</span>
        </div>
        <svg viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" className="block h-14 w-full">
          <polyline points={points(liveHistory.cpu)} fill="none" stroke={chartPalette.cpu} strokeWidth="2" />
          <polyline points={points(liveHistory.mem)} fill="none" stroke={chartPalette.mem} strokeWidth="2" />
          <polyline points={points(liveHistory.load)} fill="none" stroke={chartPalette.load} strokeWidth="2" />
        </svg>
      </div>
    );
  }

  function renderTinyTrend() {
    const maxLen = Math.max(liveHistory.cpu.length, liveHistory.mem.length, liveHistory.load.length);
    if (maxLen < 2) return <div className="h-7 w-28 rounded bg-muted/25" />;

    const width = 120;
    const height = 24;
    const px = (idx: number) => (idx / (maxLen - 1)) * (width - 1);
    const py = (v: number) => height - 2 - (Math.max(0, Math.min(100, v)) / 100) * (height - 4);
    const points = (series: number[]) => series.map((v, idx) => `${px(idx)},${py(v)}`).join(" ");

    return (
      <svg viewBox={`0 0 ${width} ${height}`} className="h-7 w-28">
        <polyline points={points(liveHistory.cpu)} fill="none" stroke={chartPalette.cpu} strokeWidth="1.5" />
        <polyline points={points(liveHistory.mem)} fill="none" stroke={chartPalette.mem} strokeWidth="1.5" />
        <polyline points={points(liveHistory.load)} fill="none" stroke={chartPalette.load} strokeWidth="1.5" />
      </svg>
    );
  }

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

        <div
          className="min-w-0 flex items-center gap-2 flex-1 h-full"
          data-tauri-drag-region
          style={{ WebkitAppRegion: "drag" } as any}
        >
          {sessions.length > 0 ? (
            <div className="flex items-center gap-1 overflow-x-auto h-full max-w-[min(720px,60vw)]">
              {sessions.map((session) => {
                const active = session.id === activeSessionId;
                const exited = session.status === "exited";
                const starting = session.status === "starting";
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
                        : starting
                          ? `${session.hostAlias} #${idx} (connecting...)`
                          : `${session.hostAlias} #${idx}`
                    }
                    aria-label={`Switch to ${session.hostAlias}`}
                    data-tauri-drag-region
                    style={{ WebkitAppRegion: "drag" } as any}
                  >
                    {exited ? <span className="h-2 w-2 rounded-full bg-red-500/70" aria-hidden="true" /> : null}
                    {starting ? <span className="h-2 w-2 rounded-full bg-amber-400/80" aria-hidden="true" /> : null}
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
            <div className="h-full flex items-center px-2 text-sm font-semibold leading-none text-muted-foreground/80 cursor-default">
              xTermius
            </div>
          )}

          <div className="flex-1 h-full" data-tauri-drag-region style={{ WebkitAppRegion: "drag" } as any} />
        </div>

        <div
          className="ml-auto flex items-center gap-1"
          data-tauri-drag-region
          style={{ WebkitAppRegion: "drag" } as any}
        >
          <button
            type="button"
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
            onClick={onOpenSyncSettings}
            title="WebDAV Sync"
            aria-label="WebDAV Sync"
            data-tauri-drag-region="false"
            style={{ WebkitAppRegion: "no-drag" } as any}
          >
            <Cloud size={18} />
          </button>
          <button
            type="button"
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
            onClick={onOpenSettings}
            title="Settings"
            aria-label="Settings"
            data-tauri-drag-region="false"
            style={{ WebkitAppRegion: "no-drag" } as any}
          >
            <Settings2 size={18} />
          </button>
          <button
            type="button"
            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
            onClick={openAddDialog}
            title="Add Host"
            aria-label="Add Host"
            data-tauri-drag-region="false"
            style={{ WebkitAppRegion: "no-drag" } as any}
          >
            <Plus size={18} />
          </button>
        </div>
      </div>

      <main className="flex-1 min-h-0 min-w-0 flex flex-col overflow-hidden">
        <div className="flex-1 min-h-0 pt-1 px-3 pb-3 flex flex-col gap-2 overflow-hidden">
          <div className="relative flex-1 min-h-0 overflow-hidden">
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
                style={!activeSessionId ? ({ visibility: "hidden", pointerEvents: "none" } as any) : undefined}
              />
            </div>

            {!activeSessionId ? (
              <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
                <div className="text-center text-slate-300">
                  <p className="text-lg mb-2">Select a host to connect</p>
                  <p className="text-sm opacity-80">{hostHintText}</p>
                </div>
              </div>
            ) : null}

            {children}
          </div>

          {activeSessionId && metricsDockEnabled ? (
            <div className="rounded-xl border border-border/60 bg-background/78 backdrop-blur px-3 py-2">
              {metricsMode === "minimal" ? (
                <div className="grid grid-cols-[auto_auto_auto_auto_auto_auto_auto_auto_minmax(120px,1fr)_auto] items-center gap-3 text-[11px]">
                  <div className="whitespace-nowrap"><span className="text-muted-foreground">CPU </span><span className="font-semibold tabular-nums">{formatPercent(liveInfo?.cpuPercent)}</span></div>
                  <div className="whitespace-nowrap"><span className="text-muted-foreground">MEM </span><span className="font-semibold tabular-nums">{formatMem(liveInfo?.memUsedKb)}/{formatMem(liveInfo?.memTotalKb)}</span></div>
                  <div className="whitespace-nowrap"><span className="text-muted-foreground">MEM% </span><span className="font-semibold tabular-nums">{Math.round(memUsedPercent())}%</span></div>
                  <div className="whitespace-nowrap"><span className="text-muted-foreground">L1 </span><span className="font-semibold tabular-nums">{formatLoad(liveInfo?.load1)}</span></div>
                  <div className="whitespace-nowrap"><span className="text-muted-foreground">L5 </span><span className="font-semibold tabular-nums">{formatLoad(liveInfo?.load5)}</span></div>
                  <div className="whitespace-nowrap"><span className="text-muted-foreground">Disk% </span><span className="font-semibold tabular-nums">{Math.round(diskUsedPercent())}%</span></div>
                  <div className="whitespace-nowrap"><span className="text-muted-foreground">Idle </span><span className="font-semibold tabular-nums">{formatPercent(liveInfo?.cpuIdlePercent)}</span></div>
                  <div className="whitespace-nowrap"><span className="text-muted-foreground">Cores </span><span className="font-semibold tabular-nums">{liveInfo?.cpuCores ?? "--"}</span></div>
                  <div className="justify-self-end">{renderTinyTrend()}</div>
                  <div className="justify-self-end flex items-center gap-1 text-[10px]">
                    <span className="text-muted-foreground">{liveLoading ? "Loading" : liveUpdatedAt ? "Live" : "Pending"}</span>
                    <button
                      type="button"
                      className="h-6 px-2 rounded-md text-[10px] text-muted-foreground hover:text-foreground hover:bg-accent"
                      onClick={() => setMetricsMode("full")}
                    >
                      Full
                    </button>
                  </div>
                </div>
              ) : (
                <div className="min-h-0">
                  <div className="flex items-center justify-end gap-1 text-[10px] mb-2">
                    <span className="text-muted-foreground">{liveLoading ? "Loading" : liveUpdatedAt ? "Live" : "Pending"}</span>
                    <button
                      type="button"
                      className="h-6 px-2 rounded-md text-[10px] text-muted-foreground hover:text-foreground hover:bg-accent border border-transparent hover:border-border/50"
                      onClick={() => setMetricsMode("minimal")}
                    >
                      Minimal
                    </button>
                  </div>
                  {liveError ? <div className="mt-1 text-[10px] text-rose-400">{liveError}</div> : null}
                  {liveInfo ? (
                    <div className="grid grid-cols-12 gap-2.5 items-stretch text-[10px]">
                      <div className="col-span-8 rounded-md bg-muted/40 p-2.5">
                        <div className="flex items-start justify-between">
                          <div className="text-[24px] leading-none font-semibold tabular-nums tracking-tight">{formatPercent(liveInfo.cpuPercent)}</div>
                          <div className="grid grid-cols-3 gap-x-2 gap-y-1 text-[10px]">
                            <div className="text-muted-foreground">SYS</div>
                            <div className="text-muted-foreground">USER</div>
                            <div className="text-muted-foreground">IOWAIT</div>
                            <div className="font-semibold tabular-nums">{formatPercent(liveInfo.cpuSystemPercent)}</div>
                            <div className="font-semibold tabular-nums">{formatPercent(liveInfo.cpuUserPercent)}</div>
                            <div className="font-semibold tabular-nums">{formatPercent(liveInfo.cpuIowaitPercent)}</div>
                          </div>
                        </div>
                        <div className="mt-1">{renderCpuGrid(liveInfo.cpuPercent)}</div>
                        <div className="mt-2 grid grid-cols-[1fr_1fr_1fr_1fr_auto] gap-2 text-[10px]">
                          <div>
                            <div className="text-muted-foreground">CORES</div>
                            <div className="font-semibold tabular-nums">{liveInfo.cpuCores ?? "--"}</div>
                          </div>
                          <div>
                            <div className="text-muted-foreground">IDLE</div>
                            <div className="font-semibold tabular-nums">{formatPercent(liveInfo.cpuIdlePercent)}</div>
                          </div>
                          <div>
                            <div className="text-muted-foreground">UPTIME</div>
                            <div className="font-semibold tabular-nums">{formatUptime(liveInfo.uptimeSeconds)}</div>
                          </div>
                          <div>
                            <div className="text-muted-foreground">LOAD</div>
                            <div className="font-semibold tabular-nums text-[10px] whitespace-nowrap tracking-tight">
                              {formatLoad(liveInfo.load1)}/{formatLoad(liveInfo.load5)}/{formatLoad(liveInfo.load15)}
                            </div>
                          </div>
                          <div className="flex items-end gap-1">{renderLoadRadar(liveInfo.load1, liveInfo.cpuCores)}</div>
                        </div>
                      </div>

                      <div className="col-span-4 rounded-md bg-muted/40 p-2.5 text-[11px]">
                        <div className="text-[10px] text-muted-foreground mb-1">Trend</div>
                        {renderCombinedTrend()}
                      </div>

                      <div className="col-span-8 rounded-md bg-muted/40 p-2.5 text-[11px] h-full flex flex-col">
                        <div className="grid grid-cols-[1fr_1fr_1fr_auto] items-start gap-2">
                          <div>
                            <div className="text-muted-foreground">FREE</div>
                            <div className="font-semibold tabular-nums whitespace-nowrap">{formatMem(liveInfo.memFreeKb)}</div>
                          </div>
                          <div>
                            <div className="text-muted-foreground whitespace-nowrap">USED</div>
                            <div className="font-semibold tabular-nums whitespace-nowrap">{formatMem(liveInfo.memUsedKb)}</div>
                          </div>
                          <div>
                            <div className="text-muted-foreground whitespace-nowrap">CACHE</div>
                            <div className="font-semibold tabular-nums whitespace-nowrap">{formatMem(liveInfo.memPageCacheKb)}</div>
                          </div>
                          <div className="pt-0.5">
                            <div
                              className="h-10 w-10 rounded-full relative"
                              style={{
                                background: `conic-gradient(${chartPalette.mem} ${memUsedPercent()}%, ${chartPalette.ringTrack} 0)`,
                              }}
                            >
                              <div className="absolute inset-[4px] rounded-full bg-[var(--app-term-bg)] flex items-center justify-center text-[9px] font-semibold tabular-nums">
                                {Math.round(memUsedPercent())}%
                              </div>
                            </div>
                          </div>
                        </div>
                        <div className="mt-2 h-2 w-full overflow-hidden flex rounded" style={{ background: chartPalette.track }}>
                          <div className="h-full" style={{ width: `${memUsedPercent()}%`, background: chartPalette.mem }} />
                          <div className="h-full" style={{ width: `${memCachePercent()}%`, background: chartPalette.load }} />
                          <div
                            className="h-full"
                            style={{ width: `${memFreePercent()}%`, background: appearance === "dark" ? "rgb(203 213 225)" : "rgb(148 163 184)" }}
                          />
                        </div>
                        <div className="mt-1 flex items-center gap-2 text-[9px] text-muted-foreground">
                          <span><i className="inline-block h-2 w-2 rounded-full bg-emerald-400 mr-1" />Used</span>
                          <span><i className="inline-block h-2 w-2 rounded-full bg-amber-400 mr-1" />Cache</span>
                          <span><i className="inline-block h-2 w-2 rounded-full bg-slate-300 mr-1" />Free</span>
                        </div>
                        <div className="mt-auto pt-3 grid grid-cols-[auto_1fr_auto] items-center gap-3 text-[10px] border-t border-white/10">
                          <div className="text-muted-foreground whitespace-nowrap">Disk /</div>
                          <div className="h-2 rounded overflow-hidden min-w-[120px]" style={{ background: chartPalette.track }}>
                            <div className="h-full" style={{ width: `${diskUsedPercent()}%`, background: chartPalette.cpu }} />
                          </div>
                          <div className="font-semibold whitespace-nowrap tabular-nums text-right">
                            {formatMem(liveInfo.diskRootUsedKb)} / {formatMem(liveInfo.diskRootTotalKb)}
                          </div>
                        </div>
                      </div>

                      <div className="col-span-4 rounded-md bg-muted/40 p-2.5">
                        <div className="text-[10px] text-muted-foreground">Top Processes</div>
                        <div className="mt-1.5 space-y-1">
                          {liveInfo.processes?.slice(0, 5).map((proc) => (
                            <div key={`${proc.command}-${proc.cpuPercent}`} className="flex items-center gap-2 text-[10px]">
                              <div className="flex-1 truncate font-mono">{proc.command}</div>
                              <div className="w-12 text-right tabular-nums font-semibold">{formatPercent(proc.cpuPercent)}</div>
                            </div>
                          ))}
                          {!liveInfo.processes?.length ? <div className="text-[10px] text-muted-foreground">No process data</div> : null}
                        </div>
                      </div>
                    </div>
                  ) : (
                    <div className="text-[11px] text-muted-foreground">No live metrics data yet.</div>
                  )}
                </div>
              )}
            </div>
          ) : null}
        </div>
      </main>
    </div>
  );
}
