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
  chunkStart: number;
  queuedChars: number;
  rafId: number | null;
  timerId: number | null;
  writing: boolean;
};

const PTY_DATA_BATCH_CHARS = 64_000;
// rAF stalls while the window is occluded/minimized (WKWebView pauses it), so
// flushes are also armed with a timer. Without it, PTY output sits unparsed
// until the window is visible again, and xterm's automatic replies to terminal
// queries (cursor-position/device-attribute reports sent by shell prompts) go
// out seconds late — the remote shell then echoes them as garbage like
// `12;1R` after the cursor.
const PTY_DATA_FLUSH_FALLBACK_MS = 50;

export function usePtyEvents(params: UsePtyEventsParams) {
  const { isInTauri, setSessions, setActiveSessionId, setConnectingHosts, terminalRefs, runtimeRefs } = params;
  const {
    sessionBuffers,
    sessionHadAnyOutput,
    sessionConnectTimers,
    sessionMeta,
    sessionConnectingCounted,
    sessionCloseReason,
  } = runtimeRefs;
  const ptyDataQueues = useRef(new Map<string, PtyDataQueueState>());

  function ensurePtyDataQueue(sessionId: string): PtyDataQueueState {
    let queue = ptyDataQueues.current.get(sessionId);
    if (!queue) {
      queue = {
        chunks: [],
        chunkStart: 0,
        queuedChars: 0,
        rafId: null,
        timerId: null,
        writing: false,
      };
      ptyDataQueues.current.set(sessionId, queue);
    }
    return queue;
  }

  function compactConsumedPtyDataChunks(queue: PtyDataQueueState) {
    if (queue.chunkStart === 0) return;
    if (queue.chunkStart >= queue.chunks.length) {
      queue.chunks = [];
      queue.chunkStart = 0;
      return;
    }
    if (queue.chunkStart < 256 || queue.chunkStart * 2 < queue.chunks.length) return;
    queue.chunks = queue.chunks.slice(queue.chunkStart);
    queue.chunkStart = 0;
  }

  function isHighSurrogate(code: number) {
    return code >= 0xd800 && code <= 0xdbff;
  }

  function isLowSurrogate(code: number) {
    return code >= 0xdc00 && code <= 0xdfff;
  }

  function takeUtf16SafePrefixLength(chunk: string, maxChars: number) {
    let length = Math.min(chunk.length, maxChars);
    // Do not cut a surrogate pair that is present in one PTY event. xterm's
    // streaming decoder preserves a pair that happens to cross two writes.
    if (
      length > 0 &&
      length < chunk.length &&
      isHighSurrogate(chunk.charCodeAt(length - 1)) &&
      isLowSurrogate(chunk.charCodeAt(length))
    ) {
      length -= 1;
    }
    return length;
  }

  function takePtyDataBatch(queue: PtyDataQueueState): string {
    const batchChunks: string[] = [];
    let batchChars = 0;
    while (queue.chunkStart < queue.chunks.length && batchChars < PTY_DATA_BATCH_CHARS) {
      const chunk = queue.chunks[queue.chunkStart] ?? "";
      const remaining = PTY_DATA_BATCH_CHARS - batchChars;
      const takeLength = takeUtf16SafePrefixLength(chunk, remaining);

      if (takeLength === 0) {
        break;
      }

      batchChunks.push(chunk.slice(0, takeLength));
      batchChars += takeLength;
      queue.queuedChars -= takeLength;
      if (takeLength === chunk.length) {
        queue.chunkStart += 1;
      } else {
        queue.chunks[queue.chunkStart] = chunk.slice(takeLength);
      }
    }
    if (queue.queuedChars < 0) queue.queuedChars = 0;
    compactConsumedPtyDataChunks(queue);
    return batchChunks.join("");
  }

  function cancelScheduledFlush(queue: PtyDataQueueState) {
    if (queue.rafId !== null) {
      window.cancelAnimationFrame(queue.rafId);
      queue.rafId = null;
    }
    if (queue.timerId !== null) {
      window.clearTimeout(queue.timerId);
      queue.timerId = null;
    }
  }

  function clearPtyDataQueue(sessionId: string) {
    const queue = ptyDataQueues.current.get(sessionId);
    if (!queue) return;
    cancelScheduledFlush(queue);
    ptyDataQueues.current.delete(sessionId);
  }

  function flushPtyDataQueueImmediately(sessionId: string) {
    const queue = ptyDataQueues.current.get(sessionId);
    if (!queue) return;
    cancelScheduledFlush(queue);
    if (queue.writing) return;
    flushPtyDataQueue(sessionId);
  }

  function flushPtyDataQueue(sessionId: string) {
    const queue = ptyDataQueues.current.get(sessionId);
    if (!queue || queue.writing) return;

    const handle = terminalRefs.sessionTerminals.current.get(sessionId);
    if (!handle) {
      if (!sessionMeta.current.has(sessionId)) {
        clearPtyDataQueue(sessionId);
        return;
      }
      const pending = queue.chunks.slice(queue.chunkStart);
      clearPtyDataQueue(sessionId);
      for (const chunk of pending) {
        appendSessionBuffer(sessionBuffers.current, sessionId, chunk, MAX_SESSION_BUFFER_CHARS);
      }
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
    if (!queue || queue.writing) return;
    if (queue.rafId !== null || queue.timerId !== null) return;
    const run = () => {
      cancelScheduledFlush(queue);
      flushPtyDataQueue(sessionId);
    };
    // Race rAF (frame-aligned when visible) against a timer (keeps flowing
    // when rAF is paused); whichever fires first cancels the other.
    queue.rafId = window.requestAnimationFrame(run);
    queue.timerId = window.setTimeout(run, PTY_DATA_FLUSH_FALLBACK_MS);
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
    schedulePtyDataFlush(sessionId);
  }

  function releaseSessionConnectingCount(sessionId: string, hostId: string) {
    if (!sessionConnectingCounted.current.has(sessionId)) return;
    sessionConnectingCounted.current.delete(sessionId);
    setConnectingHosts((prev) => {
      const current = prev[hostId];
      if (!current) return prev;
      const nextCount = Math.max(0, (current.count ?? 1) - 1);
      if (nextCount === 0) {
        const next = { ...prev };
        delete next[hostId];
        return next;
      }
      return { ...prev, [hostId]: { ...current, count: nextCount } };
    });
  }

  useEffect(() => {
    if (!isInTauri) return;

    const unlistenDataP = listen<{ session_id: string; data: string }>("pty:data", (event) => {
      const { session_id: sessionId, data } = event.payload;
      const meta = sessionMeta.current.get(sessionId);
      if (!data || !meta || meta.closed) return;

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
        releaseSessionConnectingCount(sessionId, meta.hostId);
      }

      enqueuePtyDataWrite(sessionId, data);
    });

    const unlistenExitP = listen<{ session_id: string; code: number }>("pty:exit", (event) => {
      const { session_id: sessionId, code: exitCode } = event.payload;
      const endedAt = Date.now();
      const meta = sessionMeta.current.get(sessionId);
      if (!meta || meta.closed) {
        if (meta) {
          releaseSessionConnectingCount(sessionId, meta.hostId);
          sessionMeta.current.delete(sessionId);
        }
        clearPtyDataQueue(sessionId);
        sessionBuffers.current.delete(sessionId);
        sessionHadAnyOutput.current.delete(sessionId);
        sessionConnectingCounted.current.delete(sessionId);
        sessionCloseReason.current.delete(sessionId);
        const timer = sessionConnectTimers.current.get(sessionId);
        if (timer !== undefined) {
          window.clearTimeout(timer);
          sessionConnectTimers.current.delete(sessionId);
        }
        return;
      }

      const reason = sessionCloseReason.current.get(sessionId) ?? "unknown";
      const shouldKeepFailedTab = reason === "timeout" || (exitCode > 0);

      if (shouldKeepFailedTab) {
        setSessions((prev) =>
          prev.map((session) => (session.id === sessionId ? { ...session, status: "exited", exitCode, endedAt } : session))
        );
      } else {
        setSessions((prev) => prev.filter((session) => session.id !== sessionId));
        setActiveSessionId((current) => (current === sessionId ? null : current));
      }

      releaseSessionConnectingCount(sessionId, meta.hostId);
      sessionMeta.current.delete(sessionId);
      sessionConnectingCounted.current.delete(sessionId);
      sessionCloseReason.current.delete(sessionId);
      sessionHadAnyOutput.current.delete(sessionId);
      const timer = sessionConnectTimers.current.get(sessionId);
      if (timer !== undefined) {
        window.clearTimeout(timer);
        sessionConnectTimers.current.delete(sessionId);
      }

      if (shouldKeepFailedTab) {
        flushPtyDataQueueImmediately(sessionId);
      } else {
        sessionBuffers.current.delete(sessionId);
        clearPtyDataQueue(sessionId);
      }
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
