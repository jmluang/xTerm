export type TerminalCursorStyle = "block" | "underline" | "bar";
export type TerminalBellStyle = "none" | "sound";

export type TerminalOptionsState = {
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  letterSpacing: number;
  cursorStyle: TerminalCursorStyle;
  cursorBlink: boolean;
  scrollback: number;
  macOptionIsMeta: boolean;
  rightClickSelectsWord: boolean;
  drawBoldTextInBrightColors: boolean;
  bellStyle: TerminalBellStyle;
};

const KEY = "xtermius_terminal_options";

export const DEFAULT_TERMINAL_OPTIONS: TerminalOptionsState = {
  fontFamily: "SF Mono, Menlo, Monaco, 'Courier New', monospace",
  fontSize: 14,
  lineHeight: 1.2,
  letterSpacing: 0,
  cursorStyle: "block",
  cursorBlink: true,
  scrollback: 5000,
  macOptionIsMeta: true,
  rightClickSelectsWord: false,
  drawBoldTextInBrightColors: true,
  bellStyle: "none",
};

function clampNumber(value: unknown, min: number, max: number, fallback: number): number {
  const numeric = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(numeric)) return fallback;
  return Math.min(max, Math.max(min, numeric));
}

function ensureCursorStyle(value: unknown): TerminalCursorStyle {
  return value === "block" || value === "underline" || value === "bar" ? value : DEFAULT_TERMINAL_OPTIONS.cursorStyle;
}

function ensureBellStyle(value: unknown): TerminalBellStyle {
  return value === "none" || value === "sound" ? value : DEFAULT_TERMINAL_OPTIONS.bellStyle;
}

export function sanitizeTerminalOptions(value: Partial<TerminalOptionsState> | null | undefined): TerminalOptionsState {
  const input = value ?? {};
  return {
    fontFamily:
      typeof input.fontFamily === "string" && input.fontFamily.trim()
        ? input.fontFamily.trim()
        : DEFAULT_TERMINAL_OPTIONS.fontFamily,
    fontSize: Math.round(
      clampNumber(input.fontSize, 10, 32, DEFAULT_TERMINAL_OPTIONS.fontSize)
    ),
    lineHeight: Number(clampNumber(input.lineHeight, 1, 2.2, DEFAULT_TERMINAL_OPTIONS.lineHeight).toFixed(2)),
    letterSpacing: Number(clampNumber(input.letterSpacing, -1, 6, DEFAULT_TERMINAL_OPTIONS.letterSpacing).toFixed(2)),
    cursorStyle: ensureCursorStyle(input.cursorStyle),
    cursorBlink: typeof input.cursorBlink === "boolean" ? input.cursorBlink : DEFAULT_TERMINAL_OPTIONS.cursorBlink,
    scrollback: Math.round(clampNumber(input.scrollback, 500, 50_000, DEFAULT_TERMINAL_OPTIONS.scrollback)),
    macOptionIsMeta:
      typeof input.macOptionIsMeta === "boolean" ? input.macOptionIsMeta : DEFAULT_TERMINAL_OPTIONS.macOptionIsMeta,
    rightClickSelectsWord:
      typeof input.rightClickSelectsWord === "boolean"
        ? input.rightClickSelectsWord
        : DEFAULT_TERMINAL_OPTIONS.rightClickSelectsWord,
    drawBoldTextInBrightColors:
      typeof input.drawBoldTextInBrightColors === "boolean"
        ? input.drawBoldTextInBrightColors
        : DEFAULT_TERMINAL_OPTIONS.drawBoldTextInBrightColors,
    bellStyle: ensureBellStyle(input.bellStyle),
  };
}

export function getTerminalOptions(): TerminalOptionsState {
  const raw = localStorage.getItem(KEY);
  if (!raw) return DEFAULT_TERMINAL_OPTIONS;
  try {
    const parsed = JSON.parse(raw) as Partial<TerminalOptionsState>;
    return sanitizeTerminalOptions(parsed);
  } catch {
    return DEFAULT_TERMINAL_OPTIONS;
  }
}

export function setTerminalOptions(options: TerminalOptionsState) {
  const safe = sanitizeTerminalOptions(options);
  localStorage.setItem(KEY, JSON.stringify(safe));
}
