export type PerfSnapshot = {
  appBootMs: number;
  firstTerminalReadyMs: number | null;
  firstSessionOutputMs: number | null;
  tabSwitchSamples: number[];
  avgTabSwitchMs: number | null;
  maxTabSwitchMs: number | null;
  resizeSignalCount: number;
  fitCallCount: number;
  ptyResizeSentCount: number;
  resizeJitterCount: number;
  memorySamples: Array<{ sessionCount: number; usedJsHeapBytes: number }>;
  latestPtySize: { cols: number; rows: number } | null;
};

type PerfState = {
  bootAt: number;
  firstTerminalReadyMs: number | null;
  firstSessionOutputMs: number | null;
  tabSwitchSamples: number[];
  resizeSignalCount: number;
  fitCallCount: number;
  ptyResizeSentCount: number;
  resizeJitterCount: number;
  resizeSignalTimes: number[];
  memorySamples: Array<{ sessionCount: number; usedJsHeapBytes: number }>;
  latestPtySize: { cols: number; rows: number } | null;
};

type PerfWindow = Window & {
  __xtermiusPerf?: {
    snapshot: () => PerfSnapshot;
    reset: () => void;
  };
};

const MAX_TAB_SWITCH_SAMPLES = 100;
const MAX_MEMORY_SAMPLES = 50;
const MAX_RESIZE_SIGNAL_WINDOW_MS = 250;
const RESIZE_JITTER_BURST_THRESHOLD = 5;

const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

const state: PerfState = {
  bootAt: now(),
  firstTerminalReadyMs: null,
  firstSessionOutputMs: null,
  tabSwitchSamples: [],
  resizeSignalCount: 0,
  fitCallCount: 0,
  ptyResizeSentCount: 0,
  resizeJitterCount: 0,
  resizeSignalTimes: [],
  memorySamples: [],
  latestPtySize: null,
};

function cloneSnapshot(): PerfSnapshot {
  const tabSwitchSamples = state.tabSwitchSamples.slice();
  const avgTabSwitchMs =
    tabSwitchSamples.length > 0
      ? tabSwitchSamples.reduce((acc, value) => acc + value, 0) / tabSwitchSamples.length
      : null;
  const maxTabSwitchMs = tabSwitchSamples.length > 0 ? Math.max(...tabSwitchSamples) : null;

  return {
    appBootMs: Math.round(now() - state.bootAt),
    firstTerminalReadyMs: state.firstTerminalReadyMs,
    firstSessionOutputMs: state.firstSessionOutputMs,
    tabSwitchSamples,
    avgTabSwitchMs,
    maxTabSwitchMs,
    resizeSignalCount: state.resizeSignalCount,
    fitCallCount: state.fitCallCount,
    ptyResizeSentCount: state.ptyResizeSentCount,
    resizeJitterCount: state.resizeJitterCount,
    memorySamples: state.memorySamples.slice(),
    latestPtySize: state.latestPtySize,
  };
}

function pushLimited<T>(arr: T[], value: T, max: number) {
  arr.push(value);
  if (arr.length > max) {
    arr.splice(0, arr.length - max);
  }
}

function installInspector() {
  if (typeof window === "undefined") return;
  const win = window as PerfWindow;
  if (win.__xtermiusPerf) return;
  win.__xtermiusPerf = {
    snapshot: cloneSnapshot,
    reset: () => {
      state.bootAt = now();
      state.firstTerminalReadyMs = null;
      state.firstSessionOutputMs = null;
      state.tabSwitchSamples = [];
      state.resizeSignalCount = 0;
      state.fitCallCount = 0;
      state.ptyResizeSentCount = 0;
      state.resizeJitterCount = 0;
      state.resizeSignalTimes = [];
      state.memorySamples = [];
      state.latestPtySize = null;
    },
  };
}

export function initPerfMetrics() {
  installInspector();
}

export function markFirstTerminalReady() {
  if (state.firstTerminalReadyMs !== null) return;
  state.firstTerminalReadyMs = Math.round(now() - state.bootAt);
}

export function markFirstSessionOutput() {
  if (state.firstSessionOutputMs !== null) return;
  state.firstSessionOutputMs = Math.round(now() - state.bootAt);
}

export function recordTabSwitchLatency(ms: number) {
  if (!Number.isFinite(ms) || ms < 0) return;
  pushLimited(state.tabSwitchSamples, Math.round(ms * 100) / 100, MAX_TAB_SWITCH_SAMPLES);
}

export function recordResizeSignal() {
  const t = now();
  state.resizeSignalCount += 1;
  pushLimited(state.resizeSignalTimes, t, 50);

  const active = state.resizeSignalTimes.filter((v) => t - v <= MAX_RESIZE_SIGNAL_WINDOW_MS);
  state.resizeSignalTimes = active;
  if (active.length >= RESIZE_JITTER_BURST_THRESHOLD) {
    state.resizeJitterCount += 1;
    state.resizeSignalTimes = [];
  }
}

export function recordFitCall() {
  state.fitCallCount += 1;
}

export function recordPtyResize(cols: number, rows: number) {
  state.ptyResizeSentCount += 1;
  state.latestPtySize = { cols, rows };
}

export function sampleMemory(sessionCount: number) {
  if (typeof performance === "undefined") return;
  const perf = performance as Performance & {
    memory?: {
      usedJSHeapSize?: number;
    };
  };
  const usedJsHeapBytes = perf.memory?.usedJSHeapSize;
  if (typeof usedJsHeapBytes !== "number") return;
  pushLimited(state.memorySamples, { sessionCount, usedJsHeapBytes }, MAX_MEMORY_SAMPLES);
}

export function getPerfSnapshot(): PerfSnapshot {
  return cloneSnapshot();
}
