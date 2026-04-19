import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getThemeMode, type ThemeMode } from "@/lib/theme";
import { getTerminalTheme, type TerminalThemeId } from "@/lib/terminalTheme";
import type { TerminalOptionsState } from "@/lib/terminalOptions";
import type { Session } from "@/types/models";
import type { SessionRuntimeRefs, SetActiveSessionId, TerminalRefs } from "@/hooks/terminal/types";
import { takeSessionBuffer } from "@/hooks/terminal/sessionBuffer";
import {
  markFirstTerminalReady,
  recordFitCall,
  recordPtyResize,
  recordResizeSignal,
  recordTabSwitchLatency,
  sampleMemory,
} from "@/lib/perfMetrics";

type UseTerminalRuntimeParams = {
  isInTauri: boolean;
  sidebarOpen: boolean;
  themeMode: ThemeMode;
  terminalThemeId: TerminalThemeId;
  terminalOptions: TerminalOptionsState;
  sessions: Session[];
  activeSessionId: string | null;
  setActiveSessionId: SetActiveSessionId;
  terminalRefs: TerminalRefs;
  runtimeRefs: Pick<SessionRuntimeRefs, "sessionBuffers">;
};

type PtySizeState = {
  sessionId: string;
  cols: number;
  rows: number;
};

