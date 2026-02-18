import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "@/lib/toast";
import type { Host, HostLiveInfo, HostStaticInfo, Session } from "@/types/models";

type HostStaticState = {
  info?: HostStaticInfo;
  updatedAt?: number;
};

type HostStaticStateMap = Record<string, HostStaticState>;

const STORAGE_KEY = "xtermius_host_static_info_v1";

function loadStoredState(): HostStaticStateMap {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as HostStaticStateMap;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function normalizeError(error: unknown) {
  const text = String(error || "Unknown error").trim();
  return text.length > 200 ? `${text.slice(0, 200)}...` : text;
}

export function useHostInsights(params: {
  isInTauri: boolean;
  hosts: Host[];
  sessions: Session[];
  activeSessionId: string | null;
}) {
  const { isInTauri, hosts, sessions, activeSessionId } = params;
  const [hostStaticById, setHostStaticById] = useState<HostStaticStateMap>(() => loadStoredState());
  const [refreshingHostIds, setRefreshingHostIds] = useState<Record<string, boolean>>({});
  const [liveHostId, setLiveHostId] = useState<string | null>(null);
  const [liveInfo, setLiveInfo] = useState<HostLiveInfo | null>(null);
  const [liveError, setLiveError] = useState<string | null>(null);
  const [liveLoading, setLiveLoading] = useState(false);
  const [liveUpdatedAt, setLiveUpdatedAt] = useState<number | null>(null);
  const [liveHistory, setLiveHistory] = useState<{ cpu: number[]; mem: number[]; load: number[] }>({
    cpu: [],
    mem: [],
    load: [],
  });
  const autoStaticRefreshInFlight = useRef(new Set<string>());
  const lastLiveHostIdRef = useRef<string | null>(null);

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(hostStaticById));
    } catch {
      // Ignore localStorage failures.
    }
  }, [hostStaticById]);

  const refreshHostStatic = useCallback(
    async (host: Host) => {
      if (!isInTauri) return;
      setRefreshingHostIds((prev) => ({ ...prev, [host.id]: true }));
      try {
        const info = await invoke<HostStaticInfo>("host_probe_static", { host });
        setHostStaticById((prev) => ({
          ...prev,
          [host.id]: {
            info,
            updatedAt: Date.now(),
          },
        }));
      } catch (error) {
        const reason = normalizeError(error);
        showToast({
          tone: "warning",
          title: `Refresh failed · ${host.alias || host.hostname || "Host"}`,
          description: reason,
          durationMs: 2800,
        });
      } finally {
        setRefreshingHostIds((prev) => {
          const next = { ...prev };
          delete next[host.id];
          return next;
        });
      }
    },
    [isInTauri]
  );

  useEffect(() => {
    if (!isInTauri) return;
    const activeSession = sessions.find((session) => session.id === activeSessionId);
    if (!activeSession || activeSession.status !== "running") return;
    const host = hosts.find((item) => item.id === activeSession.hostId);
    if (!host) return;

    const state = hostStaticById[host.id];
    if (state?.info) return;
    if (refreshingHostIds[host.id]) return;
    if (autoStaticRefreshInFlight.current.has(host.id)) return;

    autoStaticRefreshInFlight.current.add(host.id);
    void refreshHostStatic(host).finally(() => {
      autoStaticRefreshInFlight.current.delete(host.id);
    });
  }, [isInTauri, sessions, activeSessionId, hosts, hostStaticById, refreshingHostIds, refreshHostStatic]);

  useEffect(() => {
    if (!isInTauri) {
      lastLiveHostIdRef.current = null;
      setLiveHostId(null);
      setLiveInfo(null);
      setLiveError(null);
      setLiveLoading(false);
      setLiveUpdatedAt(null);
      setLiveHistory({ cpu: [], mem: [], load: [] });
      return;
    }

    const activeSession = sessions.find((session) => session.id === activeSessionId);
    if (!activeSession || activeSession.status === "exited") {
      lastLiveHostIdRef.current = null;
      setLiveHostId(null);
      setLiveInfo(null);
      setLiveError(null);
      setLiveLoading(false);
      setLiveUpdatedAt(null);
      setLiveHistory({ cpu: [], mem: [], load: [] });
      return;
    }

    const host = hosts.find((item) => item.id === activeSession.hostId);
    if (!host) {
      lastLiveHostIdRef.current = null;
      setLiveHostId(null);
      setLiveInfo(null);
      setLiveError("Host not found");
      setLiveLoading(false);
      setLiveUpdatedAt(null);
      setLiveHistory({ cpu: [], mem: [], load: [] });
      return;
    }

    if (lastLiveHostIdRef.current !== host.id) {
      lastLiveHostIdRef.current = host.id;
      setLiveHistory({ cpu: [], mem: [], load: [] });
    }
    setLiveHostId(host.id);

    let cancelled = false;
    let first = true;

    const tick = async () => {
      if (cancelled) return;
      if (first) setLiveLoading(true);
      try {
        const info = await invoke<HostLiveInfo>("host_probe_live", { host });
        if (cancelled) return;
        setLiveInfo(info);
        setLiveError(null);
        setLiveUpdatedAt(Date.now());
        setLiveHistory((prev) => {
          const cpu = typeof info.cpuPercent === "number" ? info.cpuPercent : null;
          const mem =
            typeof info.memTotalKb === "number" && info.memTotalKb > 0 && typeof info.memUsedKb === "number"
              ? (info.memUsedKb / info.memTotalKb) * 100
              : null;
          const load = typeof info.load1 === "number" ? info.load1 * 25 : null;
          const nextCpu = cpu === null ? prev.cpu : [...prev.cpu, Math.max(0, Math.min(100, cpu))].slice(-24);
          const nextMem = mem === null ? prev.mem : [...prev.mem, Math.max(0, Math.min(100, mem))].slice(-24);
          const nextLoad = load === null ? prev.load : [...prev.load, Math.max(0, Math.min(100, load))].slice(-24);
          return { cpu: nextCpu, mem: nextMem, load: nextLoad };
        });
      } catch (error) {
        if (cancelled) return;
        setLiveError(normalizeError(error));
      } finally {
        if (!cancelled && first) setLiveLoading(false);
        first = false;
      }
    };

    void tick();
    const timer = window.setInterval(() => {
      void tick();
    }, 5000);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
      setLiveLoading(false);
    };
  }, [isInTauri, hosts, sessions, activeSessionId]);

  const liveHost = useMemo(() => {
    if (!liveHostId) return null;
    return hosts.find((host) => host.id === liveHostId) ?? null;
  }, [hosts, liveHostId]);

  return {
    hostStaticById,
    refreshingHostIds,
    refreshHostStatic,
    liveHost,
    liveInfo,
    liveError,
    liveLoading,
    liveUpdatedAt,
    liveHistory,
  };
}
