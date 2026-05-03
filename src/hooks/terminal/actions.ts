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

export function useSessionActions(params: UseSessionActionsParams) {
  const { isInTauri, hosts, activeSessionId, setSessions, setActiveSessionId, setConnectingHosts, terminalRefs, runtimeRefs } =
    params;
  const {
    sessionBuffers,
    sessionHadAnyOutput,
    sessionConnectTimers,
    sessionMeta,
    sessionCloseReason,
  } = runtimeRefs;

  async function closeSession(sessionId: string, reason: SessionCloseReason = "user") {
    sessionCloseReason.current.set(sessionId, reason);
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
      if (activeSessionId === sessionId) setActiveSessionId(null);
    }
    const meta = sessionMeta.current.get(sessionId);
    if (meta) {
      setConnectingHosts((prev) => {
        if (!prev[meta.hostId]) return prev;
        const next = { ...prev };
        delete next[meta.hostId];
        return next;
      });
    }
    sessionMeta.current.delete(sessionId);
    sessionHadAnyOutput.current.delete(sessionId);
    const timer = sessionConnectTimers.current.get(sessionId);
    if (timer) {
      window.clearTimeout(timer);
      sessionConnectTimers.current.delete(sessionId);
    }
    if (reason === "user") {
      sessionCloseReason.current.delete(sessionId);
    }
  }

  async function connectToHost(host: Host) {
    if (!isInTauri) {
      alert("SSH only works in the desktop app (Tauri).");
      return;
    }

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

      const sessionId = await withTimeout(
        invoke<string>("pty_spawn_ssh", {
          hostId: host.id,
          cols,
          rows,
        }),
        10000,
        "pty spawn"
      );

      sessionMeta.current.set(sessionId.toString(), {
        hostId: host.id,
        hostLabel: host.alias || host.hostname,
        startedAt,
      });
      sessionHadAnyOutput.current.delete(sessionId.toString());
      setConnectingHosts((prev) => {
        const current = prev[host.id];
        if (!current) return prev;
        return { ...prev, [host.id]: { ...current, stage: "connecting" } };
      });
      sessionCloseReason.current.delete(sessionId.toString());

      const connectTimer = window.setTimeout(async () => {
        if (sessionHadAnyOutput.current.has(sessionId.toString())) return;
        const confirmed = await confirm(
          `Connecting to "${host.alias || host.hostname}" is taking longer than expected.\n\nThis often means the hostname/port is wrong or blocked by a firewall.\n\nCancel this connection?`,
          { title: "Connection Timeout", kind: "warning" }
        );
        if (confirmed) await closeSession(sessionId.toString(), "timeout");
      }, 15_000);
      sessionConnectTimers.current.set(sessionId.toString(), connectTimer);

      setSessions((prev) => [
        ...prev,
        { id: sessionId.toString(), hostAlias: host.alias, hostId: host.id, startedAt, status: "starting" },
      ]);
      setActiveSessionId(sessionId.toString());
      requestAnimationFrame(() => {
        try {
          terminalRefs.terminalInstance.current?.focus();
        } catch (error) {
          console.debug("[xterm] focus skipped after connect", error);
        }
      });
    } catch (error) {
      console.error("Failed to connect:", error);
      alert(`Failed to connect: ${error}`);
    }
  }

  return {
    closeSession,
    connectToHost,
  };
}
