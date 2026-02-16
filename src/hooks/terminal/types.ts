import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type { FitAddon } from "@xterm/addon-fit";
import type { Terminal } from "@xterm/xterm";
import type { Session } from "@/types/models";

export type SessionCloseReason = "user" | "timeout" | "unknown";

export type ConnectingHostStatus = {
  stage: string;
  startedAt: number;
  count: number;
};

export type ConnectingHosts = Record<string, ConnectingHostStatus>;

export type SetSessions = Dispatch<SetStateAction<Session[]>>;
export type SetActiveSessionId = Dispatch<SetStateAction<string | null>>;
export type SetConnectingHosts = Dispatch<SetStateAction<ConnectingHosts>>;

export type SessionMeta = {
  hostId: string;
  hostLabel: string;
  startedAt: number;
};

export type TerminalRefs = {
  terminalContainerRef: MutableRefObject<HTMLDivElement | null>;
  terminalRef: MutableRefObject<HTMLDivElement | null>;
  terminalInstance: MutableRefObject<Terminal | null>;
  fitAddon: MutableRefObject<FitAddon | null>;
  activeSessionIdRef: MutableRefObject<string | null>;
  resizeDebounceTimer: MutableRefObject<number | null>;
};

export type SessionRuntimeRefs = {
  sessionBuffers: MutableRefObject<Map<string, string>>;
  sessionAutoPasswords: MutableRefObject<Map<string, string>>;
  sessionPromptTails: MutableRefObject<Map<string, string>>;
  sessionAutoPasswordSent: MutableRefObject<Set<string>>;
  sessionHadAnyOutput: MutableRefObject<Set<string>>;
  sessionConnectTimers: MutableRefObject<Map<string, number>>;
  sessionMeta: MutableRefObject<Map<string, SessionMeta>>;
  sessionCloseReason: MutableRefObject<Map<string, SessionCloseReason>>;
};

export const MAX_SESSION_BUFFER_CHARS = 2_000_000;
