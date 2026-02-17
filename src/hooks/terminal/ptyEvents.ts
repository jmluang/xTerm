import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MAX_SESSION_BUFFER_CHARS } from "@/hooks/terminal/types";
import type { SessionRuntimeRefs, SetActiveSessionId, SetConnectingHosts, SetSessions, TerminalRefs } from "@/hooks/terminal/types";
import { appendSessionBuffer } from "@/hooks/terminal/sessionBuffer";
import { markFirstSessionOutput } from "@/lib/perfMetrics";

type UsePtyEventsParams = {
  isInTauri: boolean;
  setSessions: SetSessions;
  setActiveSessionId: SetActiveSessionId;
  setConnectingHosts: SetConnectingHosts;
  terminalRefs: Pick<TerminalRefs, "terminalInstance" | "activeSessionIdRef">;
  runtimeRefs: SessionRuntimeRefs;
};

export function usePtyEvents(params: UsePtyEventsParams) {
  const { isInTauri, setSessions, setActiveSessionId, setConnectingHosts, terminalRefs, runtimeRefs } = params;
  const {
    sessionBuffers,
    sessionAutoPasswords,
    sessionPromptTails,
    sessionAutoPasswordSent,
    sessionHadAnyOutput,
    sessionConnectTimers,
    sessionMeta,
    sessionCloseReason,
  } = runtimeRefs;

  useEffect(() => {
    if (!isInTauri) return;

    const unlistenDataP = listen<{ session_id: string; data: string }>("pty:data", (event) => {
      const { session_id: sessionId, data } = event.payload;
      if (!data) return;

      const tail = (sessionPromptTails.current.get(sessionId) ?? "") + data;
      sessionPromptTails.current.set(sessionId, tail.slice(Math.max(0, tail.length - 800)));

      if (!sessionHadAnyOutput.current.has(sessionId)) {
        sessionHadAnyOutput.current.add(sessionId);
        markFirstSessionOutput();
        setSessions((prev) =>
          prev.map((session) => (session.id === sessionId && session.status === "starting" ? { ...session, status: "running" } : session))
        );
        const timer = sessionConnectTimers.current.get(sessionId);
        if (timer) {
          window.clearTimeout(timer);
          sessionConnectTimers.current.delete(sessionId);
        }
        const meta = sessionMeta.current.get(sessionId);
        if (meta) {
          setConnectingHosts((prev) => {
            const current = prev[meta.hostId];
            if (!current) return prev;
            const nextCount = Math.max(0, (current.count ?? 1) - 1);
            if (nextCount === 0) {
              const next = { ...prev };
              delete next[meta.hostId];
              return next;
            }
            return { ...prev, [meta.hostId]: { ...current, count: nextCount } };
          });
        }
      }

      if (!sessionAutoPasswordSent.current.has(sessionId)) {
        const password = sessionAutoPasswords.current.get(sessionId);
        if (password) {
          const promptTail = sessionPromptTails.current.get(sessionId) ?? "";
          const looksLikePasswordPrompt =
            /(^|\n|\r)\s*(?:[A-Za-z0-9_.-]+@[^:\n\r]+(?:['’]s)?\s+)?password(?:\s+for\s+[^:\n\r]+)?\s*:\s*$/i.test(promptTail);
          if (looksLikePasswordPrompt) {
            sessionAutoPasswordSent.current.add(sessionId);
            invoke("pty_write", { sessionId, data: `${password}\n` }).catch((error) =>
              console.error("[pty] auto password write error", error)
            );
          }
        }
      }

      appendSessionBuffer(sessionBuffers.current, sessionId, data, MAX_SESSION_BUFFER_CHARS);

      if (terminalRefs.activeSessionIdRef.current === sessionId && terminalRefs.terminalInstance.current) {
        terminalRefs.terminalInstance.current.write(data);
      }
    });

    const unlistenExitP = listen<{ session_id: string; code: number }>("pty:exit", (event) => {
      const { session_id: sessionId, code: exitCode } = event.payload;
      const endedAt = Date.now();
      const reason = sessionCloseReason.current.get(sessionId) ?? "unknown";
      const shouldKeepFailedTab = reason === "timeout" || (reason === "unknown" && exitCode !== 0);

      if (shouldKeepFailedTab) {
        setSessions((prev) =>
          prev.map((session) => (session.id === sessionId ? { ...session, status: "exited", exitCode, endedAt } : session))
        );
      } else {
        setSessions((prev) => prev.filter((session) => session.id !== sessionId));
        if (terminalRefs.activeSessionIdRef.current === sessionId) {
          setActiveSessionId(null);
        }
      }

      const meta = sessionMeta.current.get(sessionId);
      sessionMeta.current.delete(sessionId);
      sessionCloseReason.current.delete(sessionId);
      sessionHadAnyOutput.current.delete(sessionId);
      const timer = sessionConnectTimers.current.get(sessionId);
      if (timer) {
        window.clearTimeout(timer);
        sessionConnectTimers.current.delete(sessionId);
      }

      if (meta) {
        setConnectingHosts((prev) => {
          const current = prev[meta.hostId];
          if (!current) return prev;
          const nextCount = Math.max(0, (current.count ?? 1) - 1);
          if (nextCount === 0) {
            const next = { ...prev };
            delete next[meta.hostId];
            return next;
          }
          return { ...prev, [meta.hostId]: { ...current, count: nextCount } };
        });
      }

      if (!shouldKeepFailedTab) sessionBuffers.current.delete(sessionId);
      sessionPromptTails.current.delete(sessionId);
      sessionAutoPasswords.current.delete(sessionId);
      sessionAutoPasswordSent.current.delete(sessionId);
    });

    return () => {
      unlistenDataP.then((fn) => fn());
      unlistenExitP.then((fn) => fn());
    };
  }, [isInTauri, runtimeRefs, setActiveSessionId, setConnectingHosts, setSessions, terminalRefs]);
}
