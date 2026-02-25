import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getThemeMode, type ThemeMode } from "@/lib/theme";
import { getTerminalTheme, type TerminalThemeId } from "@/lib/terminalTheme";
import type { TerminalOptionsState } from "@/lib/terminalOptions";
import type { Session } from "@/types/models";
import type { SessionRuntimeRefs, SetActiveSessionId, TerminalRefs } from "@/hooks/terminal/types";
import { readSessionBuffer } from "@/hooks/terminal/sessionBuffer";
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
  const [terminalReady, setTerminalReady] = useState(false);
  const pendingDisposeTimerRef = useRef<number | null>(null);
  const fitTimerRef = useRef<number | null>(null);
  const fitRafRef = useRef<number | null>(null);
  const lastSentPtySizeRef = useRef<PtySizeState | null>(null);

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

  function applyTerminalTheme() {
    const term = terminalRefs.terminalInstance.current;
    if (!term) return;
    try {
      const appearance = themeMode === "light" || themeMode === "dark" ? themeMode : resolvedTheme();
      const cssBackground = getComputedStyle(document.documentElement).getPropertyValue("--app-term-bg").trim();
      const background = cssBackground || (appearance === "dark" ? "#0b0f16" : "#ffffff");
      const nextTheme = getTerminalTheme(terminalThemeId, appearance, background);
      term.options.theme = nextTheme;
      const surfaceBg = nextTheme.background || background;
      terminalRefs.terminalContainerRef.current?.style.setProperty("--xterm-surface-bg", surfaceBg);
      terminalRefs.terminalRef.current?.style.setProperty("--xterm-surface-bg", surfaceBg);
      term.refresh(0, term.rows - 1);
    } catch {
      // Ignore; renderer can be temporarily unavailable during init/dispose.
    }
  }

  function applyTerminalOptions() {
    const term = terminalRefs.terminalInstance.current;
    if (!term) return;
    try {
      term.options.fontFamily = terminalOptions.fontFamily;
      term.options.fontSize = terminalOptions.fontSize;
      term.options.lineHeight = terminalOptions.lineHeight;
      term.options.letterSpacing = terminalOptions.letterSpacing;
      term.options.cursorStyle = terminalOptions.cursorStyle;
      term.options.scrollback = terminalOptions.scrollback;
      term.options.macOptionIsMeta = terminalOptions.macOptionIsMeta;
      term.options.rightClickSelectsWord = terminalOptions.rightClickSelectsWord;
      term.options.drawBoldTextInBrightColors = terminalOptions.drawBoldTextInBrightColors;
      term.options.cursorBlink = sessions.length > 0 && terminalOptions.cursorBlink;
      term.refresh(0, term.rows - 1);
    } catch {
      // Ignore option updates during init/dispose races.
    }
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

  function fitAndResizeActivePty() {
    const term = terminalRefs.terminalInstance.current;
    const fit = terminalRefs.fitAddon.current;
    const sessionId = terminalRefs.activeSessionIdRef.current;
    if (!term || !fit) return;

    recordFitCall();
    const fitOk = safeFit(fit);
    if (!fitOk) return;

    if (!sessionId || !isInTauri) return;
    const nextCols = term.cols;
    const nextRows = term.rows;
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

  useEffect(() => {
    terminalRefs.activeSessionIdRef.current = activeSessionId;
    lastSentPtySizeRef.current = null;
  }, [activeSessionId, terminalRefs.activeSessionIdRef]);

  useEffect(() => {
    applyTerminalTheme();
    const raf = window.requestAnimationFrame(() => {
      applyTerminalTheme();
    });
    return () => window.cancelAnimationFrame(raf);
  }, [themeMode, terminalThemeId]);

  useEffect(() => {
    applyTerminalOptions();
    if (!terminalReady) return;
    scheduleFitAndResize(20);
  }, [terminalOptions, sessions.length, terminalReady]);

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
    if (!terminalReady) return;
    scheduleFitAndResize(20);
  }, [sidebarOpen, terminalReady]);

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
  }, [isInTauri, terminalReady]);

  useEffect(() => {
    if (sessions.length === 0) return;
    if (activeSessionId && sessions.some((session) => session.id === activeSessionId)) return;
    setActiveSessionId(sessions[0].id);
  }, [sessions, activeSessionId, setActiveSessionId]);

  useEffect(() => {
    sampleMemory(sessions.length);
  }, [sessions.length]);

  useEffect(() => {
    if (!terminalReady) return;
    if (sessions.length !== 0) return;
    const term = terminalRefs.terminalInstance.current;
    if (!term) return;
    term.clearSelection();
    term.blur();
    // Reset emulator state when the last session closes so DEC modes/focus tracking
    // from a prior session don't leak into the next SSH connection.
    term.reset();
    applyTerminalTheme();
    applyTerminalOptions();
  }, [sessions.length, terminalReady, terminalRefs.terminalInstance]);

  useEffect(() => {
    if (pendingDisposeTimerRef.current) {
      window.clearTimeout(pendingDisposeTimerRef.current);
      pendingDisposeTimerRef.current = null;
    }
    if (!terminalRefs.terminalRef.current || terminalRefs.terminalInstance.current) return;

    const term = new Terminal({
      cursorBlink: false,
      cursorInactiveStyle: "none",
      cursorStyle: terminalOptions.cursorStyle,
      fontSize: terminalOptions.fontSize,
      fontFamily: terminalOptions.fontFamily,
      lineHeight: terminalOptions.lineHeight,
      letterSpacing: terminalOptions.letterSpacing,
      scrollback: terminalOptions.scrollback,
      macOptionIsMeta: terminalOptions.macOptionIsMeta,
      rightClickSelectsWord: terminalOptions.rightClickSelectsWord,
      drawBoldTextInBrightColors: terminalOptions.drawBoldTextInBrightColors,
      theme: { background: "transparent", foreground: "#e5e7eb" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(terminalRefs.terminalRef.current);
    safeFit(fit);
    terminalRefs.terminalInstance.current = term;
    terminalRefs.fitAddon.current = fit;
    setTerminalReady(true);
    markFirstTerminalReady();
    applyTerminalTheme();
    applyTerminalOptions();
    scheduleFitAndResize(0);

    const onData = term.onData((data) => {
      const sessionId = terminalRefs.activeSessionIdRef.current;
      if (!sessionId || !isInTauri) return;
      invoke("pty_write", { sessionId, data }).catch((error) => console.error("[pty] write error", error));
    });
    const onResize = () => scheduleFitAndResize(30);
    window.addEventListener("resize", onResize);

    return () => {
      pendingDisposeTimerRef.current = window.setTimeout(() => {
        clearScheduledFit();
        onData.dispose();
        window.removeEventListener("resize", onResize);
        try {
          term.dispose();
        } catch {
          // ignore
        }
        if (terminalRefs.terminalInstance.current === term) {
          terminalRefs.terminalInstance.current = null;
          terminalRefs.fitAddon.current = null;
        }
        lastSentPtySizeRef.current = null;
        setTerminalReady(false);
        pendingDisposeTimerRef.current = null;
      }, 0);
    };
  }, [isInTauri]);

  useEffect(() => {
    const element = terminalRefs.terminalContainerRef.current ?? terminalRefs.terminalRef.current;
    if (!element) return;

    const observer = new ResizeObserver(() => {
      scheduleFitAndResize(50);
    });
    observer.observe(element);

    return () => {
      observer.disconnect();
    };
  }, [isInTauri, terminalReady]);

  useEffect(() => {
    const term = terminalRefs.terminalInstance.current;
    if (!terminalReady || !term) return;
    if (!activeSessionId) return;
    let cancelled = false;

    const raf = window.requestAnimationFrame(() => {
      if (cancelled) return;
      const start = performance.now();
      try {
        // `clear()` preserves the current prompt line and terminal modes in xterm.js.
        // We need a full reset when reusing one xterm instance across sessions,
        // otherwise prompt text and DEC modes (eg focus tracking) can leak.
        term.reset();
      } catch {
        return;
      }

      applyTerminalTheme();
      applyTerminalOptions();
      const text = readSessionBuffer(runtimeRefs.sessionBuffers.current, activeSessionId);

      const finishSwitch = () => {
        if (cancelled) return;
        scheduleFitAndResize(0);
        window.requestAnimationFrame(() => {
          if (cancelled) return;
          try {
            term.focus();
          } catch (error) {
            console.debug("[xterm] focus skipped after session switch", error);
          }
          recordTabSwitchLatency(performance.now() - start);
        });
      };

      try {
        if (text) {
          term.write(text, finishSwitch);
        } else {
          finishSwitch();
        }
      } catch (error) {
        console.debug("[xterm] write skipped after session switch", error);
      }
    });

    return () => {
      cancelled = true;
      window.cancelAnimationFrame(raf);
    };
  }, [activeSessionId, runtimeRefs.sessionBuffers, terminalReady]);
}
