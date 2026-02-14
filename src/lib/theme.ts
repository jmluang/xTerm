export type ThemeMode = "system" | "light" | "dark";

const KEY = "xtermius_theme_mode";

function getSystemTheme(): "light" | "dark" {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(theme: "light" | "dark") {
  document.documentElement.dataset.theme = theme;
}

export function getThemeMode(): ThemeMode {
  const v = localStorage.getItem(KEY);
  if (v === "light" || v === "dark" || v === "system") return v;
  return "system";
}

export function setThemeMode(mode: ThemeMode) {
  localStorage.setItem(KEY, mode);
  syncThemeMode(mode);
}

export function syncThemeMode(mode: ThemeMode = getThemeMode()) {
  if (mode === "system") {
    applyTheme(getSystemTheme());
  } else {
    applyTheme(mode);
  }
}

export function initTheme() {
  const mode = getThemeMode();
  syncThemeMode(mode);

  if (!window.matchMedia) return;
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  const onChange = () => {
    if (getThemeMode() === "system") syncThemeMode("system");
  };

  // Safari compatibility: addEventListener may be missing.
  if (typeof mq.addEventListener === "function") mq.addEventListener("change", onChange);
  else if (typeof (mq as any).addListener === "function") (mq as any).addListener(onChange);
}

