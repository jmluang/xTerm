import { useEffect, useRef } from "react";
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
  terminalRefs: Pick<TerminalRefs, "activeSessionIdRef" | "sessionTerminals">;
  runtimeRefs: SessionRuntimeRefs;
};

type PtyDataQueueState = {
  chunks: string[];
  queuedChars: number;
  rafId: number | null;
  writing: boolean;
  backpressureCompactions: number;
};

const PTY_DATA_BATCH_CHARS = 64_000;
const PTY_DATA_BACKPRESSURE_CHARS = 512_000;

export function usePtyEvents(params: UsePtyEventsParams) {
  const { isInTauri, setSessions, setActiveSessionId, setConnectingHosts, terminalRefs, runtimeRefs } = params;
  const {
    sessionBuffers,
    sessionHadAnyOutput,
    sessionConnectTimers,
    sessionMeta,
    sessionCloseReason,
  } = runtimeRefs;
  const ptyDataQueues = useRef(new Map<string, PtyDataQueueState>());

  function ensurePtyDataQueue(sessionId: string): PtyDataQueueState {
    let queue = ptyDataQueues.current.get(sessionId);
    if (!queue) {
      queue = {
        chunks: [],
        queuedChars: 0,
        rafId: null,
        writing: false,
        backpressureCompactions: 0,
      };
      ptyDataQueues.current.set(sessionId, queue);
    }
    return queue;
  }

  function compactPtyDataQueueForBackpressure(queue: PtyDataQueueState) {
    if (queue.queuedChars < PTY_DATA_BACKPRESSURE_CHARS) return;
    if (queue.chunks.length < 2) return;
    queue.chunks = [queue.chunks.join("")];
    queue.backpressureCompactions += 1;
  }

  function takePtyDataBatch(queue: PtyDataQueueState): string {
    let batch = "";
    while (queue.chunks.length > 0 && batch.length < PTY_DATA_BATCH_CHARS) {
      const chunk = queue.chunks[0] ?? "";
      const remaining = PTY_DATA_BATCH_CHARS - batch.length;
      if (chunk.length <= remaining || batch.length === 0) {
        batch += chunk;
        queue.chunks.shift();
        queue.queuedChars -= chunk.length;
        continue;
      }

      batch += chunk.slice(0, remaining);
      queue.chunks[0] = chunk.slice(remaining);
      queue.queuedChars -= remaining;
    }
    if (queue.queuedChars < 0) queue.queuedChars = 0;
    return batch;
  }

  function clearPtyDataQueue(sessionId: string) {
    const queue = ptyDataQueues.current.get(sessionId);
    if (!queue) return;
    if (queue.rafId !== null) {
      window.cancelAnimationFrame(queue.rafId);
      queue.rafId = null;
    }
    ptyDataQueues.current.delete(sessionId);
  }

  function flushPtyDataQueue(sessionId: string) {
    const queue = ptyDataQueues.current.get(sessionId);
    if (!queue || queue.writing) return;

    const handle = terminalRefs.sessionTerminals.current.get(sessionId);
    if (!handle) {
      const pending = queue.chunks.join("");
      clearPtyDataQueue(sessionId);
      appendSessionBuffer(sessionBuffers.current, sessionId, pending, MAX_SESSION_BUFFER_CHARS);
      return;
    }

    const batch = takePtyDataBatch(queue);
    if (!batch) {
      if (queue.queuedChars === 0) ptyDataQueues.current.delete(sessionId);
      return;
    }

    queue.writing = true;
    try {
      handle.terminal.write(batch, () => {
        queue.writing = false;
        if (queue.queuedChars > 0) {
          schedulePtyDataFlush(sessionId);
        } else {
          ptyDataQueues.current.delete(sessionId);
        }
      });
    } catch (error) {
      queue.writing = false;
      console.debug("[xterm] queued write skipped (pty:data)", error);
      if (queue.queuedChars > 0) schedulePtyDataFlush(sessionId);
    }
  }

  function schedulePtyDataFlush(sessionId: string) {
    const queue = ptyDataQueues.current.get(sessionId);
    if (!queue || queue.rafId !== null || queue.writing) return;
    queue.rafId = window.requestAnimationFrame(() => {
      queue.rafId = null;
      flushPtyDataQueue(sessionId);
    });
  }

  function enqueuePtyDataWrite(sessionId: string, data: string) {
    const handle = terminalRefs.sessionTerminals.current.get(sessionId);
    if (!handle) {
      appendSessionBuffer(sessionBuffers.current, sessionId, data, MAX_SESSION_BUFFER_CHARS);
      return;
    }

    const queue = ensurePtyDataQueue(sessionId);
    queue.chunks.push(data);
    queue.queuedChars += data.length;
    compactPtyDataQueueForBackpressure(queue);
    schedulePtyDataFlush(sessionId);
  }

  useEffect(() => {
    if (!isInTauri) return;

    const unlistenDataP = listen<{ session_id: string; data: string }>("pty:data", (event) => {
      const { session_id: sessionId, data } = event.payload;
      if (!data) return;

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

      enqueuePtyDataWrite(sessionId, data);
    });

    const unlistenExitP = listen<{ session_id: string; code: number }>("pty:exit", (event) => {
      const { session_id: sessionId, code: exitCode } = event.payload;
      const endedAt = Date.now();
      const reason = sessionCloseReason.current.get(sessionId) ?? "unknown";
      const shouldKeepFailedTab = reason === "timeout" || (exitCode > 0);

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
      clearPtyDataQueue(sessionId);
    });

    return () => {
      unlistenDataP.then((fn) => fn());
      unlistenExitP.then((fn) => fn());
      for (const sessionId of Array.from(ptyDataQueues.current.keys())) {
        clearPtyDataQueue(sessionId);
      }
    };
  }, [isInTauri, runtimeRefs, setActiveSessionId, setConnectingHosts, setSessions, terminalRefs]);
}
