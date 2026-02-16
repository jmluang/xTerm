import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getThemeMode, type ThemeMode } from "@/lib/theme";
import type { Session } from "@/types/models";
import type { SessionRuntimeRefs, SetActiveSessionId, TerminalRefs } from "@/hooks/terminal/types";

type UseTerminalRuntimeParams = {
  isInTauri: boolean;
  sidebarOpen: boolean;
  themeMode: ThemeMode;
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
  const { isInTauri, sidebarOpen, themeMode, sessions, activeSessionId, setActiveSessionId, terminalRefs, runtimeRefs } = params;
  const [terminalReady, setTerminalReady] = useState(false);
  const pendingDisposeTimerRef = useRef<number | null>(null);

  function applyTerminalTheme() {
    const term = terminalRefs.terminalInstance.current;
    if (!term) return;
    try {
      const theme = resolvedTheme();
      const cssBackground = getComputedStyle(document.documentElement).getPropertyValue("--app-term-bg").trim();
      const background = cssBackground || (theme === "dark" ? "#0b0f16" : "#ffffff");
      term.options.theme =
        theme === "dark"
          ? {
              background,
              foreground: "#e5e7eb",
              cursor: "#e5e7eb",
              selectionBackground: "rgba(148, 163, 184, 0.35)",
            }
          : {
              background,
              foreground: "#0b1220",
              cursor: "#0b1220",
              selectionBackground: "rgba(2, 132, 199, 0.22)",
            };
      term.refresh(0, term.rows - 1);
    } catch {
      // Ignore; renderer can be temporarily unavailable during init/dispose.
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
  }, [themeMode]);

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
  }, [terminalReady, sidebarOpen, themeMode]);

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
      fontSize: 14,
      fontFamily: "SF Mono, Menlo, Monaco, 'Courier New', monospace",
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
    term.options.cursorBlink = hasSession;
    if (!hasSession) term.blur();
  }, [sessions.length, terminalReady, terminalRefs.terminalInstance]);

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
