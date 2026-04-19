export type SettingsSection = "terminal" | "sync" | "import" | "about";

export type UpdaterStatus =
  | "idle"
  | "checking"
  | "available"
  | "up-to-date"
  | "downloading"
  | "installing"
  | "restart-required"
  | "error";

export type UpdaterViewState = {
  appName: string;
  channel: "stable";
  currentVersion: string;
  enabled: boolean;
  status: UpdaterStatus;
  statusText: string;
  error: string | null;
  availableVersion: string | null;
  releaseNotes: string | null;
  checkForUpdates: () => Promise<void>;
  downloadAndInstall: () => Promise<void>;
};
