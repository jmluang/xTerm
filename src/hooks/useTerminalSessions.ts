import { useMemo, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type { Host, Session } from "@/types/models";
import { usePtyEvents } from "@/hooks/terminal/ptyEvents";
import { useSessionActions } from "@/hooks/terminal/actions";
import { useTerminalRuntime } from "@/hooks/terminal/runtime";
import type { SessionRuntimeRefs, TerminalRefs } from "@/hooks/terminal/types";
import type { ThemeMode } from "@/lib/theme";

export function useTerminalSessions(params: {
  isInTauri: boolean;
  hosts: Host[];
  sidebarOpen: boolean;
  themeMode: ThemeMode;
}) {
  const { isInTauri, hosts, sidebarOpen, themeMode } = params;
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [connectingHosts, setConnectingHosts] = useState<
    Record<string, { stage: string; startedAt: number; count: number }>
  >({});

  const terminalContainerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<HTMLDivElement>(null);
  const terminalInstance = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);
  const activeSessionIdRef = useRef<string | null>(null);
  const resizeDebounceTimer = useRef<number | null>(null);

  const sessionBuffers = useRef(new Map<string, string>());
  const sessionAutoPasswords = useRef(new Map<string, string>());
  const sessionPromptTails = useRef(new Map<string, string>());
  const sessionAutoPasswordSent = useRef(new Set<string>());
  const sessionHadAnyOutput = useRef(new Set<string>());
  const sessionConnectTimers = useRef(new Map<string, number>());
  const sessionMeta = useRef(new Map<string, { hostId: string; hostLabel: string; startedAt: number }>());
  const sessionCloseReason = useRef(new Map<string, "user" | "timeout" | "unknown">());

  const terminalRefs = useMemo<TerminalRefs>(
    () => ({
      terminalContainerRef,
      terminalRef,
      terminalInstance,
      fitAddon,
      activeSessionIdRef,
      resizeDebounceTimer,
    }),
    []
  );

  const runtimeRefs = useMemo<SessionRuntimeRefs>(
    () => ({
      sessionBuffers,
      sessionAutoPasswords,
      sessionPromptTails,
      sessionAutoPasswordSent,
      sessionHadAnyOutput,
      sessionConnectTimers,
      sessionMeta,
      sessionCloseReason,
    }),
    []
  );

  const { closeSession, connectToHost } = useSessionActions({
    isInTauri,
    hosts,
    activeSessionId,
    setSessions,
    setActiveSessionId,
    setConnectingHosts,
    terminalRefs,
    runtimeRefs,
  });

  usePtyEvents({
    isInTauri,
    setSessions,
    setActiveSessionId,
    setConnectingHosts,
    terminalRefs,
    runtimeRefs,
  });

  useTerminalRuntime({
    isInTauri,
    sidebarOpen,
    themeMode,
    sessions,
    activeSessionId,
    setActiveSessionId,
    terminalRefs,
    runtimeRefs: { sessionBuffers },
  });

  const sessionIndexById = useMemo(() => {
    const byHost = new Map<string, Session[]>();
    for (const session of sessions) {
      const arr = byHost.get(session.hostId);
      if (arr) arr.push(session);
      else byHost.set(session.hostId, [session]);
    }
    const indexById = new Map<string, number>();
    for (const arr of byHost.values()) {
      arr.sort((a, b) => a.startedAt - b.startedAt);
      for (let i = 0; i < arr.length; i += 1) {
        indexById.set(arr[i].id, i + 1);
      }
    }
    return indexById;
  }, [sessions]);

  return {
    sessions,
    activeSessionId,
    setActiveSessionId,
    connectingHosts,
    terminalContainerRef,
    terminalRef,
    terminalInstance,
    sessionIndexById,
    connectToHost,
    closeSession,
  };
}
