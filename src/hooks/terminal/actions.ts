import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import type { Host } from "@/types/models";
import type {
  SessionCloseReason,
  SessionRuntimeRefs,
  SetActiveSessionId,
  SetConnectingHosts,
  SetSessions,
  TerminalRefs,
} from "@/hooks/terminal/types";

type UseSessionActionsParams = {
  isInTauri: boolean;
  hosts: Host[];
  activeSessionId: string | null;
  setSessions: SetSessions;
  setActiveSessionId: SetActiveSessionId;
  setConnectingHosts: SetConnectingHosts;
  terminalRefs: Pick<TerminalRefs, "terminalInstance">;
  runtimeRefs: SessionRuntimeRefs;
};

async function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  let timer: number | null = null;
  try {
    return await Promise.race([
      promise,
      new Promise<T>((_, reject) => {
        timer = window.setTimeout(() => reject(new Error(`Timeout: ${label} (${ms}ms)`)), ms);
      }),
    ]);
  } finally {
    if (timer) window.clearTimeout(timer);
  }
}

function decrementConnectingHost(setConnectingHosts: SetConnectingHosts, hostId: string) {
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

function createSessionId() {
  const randomUuid = globalThis.crypto?.randomUUID;
  if (randomUuid) return randomUuid.call(globalThis.crypto);
  return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

async function spawnSshWithTimeout(
  sessionId: string,
  hostId: string,
  cols: number,
  rows: number,
  ms: number
): Promise<string> {
  let timer: number | null = null;
  let timedOut = false;
  const spawnPromise = invoke<string>("pty_spawn_ssh", { sessionId, hostId, cols, rows });

  spawnPromise.then(
    (sessionId) => {
      if (timedOut) {
        void invoke("pty_kill", { sessionId }).catch((error) => {
          console.error("Failed to clean up late SSH session:", error);
        });
      }
    },
    () => {}
  );

  try {
    return await Promise.race([
      spawnPromise,
      new Promise<string>((_, reject) => {
        timer = window.setTimeout(() => {
          timedOut = true;
          reject(new Error(`Timeout: pty spawn (${ms}ms)`));
        }, ms);
      }),
    ]);
  } finally {
    if (timer) window.clearTimeout(timer);
  }
}

export function useSessionActions(params: UseSessionActionsParams) {
  const { isInTauri, hosts, setSessions, setActiveSessionId, setConnectingHosts, terminalRefs, runtimeRefs } =
    params;
  const {
    sessionBuffers,
    sessionHadAnyOutput,
    sessionConnectTimers,
    sessionMeta,
    sessionConnectingCounted,
    sessionCloseReason,
  } = runtimeRefs;

  function clearSessionRegistration(sessionId: string, hostId: string) {
    if (sessionConnectingCounted.current.has(sessionId)) {
      sessionConnectingCounted.current.delete(sessionId);
      decrementConnectingHost(setConnectingHosts, hostId);
    }
    sessionMeta.current.delete(sessionId);
    sessionHadAnyOutput.current.delete(sessionId);
    sessionCloseReason.current.delete(sessionId);
    const timer = sessionConnectTimers.current.get(sessionId);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      sessionConnectTimers.current.delete(sessionId);
    }
  }

  async function closeSession(sessionId: string, reason: SessionCloseReason = "user") {
    sessionCloseReason.current.set(sessionId, reason);
    const registration = sessionMeta.current.get(sessionId);
    if (reason === "user" && registration) registration.closed = true;
    if (isInTauri) {
      try {
        await invoke("pty_kill", { sessionId });
      } catch (error) {
        console.error(error);
      }
    }
    if (reason === "user") {
      sessionBuffers.current.delete(sessionId);
      setSessions((prev) => prev.filter((session) => session.id !== sessionId));
      setActiveSessionId((current) => (current === sessionId ? null : current));
    }
    const meta = sessionMeta.current.get(sessionId);
    if (meta && sessionConnectingCounted.current.has(sessionId)) {
      sessionConnectingCounted.current.delete(sessionId);
      decrementConnectingHost(setConnectingHosts, meta.hostId);
    }
    const timer = sessionConnectTimers.current.get(sessionId);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      sessionConnectTimers.current.delete(sessionId);
    }
    if (reason === "timeout") return;

    sessionMeta.current.delete(sessionId);
    sessionConnectingCounted.current.delete(sessionId);
    sessionHadAnyOutput.current.delete(sessionId);
    sessionCloseReason.current.delete(sessionId);
  }

  async function connectToHost(host: Host) {
    if (!isInTauri) {
      alert("SSH only works in the desktop app (Tauri).");
      return;
    }

    let sessionId: string | null = null;
    try {
      const startedAt = Date.now();
      setConnectingHosts((prev) => {
        const current = prev[host.id];
        const startedAt0 = current?.startedAt ?? startedAt;
        const count = (current?.count ?? 0) + 1;
        return { ...prev, [host.id]: { stage: "config", startedAt: startedAt0, count } };
      });

      setConnectingHosts((prev) => {
        const current = prev[host.id];
        if (!current) return prev;
        return { ...prev, [host.id]: { ...current, stage: "save" } };
      });
      await withTimeout(invoke("hosts_save", { hosts }), 5000, "hosts_save");

      const term = terminalRefs.terminalInstance.current;
      const cols = term?.cols ?? 80;
      const rows = term?.rows ?? 24;

      setConnectingHosts((prev) => {
        const current = prev[host.id];
        if (!current) return prev;
        return { ...prev, [host.id]: { ...current, stage: "spawn" } };
      });

      const registeredSessionId = createSessionId();
      sessionId = registeredSessionId;
      const registration = {
        hostId: host.id,
        hostLabel: host.alias || host.hostname,
        startedAt,
        closed: false,
      };
      sessionMeta.current.set(registeredSessionId, registration);
      sessionConnectingCounted.current.add(registeredSessionId);
      setConnectingHosts((prev) => {
        const current = prev[host.id];
        if (!current) return prev;
        return { ...prev, [host.id]: { ...current, stage: "connecting" } };
      });

      setSessions((prev) => [
        ...prev,
        {
          id: registeredSessionId,
          hostAlias: host.alias,
          hostId: host.id,
          startedAt,
          status: "starting",
        },
      ]);
      setActiveSessionId(registeredSessionId);

      const returnedSessionId = await spawnSshWithTimeout(registeredSessionId, host.id, cols, rows, 10000);
      if (returnedSessionId !== registeredSessionId) {
        void invoke("pty_kill", { sessionId: returnedSessionId }).catch((killError) => {
          console.error("Failed to clean up mismatched SSH session:", killError);
        });
        throw new Error("PTY spawn returned a different session ID");
      }
      if (registration.closed) {
        void invoke("pty_kill", { sessionId: returnedSessionId }).catch((killError) => {
          console.error("Failed to clean up SSH session closed during spawn:", killError);
        });
        return;
      }
      if (!sessionMeta.current.has(registeredSessionId)) {
        return;
      }

      setConnectingHosts((prev) => {
        const current = prev[host.id];
        if (!current) return prev;
        return { ...prev, [host.id]: { ...current, stage: "connecting" } };
      });
      sessionCloseReason.current.delete(registeredSessionId);

      if (!sessionHadAnyOutput.current.has(registeredSessionId)) {
        const connectTimer = window.setTimeout(async () => {
          if (sessionHadAnyOutput.current.has(registeredSessionId)) return;
          const confirmed = await confirm(
            `Connecting to "${host.alias || host.hostname}" is taking longer than expected.\n\nThis often means the hostname/port is wrong or blocked by a firewall.\n\nCancel this connection?`,
            { title: "Connection Timeout", kind: "warning" }
          );
          if (confirmed) await closeSession(registeredSessionId, "timeout");
        }, 15_000);
        sessionConnectTimers.current.set(registeredSessionId, connectTimer);
      }

      requestAnimationFrame(() => {
        try {
          terminalRefs.terminalInstance.current?.focus();
        } catch (error) {
          console.debug("[xterm] focus skipped after connect", error);
        }
      });
    } catch (error) {
      if (sessionId) {
        clearSessionRegistration(sessionId, host.id);
        sessionBuffers.current.delete(sessionId);
        setSessions((prev) => prev.filter((session) => session.id !== sessionId));
        setActiveSessionId((current) => (current === sessionId ? null : current));
      } else {
        decrementConnectingHost(setConnectingHosts, host.id);
      }
      console.error("Failed to connect:", error);
      alert(`Failed to connect: ${error}`);
    }
  }

  return {
    closeSession,
    connectToHost,
  };
}
