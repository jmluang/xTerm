import type { ITheme } from "@xterm/xterm";

export const TERMINAL_THEME_OPTIONS = [
  { id: "auto", label: "Auto", preview: ["#0b1220", "#0284c7", "#7c3aed"] },
  { id: "nord", label: "Nord", preview: ["#2e3440", "#5e81ac", "#8fbcbb"] },
  { id: "solarized", label: "Solarized", preview: ["#586e75", "#b58900", "#2aa198"] },
  { id: "dracula", label: "Dracula", preview: ["#282a36", "#bd93f9", "#ff79c6"] },
  { id: "monokai", label: "Monokai", preview: ["#272822", "#a6e22e", "#f92672"] },
] as const;

export type TerminalThemeId = (typeof TERMINAL_THEME_OPTIONS)[number]["id"];

const KEY = "xtermius_terminal_theme";

type Appearance = "light" | "dark";
type ThemePalette = Omit<ITheme, "background">;

const PRESETS: Record<Exclude<TerminalThemeId, "auto">, Record<Appearance, ThemePalette>> = {
  nord: {
    dark: {
      foreground: "#d8dee9",
      cursor: "#88c0d0",
      selectionBackground: "rgba(129, 161, 193, 0.35)",
      black: "#3b4252",
      red: "#bf616a",
      green: "#a3be8c",
      yellow: "#ebcb8b",
      blue: "#81a1c1",
      magenta: "#b48ead",
      cyan: "#88c0d0",
      white: "#e5e9f0",
      brightBlack: "#4c566a",
      brightRed: "#bf616a",
      brightGreen: "#a3be8c",
      brightYellow: "#ebcb8b",
      brightBlue: "#81a1c1",
      brightMagenta: "#b48ead",
      brightCyan: "#8fbcbb",
      brightWhite: "#eceff4",
    },
    light: {
      foreground: "#2e3440",
      cursor: "#5e81ac",
      selectionBackground: "rgba(94, 129, 172, 0.26)",
      black: "#3b4252",
      red: "#bf616a",
      green: "#5f7e4f",
      yellow: "#8f6a1c",
      blue: "#5e81ac",
      magenta: "#7f5e92",
      cyan: "#4c7b86",
      white: "#d8dee9",
      brightBlack: "#4c566a",
      brightRed: "#b04f59",
      brightGreen: "#6f8c5f",
      brightYellow: "#9e7a2a",
      brightBlue: "#6d8fb8",
      brightMagenta: "#8d6da0",
      brightCyan: "#5b8e9a",
      brightWhite: "#eceff4",
    },
  },
  solarized: {
    dark: {
      foreground: "#839496",
      cursor: "#93a1a1",
      selectionBackground: "rgba(147, 161, 161, 0.28)",
      black: "#073642",
      red: "#dc322f",
      green: "#859900",
      yellow: "#b58900",
      blue: "#268bd2",
      magenta: "#d33682",
      cyan: "#2aa198",
      white: "#eee8d5",
      brightBlack: "#002b36",
      brightRed: "#cb4b16",
      brightGreen: "#586e75",
      brightYellow: "#657b83",
      brightBlue: "#839496",
      brightMagenta: "#6c71c4",
      brightCyan: "#93a1a1",
      brightWhite: "#fdf6e3",
    },
    light: {
      foreground: "#586e75",
      cursor: "#657b83",
      selectionBackground: "rgba(101, 123, 131, 0.22)",
      black: "#073642",
      red: "#dc322f",
      green: "#859900",
      yellow: "#b58900",
      blue: "#268bd2",
      magenta: "#d33682",
      cyan: "#2aa198",
      white: "#eee8d5",
      brightBlack: "#002b36",
      brightRed: "#cb4b16",
      brightGreen: "#586e75",
      brightYellow: "#657b83",
      brightBlue: "#839496",
      brightMagenta: "#6c71c4",
      brightCyan: "#93a1a1",
      brightWhite: "#fdf6e3",
    },
  },
  dracula: {
    dark: {
      foreground: "#f8f8f2",
      cursor: "#f8f8f2",
      selectionBackground: "rgba(189, 147, 249, 0.30)",
      black: "#21222c",
      red: "#ff5555",
      green: "#50fa7b",
      yellow: "#f1fa8c",
      blue: "#bd93f9",
      magenta: "#ff79c6",
      cyan: "#8be9fd",
      white: "#f8f8f2",
      brightBlack: "#6272a4",
      brightRed: "#ff6e6e",
      brightGreen: "#69ff94",
      brightYellow: "#ffffa5",
      brightBlue: "#d6acff",
      brightMagenta: "#ff92df",
      brightCyan: "#a4ffff",
      brightWhite: "#ffffff",
    },
    light: {
      foreground: "#282a36",
      cursor: "#44475a",
      selectionBackground: "rgba(68, 71, 90, 0.20)",
      black: "#1f2330",
      red: "#d64045",
      green: "#2a9d50",
      yellow: "#a17900",
      blue: "#6f5bd9",
      magenta: "#c04f9f",
      cyan: "#2a8da8",
      white: "#f6f6f6",
      brightBlack: "#59627a",
      brightRed: "#e65a5e",
      brightGreen: "#3ab162",
      brightYellow: "#b78f16",
      brightBlue: "#846ff1",
      brightMagenta: "#d766b7",
      brightCyan: "#40a4be",
      brightWhite: "#ffffff",
    },
  },
  monokai: {
    dark: {
      foreground: "#f8f8f2",
      cursor: "#f8f8f0",
      selectionBackground: "rgba(117, 113, 94, 0.35)",
      black: "#272822",
      red: "#f92672",
      green: "#a6e22e",
      yellow: "#e6db74",
      blue: "#66d9ef",
      magenta: "#ae81ff",
      cyan: "#a1efe4",
      white: "#f8f8f2",
      brightBlack: "#75715e",
      brightRed: "#f92672",
      brightGreen: "#a6e22e",
      brightYellow: "#e6db74",
      brightBlue: "#66d9ef",
      brightMagenta: "#ae81ff",
      brightCyan: "#a1efe4",
      brightWhite: "#f9f8f5",
    },
    light: {
      foreground: "#2f3129",
      cursor: "#3a3d32",
      selectionBackground: "rgba(58, 61, 50, 0.22)",
      black: "#272822",
      red: "#d81b60",
      green: "#558b2f",
      yellow: "#9e7b00",
      blue: "#0277bd",
      magenta: "#7b4bc2",
      cyan: "#00796b",
      white: "#f8f8f2",
      brightBlack: "#75715e",
      brightRed: "#e91e63",
      brightGreen: "#689f38",
      brightYellow: "#b28d14",
      brightBlue: "#0288d1",
      brightMagenta: "#8e5dd5",
      brightCyan: "#00897b",
      brightWhite: "#ffffff",
    },
  },
};

