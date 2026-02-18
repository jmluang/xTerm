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
import type { Settings } from "@/types/models";
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
  } = props;
  const [activeSection, setActiveSection] = useState<SettingsSection>(initialSection);
  const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

  useEffect(() => {
    if (!open) return;
    setActiveSection(initialSection);
  }, [open, initialSection]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onOpenChange(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onOpenChange]);

  const appearance = useMemo<"light" | "dark">(() => {
    if (themeMode === "light" || themeMode === "dark") return themeMode;
    return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }, [themeMode]);

  const previewTheme = useMemo(() => getTerminalTheme(terminalThemeId, appearance, "#ffffff"), [terminalThemeId, appearance]);

  function patchTerminalOptions(patch: Partial<TerminalOptionsState>) {
    setTerminalOptions((prev) => sanitizeTerminalOptions({ ...prev, ...patch }));
  }

  if (!open) return null;

  return (
    <div className="absolute inset-0 z-[70]" data-tauri-drag-region="false" style={{ WebkitAppRegion: "no-drag" } as any}>
      <div className="absolute inset-0 bg-background/95 backdrop-blur-xl" />
      <div className="absolute inset-0 flex flex-col">
        <header
          data-tauri-drag-region
          className={[
            "h-[44px] pt-[4px] border-b border-border pr-4 flex items-center justify-between select-none",
            isMac ? "pl-[88px]" : "pl-4",
          ].join(" ")}
          style={{ WebkitAppRegion: "drag" } as any}
        >
          <div className="text-xl font-semibold">Settings</div>
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
          <aside className="border-r border-border p-3 overflow-auto">
            <div className="grid gap-1">
              {[
                { id: "terminal", label: "Terminal" },
                { id: "appearance", label: "Appearance" },
                { id: "sync", label: "Sync" },
              ].map((section) => (
                <button
                  key={section.id}
                  type="button"
                  onClick={() => setActiveSection(section.id as SettingsSection)}
                  className={[
                    "h-10 px-3 rounded-lg text-left text-sm",
                    activeSection === section.id
                      ? "bg-accent text-foreground"
                      : "text-muted-foreground hover:text-foreground hover:bg-accent/60",
                  ].join(" ")}
                >
                  {section.label}
                </button>
              ))}
            </div>
          </aside>

          <main className="min-h-0 overflow-auto p-6">
            {activeSection === "terminal" ? (
              <div className="mx-auto max-w-4xl grid gap-4">
                <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                  <div className="text-lg font-semibold">Terminal Settings</div>

                  <div className="grid gap-2">
                    <div className="text-sm font-medium">Theme Presets</div>
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

                <div className="flex items-center justify-between">
                  <div className="text-xs text-muted-foreground">All terminal settings are saved locally.</div>
                  <Button type="button" variant="outline" onClick={() => setTerminalOptions(DEFAULT_TERMINAL_OPTIONS)}>
                    Reset Defaults
                  </Button>
                </div>
              </div>
            ) : null}

            {activeSection === "appearance" ? (
              <div className="mx-auto max-w-4xl rounded-2xl border border-border bg-card/80 p-5 grid gap-4">
                <div className="text-lg font-semibold">Appearance</div>
                <div className="grid gap-2">
                  <div className="text-sm font-medium">App Theme</div>
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
          </main>
        </div>
      </div>
    </div>
  );
}
