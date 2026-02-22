import { useEffect, useMemo, useState, type Dispatch, type SetStateAction } from "react";
import { ChevronDown, Minus, Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { resolveWebdavHostsDbUrl } from "@/lib/webdav";
import { getTerminalTheme, TERMINAL_THEME_OPTIONS, type TerminalThemeId } from "@/lib/terminalTheme";
import {
  DEFAULT_TERMINAL_OPTIONS,
  sanitizeTerminalOptions,
  type TerminalBellStyle,
  type TerminalCursorStyle,
  type TerminalOptionsState,
} from "@/lib/terminalOptions";
import type { ThemeMode } from "@/lib/theme";
import type { Settings, SshConfigImportCandidate } from "@/types/models";
import type { SettingsSection } from "@/types/settings";

function Toggle(props: { checked: boolean; onChange: (next: boolean) => void; ariaLabel: string }) {
  const { checked, onChange, ariaLabel } = props;
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      onClick={() => onChange(!checked)}
      className={[
        "h-7 w-12 rounded-full transition-colors inline-flex items-center px-1",
        checked ? "bg-sky-500/80" : "bg-muted",
      ].join(" ")}
    >
      <span
        className={[
          "h-5 w-5 rounded-full bg-white transition-transform shadow-sm",
          checked ? "translate-x-5" : "translate-x-0",
        ].join(" ")}
      />
    </button>
  );
}

