const METRICS_DOCK_ENABLED_KEY = "xtermius_metrics_dock_enabled";

export function getMetricsDockEnabled(): boolean {
  try {
    return localStorage.getItem(METRICS_DOCK_ENABLED_KEY) === "1";
  } catch {
    return false;
  }
}

export function setMetricsDockEnabled(enabled: boolean) {
  try {
    localStorage.setItem(METRICS_DOCK_ENABLED_KEY, enabled ? "1" : "0");
  } catch {
    // Ignore storage failures.
  }
}
