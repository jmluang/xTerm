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

  function applyTerminalTheme() {
    const term = terminalRefs.terminalInstance.current;
    if (!term) return;
    try {
      const appearance = resolvedTheme();
      const cssBackground = getComputedStyle(document.documentElement).getPropertyValue("--app-term-bg").trim();
      const background = cssBackground || (appearance === "dark" ? "#0b0f16" : "#ffffff");
      term.options.theme = getTerminalTheme(terminalThemeId, appearance, background);
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
      term.options.bellStyle = terminalOptions.bellStyle;
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

    const fitOk = safeFit(fit);
    if (!fitOk) return;

    if (!sessionId || !isInTauri) return;
    invoke("pty_resize", {
      sessionId,
      cols: term.cols,
      rows: term.rows,
    }).catch((error) => console.error("[pty] resize error", error));
  }

  useEffect(() => {
    terminalRefs.activeSessionIdRef.current = activeSessionId;
  }, [activeSessionId, terminalRefs.activeSessionIdRef]);

  useEffect(() => {
    applyTerminalTheme();
    applyTerminalOptions();
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
  }, [themeMode, terminalThemeId, terminalOptions]);

  useEffect(() => {
    requestAnimationFrame(() => fitAndResizeActivePty());
    window.setTimeout(() => fitAndResizeActivePty(), 120);
  }, [sidebarOpen]);

  useEffect(() => {
    if (!terminalReady) return;
    let loopCount = 0;
    const timerId = window.setInterval(() => {
      fitAndResizeActivePty();
      loopCount += 1;
      if (loopCount >= 12) window.clearInterval(timerId);
    }, 100);
    return () => window.clearInterval(timerId);
  }, [terminalReady, sidebarOpen, themeMode, terminalThemeId, terminalOptions]);

  useEffect(() => {
    if (!isInTauri) return;
    let unlisten: null | (() => void) = null;
    (async () => {
      try {
        const currentWindow = getCurrentWindow();
        unlisten = await currentWindow.onResized(() => {
          requestAnimationFrame(() => fitAndResizeActivePty());
          window.setTimeout(() => fitAndResizeActivePty(), 120);
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
  }, [isInTauri]);

  useEffect(() => {
    if (sessions.length === 0) return;
    if (activeSessionId && sessions.some((session) => session.id === activeSessionId)) return;
    setActiveSessionId(sessions[0].id);
  }, [sessions, activeSessionId, setActiveSessionId]);

  useEffect(() => {
    if (!terminalReady) return;
    if (sessions.length !== 0) return;
    terminalRefs.terminalInstance.current?.clearSelection();
    terminalRefs.terminalInstance.current?.blur();
    terminalRefs.terminalInstance.current?.clear();
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
      bellStyle: terminalOptions.bellStyle,
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
    applyTerminalTheme();
    applyTerminalOptions();

    const onData = term.onData((data) => {
      const sessionId = terminalRefs.activeSessionIdRef.current;
      if (!sessionId || !isInTauri) return;
      invoke("pty_write", { sessionId, data }).catch((error) => console.error("[pty] write error", error));
    });
    const onResize = () => fitAndResizeActivePty();
    window.addEventListener("resize", onResize);

    return () => {
      pendingDisposeTimerRef.current = window.setTimeout(() => {
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
        setTerminalReady(false);
        pendingDisposeTimerRef.current = null;
      }, 0);
    };
  }, [isInTauri]);

  useEffect(() => {
    const element = terminalRefs.terminalContainerRef.current ?? terminalRefs.terminalRef.current;
    if (!element) return;

    const observer = new ResizeObserver(() => {
      if (terminalRefs.resizeDebounceTimer.current) window.clearTimeout(terminalRefs.resizeDebounceTimer.current);
      terminalRefs.resizeDebounceTimer.current = window.setTimeout(() => {
        fitAndResizeActivePty();
      }, 50);
    });
    observer.observe(element);

    return () => {
      observer.disconnect();
      if (terminalRefs.resizeDebounceTimer.current) window.clearTimeout(terminalRefs.resizeDebounceTimer.current);
      terminalRefs.resizeDebounceTimer.current = null;
    };
  }, [isInTauri]);

  useEffect(() => {
    const term = terminalRefs.terminalInstance.current;
    if (!terminalReady || !term) return;
    const hasSession = sessions.length > 0;
    term.options.cursorBlink = hasSession && terminalOptions.cursorBlink;
    if (!hasSession) term.blur();
  }, [sessions.length, terminalReady, terminalRefs.terminalInstance, terminalOptions.cursorBlink]);

  useEffect(() => {
    if (!terminalReady) return;
    applyTerminalOptions();
    requestAnimationFrame(() => fitAndResizeActivePty());
  }, [terminalReady, terminalOptions]);

  useEffect(() => {
    const term = terminalRefs.terminalInstance.current;
    if (!terminalReady || !term) return;
    if (!activeSessionId) return;

    try {
      term.reset();
    } catch {
      return;
    }
    applyTerminalTheme();
    const buffer = runtimeRefs.sessionBuffers.current.get(activeSessionId);
    if (buffer) term.write(buffer);
    requestAnimationFrame(() => fitAndResizeActivePty());
    requestAnimationFrame(() => term.focus());
  }, [activeSessionId, runtimeRefs.sessionBuffers, terminalReady]);
}