export function SettingsPanel(props: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialSection: SettingsSection;
  themeMode: ThemeMode;
  setThemeMode: (mode: ThemeMode) => void;
  terminalThemeId: TerminalThemeId;
  setTerminalThemeId: (themeId: TerminalThemeId) => void;
  terminalOptions: TerminalOptionsState;
  setTerminalOptions: Dispatch<SetStateAction<TerminalOptionsState>>;
  metricsDockEnabled: boolean;
  setMetricsDockEnabled: Dispatch<SetStateAction<boolean>>;
  settings: Settings;
  setSettings: Dispatch<SetStateAction<Settings>>;
  localHostsDbPath: string;
  syncBusy: null | "pull" | "push" | "save";
  syncNotice: null | { kind: "ok" | "err"; text: string };
  isInTauri: boolean;
  onSaveSettings: () => Promise<void>;
  onPull: () => Promise<void>;
  onPush: () => Promise<void>;
  onRefreshSshImport?: () => Promise<void> | void;
  onImportSshConfigSelected?: (aliases: string[]) => Promise<void> | void;
  sshImportBusy?: boolean;
  sshImportLoading?: boolean;
  sshImportCandidates?: SshConfigImportCandidate[];
}) {
  const {
    open,
    onOpenChange,
    initialSection,
    themeMode,
    setThemeMode,
    terminalThemeId,
    setTerminalThemeId,
    terminalOptions,
    setTerminalOptions,
    metricsDockEnabled,
    setMetricsDockEnabled,
    settings,
    setSettings,
    localHostsDbPath,
    syncBusy,
    syncNotice,
    isInTauri,
    onSaveSettings,
    onPull,
    onPush,
    onRefreshSshImport,
    onImportSshConfigSelected,
    sshImportBusy = false,
    sshImportLoading = false,
    sshImportCandidates = [],
  } = props;
  const [activeSection, setActiveSection] = useState<SettingsSection>(initialSection);
  const [selectedImportAliases, setSelectedImportAliases] = useState<Set<string>>(new Set());
  const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

  useEffect(() => {
    if (!open) return;
    setActiveSection(initialSection);
  }, [open, initialSection]);

  useEffect(() => {
    if (activeSection !== "import") return;
    if (!isInTauri) return;
    if (sshImportCandidates.length > 0 || sshImportLoading) return;
    void onRefreshSshImport?.();
  }, [activeSection, isInTauri, sshImportCandidates.length, sshImportLoading, onRefreshSshImport]);

  useEffect(() => {
    setSelectedImportAliases(new Set(sshImportCandidates.map((item) => item.alias)));
  }, [sshImportCandidates]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onOpenChange(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onOpenChange]);

  const [appearance, setAppearance] = useState<"light" | "dark">(() => {
    const datasetTheme = typeof document !== "undefined" ? document.documentElement.dataset.theme : undefined;
    if (datasetTheme === "light" || datasetTheme === "dark") return datasetTheme;
    return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  });

  useEffect(() => {
    if (themeMode === "light" || themeMode === "dark") {
      setAppearance(themeMode);
      return;
    }

    const readCurrentAppearance = () => {
      const datasetTheme = document.documentElement.dataset.theme;
      if (datasetTheme === "light" || datasetTheme === "dark") return datasetTheme;
      return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    };

    setAppearance(readCurrentAppearance());
    const raf = window.requestAnimationFrame(() => {
      setAppearance(readCurrentAppearance());
    });

    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    const onChange = () => setAppearance(readCurrentAppearance());
    if (mq) {
      if (typeof mq.addEventListener === "function") mq.addEventListener("change", onChange);
      else if (typeof (mq as any).addListener === "function") (mq as any).addListener(onChange);
    }

    return () => {
      window.cancelAnimationFrame(raf);
      if (!mq) return;
      if (typeof mq.removeEventListener === "function") mq.removeEventListener("change", onChange);
      else if (typeof (mq as any).removeListener === "function") (mq as any).removeListener(onChange);
    };
  }, [themeMode]);

  const previewTheme = useMemo(
    () => getTerminalTheme(terminalThemeId, appearance, appearance === "dark" ? "#0f1112" : "#ffffff"),
    [terminalThemeId, appearance]
  );

  function patchTerminalOptions(patch: Partial<TerminalOptionsState>) {
    setTerminalOptions((prev) => sanitizeTerminalOptions({ ...prev, ...patch }));
  }

  const isStandaloneSettingsWindow =
    typeof window !== "undefined" && new URLSearchParams(window.location.search).get("panel") === "settings";

  if (!open) return null;

  return (
    <div className="absolute inset-0 z-[70]" data-tauri-drag-region="false" style={{ WebkitAppRegion: "no-drag" } as any}>
      <div
        className={
          isStandaloneSettingsWindow
            ? "absolute inset-0"
            : "absolute inset-0 bg-background/95 backdrop-blur-xl"
        }
        style={isStandaloneSettingsWindow ? ({ background: "var(--app-settings-shell-bg)" } as any) : undefined}
      />
      <div className="absolute inset-0 flex flex-col">
        <div
          className="min-h-0 flex flex-col flex-1 overflow-hidden"
          style={{ background: isStandaloneSettingsWindow ? "var(--app-settings-shell-bg)" : "var(--app-mainpane-bg)" } as any}
        >
          <header
            data-tauri-drag-region
            className={[
              "h-[44px] pt-[4px] pr-4 flex items-center justify-between select-none",
              isMac ? "pl-[88px]" : "pl-4",
            ].join(" ")}
            style={{ background: "var(--app-settings-shell-bg)", WebkitAppRegion: "drag" } as any}
          >
            <div className="text-sm font-semibold leading-none text-muted-foreground/80">Settings</div>
            <button
              type="button"
              onClick={() => onOpenChange(false)}
              className="h-8 w-8 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
              aria-label="Close settings"
              title="Close"
              data-tauri-drag-region="false"
              style={{ WebkitAppRegion: "no-drag" } as any}
            >
              <X size={18} />
            </button>
          </header>

          <div className="flex-1 min-h-0 grid grid-cols-[260px_1fr]">
            <aside className="p-3 overflow-auto" style={{ background: "var(--app-settings-sidebar-bg)" } as any}>
              <div className="grid gap-1">
                {[
                  { id: "terminal", label: "Terminal" },
                  { id: "sync", label: "Sync" },
                  { id: "import", label: "Import SSH Config" },
                ].map((section) => (
                  <button
                    key={section.id}
                    type="button"
                    onClick={() => setActiveSection(section.id as SettingsSection)}
                    className={[
                      "h-10 px-3 rounded-lg text-left text-sm",
                      activeSection === section.id
                        ? "text-foreground bg-[var(--app-settings-nav-active-bg)]"
                        : "text-muted-foreground hover:text-foreground hover:bg-[var(--app-settings-nav-hover-bg)]",
                    ].join(" ")}
                  >
                    {section.label}
                  </button>
                ))}
              </div>
            </aside>

            <main
              className={[
                "min-h-0 overflow-auto p-5 md:p-6",
                isStandaloneSettingsWindow ? "rounded-l-xl overflow-hidden" : "",
              ].join(" ")}
              style={{ background: "var(--app-mainpane-bg)" } as any}
            >
              {activeSection === "terminal" ? (
              <div className="mx-auto max-w-4xl grid gap-4">
                <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                  <div className="flex items-start justify-between gap-4">
                    <div className="min-w-0">
                      <div className="text-lg font-semibold">Terminal Settings</div>
                      <div className="text-xs text-muted-foreground mt-1">All terminal settings are saved locally.</div>
                    </div>
                    <Button
                      type="button"
                      variant="outline"
                      className="shrink-0"
                      onClick={() => setTerminalOptions(DEFAULT_TERMINAL_OPTIONS)}
                    >
                      Reset Defaults
                    </Button>
                  </div>
                </div>

                <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                  <div className="text-sm font-medium">App Theme</div>
                  <div className="grid gap-2">
                    <div className="flex rounded-lg border border-border bg-card p-1 max-w-sm">
                      {(["system", "light", "dark"] as ThemeMode[]).map((mode) => (
                        <button
                          key={mode}
                          type="button"
                          className={[
                            "flex-1 h-8 rounded-md text-sm",
                            themeMode === mode
                              ? "bg-accent text-foreground"
                              : "text-muted-foreground hover:text-foreground",
                          ].join(" ")}
                          onClick={() => setThemeMode(mode)}
                        >
                          {mode === "system" ? "System" : mode === "light" ? "Light" : "Dark"}
                        </button>
                      ))}
                    </div>
                  </div>
                </div>

                <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                  <div className="text-sm font-medium">Theme Presets</div>
                  <div className="grid gap-2">
                    <div className="flex flex-wrap gap-2">
                      {TERMINAL_THEME_OPTIONS.map((option) => (
                        <button
                          key={option.id}
                          type="button"
                          onClick={() => setTerminalThemeId(option.id)}
                          className={[
                            "h-9 px-3 rounded-md border border-border inline-flex items-center gap-2",
                            terminalThemeId === option.id
                              ? "bg-accent text-foreground"
                              : "text-muted-foreground hover:text-foreground hover:bg-accent/60",
                          ].join(" ")}
                        >
                          <span className="flex items-center gap-1" aria-hidden="true">
                            {option.preview.map((color) => (
                              <span key={color} className="h-2.5 w-2.5 rounded-full" style={{ backgroundColor: color }} />
                            ))}
                          </span>
                          <span>{option.label}</span>
                        </button>
                      ))}
                    </div>
                    <div
                      className="rounded-xl border border-border px-3 py-2 text-sm font-mono"
                      style={{ backgroundColor: previewTheme.background, color: previewTheme.foreground }}
                    >
                      <span style={{ color: previewTheme.green }}>$ ssh prod-server</span>
                      <span style={{ color: previewTheme.cyan }}> connected</span>
                      <span style={{ color: previewTheme.yellow }}> in 18ms</span>
                    </div>
                  </div>
                </div>

                <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                  <div className="text-sm font-medium">Typography</div>
                  <div className="grid gap-3 md:grid-cols-2">
                    <div className="grid gap-2">
                      <label className="text-sm text-muted-foreground">Font Family</label>
                      <Input
                        value={terminalOptions.fontFamily}
                        onChange={(event) => patchTerminalOptions({ fontFamily: event.target.value })}
                        placeholder="SF Mono, Menlo, Monaco, monospace"
                      />
                    </div>
                    <div className="grid gap-2">
                      <label className="text-sm text-muted-foreground">Text Size</label>
                      <div className="flex items-center gap-2">
                        <Button
                          type="button"
                          variant="outline"
                          size="icon"
                          onClick={() => patchTerminalOptions({ fontSize: terminalOptions.fontSize - 1 })}
                          aria-label="Decrease font size"
                        >
                          <Minus size={16} />
                        </Button>
                        <Input
                          type="number"
                          value={terminalOptions.fontSize}
                          onChange={(event) => patchTerminalOptions({ fontSize: Number(event.target.value) })}
                          min={10}
                          max={32}
                          className="w-24"
                        />
                        <Button
                          type="button"
                          variant="outline"
                          size="icon"
                          onClick={() => patchTerminalOptions({ fontSize: terminalOptions.fontSize + 1 })}
                          aria-label="Increase font size"
                        >
                          <Plus size={16} />
                        </Button>
                      </div>
                    </div>

                    <div className="grid gap-2">
                      <label className="text-sm text-muted-foreground">Line Height</label>
                      <Input
                        type="number"
                        step="0.05"
                        min={1}
                        max={2.2}
                        value={terminalOptions.lineHeight}
                        onChange={(event) => patchTerminalOptions({ lineHeight: Number(event.target.value) })}
                      />
                    </div>
                    <div className="grid gap-2">
                      <label className="text-sm text-muted-foreground">Letter Spacing</label>
                      <Input
                        type="number"
                        step="0.1"
                        min={-1}
                        max={6}
                        value={terminalOptions.letterSpacing}
                        onChange={(event) => patchTerminalOptions({ letterSpacing: Number(event.target.value) })}
                      />
                    </div>
                  </div>
                </div>

                <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                  <div className="text-sm font-medium">Behavior</div>
                  <div className="grid gap-3">
                    <div className="flex items-center justify-between gap-4">
                      <div className="text-sm">Cursor Blink</div>
                      <Toggle
                        checked={terminalOptions.cursorBlink}
                        onChange={(next) => patchTerminalOptions({ cursorBlink: next })}
                        ariaLabel="Toggle cursor blink"
                      />
                    </div>
                    <div className="flex items-center justify-between gap-4">
                      <div className="text-sm">Use Option As Meta (macOS)</div>
                      <Toggle
                        checked={terminalOptions.macOptionIsMeta}
                        onChange={(next) => patchTerminalOptions({ macOptionIsMeta: next })}
                        ariaLabel="Toggle mac option meta"
                      />
                    </div>
                    <div className="flex items-center justify-between gap-4">
                      <div className="text-sm">Right Click Selects Word</div>
                      <Toggle
                        checked={terminalOptions.rightClickSelectsWord}
                        onChange={(next) => patchTerminalOptions({ rightClickSelectsWord: next })}
                        ariaLabel="Toggle right click select"
                      />
                    </div>
                    <div className="flex items-center justify-between gap-4">
                      <div className="text-sm">Bright Colors For Bold Text</div>
                      <Toggle
                        checked={terminalOptions.drawBoldTextInBrightColors}
                        onChange={(next) => patchTerminalOptions({ drawBoldTextInBrightColors: next })}
                        ariaLabel="Toggle bright bold text"
                      />
                    </div>
                    <div className="flex items-center justify-between gap-4">
                      <div>
                        <div className="text-sm">Host Metrics Dock</div>
                        <div className="text-xs text-muted-foreground">Show CPU/MEM/Load panel under terminal</div>
                      </div>
                      <Toggle
                        checked={metricsDockEnabled}
                        onChange={(next) => setMetricsDockEnabled(next)}
                        ariaLabel="Toggle host metrics dock"
                      />
                    </div>

                    <div className="grid gap-3 md:grid-cols-3 pt-2">
                      <div className="grid gap-2">
                        <label className="text-sm text-muted-foreground">Cursor Style</label>
                        <div className="relative">
                          <select
                            className="h-9 w-full appearance-none rounded-md border border-input bg-transparent px-3 pr-9 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            value={terminalOptions.cursorStyle}
                            onChange={(event) =>
                              patchTerminalOptions({ cursorStyle: event.target.value as TerminalCursorStyle })
                            }
                          >
                            <option value="block">Block</option>
                            <option value="underline">Underline</option>
                            <option value="bar">Bar</option>
                          </select>
                          <ChevronDown
                            size={16}
                            className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground"
                            aria-hidden="true"
                          />
                        </div>
                      </div>

                      <div className="grid gap-2">
                        <label className="text-sm text-muted-foreground">Bell</label>
                        <div className="relative">
                          <select
                            className="h-9 w-full appearance-none rounded-md border border-input bg-transparent px-3 pr-9 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            value={terminalOptions.bellStyle}
                            onChange={(event) =>
                              patchTerminalOptions({ bellStyle: event.target.value as TerminalBellStyle })
                            }
                          >
                            <option value="none">Disabled</option>
                            <option value="sound">Sound</option>
                          </select>
                          <ChevronDown
                            size={16}
                            className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground"
                            aria-hidden="true"
                          />
                        </div>
                      </div>

                      <div className="grid gap-2">
                        <label className="text-sm text-muted-foreground">Scrollback</label>
                        <Input
                          type="number"
                          min={500}
                          max={50000}
                          value={terminalOptions.scrollback}
                          onChange={(event) => patchTerminalOptions({ scrollback: Number(event.target.value) })}
                        />
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            ) : null}

            {activeSection === "sync" ? (
              <div className="mx-auto max-w-4xl rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                <div className="text-lg font-semibold">Sync</div>
                <div className="grid gap-3">
                  <div className="grid gap-2">
                    <label className="text-xs text-muted-foreground">WebDAV URL</label>
                    <Input
                      value={settings.webdav_url ?? ""}
                      onChange={(event) => setSettings((prev) => ({ ...prev, webdav_url: event.target.value }))}
                      placeholder="https://dav.example.com/dav/"
                    />
                  </div>
                  <div className="grid gap-2">
                    <label className="text-xs text-muted-foreground">Remote Folder</label>
                    <Input
                      value={settings.webdav_folder ?? "xTermius"}
                      onChange={(event) => setSettings((prev) => ({ ...prev, webdav_folder: event.target.value }))}
                      placeholder="xTermius"
                    />
                  </div>
                  <div className="grid gap-2 md:grid-cols-2 md:gap-3">
                    <div className="grid gap-2">
                      <label className="text-xs text-muted-foreground">Username</label>
                      <Input
                        value={settings.webdav_username ?? ""}
                        onChange={(event) => setSettings((prev) => ({ ...prev, webdav_username: event.target.value }))}
                        placeholder=""
                      />
                    </div>
                    <div className="grid gap-2">
                      <label className="text-xs text-muted-foreground">Password</label>
                      <Input
                        type="password"
                        value={settings.webdav_password ?? ""}
                        onChange={(event) => setSettings((prev) => ({ ...prev, webdav_password: event.target.value }))}
                        placeholder=""
                      />
                    </div>
                  </div>
                </div>

                <div className="rounded-lg border border-border bg-muted/25 px-3 py-2 grid gap-1 text-xs text-muted-foreground">
                  <div>
                    Remote file:{" "}
                    <code className="font-mono">
                      {resolveWebdavHostsDbUrl(settings.webdav_url ?? "", settings.webdav_folder ?? "xTermius") ||
                        "(not set)"}
                    </code>
                  </div>
                  <div>
                    Local file: <code className="font-mono">{localHostsDbPath || "(unknown)"}</code>
                  </div>
                </div>

                {syncBusy ? (
                  <div className="text-xs text-muted-foreground">
                    {syncBusy === "save" ? "Saving..." : syncBusy === "pull" ? "Pulling..." : "Pushing..."}
                  </div>
                ) : syncNotice ? (
                  <div className={"text-xs " + (syncNotice.kind === "ok" ? "text-foreground/70" : "text-destructive")}>
                    {syncNotice.text}
                  </div>
                ) : null}

                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    disabled={!isInTauri || syncBusy !== null}
                    onClick={() => {
                      void onSaveSettings();
                    }}
                  >
                    Save
                  </Button>
                  <Button
                    variant="outline"
                    disabled={!isInTauri || syncBusy !== null}
                    onClick={() => {
                      void onPull();
                    }}
                  >
                    Pull
                  </Button>
                  <Button
                    variant="default"
                    disabled={!isInTauri || syncBusy !== null}
                    onClick={() => {
                      void onPush();
                    }}
                    >
                      Push
                    </Button>
                </div>
              </div>
            ) : null}

            {activeSection === "import" ? (
              <div className="mx-auto max-w-4xl rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <div className="text-lg font-semibold">Import SSH Config</div>
                    <div className="text-xs text-muted-foreground mt-1">
                      Detect hosts from <code className="font-mono">~/.ssh/config</code> and import selected entries.
                    </div>
                  </div>
                  <Button
                    variant="outline"
                    disabled={!isInTauri || sshImportLoading || sshImportBusy}
                    onClick={() => {
                      void onRefreshSshImport?.();
                    }}
                  >
                    {sshImportLoading ? "Scanning..." : "Scan"}
                  </Button>
                </div>

                {!isInTauri ? (
                  <div className="text-sm text-muted-foreground">SSH config import is only available in the desktop app.</div>
                ) : (
                  <>
                    <div className="flex items-center gap-2">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={sshImportBusy || sshImportLoading || sshImportCandidates.length === 0}
                        onClick={() => setSelectedImportAliases(new Set(sshImportCandidates.map((v) => v.alias)))}
                      >
                        Select All
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={sshImportBusy || sshImportLoading}
                        onClick={() => setSelectedImportAliases(new Set())}
                      >
                        Clear
                      </Button>
                      <div className="ml-auto text-xs text-muted-foreground">
                        {selectedImportAliases.size} selected
                      </div>
                    </div>

                    <div className="rounded-xl border border-border bg-background/40 overflow-hidden">
                      <div className="overflow-auto min-h-[280px] max-h-[calc(100dvh-300px)]">
                        {sshImportLoading ? (
                          <div className="px-4 py-8 text-sm text-muted-foreground">Scanning ~/.ssh ...</div>
                        ) : sshImportCandidates.length === 0 ? (
                          <div className="px-4 py-8 text-sm text-muted-foreground">No importable hosts found.</div>
                        ) : (
                          <div className="p-3 grid gap-2">
                            {sshImportCandidates
                              .slice()
                              .sort((a, b) => a.alias.toLowerCase().localeCompare(b.alias.toLowerCase()))
                              .map((item) => {
                                const checked = selectedImportAliases.has(item.alias);
                                return (
                                  <label
                                    key={`${item.alias}-${item.sourcePath}`}
                                    className="flex items-start gap-3 p-3 rounded-lg border border-border hover:bg-muted/40 cursor-pointer"
                                  >
                                    <input
                                      type="checkbox"
                                      checked={checked}
                                      onChange={() =>
                                        setSelectedImportAliases((prev) => {
                                          const next = new Set(prev);
                                          if (next.has(item.alias)) next.delete(item.alias);
                                          else next.add(item.alias);
                                          return next;
                                        })
                                      }
                                      className="mt-1"
                                    />
                                    <div className="min-w-0 flex-1">
                                      <div className="text-sm font-semibold break-words">{item.alias}</div>
                                      <div className="text-xs text-muted-foreground break-words mt-0.5">
                                        {item.user ? `${item.user}@` : ""}
                                        {item.hostname}
                                        {item.port && item.port !== 22 ? `:${item.port}` : ""}
                                      </div>
                                      <div className="text-[11px] text-muted-foreground/80 break-all mt-1">{item.sourcePath}</div>
                                    </div>
                                  </label>
                                );
                              })}
                          </div>
                        )}
                      </div>
                    </div>

                    <div className="flex items-center justify-end gap-2">
                      <Button
                        variant="default"
                        disabled={
                          sshImportBusy ||
                          sshImportLoading ||
                          selectedImportAliases.size === 0 ||
                          !onImportSshConfigSelected
                        }
                        onClick={() => {
                          void onImportSshConfigSelected?.(Array.from(selectedImportAliases));
                        }}
                      >
                        {sshImportBusy ? "Importing..." : "Import Selected"}
                      </Button>
                    </div>
                  </>
                )}
              </div>
            ) : null}
            </main>
          </div>
        </div>
      </div>
    </div>
  );
}
