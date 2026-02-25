import type { ThemeMode } from "@/lib/theme";
import type { TerminalThemeId } from "@/lib/terminalTheme";
import type { TerminalOptionsState } from "@/lib/terminalOptions";
import type { SettingsSection } from "@/types/settings";

export const SETTINGS_NAVIGATE_EVENT = "settings:navigate";
export const SETTINGS_THEME_MODE_EVENT = "settings:theme-mode";
export const SETTINGS_TERMINAL_THEME_EVENT = "settings:terminal-theme";
export const SETTINGS_TERMINAL_OPTIONS_EVENT = "settings:terminal-options";
export const SETTINGS_METRICS_DOCK_EVENT = "settings:metrics-dock";
export const SETTINGS_HOSTS_RELOAD_EVENT = "settings:hosts-reload";

export type SettingsNavigatePayload = {
  section: SettingsSection;
  target?: "host-metrics-dock";
};

export type SettingsThemeModePayload = {
  mode: ThemeMode;
};

export type SettingsTerminalThemePayload = {
  themeId: TerminalThemeId;
};

export type SettingsTerminalOptionsPayload = {
  options: TerminalOptionsState;
};

export type SettingsMetricsDockPayload = {
  enabled: boolean;
};

export type SettingsHostsReloadPayload = {
  reason?: "webdav-pull" | "webdav-push" | "manual";
};