const PRESET_BACKGROUNDS: Record<Exclude<TerminalThemeId, "auto">, Record<Appearance, string>> = {
  nord: {
    dark: "#2e3440",
    light: "#eceff4",
  },
  solarized: {
    dark: "#002b36",
    light: "#fdf6e3",
  },
  dracula: {
    dark: "#282a36",
    light: "#f7f4ff",
  },
  monokai: {
    dark: "#272822",
    light: "#f7f5ea",
  },
};

function isTerminalThemeId(value: string | null): value is TerminalThemeId {
  return TERMINAL_THEME_OPTIONS.some((option) => option.id === value);
}

function getAutoTheme(appearance: Appearance, background: string): ITheme {
  if (appearance === "dark") {
    return {
      background,
      foreground: "#e5e7eb",
      cursor: "#e5e7eb",
      selectionBackground: "rgba(148, 163, 184, 0.35)",
    };
  }
  return {
    background,
    foreground: "#0b1220",
    cursor: "#0b1220",
    selectionBackground: "rgba(2, 132, 199, 0.22)",
  };
}

export function getTerminalThemeId(): TerminalThemeId {
  const value = localStorage.getItem(KEY);
  if (isTerminalThemeId(value)) return value;
  return "auto";
}

export function setTerminalThemeId(themeId: TerminalThemeId) {
  localStorage.setItem(KEY, themeId);
}

export function getTerminalTheme(themeId: TerminalThemeId, appearance: Appearance, background: string): ITheme {
  if (themeId === "auto") return getAutoTheme(appearance, background);
  return {
    background: PRESET_BACKGROUNDS[themeId][appearance],
    ...PRESETS[themeId][appearance],
  };
}