function resolvedTheme(): "light" | "dark" {
  const datasetTheme = document.documentElement.dataset.theme;
  if (datasetTheme === "light" || datasetTheme === "dark") return datasetTheme;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function useTerminalRuntime(params: UseTerminalRuntimeParams) {
  const {
    isInTauri,
    sidebarOpen,
    themeMode,
    terminalThemeId,
    terminalOptions,
    sessions,
    activeSessionId,
    setActiveSessionId,
    terminalRefs,
    runtimeRefs,
  } = params;

  const fitTimerRef = useRef<number | null>(null);
  const fitRafRef = useRef<number | null>(null);
  const lastSentPtySizeRef = useRef<PtySizeState | null>(null);
  const bindSessionTerminalRefCache = useRef(new Map<string, (node: HTMLDivElement | null) => void>());
  const didMarkFirstTerminalReadyRef = useRef(false);

  const isInTauriRef = useRef(isInTauri);
  const themeModeRef = useRef(themeMode);
  const terminalThemeIdRef = useRef(terminalThemeId);
  const terminalOptionsRef = useRef(terminalOptions);
  const sessionsCountRef = useRef(sessions.length);

  useEffect(() => {
    isInTauriRef.current = isInTauri;
  }, [isInTauri]);

  useEffect(() => {
    themeModeRef.current = themeMode;
  }, [themeMode]);

  useEffect(() => {
    terminalThemeIdRef.current = terminalThemeId;
  }, [terminalThemeId]);

  useEffect(() => {
    terminalOptionsRef.current = terminalOptions;
  }, [terminalOptions]);

  useEffect(() => {
    sessionsCountRef.current = sessions.length;
  }, [sessions.length]);

  function clearScheduledFit() {
    if (fitTimerRef.current !== null) {
      window.clearTimeout(fitTimerRef.current);
      fitTimerRef.current = null;
    }
    if (fitRafRef.current !== null) {
      window.cancelAnimationFrame(fitRafRef.current);
      fitRafRef.current = null;
    }
  }

  function syncActiveTerminalHandle() {
    const sessionId = terminalRefs.activeSessionIdRef.current;
    const handle = sessionId ? terminalRefs.sessionTerminals.current.get(sessionId) ?? null : null;
    terminalRefs.terminalInstance.current = handle?.terminal ?? null;
    terminalRefs.fitAddon.current = handle?.fitAddon ?? null;
  }

  function safeFit(fit: FitAddon): boolean {
    try {
      fit.fit();
      return true;
    } catch (error) {
      console.debug("[xterm] fit skipped", error);
      return false;
    }
  }

  function applyTerminalTheme(sessionId?: string) {
    const appearance =
      themeModeRef.current === "light" || themeModeRef.current === "dark" ? themeModeRef.current : resolvedTheme();
    const cssBackground = getComputedStyle(document.documentElement).getPropertyValue("--app-term-bg").trim();
    const background = cssBackground || (appearance === "dark" ? "#0b0f16" : "#ffffff");
    const nextTheme = getTerminalTheme(terminalThemeIdRef.current, appearance, background);
    const surfaceBg = nextTheme.background || background;
    terminalRefs.terminalContainerRef.current?.style.setProperty("--xterm-surface-bg", surfaceBg);

    const sessionIds = sessionId ? [sessionId] : Array.from(terminalRefs.sessionTerminals.current.keys());
    for (const id of sessionIds) {
      const handle = terminalRefs.sessionTerminals.current.get(id);
      if (!handle) continue;
      try {
        handle.terminal.options.theme = nextTheme;
        terminalRefs.sessionViewportRefs.current.get(id)?.style.setProperty("--xterm-surface-bg", surfaceBg);
        handle.terminal.refresh(0, handle.terminal.rows - 1);
      } catch {
        // Ignore theme updates during init/dispose races.
      }
    }
  }

  function applyTerminalOptions(sessionId?: string) {
    const options = terminalOptionsRef.current;
    const sessionIds = sessionId ? [sessionId] : Array.from(terminalRefs.sessionTerminals.current.keys());
    for (const id of sessionIds) {
      const handle = terminalRefs.sessionTerminals.current.get(id);
      if (!handle) continue;
      try {
        handle.terminal.options.fontFamily = options.fontFamily;
        handle.terminal.options.fontSize = options.fontSize;
        handle.terminal.options.lineHeight = options.lineHeight;
        handle.terminal.options.letterSpacing = options.letterSpacing;
        handle.terminal.options.cursorStyle = options.cursorStyle;
        handle.terminal.options.scrollback = options.scrollback;
        handle.terminal.options.macOptionIsMeta = options.macOptionIsMeta;
        handle.terminal.options.rightClickSelectsWord = options.rightClickSelectsWord;
        handle.terminal.options.drawBoldTextInBrightColors = options.drawBoldTextInBrightColors;
        handle.terminal.options.cursorBlink =
          id === terminalRefs.activeSessionIdRef.current && sessionsCountRef.current > 0 && options.cursorBlink;
        handle.terminal.refresh(0, handle.terminal.rows - 1);
      } catch {
        // Ignore option updates during init/dispose races.
      }
    }
  }

  function fitAndResizeActivePty() {
    const sessionId = terminalRefs.activeSessionIdRef.current;
    const handle = sessionId ? terminalRefs.sessionTerminals.current.get(sessionId) : null;
    if (!sessionId || !handle) return;

    recordFitCall();
    const fitOk = safeFit(handle.fitAddon);
    if (!fitOk) return;

    if (!isInTauriRef.current) return;
    const nextCols = handle.terminal.cols;
    const nextRows = handle.terminal.rows;
    const prev = lastSentPtySizeRef.current;
    if (prev && prev.sessionId === sessionId && prev.cols === nextCols && prev.rows === nextRows) return;

    lastSentPtySizeRef.current = { sessionId, cols: nextCols, rows: nextRows };
    recordPtyResize(nextCols, nextRows);
    invoke("pty_resize", {
      sessionId,
      cols: nextCols,
      rows: nextRows,
    }).catch((error) => {
      console.error("[pty] resize error", error);
      lastSentPtySizeRef.current = null;
    });
  }

  function scheduleFitAndResize(delayMs = 50) {
    recordResizeSignal();
    if (fitTimerRef.current !== null) {
      window.clearTimeout(fitTimerRef.current);
    }
    fitTimerRef.current = window.setTimeout(() => {
      fitTimerRef.current = null;
      if (fitRafRef.current !== null) {
        window.cancelAnimationFrame(fitRafRef.current);
      }
      fitRafRef.current = window.requestAnimationFrame(() => {
        fitRafRef.current = null;
        fitAndResizeActivePty();
      });
    }, Math.max(0, delayMs));
  }

  function createSessionTerminal(sessionId: string, node: HTMLDivElement) {
    if (terminalRefs.sessionTerminals.current.has(sessionId)) return;

    const options = terminalOptionsRef.current;
    const term = new Terminal({
      cursorBlink: sessionId === terminalRefs.activeSessionIdRef.current && sessionsCountRef.current > 0 && options.cursorBlink,
      cursorInactiveStyle: "none",
      cursorStyle: options.cursorStyle,
      fontSize: options.fontSize,
      fontFamily: options.fontFamily,
      lineHeight: options.lineHeight,
      letterSpacing: options.letterSpacing,
      scrollback: options.scrollback,
      macOptionIsMeta: options.macOptionIsMeta,
      rightClickSelectsWord: options.rightClickSelectsWord,
      drawBoldTextInBrightColors: options.drawBoldTextInBrightColors,
      theme: { background: "transparent", foreground: "#e5e7eb" },
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(node);
    if (term.element) {
      term.element.style.height = "100%";
      term.element.style.boxSizing = "border-box";
      term.element.style.padding = "0.5rem 0.75rem 0.75rem";
    }

    const inputDisposable = term.onData((data) => {
      if (!isInTauriRef.current) return;
      invoke("pty_write", { sessionId, data }).catch((error) => console.error("[pty] write error", error));
    });

    terminalRefs.sessionTerminals.current.set(sessionId, { terminal: term, fitAddon, inputDisposable });
    syncActiveTerminalHandle();

    if (!didMarkFirstTerminalReadyRef.current) {
      didMarkFirstTerminalReadyRef.current = true;
      markFirstTerminalReady();
    }

    applyTerminalTheme(sessionId);
    applyTerminalOptions(sessionId);
    safeFit(fitAddon);

    const pendingText = takeSessionBuffer(runtimeRefs.sessionBuffers.current, sessionId);
    if (pendingText) {
      try {
        term.write(pendingText, () => {
          if (terminalRefs.activeSessionIdRef.current === sessionId) scheduleFitAndResize(0);
        });
      } catch (error) {
        console.debug("[xterm] pending buffer flush skipped", error);
      }
    } else if (terminalRefs.activeSessionIdRef.current === sessionId) {
      scheduleFitAndResize(0);
    }
  }

  function disposeSessionTerminal(sessionId: string) {
    const handle = terminalRefs.sessionTerminals.current.get(sessionId);
    terminalRefs.sessionViewportRefs.current.delete(sessionId);
    bindSessionTerminalRefCache.current.delete(sessionId);
    if (!handle) {
      syncActiveTerminalHandle();
      return;
    }

    try {
      handle.terminal.clearSelection();
      handle.terminal.blur();
    } catch {
      // ignore
    }

    handle.inputDisposable.dispose();
    try {
      handle.terminal.dispose();
    } catch {
      // ignore
    }

    terminalRefs.sessionTerminals.current.delete(sessionId);
    if (terminalRefs.activeSessionIdRef.current === sessionId) {
      lastSentPtySizeRef.current = null;
    }
    syncActiveTerminalHandle();
  }

  function bindSessionTerminalRef(sessionId: string) {
    const cached = bindSessionTerminalRefCache.current.get(sessionId);
    if (cached) return cached;

    const callback = (node: HTMLDivElement | null) => {
      if (node) {
        terminalRefs.sessionViewportRefs.current.set(sessionId, node);
        createSessionTerminal(sessionId, node);
      } else {
        disposeSessionTerminal(sessionId);
      }
    };

    bindSessionTerminalRefCache.current.set(sessionId, callback);
    return callback;
  }

  useEffect(() => {
    terminalRefs.activeSessionIdRef.current = activeSessionId;
    lastSentPtySizeRef.current = null;
    syncActiveTerminalHandle();
    applyTerminalOptions();
  }, [activeSessionId]);

  useEffect(() => {
    const knownIds = new Set(sessions.map((session) => session.id));
    for (const sessionId of Array.from(terminalRefs.sessionTerminals.current.keys())) {
      if (!knownIds.has(sessionId)) {
        disposeSessionTerminal(sessionId);
      }
    }
  }, [sessions]);

  useEffect(() => {
    if (sessions.length === 0) return;
    if (activeSessionId && sessions.some((session) => session.id === activeSessionId)) return;
    setActiveSessionId(sessions[0].id);
  }, [sessions, activeSessionId, setActiveSessionId]);

  useEffect(() => {
    sampleMemory(sessions.length);
  }, [sessions.length]);

  useEffect(() => {
    applyTerminalTheme();
    const raf = window.requestAnimationFrame(() => {
      applyTerminalTheme();
    });
    return () => window.cancelAnimationFrame(raf);
  }, [themeMode, terminalThemeId]);

  useEffect(() => {
    applyTerminalOptions();
    if (!activeSessionId) return;
    scheduleFitAndResize(20);
  }, [terminalOptions, sessions.length, activeSessionId]);

  useEffect(() => {
    if (!window.matchMedia) return;
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      if (getThemeMode() === "system") applyTerminalTheme();
    };
    if (typeof mediaQuery.addEventListener === "function") mediaQuery.addEventListener("change", onChange);
    else if (typeof (mediaQuery as any).addListener === "function") (mediaQuery as any).addListener(onChange);
    return () => {
      if (typeof mediaQuery.removeEventListener === "function") mediaQuery.removeEventListener("change", onChange);
      else if (typeof (mediaQuery as any).removeListener === "function") (mediaQuery as any).removeListener(onChange);
    };
  }, []);

  useEffect(() => {
    if (!activeSessionId) return;
    scheduleFitAndResize(20);
  }, [sidebarOpen, activeSessionId]);

  useEffect(() => {
    if (!isInTauri) return;
    let unlisten: null | (() => void) = null;
    (async () => {
      try {
        const currentWindow = getCurrentWindow();
        unlisten = await currentWindow.onResized(() => {
          scheduleFitAndResize(30);
        });
      } catch (error) {
        console.debug("[ui] window.onResized unavailable", error);
      }
    })();
    return () => {
      try {
        unlisten?.();
      } catch {
        // ignore
      }
    };
  }, [isInTauri, activeSessionId]);

  useEffect(() => {
    const element = terminalRefs.terminalContainerRef.current;
    if (!element) return;

    const observer = new ResizeObserver(() => {
      scheduleFitAndResize(50);
    });
    observer.observe(element);

    return () => {
      observer.disconnect();
    };
  }, [isInTauri, activeSessionId]);

  useEffect(() => {
    if (!activeSessionId) {
      syncActiveTerminalHandle();
      return;
    }

    const start = performance.now();
    const raf = window.requestAnimationFrame(() => {
      syncActiveTerminalHandle();
      applyTerminalOptions();
      scheduleFitAndResize(0);
      window.requestAnimationFrame(() => {
        try {
          terminalRefs.terminalInstance.current?.focus();
        } catch (error) {
          console.debug("[xterm] focus skipped after session switch", error);
        }
        recordTabSwitchLatency(performance.now() - start);
      });
    });

    return () => {
      window.cancelAnimationFrame(raf);
    };
  }, [activeSessionId]);

  useEffect(() => {
    return () => {
      clearScheduledFit();
      for (const sessionId of Array.from(terminalRefs.sessionTerminals.current.keys())) {
        disposeSessionTerminal(sessionId);
      }
    };
  }, []);

  return {
    bindSessionTerminalRef,
  };
}
