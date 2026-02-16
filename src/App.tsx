import { useState, useEffect, useMemo, useRef, type ReactNode } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { configDir } from "@tauri-apps/api/path";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { confirm, message, open } from "@tauri-apps/plugin-dialog";
import "@xterm/xterm/css/xterm.css";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DndContext, PointerSensor, closestCenter, useSensor, useSensors, type DragEndEvent, type DragStartEvent } from "@dnd-kit/core";
import { arrayMove, SortableContext, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  ArrowUpDown,
  Check,
  Cloud,
  GripVertical,
  ChevronDown,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  Plus,
  Search,
  Settings2,
  Trash2,
  X,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import { getThemeMode, setThemeMode, type ThemeMode } from "@/lib/theme";

interface Host {
  id: string;
  name: string;
  alias: string;
  hostname: string;
  user: string;
  port: number;
  password?: string;
  hasPassword?: boolean;
  identityFile?: string;
  proxyJump?: string;
  envVars?: string;
  encoding?: string;
  sortOrder?: number;
  tags: string[];
  notes: string;
  updatedAt: string;
  deleted: boolean;
}

interface Session {
  id: string;
  hostAlias: string;
  hostId: string;
  startedAt: number;
  endedAt?: number;
  status: "running" | "exited";
  exitCode?: number;
}

interface Settings {
  webdav_url?: string | null;
  webdav_folder?: string | null;
  webdav_username?: string | null;
  webdav_password?: string | null;
}

function resolveWebdavHostsDbUrl(input: string, folder: string): string {
  const raw = (input ?? "").trim();
  if (!raw) return "";

  // UI hint only. Backend does the real normalization.
  const last = raw.split("/").pop() ?? "";
  const looksLikeDbOrJson = /\.((db|json|sqlite))$/i.test(last);
  if (looksLikeDbOrJson) return `${raw.split("/").slice(0, -1).join("/")}/hosts.db`;

  const base = raw.endsWith("/") ? raw.slice(0, -1) : raw;
  const f = (folder ?? "").trim().replace(/^\/+|\/+$/g, "");
  if (!f) return `${base}/hosts.db`;
  return `${base}/${f}/hosts.db`;
}

function SortableHostRow(props: {
  host: Host;
  reorderMode: boolean;
  hostSearch: string;
  left: ReactNode;
  right: ReactNode;
  onRowClick: () => void;
}) {
  const { host, reorderMode, hostSearch, left, right, onRowClick } = props;
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, transition, isDragging } =
    useSortable({ id: host.id });

  const showHandle = reorderMode && !hostSearch.trim();
  const dragProps = showHandle ? { ...attributes, ...listeners } : {};

  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={[
        "relative px-3 py-2 rounded-lg mb-1 group select-none",
        reorderMode ? "cursor-grab active:cursor-grabbing bg-black/5 hover:bg-black/10" : "cursor-pointer hover:bg-muted",
        isDragging ? "opacity-70" : "",
      ].join(" ")}
      onClick={onRowClick}
      {...dragProps}
    >
      <div className="min-w-0">
        <div className="flex items-start gap-2">
          <button
            ref={setActivatorNodeRef}
            type="button"
            data-dnd-handle
            className={[
              "mt-0.5 -ml-1 h-6 w-6 rounded-md text-muted-foreground/70 hover:text-foreground hover:bg-accent inline-flex items-center justify-center cursor-grab active:cursor-grabbing",
              showHandle ? "opacity-100" : "hidden",
            ].join(" ")}
            title="Drag to reorder"
            aria-label="Drag to reorder"
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => e.stopPropagation()}
          >
            <GripVertical size={16} />
          </button>
          <div className="flex-1 min-w-0">{left}</div>
        </div>
      </div>

      <div
        className={[
          "absolute right-2 top-2 flex items-center gap-1 transition-opacity pointer-events-none",
          reorderMode ? "opacity-0" : "opacity-0 group-hover:opacity-100",
        ].join(" ")}
      >
        {right}
      </div>
    </div>
  );
}

function App() {
  const [hosts, setHosts] = useState<Host[]>([]);
  const hostsRef = useRef<Host[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [connectingHosts, setConnectingHosts] = useState<Record<string, { stage: string; startedAt: number; count: number }>>({});
  const [hostSearch, setHostSearch] = useState("");
  const hostListRef = useRef<HTMLDivElement>(null);
  const [hostListScrollable, setHostListScrollable] = useState(false);
  const [reorderMode, setReorderMode] = useState(false);
  const [, setActiveDragHostId] = useState<string | null>(null);
  const [terminalReady, setTerminalReady] = useState(false);
  const [showDialog, setShowDialog] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showWebdavSync, setShowWebdavSync] = useState(false);
  const [settings, setSettings] = useState<Settings>({});
  const [syncBusy, setSyncBusy] = useState<null | "pull" | "push" | "save">(null);
  const [syncNotice, setSyncNotice] = useState<null | { kind: "ok" | "err"; text: string }>(null);
  const [localHostsDbPath, setLocalHostsDbPath] = useState<string>("");
  const [editingHost, setEditingHost] = useState<Host | null>(null);
  const [formData, setFormData] = useState<Partial<Host>>({});
  const [isInTauri] = useState(() => {
    const w = window as any;
    return !!(w.__TAURI__ || w.__TAURI_INTERNALS__);
  });
  const terminalContainerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<HTMLDivElement>(null);
  const terminalInstance = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);
  const sessionBuffers = useRef(new Map<string, string>());
  const MAX_SESSION_BUFFER_CHARS = 2_000_000;
  const sessionAutoPasswords = useRef(new Map<string, string>());
  const sessionPromptTails = useRef(new Map<string, string>());
  const sessionAutoPasswordSent = useRef(new Set<string>());
  const sessionHadAnyOutput = useRef(new Set<string>());
  const sessionConnectTimers = useRef(new Map<string, number>());
  const sessionMeta = useRef(new Map<string, { hostId: string; hostLabel: string; startedAt: number }>());
  const sessionCloseReason = useRef(new Map<string, "user" | "timeout" | "unknown">());
  const activeSessionIdRef = useRef<string | null>(null);
  const resizeDebounceTimer = useRef<number | null>(null);
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() => getThemeMode());
  const [sidebarOpen, setSidebarOpen] = useState(() => localStorage.getItem("xtermius_sidebar_open") !== "0");

  function resolvedTheme(): "light" | "dark" {
    const d = document.documentElement.dataset.theme;
    if (d === "light" || d === "dark") return d;
    return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }

  function applyTerminalTheme() {
    const term = terminalInstance.current;
    if (!term) return;
    const t = resolvedTheme();
    const cssBg = getComputedStyle(document.documentElement).getPropertyValue("--app-term-bg").trim();
    const bg = cssBg || (t === "dark" ? "#0b0f16" : "#ffffff");
    term.options.theme =
      t === "dark"
        ? {
            background: bg,
            foreground: "#e5e7eb",
            cursor: "#e5e7eb",
            selectionBackground: "rgba(148, 163, 184, 0.35)",
          }
        : {
            background: bg,
            foreground: "#0b1220", // near-black
            cursor: "#0b1220",
            selectionBackground: "rgba(2, 132, 199, 0.22)",
          };
    try {
      term.refresh(0, term.rows - 1);
    } catch {
      // Ignore; refresh isn't critical and may fail during early init.
    }
  }

  async function refreshSettingsFromBackend() {
    if (!isInTauri) return;
    try {
      const s = await invoke<Settings>("settings_load");
      setSettings(s ?? {});
      const cd = await configDir().catch(() => "");
      if (cd) setLocalHostsDbPath(`${cd}/xtermius/hosts.db`);
    } catch (e) {
      console.error("[settings] load error", e);
    }
  }

  async function doWebdavPull() {
    if (!isInTauri) return;
    setSyncBusy("pull");
    setSyncNotice(null);
    try {
      await invoke("webdav_pull");
      await loadHosts();
      setSyncNotice({ kind: "ok", text: "Pulled" });
      await message("Pulled from WebDAV.", { title: "WebDAV", kind: "info" });
    } catch (e) {
      const msg = `WebDAV pull failed.\n\n${String(e)}`;
      setSyncNotice({ kind: "err", text: "Pull failed" });
      try {
        await message(msg, { title: "WebDAV", kind: "error" });
      } catch {
        // Ignore.
      }
    } finally {
      setSyncBusy(null);
    }
  }

  async function doWebdavPush() {
    if (!isInTauri) return;
    const aliveCount = hostsRef.current.filter((h) => !h.deleted).length;
    if (aliveCount === 0) {
      const ok = await confirm(
        "No hosts found.\n\nPushing now will overwrite the remote hosts.db with an empty database.\n\nContinue?",
        { title: "WebDAV Push", kind: "warning" }
      );
      if (!ok) return;
    }

    setSyncBusy("push");
    setSyncNotice(null);
    try {
      // Always persist current settings + hosts before uploading the single-file DB.
      await invoke("settings_save", { settings });
      await invoke("hosts_save", { hosts: hostsRef.current });
      await invoke("webdav_push");
      setSyncNotice({ kind: "ok", text: "Pushed" });
      await message("Pushed to WebDAV.", { title: "WebDAV", kind: "info" });
    } catch (e) {
      const msg = `WebDAV push failed.\n\n${String(e)}`;
      setSyncNotice({ kind: "err", text: "Push failed" });
      try {
        await message(msg, { title: "WebDAV", kind: "error" });
      } catch {
        // Ignore.
      }
    } finally {
      setSyncBusy(null);
    }
  }

  useEffect(() => {
    activeSessionIdRef.current = activeSessionId;
  }, [activeSessionId]);

  useEffect(() => {
    hostsRef.current = hosts;
  }, [hosts]);

  useEffect(() => {
    // Keep persisted theme in sync with UI state.
    setThemeMode(themeMode);
  }, [themeMode]);

  useEffect(() => {
    if (!showSettings) return;
    if (!isInTauri) return;
    refreshSettingsFromBackend();
  }, [showSettings, isInTauri]);

  useEffect(() => {
    if (!showWebdavSync) return;
    if (!isInTauri) return;
    refreshSettingsFromBackend();
  }, [showWebdavSync, isInTauri]);

  useEffect(() => {
    // Keep xterm colors readable across Light/Dark, including System mode changes.
    applyTerminalTheme();
    if (!window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      // theme.ts updates documentElement.dataset.theme; re-apply so xterm stays in sync.
      if (getThemeMode() === "system") applyTerminalTheme();
    };
    if (typeof mq.addEventListener === "function") mq.addEventListener("change", onChange);
    else if (typeof (mq as any).addListener === "function") (mq as any).addListener(onChange);
    return () => {
      if (typeof mq.removeEventListener === "function") mq.removeEventListener("change", onChange);
      else if (typeof (mq as any).removeListener === "function") (mq as any).removeListener(onChange);
    };
  }, [themeMode]);

  useEffect(() => {
    localStorage.setItem("xtermius_sidebar_open", sidebarOpen ? "1" : "0");
  }, [sidebarOpen]);

  useEffect(() => {
    // Sidebar toggle changes available terminal size; force a refit to avoid transient overflow/scrollbars.
    requestAnimationFrame(() => fitAndResizeActivePty());
    window.setTimeout(() => fitAndResizeActivePty(), 120);
  }, [sidebarOpen]);

  useEffect(() => {
    // During initial mount and some layout changes, WebView layout can "settle" over a few frames.
    // Nudge xterm to refit a few times so it reaches the final size (especially in light mode).
    if (!terminalReady) return;
    let n = 0;
    const id = window.setInterval(() => {
      fitAndResizeActivePty();
      n += 1;
      if (n >= 12) window.clearInterval(id);
    }, 100);
    return () => window.clearInterval(id);
  }, [terminalReady, sidebarOpen, themeMode]);

  useEffect(() => {
    // In Tauri WebView, window.resize / ResizeObserver can miss shrink events.
    // Listen to the native window resized event to keep rows/cols correct.
    if (!isInTauri) return;
    let unlisten: null | (() => void) = null;
    (async () => {
      try {
        const win = getCurrentWindow();
        unlisten = await win.onResized(() => {
          requestAnimationFrame(() => fitAndResizeActivePty());
          window.setTimeout(() => fitAndResizeActivePty(), 120);
        });
      } catch (e) {
        console.debug("[ui] window.onResized unavailable", e);
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

  function fitAndResizeActivePty() {
    const term = terminalInstance.current;
    const fit = fitAddon.current;
    const sid = activeSessionIdRef.current;
    if (!term || !fit) return;

    fit.fit();

    if (!sid || !isInTauri) return;
    invoke("pty_resize", {
      sessionId: sid,
      cols: term.cols,
      rows: term.rows,
    }).catch((e) => console.error("[pty] resize error", e));
  }

  async function withTimeout<T>(p: Promise<T>, ms: number, label: string): Promise<T> {
    let t: number | null = null;
    try {
      return await Promise.race([
        p,
        new Promise<T>((_, rej) => {
          t = window.setTimeout(() => rej(new Error(`Timeout: ${label} (${ms}ms)`)), ms);
        }),
      ]);
    } finally {
      if (t) window.clearTimeout(t);
    }
  }

  useEffect(() => {
    loadHosts();
  }, [isInTauri]);

  useEffect(() => {
    if (sessions.length === 0) return;
    if (activeSessionId && sessions.some((s) => s.id === activeSessionId)) return;
    setActiveSessionId(sessions[0].id);
  }, [sessions, activeSessionId]);

  useEffect(() => {
    if (!isInTauri) return;

    const unlistenDataP = listen<{ session_id: string; data: string }>("pty:data", (event) => {
      const { session_id, data } = event.payload;
      if (!data) return;

      // Best-effort auto password entry for hosts that have password saved.
      // OpenSSH doesn't read passwords from ssh_config, so we detect the prompt and write it once.
      const tail = (sessionPromptTails.current.get(session_id) ?? "") + data;
      sessionPromptTails.current.set(session_id, tail.slice(Math.max(0, tail.length - 800)));

      // First output means the ssh process is alive and talking; clear "connect timeout" checks.
      if (!sessionHadAnyOutput.current.has(session_id)) {
        sessionHadAnyOutput.current.add(session_id);
        const t = sessionConnectTimers.current.get(session_id);
        if (t) {
          window.clearTimeout(t);
          sessionConnectTimers.current.delete(session_id);
        }
        const meta = sessionMeta.current.get(session_id);
        if (meta) {
          setConnectingHosts((prev) => {
            const cur = prev[meta.hostId];
            if (!cur) return prev;
            const nextCount = Math.max(0, (cur.count ?? 1) - 1);
            if (nextCount === 0) {
              const next = { ...prev };
              delete next[meta.hostId];
              return next;
            }
            return { ...prev, [meta.hostId]: { ...cur, count: nextCount } };
          });
        }
      }
      if (!sessionAutoPasswordSent.current.has(session_id)) {
        const pw = sessionAutoPasswords.current.get(session_id);
        if (pw) {
          const t = sessionPromptTails.current.get(session_id) ?? "";
          // Common prompts:
          // - "user@host's password:"
          // - "Password:"
          // - "password for user:"
          // Be tolerant: ssh prompts vary by locale and version, and can appear in chunks.
          // Common prompts:
          // - "user@host's password:"
          // - "user@host password:"
          // - "Password:"
          // - "Password for user@host:"
          const looksLikePasswordPrompt =
            /(^|\n|\r)\s*(?:[A-Za-z0-9_.-]+@[^:\n\r]+(?:['’]s)?\s+)?password(?:\s+for\s+[^:\n\r]+)?\s*:\s*$/i.test(t);
          if (looksLikePasswordPrompt) {
            sessionAutoPasswordSent.current.add(session_id);
            invoke("pty_write", { sessionId: session_id, data: `${pw}\n` }).catch((e) =>
              console.error("[pty] auto password write error", e)
            );
          }
        }
      }

      // Always buffer output so tab switching can fully restore the session view.
      // Keep a cap to avoid unbounded memory growth.
      const prev = sessionBuffers.current.get(session_id) ?? "";
      let next = prev + data;
      if (next.length > MAX_SESSION_BUFFER_CHARS) next = next.slice(next.length - MAX_SESSION_BUFFER_CHARS);
      sessionBuffers.current.set(session_id, next);

      if (activeSessionIdRef.current === session_id && terminalInstance.current) {
        terminalInstance.current.write(data);
      }
    });

    const unlistenExitP = listen<{ session_id: string; code: number }>("pty:exit", (event) => {
      const { session_id } = event.payload;
      const exitCode = event.payload.code;
      const endedAt = Date.now();
      const reason = sessionCloseReason.current.get(session_id) ?? "unknown";
      const shouldKeepFailedTab = reason === "timeout" || (reason === "unknown" && exitCode !== 0);

      if (shouldKeepFailedTab) {
        // Keep failed sessions visible for diagnosis (e.g. network timeout / auth failure).
        setSessions((prev) =>
          prev.map((s) => (s.id === session_id ? { ...s, status: "exited", exitCode, endedAt } : s))
        );
      } else {
        // Successful natural exit (`exit`) and user-close should remove the tab.
        setSessions((prev) => prev.filter((s) => s.id !== session_id));
        if (activeSessionIdRef.current === session_id) {
          setActiveSessionId(null);
        }
      }

      const meta = sessionMeta.current.get(session_id);

      // Stop "connecting" indicators/timers.
      sessionMeta.current.delete(session_id);
      sessionCloseReason.current.delete(session_id);
      sessionHadAnyOutput.current.delete(session_id);
      const t = sessionConnectTimers.current.get(session_id);
      if (t) {
        window.clearTimeout(t);
        sessionConnectTimers.current.delete(session_id);
      }

      if (meta) {
        setConnectingHosts((prev) => {
          const cur = prev[meta.hostId];
          if (!cur) return prev;
          const nextCount = Math.max(0, (cur.count ?? 1) - 1);
          if (nextCount === 0) {
            const next = { ...prev };
            delete next[meta.hostId];
            return next;
          }
          return { ...prev, [meta.hostId]: { ...cur, count: nextCount } };
        });
      }

      if (!shouldKeepFailedTab) {
        sessionBuffers.current.delete(session_id);
      }
      sessionPromptTails.current.delete(session_id);
      sessionAutoPasswords.current.delete(session_id);
      sessionAutoPasswordSent.current.delete(session_id);
    });

    return () => {
      unlistenDataP.then((fn) => fn());
      unlistenExitP.then((fn) => fn());
    };
  }, [isInTauri]);

  useEffect(() => {
    // When there is no active session, keep the terminal view clean.
    if (!terminalReady) return;
    if (sessions.length !== 0) return;
    terminalInstance.current?.clearSelection();
    terminalInstance.current?.blur();
    terminalInstance.current?.clear();
  }, [sessions.length, terminalReady]);

  useEffect(() => {
    if (terminalRef.current && !terminalInstance.current) {
      const term = new Terminal({
        cursorBlink: false,
        cursorInactiveStyle: "none",
        fontSize: 14,
        fontFamily: "SF Mono, Menlo, Monaco, 'Courier New', monospace",
        theme: { background: "transparent", foreground: "#e5e7eb" },
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(terminalRef.current);
      fit.fit();
      terminalInstance.current = term;
      applyTerminalTheme();
      fitAddon.current = fit;
      setTerminalReady(true);

      term.onData((data) => {
        const sid = activeSessionIdRef.current;
        if (!sid || !isInTauri) return;
        invoke("pty_write", { sessionId: sid, data }).catch((e) =>
          console.error("[pty] write error", e)
        );
      });

      window.addEventListener("resize", () => {
        if (fitAddon.current && terminalInstance.current) {
          fitAndResizeActivePty();
        }
      });
    }
  }, [activeSessionId, isInTauri]);

  useEffect(() => {
    // Observe the container, not the xterm mount node. Some webviews don't reliably
    // fire window.resize during live window resizing, and the xterm mount can stay
    // "same size" while the container changes.
    const el = terminalContainerRef.current ?? terminalRef.current;
    if (!el) return;

    const ro = new ResizeObserver(() => {
      if (resizeDebounceTimer.current) window.clearTimeout(resizeDebounceTimer.current);
      resizeDebounceTimer.current = window.setTimeout(() => {
        fitAndResizeActivePty();
      }, 50);
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (resizeDebounceTimer.current) window.clearTimeout(resizeDebounceTimer.current);
      resizeDebounceTimer.current = null;
    };
  }, [isInTauri]);

  useEffect(() => {
    const term = terminalInstance.current;
    if (!terminalReady || !term) return;
    const hasSession = sessions.length > 0;
    term.options.cursorBlink = hasSession;
    if (!hasSession) term.blur();
  }, [sessions.length, terminalReady]);

  useEffect(() => {
    const term = terminalInstance.current;
    if (!terminalReady || !term) return;
    if (!activeSessionId) return;

    // Basic tab switching support: clear and replay buffer for the selected session.
    // (Long-term: give each session its own xterm instance.)
    term.reset();
    applyTerminalTheme();
    const buf = sessionBuffers.current.get(activeSessionId);
    if (buf) term.write(buf);
    requestAnimationFrame(() => fitAndResizeActivePty());
    requestAnimationFrame(() => term.focus());
  }, [activeSessionId, terminalReady]);

  async function loadHosts() {
    try {
      if (isInTauri) {
        const data = await invoke<Host[]>("hosts_load");
        // Ensure sortOrder is sane so manual reorder works (migrations may default many rows to 0).
        const alive = data.filter((h) => !h.deleted);
        const orders = alive
          .map((h) => h.sortOrder)
          .filter((v): v is number => typeof v === "number");
        const unique = new Set(orders);
        const needsNormalize = alive.length > 0 && (orders.length !== alive.length || unique.size !== alive.length);
        if (needsNormalize) {
          const normalized = normalizeHostOrder(data);
          setHosts(normalized);
          await invoke("hosts_save", { hosts: normalized });
        } else {
          setHosts(data);
        }
      } else {
        const saved = localStorage.getItem("xtermius_hosts");
        if (saved) setHosts(JSON.parse(saved));
        else setHosts([]);
      }
    } catch (e) {
      console.error("Failed to load hosts:", e);
      const saved = localStorage.getItem("xtermius_hosts");
      if (saved) setHosts(JSON.parse(saved));
      else setHosts([]);
    }
  }

  async function saveHostsToBackend(newHosts: Host[]) {
    if (isInTauri) {
      // On desktop we must not silently fall back, especially when saving secrets to Keychain.
      await invoke("hosts_save", { hosts: newHosts });
      return;
    }
    // Web fallback (dev in browser).
    localStorage.setItem("xtermius_hosts", JSON.stringify(newHosts));
  }

  async function connectToHost(host: Host) {
    if (!isInTauri) {
      alert("SSH only works in the desktop app (Tauri).");
      return;
    }

    try {
      let savedPassword: string | null = null;
      if (host.password && host.password.trim()) {
        savedPassword = host.password;
      } else if (host.hasPassword) {
        try {
          savedPassword = await invoke<string | null>("host_password_get", { hostId: host.id });
        } catch (e) {
          console.debug("[keychain] get password failed", e);
        }
      }

      console.debug("[ui] connect click", { hostAlias: host.alias, host: host.hostname });
      const startedAt = Date.now();
      setConnectingHosts((prev) => {
        const cur = prev[host.id];
        const startedAt0 = cur?.startedAt ?? startedAt;
        const count = (cur?.count ?? 0) + 1;
        return { ...prev, [host.id]: { stage: "config", startedAt: startedAt0, count } };
      });
      const cd = await withTimeout(configDir().catch(() => ""), 5000, "configDir()");
      const sshConfigPath = `${cd}/xtermius/ssh_config`;
      const targetAlias = host.alias || host.hostname;
      
      setConnectingHosts((prev) => {
        const cur = prev[host.id];
        if (!cur) return prev;
        return { ...prev, [host.id]: { ...cur, stage: "save" } };
      });
      await withTimeout(invoke("hosts_save", { hosts: hosts }), 5000, "hosts_save");

      const term = terminalInstance.current;
      const cols = term?.cols ?? 80;
      const rows = term?.rows ?? 24;

      setConnectingHosts((prev) => {
        const cur = prev[host.id];
        if (!cur) return prev;
        return { ...prev, [host.id]: { ...cur, stage: "spawn" } };
      });
      const sessionId = await withTimeout(invoke<string>("pty_spawn", {
        file: "/usr/bin/ssh",
        // Fail fast for wrong host/port; otherwise ssh can appear "stuck" for a long time.
        args: ["-F", sshConfigPath, "-o", "ConnectTimeout=10", "-o", "ConnectionAttempts=1", targetAlias],
        cwd: null,
        env: {},
        cols,
        rows,
      }), 10000, "pty spawn");
      console.debug("[pty] spawned", { sessionId });

      // Track "connecting" until we see any output from ssh (banner/error/password prompt).
      sessionMeta.current.set(sessionId.toString(), {
        hostId: host.id,
        hostLabel: host.alias || host.hostname,
        startedAt,
      });
      sessionHadAnyOutput.current.delete(sessionId.toString());
      setConnectingHosts((prev) => {
        const cur = prev[host.id];
        if (!cur) return prev;
        return { ...prev, [host.id]: { ...cur, stage: "connecting" } };
      });
      sessionCloseReason.current.delete(sessionId.toString());

      // If ssh produces no output for a while, prompt the user to cancel.
      const connectTimer = window.setTimeout(async () => {
        if (sessionHadAnyOutput.current.has(sessionId.toString())) return;
        const ok = await confirm(
          `Connecting to "${host.alias || host.hostname}" is taking longer than expected.\n\nThis often means the hostname/port is wrong or blocked by a firewall.\n\nCancel this connection?`,
          { title: "Connection Timeout", kind: "warning" }
        );
        if (ok) {
          await closeSession(sessionId.toString(), "timeout");
        }
      }, 15_000);
      sessionConnectTimers.current.set(sessionId.toString(), connectTimer);

      if (savedPassword && savedPassword.trim()) {
        sessionAutoPasswords.current.set(sessionId.toString(), savedPassword);
        sessionAutoPasswordSent.current.delete(sessionId.toString());
        sessionPromptTails.current.delete(sessionId.toString());
      } else if (host.hasPassword) {
        // Host claims a saved password, but keychain has none on this device.
        // This happens after syncing hosts.db to a new machine (passwords are not synced).
        try {
          await message(
            `No saved password found in Keychain for "${host.alias || host.hostname}".\n\nPasswords are stored per-device.\nPlease enter the password in the Host and save it again on this device.`,
            { title: "Password Needed", kind: "info" }
          );
        } catch {
          // Ignore.
        }
      }
      setSessions(prev => [...prev, { id: sessionId.toString(), hostAlias: host.alias, hostId: host.id, startedAt, status: "running" }]);
      setActiveSessionId(sessionId.toString());
      requestAnimationFrame(() => terminalInstance.current?.focus());
    } catch (e) {
      console.error("Failed to connect:", e);
      alert("Failed to connect: " + e);
    } finally {
      // Don't clear connecting state here; we clear it when ssh produces output (or session exits).
    }
  }

  async function closeSession(sessionId: string, reason: "user" | "timeout" | "unknown" = "user") {
    sessionCloseReason.current.set(sessionId, reason);
    if (isInTauri) {
      try { await invoke("pty_kill", { sessionId }); } 
      catch (e) { console.error(e); }
    }
    // Keep the tab open on non-user closes; user can close manually.
    if (reason === "user") {
      sessionBuffers.current.delete(sessionId);
      sessionPromptTails.current.delete(sessionId);
      sessionAutoPasswords.current.delete(sessionId);
      sessionAutoPasswordSent.current.delete(sessionId);
      setSessions(prev => prev.filter(s => s.id !== sessionId));
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
    const t = sessionConnectTimers.current.get(sessionId);
    if (t) {
      window.clearTimeout(t);
      sessionConnectTimers.current.delete(sessionId);
    }
    if (reason === "user") {
      sessionCloseReason.current.delete(sessionId);
    }
  }

  async function selectIdentityFile() {
    if (!isInTauri) {
      alert("File selection only works in the desktop app");
      return;
    }
    try {
      const selected = await open({
        title: "Select Identity File",
        multiple: false,
        directory: false,
      });
      if (selected) {
        setFormData({ ...formData, identityFile: selected as string });
      }
    } catch (e) {
      console.error("Failed to select file:", e);
    }
  }

  function openAddDialog() {
    setEditingHost(null);
    setFormData({ name: "", alias: "", hostname: "", user: "", port: 22, tags: [], notes: "" });
    setShowDialog(true);
  }

  function openEditDialog(host: Host) {
    setEditingHost(host);
    setFormData({ ...host });
    setShowDialog(true);
  }

  async function handleSave() {
    const now = new Date().toISOString();
    let newHosts: Host[];
    let targetId: string | null = null;
    let pwAction: "keep" | "set" | "clear" = "keep";
    let pwValue = "";
    
    if (editingHost) {
      const patch: any = { ...formData, updatedAt: now };
      if (typeof formData.password === "string") {
        patch.hasPassword = formData.password.trim().length > 0;
      }
      newHosts = hosts.map(h => h.id === editingHost.id ? { ...h, ...patch } as Host : h);
      targetId = editingHost.id;
      if (typeof formData.password === "string") {
        const t = formData.password.trim();
        if (!t) {
          pwAction = "clear";
        } else {
          pwAction = "set";
          pwValue = formData.password;
        }
      }
    } else {
      const newHost: Host = {
        id: Date.now().toString(),
        name: formData.hostname || "Unnamed",
        alias: formData.alias || formData.hostname?.toLowerCase().replace(/\s+/g, "-") || "new-host",
        hostname: formData.hostname || "",
        user: formData.user || "",
        port: formData.port || 22,
        password: formData.password,
        hasPassword: !!(formData.password && String(formData.password).trim()),
        identityFile: formData.identityFile,
        proxyJump: formData.proxyJump,
        envVars: formData.envVars,
        encoding: formData.encoding || "utf-8",
        tags: formData.tags || [],
        notes: formData.notes || "",
        updatedAt: now,
        deleted: false,
      };
      newHosts = [...hosts, newHost];
      targetId = newHost.id;
      if (typeof formData.password === "string") {
        const t = formData.password.trim();
        if (!t) {
          pwAction = "keep";
        } else {
          pwAction = "set";
          pwValue = formData.password;
        }
      }
    }
    
    // Never persist plaintext password in hosts.db; Keychain is handled via explicit commands below.
    newHosts = newHosts.map((h) => (h.password ? { ...h, password: undefined } : h));

    try {
      await saveHostsToBackend(newHosts);

      if (isInTauri && targetId) {
        if (pwAction === "set") {
          await invoke("host_password_set", { hostId: targetId, password: pwValue });
        } else if (pwAction === "clear") {
          await invoke("host_password_delete", { hostId: targetId });
        }
      }
    } catch (e) {
      // Keychain errors surface here; do not close the dialog or mark as "saved".
      try {
        await message(`Failed to save host.\n\n${String(e)}`, { title: "Save Failed", kind: "error" });
      } catch {
        // Ignore.
      }
      return;
    }
    setShowDialog(false);
    // Reload from backend so hasPassword reflects actual Keychain state on this device.
    await loadHosts();
  }

  async function deleteHost(host: Host) {
    // window.confirm is unreliable in Tauri/WKWebView (dialog can be async but return immediately).
    // Use the official dialog API so Cancel truly cancels.
    const ok = await confirm(`Delete "${host.hostname}"?`, {
      title: "Delete Host",
      kind: "warning",
    });
    if (!ok) return;
    
    const newHosts = hosts.map(h => h.id === host.id ? { ...h, deleted: true, updatedAt: new Date().toISOString() } : h);
    await saveHostsToBackend(newHosts);
    if (isInTauri) {
      try {
        await invoke("host_password_delete", { hostId: host.id });
      } catch {
        // Ignore.
      }
    }
    setHosts(newHosts);
  }

  function normalizeHostOrder(list: Host[]): Host[] {
    // Normalize sortOrder for non-deleted hosts to be dense [0..n-1].
    const alive = list.filter((h) => !h.deleted);
    alive.sort((a, b) => {
      const ao = typeof a.sortOrder === "number" ? a.sortOrder : Number.MAX_SAFE_INTEGER;
      const bo = typeof b.sortOrder === "number" ? b.sortOrder : Number.MAX_SAFE_INTEGER;
      if (ao !== bo) return ao - bo;
      const at = Date.parse(a.updatedAt || "") || 0;
      const bt = Date.parse(b.updatedAt || "") || 0;
      return bt - at;
    });
    const dense = new Map<string, number>();
    for (let i = 0; i < alive.length; i += 1) dense.set(alive[i].id, i);
    return list.map((h) => (h.deleted ? h : { ...h, sortOrder: dense.get(h.id) ?? h.sortOrder }));
  }

  async function persistHostOrder(nextHosts: Host[]) {
    const normalized = normalizeHostOrder(nextHosts);
    setHosts(normalized);
    await saveHostsToBackend(normalized);
  }

  const dndSensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } })
  );

  const activeHosts = hosts.filter(h => !h.deleted);
  const filteredHosts = useMemo(() => {
    const q = hostSearch.trim().toLowerCase();
    if (!q) return activeHosts;
    return activeHosts.filter((h) => {
      const hay = [
        h.alias,
        h.hostname,
        h.user,
        `${h.user || ""}@${h.hostname || ""}`,
        (h.tags || []).join(" "),
        h.notes,
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      return hay.includes(q);
    });
  }, [activeHosts, hostSearch]);

  const sortedHosts = useMemo(() => {
    // Manual order (drag & drop) via sortOrder, then updatedAt as fallback.
    const arr = filteredHosts.slice();
    arr.sort((a, b) => {
      const ao = typeof a.sortOrder === "number" ? a.sortOrder : Number.MAX_SAFE_INTEGER;
      const bo = typeof b.sortOrder === "number" ? b.sortOrder : Number.MAX_SAFE_INTEGER;
      if (ao !== bo) return ao - bo;
      const at = Date.parse(a.updatedAt || "") || 0;
      const bt = Date.parse(b.updatedAt || "") || 0;
      return bt - at;
    });
    return arr;
  }, [filteredHosts]);

  useEffect(() => {
    // Show search bar only when the list is scrollable, or user already started typing.
    const el = hostListRef.current;
    if (!el) return;
    const check = () => setHostListScrollable(el.scrollHeight > el.clientHeight + 1);
    check();
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => ro.disconnect();
  }, [sortedHosts.length, sidebarOpen]);
  // Compute per-host tab indices based on currently open tabs (not lifetime attempts).
  // This makes numbering stable and renumbers when tabs are closed.
  const sessionIndexById = useMemo(() => {
    const byHost = new Map<string, Session[]>();
    for (const s of sessions) {
      const arr = byHost.get(s.hostId);
      if (arr) arr.push(s);
      else byHost.set(s.hostId, [s]);
    }
    const m = new Map<string, number>();
    for (const arr of byHost.values()) {
      arr.sort((a, b) => a.startedAt - b.startedAt);
      for (let i = 0; i < arr.length; i += 1) {
        m.set(arr[i].id, i + 1);
      }
    }
    return m;
  }, [sessions]);

  return (
    <div className="h-screen text-foreground overflow-hidden" style={{ background: "var(--app-bg)" } as any}>
      {/* Unified macOS titlebar + content area (sidebar left, shell right). */}
      <div className={["grid h-full min-h-0 min-w-0", sidebarOpen ? "grid-cols-[288px_1fr]" : "grid-cols-1"].join(" ")}>
        {sidebarOpen ? (
          <div className="min-w-0 min-h-0 flex flex-col" style={{ background: "var(--app-sidebar-bg)" } as any}>
            <div
              data-tauri-drag-region
              className="h-[44px] pt-[4px] pb-0 flex items-center gap-2 pl-[88px] pr-2"
              style={{ WebkitAppRegion: "drag" } as any}
            >
              <button
                type="button"
                title="Hide Hosts"
                aria-label="Hide Hosts"
                onClick={() => setSidebarOpen(false)}
                className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                data-tauri-drag-region="false"
                style={{ WebkitAppRegion: "no-drag" } as any}
              >
                <PanelLeftClose size={18} />
              </button>
              <div className="flex-1" />
              <button
                type="button"
                className={[
                  "h-7 w-7 rounded-md inline-flex items-center justify-center",
                  reorderMode ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground hover:bg-accent",
                ].join(" ")}
                title={reorderMode ? "Done reordering" : "Reorder hosts"}
                aria-label={reorderMode ? "Done reordering" : "Reorder hosts"}
                onClick={(e) => {
                  e.stopPropagation();
                  setReorderMode((v) => !v);
                }}
                data-tauri-drag-region="false"
                style={{ WebkitAppRegion: "no-drag" } as any}
              >
                {reorderMode ? <Check size={18} /> : <ArrowUpDown size={18} />}
              </button>
            </div>

            <aside className="flex-1 min-h-0 overflow-hidden">
              <div ref={hostListRef} className="h-full overflow-auto px-2 pt-0 pb-1">
                {hostListScrollable || hostSearch.trim() ? (
                  <div
                    className="sticky top-0 z-20 -mx-2 px-2 pt-1 pb-1"
                    style={{ background: "var(--app-sidebar-bg)" } as any}
                  >
                    <div className="px-1">
                      <div className="relative">
                        <Search size={14} className="absolute left-2 top-1/2 -translate-y-1/2 text-foreground/70" />
                        <Input
                          value={hostSearch}
                          onChange={(e) => setHostSearch(e.target.value)}
                          placeholder=""
                          className="pl-7 pr-7 h-8 bg-transparent shadow-none border-border/50 focus-visible:ring-1"
                          onKeyDown={(e) => {
                            if (e.key === "Escape") setHostSearch("");
                          }}
                        />
                        {hostSearch.trim() ? (
                          <button
                            type="button"
                            className="absolute right-1 top-1/2 -translate-y-1/2 h-6 w-6 rounded-md text-foreground/70 hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                            onClick={() => setHostSearch("")}
                            title="Clear"
                            aria-label="Clear search"
                          >
                            <X size={14} />
                          </button>
                        ) : null}
                      </div>
                    </div>
                  </div>
                ) : null}
                {sortedHosts.length > 0 ? (
                  (reorderMode && !hostSearch.trim()) ? (
                    <DndContext
                      sensors={dndSensors}
                      collisionDetection={closestCenter}
                      onDragStart={(e: DragStartEvent) => {
                        setActiveDragHostId(String(e.active.id));
                      }}
                      onDragCancel={() => setActiveDragHostId(null)}
                      onDragEnd={async (e: DragEndEvent) => {
                        setActiveDragHostId(null);
                        if (!e.over) return;
                        const fromId = String(e.active.id);
                        const toId = String(e.over.id);
                        if (!fromId || !toId || fromId === toId) return;

                        const ids = sortedHosts.map((h) => h.id);
                        const oldIndex = ids.indexOf(fromId);
                        const newIndex = ids.indexOf(toId);
                        if (oldIndex < 0 || newIndex < 0) return;
                        const nextIds = arrayMove(ids, oldIndex, newIndex);

                        const order = new Map<string, number>();
                        for (let i = 0; i < nextIds.length; i += 1) order.set(nextIds[i], i);
                        const nextHosts = hosts.map((h) =>
                          h.deleted ? h : { ...h, sortOrder: order.get(h.id) ?? h.sortOrder }
                        );
                        await persistHostOrder(nextHosts);
                      }}
                    >
                      <SortableContext items={sortedHosts.map((h) => h.id)} strategy={verticalListSortingStrategy}>
                        {sortedHosts.map((host) => {
                          const left = (
                            <>
                              <div
                                className="text-sm font-semibold break-words leading-snug"
                                title={host.alias || host.hostname || "Unnamed"}
                              >
                                {host.alias || host.hostname || "Unnamed"}
                              </div>
                              <div className="text-[11px] text-muted-foreground break-words leading-snug">
                                {host.user ? `${host.user}@` : ""}{host.hostname}{host.port && host.port !== 22 ? `:${host.port}` : ""}
                              </div>
                              {connectingHosts[host.id] ? (
                                <div className="text-[11px] text-muted-foreground mt-1">
                                  {connectingHosts[host.id]?.count > 1 ? (
                                    <>Connecting · {connectingHosts[host.id]?.count}</>
                                  ) : (
                                    <>Connecting</>
                                  )}
                                  {connectingHosts[host.id]?.stage && connectingHosts[host.id]?.stage !== "connecting"
                                    ? ` (${connectingHosts[host.id]?.stage})`
                                    : ""}
                                  ...
                                </div>
                              ) : null}
                            </>
                          );
                          const right = (
                            <>
                              <button
                                type="button"
                                className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center pointer-events-auto bg-background/70 backdrop-blur ring-1 ring-black/5"
                                onClick={(e) => { e.stopPropagation(); openEditDialog(host); }}
                                title="Edit"
                                aria-label="Edit host"
                              >
                                <Pencil size={16} />
                              </button>
                              <button
                                type="button"
                                className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center pointer-events-auto bg-background/70 backdrop-blur ring-1 ring-black/5"
                                onClick={(e) => { e.stopPropagation(); deleteHost(host); }}
                                title="Delete"
                                aria-label="Delete host"
                              >
                                <Trash2 size={16} />
                              </button>
                            </>
                          );
                          return (
                            <SortableHostRow
                              key={host.id}
                              host={host}
                              reorderMode={reorderMode}
                              hostSearch={hostSearch}
                              left={left}
                              right={right}
                              onRowClick={() => {
                                // no click in reorder mode
                              }}
                            />
                          );
                        })}
                      </SortableContext>
                    </DndContext>
                  ) : (
                    sortedHosts.map((host) => (
                      <div
                        key={host.id}
                        className={[
                          "relative px-3 py-2 rounded-lg mb-1 group",
                          reorderMode ? "cursor-default" : "cursor-pointer hover:bg-muted",
                        ].join(" ")}
                        onClick={() => {
                          if (reorderMode) return;
                          connectToHost(host);
                        }}
                      >
                        <div className="min-w-0">
                          <div className="flex items-start gap-2">
                            <div className="flex-1 min-w-0">
                              <div
                                className="text-sm font-semibold break-words leading-snug"
                                title={host.alias || host.hostname || "Unnamed"}
                              >
                                {host.alias || host.hostname || "Unnamed"}
                              </div>
                              <div className="text-[11px] text-muted-foreground break-words leading-snug">
                                {host.user ? `${host.user}@` : ""}{host.hostname}{host.port && host.port !== 22 ? `:${host.port}` : ""}
                              </div>
                              {connectingHosts[host.id] ? (
                                <div className="text-[11px] text-muted-foreground mt-1">
                                  {connectingHosts[host.id]?.count > 1 ? (
                                    <>Connecting · {connectingHosts[host.id]?.count}</>
                                  ) : (
                                    <>Connecting</>
                                  )}
                                  {connectingHosts[host.id]?.stage && connectingHosts[host.id]?.stage !== "connecting"
                                    ? ` (${connectingHosts[host.id]?.stage})`
                                    : ""}
                                  ...
                                </div>
                              ) : null}
                            </div>
                          </div>
                        </div>
                        <div
                          className={[
                            "absolute right-2 top-2 flex items-center gap-1 transition-opacity pointer-events-none",
                            reorderMode ? "opacity-0" : "opacity-0 group-hover:opacity-100",
                          ].join(" ")}
                        >
                          <button
                            type="button"
                            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center pointer-events-auto bg-background/70 backdrop-blur ring-1 ring-black/5"
                            onClick={(e) => { e.stopPropagation(); openEditDialog(host); }}
                            title="Edit"
                            aria-label="Edit host"
                          >
                            <Pencil size={16} />
                          </button>
                          <button
                            type="button"
                            className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center pointer-events-auto bg-background/70 backdrop-blur ring-1 ring-black/5"
                            onClick={(e) => { e.stopPropagation(); deleteHost(host); }}
                            title="Delete"
                            aria-label="Delete host"
                          >
                            <Trash2 size={16} />
                          </button>
                        </div>
                      </div>
                    ))
                  )
                ) : (
                  <div className="flex flex-col items-center justify-center h-full text-center px-4">
                    <p className="text-sm text-muted-foreground mb-3">No hosts yet</p>
                    <Button onClick={openAddDialog}>Create Host</Button>
                  </div>
                )}
                {!isInTauri ? (
                  <div className="mt-2 px-3 text-[11px] text-muted-foreground">
                    Web Mode: PTY/SSH requires the desktop app.
                  </div>
                ) : null}
              </div>
            </aside>
          </div>
        ) : null}

        <div className="min-w-0 min-h-0 flex flex-col" style={{ background: "var(--app-bg)" } as any}>
          <div
            data-tauri-drag-region
            className={[
              "h-[44px] pt-[4px] pb-0 flex items-center gap-2 pr-3 select-none",
              sidebarOpen ? "pl-3" : "pl-[88px]",
            ].join(" ")}
            style={{ WebkitAppRegion: "drag" } as any}
          >
            {!sidebarOpen ? (
              <button
                type="button"
                title="Show Hosts"
                aria-label="Show Hosts"
                onClick={() => setSidebarOpen(true)}
                className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                data-tauri-drag-region="false"
                style={{ WebkitAppRegion: "no-drag" } as any}
              >
                <PanelLeftOpen size={18} />
              </button>
            ) : null}

            {/* Session list in titlebar (left-aligned) */}
            <div className="min-w-0 flex items-center gap-2 flex-1">
              {sessions.length > 0 ? (
                <div className="flex items-center gap-1 overflow-x-auto h-full max-w-[min(720px,60vw)]">
                  {sessions.map((session) => {
                    const active = session.id === activeSessionId;
                    const exited = session.status === "exited";
                    const idx = sessionIndexById.get(session.id) ?? 1;
                    return (
                      <div
                        key={session.id}
                        className={[
                          "group shrink-0 inline-flex items-center gap-2 h-7 px-2 rounded-lg cursor-pointer",
                          exited
                            ? "bg-[var(--app-chip-failed)] hover:bg-[var(--app-chip-failed-hover)] text-[var(--app-chip-failed-text)] ring-1 ring-red-500/10"
                            : active
                              ? "bg-[var(--app-chip-active)] text-foreground"
                              : "bg-[var(--app-chip)] hover:bg-[var(--app-chip-hover)] text-foreground",
                        ].join(" ")}
                        onClick={() => setActiveSessionId(session.id)}
                        title={
                          exited
                            ? `${session.hostAlias} #${idx} (exited${typeof session.exitCode === "number" ? `, code ${session.exitCode}` : ""})`
                            : `${session.hostAlias} #${idx}`
                        }
                        aria-label={`Switch to ${session.hostAlias}`}
                        data-tauri-drag-region
                        style={{ WebkitAppRegion: "drag" } as any}
                      >
                        {exited ? (
                          <span className="h-2 w-2 rounded-full bg-red-500/70" aria-hidden="true" />
                        ) : null}
                        {idx > 1 ? <span className="text-[11px] font-semibold opacity-70">#{idx}</span> : null}
                        <span className="text-sm font-semibold leading-none whitespace-nowrap">
                          {session.hostAlias}
                        </span>
                        <button
                          type="button"
                          className="h-6 w-6 rounded-md text-muted-foreground hover:text-foreground hover:bg-background/60 inline-flex items-center justify-center"
                          onClick={(e) => { e.stopPropagation(); closeSession(session.id, "user"); }}
                          aria-label="Close session"
                          title="Close"
                          data-tauri-drag-region="false"
                          style={{ WebkitAppRegion: "no-drag" } as any}
                        >
                          ×
                        </button>
                      </div>
                    );
                  })}
                </div>
              ) : (
                <div className="h-full flex items-center px-2 text-sm font-semibold leading-none text-muted-foreground/80">
                  xTermius
                </div>
              )}

              {/* Drag-friendly spacer */}
              <div className="flex-1" />
            </div>

            <div
              className="ml-auto flex items-center gap-1"
              data-tauri-drag-region="false"
              style={{ WebkitAppRegion: "no-drag" } as any}
            >
              <button
                type="button"
                className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                onClick={() => setShowWebdavSync(true)}
                title="WebDAV Sync"
                aria-label="WebDAV Sync"
              >
                <Cloud size={18} />
              </button>
              <button
                type="button"
                className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                onClick={() => setShowSettings(true)}
                title="Settings"
                aria-label="Settings"
              >
                <Settings2 size={18} />
              </button>
              <button
                type="button"
                className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                onClick={openAddDialog}
                title="Add Host"
                aria-label="Add Host"
              >
                <Plus size={18} />
              </button>
            </div>
          </div>

          <main className="flex-1 min-h-0 min-w-0 flex flex-col overflow-hidden">
            <div className="flex-1 min-h-0 pt-1 px-3 pb-3 relative overflow-hidden">
              <div
                ref={terminalContainerRef}
                className="h-full w-full min-h-0 rounded-xl p-2 overflow-hidden"
                data-has-session={sessions.length > 0 ? "1" : "0"}
                style={{ background: "var(--app-term-bg)" } as any}
                onMouseDown={() => {
                  if (activeSessionId) terminalInstance.current?.focus();
                }}
              >
                <div
                  ref={terminalRef}
                  className="h-full w-full"
                  style={
                    sessions.length === 0
                      ? ({ visibility: "hidden", pointerEvents: "none" } as any)
                      : undefined
                  }
                />
              </div>
              {sessions.length === 0 ? (
                <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
                  <div className="text-center text-slate-300">
                    <p className="text-lg mb-2">Select a host to connect</p>
                    <p className="text-sm opacity-80">
                      {sidebarOpen ? "Or click a host from the sidebar" : "Show Hosts from the toolbar"}
                    </p>
                  </div>
                </div>
              ) : null}

              {showDialog ? (
                <div
                  className="absolute inset-0 z-50"
                  data-tauri-drag-region="false"
                  style={{ WebkitAppRegion: "no-drag" } as any}
                >
                  <div
                    className="absolute inset-0 bg-black/20 backdrop-blur-sm"
                    onMouseDown={(e) => { e.stopPropagation(); }}
                    onClick={() => setShowDialog(false)}
                  />
                  <div
                    className="absolute inset-0 overflow-auto p-4"
                    onMouseDown={(e) => { e.stopPropagation(); }}
                    onClick={(e) => { e.stopPropagation(); }}
                  >
                    <div className="mx-auto max-w-4xl">
                      <div className="rounded-2xl bg-background/95 backdrop-blur ring-1 ring-black/10 shadow-xl overflow-hidden">
                        <div className="px-5 py-4 border-b border-border flex items-start gap-3">
                          <div className="min-w-0 flex-1">
                            <div className="text-lg font-semibold">
                              {editingHost ? "Edit Host" : "Add Host"}
                            </div>
                          </div>
                          <button
                            type="button"
                            className="h-8 w-8 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                            onClick={() => setShowDialog(false)}
                            title="Close"
                            aria-label="Close"
                          >
                            ×
                          </button>
                        </div>

                        <div className="px-5 py-5 grid gap-5">
                          <div className="grid gap-4 md:grid-cols-2">
                            <div className="rounded-xl border border-border bg-card/60 p-4">
                              <div className="text-sm font-semibold mb-3">Connection</div>
                              <div className="grid gap-3">
                                <div className="grid gap-2">
                                  <label className="text-sm font-medium">Hostname</label>
                                  <Input
                                    autoFocus
                                    value={formData.hostname || ""}
                                    onChange={e => setFormData({ ...formData, hostname: e.target.value, name: e.target.value })}
                                    placeholder="192.168.1.1 or server.example.com"
                                  />
                                </div>
                                <div className="grid gap-2">
                                  <label className="text-sm font-medium">Alias</label>
                                  <Input
                                    value={formData.alias || ""}
                                    onChange={e => setFormData({ ...formData, alias: e.target.value })}
                                    placeholder="my-server"
                                  />
                                </div>
                                <div className="grid gap-3 grid-cols-2">
                                  <div className="grid gap-2">
                                    <label className="text-sm font-medium">User</label>
                                    <Input
                                      value={formData.user || ""}
                                      onChange={e => setFormData({ ...formData, user: e.target.value })}
                                      placeholder="root"
                                    />
                                  </div>
                                  <div className="grid gap-2">
                                    <label className="text-sm font-medium">Port</label>
                                    <Input
                                      type="number"
                                      value={formData.port || 22}
                                      onChange={e => setFormData({ ...formData, port: parseInt(e.target.value || "22", 10) })}
                                    />
                                  </div>
                                </div>
                              </div>
                            </div>

                            <div className="rounded-xl border border-border bg-card/60 p-4">
                              <div className="text-sm font-semibold mb-3">Authentication</div>
                              <div className="grid gap-3">
                                <div className="grid gap-2">
                                  <div className="flex items-center justify-between gap-2">
                                    <label className="text-sm font-medium">Password</label>
                                    {(editingHost?.hasPassword || formData.hasPassword) ? (
                                      <span className="text-[11px] px-2 py-0.5 rounded-full bg-muted/40 text-muted-foreground">
                                        Saved
                                      </span>
                                    ) : null}
                                  </div>
                                  <div className="flex items-center gap-2">
                                    <Input
                                      type="password"
                                      value={formData.password ?? ""}
                                      onChange={e => setFormData({ ...formData, password: e.target.value })}
                                      placeholder={(editingHost?.hasPassword || formData.hasPassword) ? "Saved in Keychain (leave empty to keep)" : "Leave empty to prompt"}
                                      className="flex-1"
                                    />
                                    {(editingHost?.hasPassword || formData.hasPassword) ? (
                                      <Button
                                        type="button"
                                        variant="outline"
                                        size="sm"
                                        onClick={async () => {
                                          if (!isInTauri) return;
                                          if (!editingHost) return;
                                          const ok = await confirm(
                                            "Reveal the saved password from Keychain on this device?",
                                            { title: "Reveal Password", kind: "warning" }
                                          );
                                          if (!ok) return;
                                          try {
                                            const pw = await invoke<string | null>("host_password_get", { hostId: editingHost.id });
                                            setFormData((prev) => ({ ...prev, password: pw ?? "" }));
                                          } catch (e) {
                                            await message(`Failed to read password from Keychain.\n\n${String(e)}`, { title: "Keychain", kind: "error" });
                                          }
                                        }}
                                      >
                                        Reveal
                                      </Button>
                                    ) : null}
                                    <Button
                                      type="button"
                                      variant="outline"
                                      size="sm"
                                      onClick={() => setFormData((prev) => ({ ...prev, password: "", hasPassword: false }))}
                                      title="Clear saved password"
                                      aria-label="Clear saved password"
                                    >
                                      Clear
                                    </Button>
                                  </div>
                                  <div className="text-[11px] text-muted-foreground">
                                    Passwords are stored in Keychain on this device and are not synced via WebDAV.
                                  </div>
                                </div>
                                <div className="grid gap-2">
                                  <label className="text-sm font-medium">Identity File</label>
                                  <div className="flex gap-2">
                                    <Input
                                      value={formData.identityFile || ""}
                                      onChange={e => setFormData({ ...formData, identityFile: e.target.value })}
                                      placeholder="~/.ssh/id_rsa"
                                      className="flex-1"
                                    />
                                    <Button type="button" variant="outline" size="sm" onClick={selectIdentityFile}>
                                      Browse
                                    </Button>
                                  </div>
                                </div>
                                <div className="grid gap-2">
                                  <label className="text-sm font-medium">Proxy Jump</label>
                                  <Input
                                    value={formData.proxyJump || ""}
                                    onChange={e => setFormData({ ...formData, proxyJump: e.target.value })}
                                    placeholder="jump-host or user@jump:port"
                                  />
                                </div>
                              </div>
                            </div>
                          </div>

                          <details className="rounded-xl border border-border bg-card/40 p-4">
                            <summary className="cursor-pointer select-none text-sm font-semibold">
                              Advanced
                            </summary>
                            <div className="grid gap-4 mt-4">
                              <div className="grid gap-2">
                                <label className="text-sm font-medium">Environment Variables</label>
                                <Input
                                  value={formData.envVars || ""}
                                  onChange={e => setFormData({ ...formData, envVars: e.target.value })}
                                  placeholder="VAR1=value1, VAR2=value2"
                                />
                              </div>
                              <div className="grid gap-2">
                                <label className="text-sm font-medium">Encoding</label>
                                <div className="relative">
                                  <select
                                    className="h-10 w-full appearance-none rounded-lg border border-border bg-background/70 px-3 pr-9 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                    value={formData.encoding || "utf-8"}
                                    onChange={e => setFormData({ ...formData, encoding: e.target.value })}
                                  >
                                    <option value="utf-8">UTF-8 (Default)</option>
                                    <option value="gbk">GBK</option>
                                    <option value="gb2312">GB2312</option>
                                    <option value="big5">Big5</option>
                                    <option value="shift-jis">Shift-JIS</option>
                                    <option value="euc-kr">EUC-KR</option>
                                  </select>
                                  <ChevronDown
                                    size={16}
                                    className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground"
                                    aria-hidden="true"
                                  />
                                </div>
                              </div>
                              <div className="grid gap-2">
                                <label className="text-sm font-medium">Notes</label>
                                <textarea
                                  className="min-h-[96px] w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                  value={formData.notes || ""}
                                  onChange={(e) => setFormData({ ...formData, notes: e.target.value })}
                                  placeholder="Optional"
                                />
                              </div>
                            </div>
                          </details>
                        </div>

                        <div className="px-5 py-4 border-t border-border flex items-center justify-end gap-2">
                          <Button variant="outline" onClick={() => setShowDialog(false)}>Cancel</Button>
                          <Button onClick={handleSave}>Save</Button>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              ) : null}
            </div>
          </main>
        </div>
      </div>

      <Dialog open={showSettings} onOpenChange={setShowSettings}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>Settings</DialogTitle>
          </DialogHeader>
          <div className="grid gap-4 py-2">
            <div className="grid gap-2">
              <div className="text-sm font-medium">Appearance</div>
              <div className="flex rounded-lg border border-border bg-card p-1">
                {(["system", "light", "dark"] as ThemeMode[]).map((m) => (
                  <button
                    key={m}
                    type="button"
                    className={[
                      "flex-1 h-8 rounded-md text-sm",
                      themeMode === m ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground",
                    ].join(" ")}
                    onClick={() => setThemeModeState(m)}
                  >
                    {m === "system" ? "System" : m === "light" ? "Light" : "Dark"}
                  </button>
                ))}
              </div>
              <div className="text-xs text-muted-foreground">
                Choose Light/Dark or follow system appearance.
              </div>
            </div>

            <div className="grid gap-2">
              <div className="text-sm font-medium">Sync</div>
              <div className="flex items-center justify-between rounded-lg border border-border bg-card px-3 py-2">
                <div className="text-sm text-muted-foreground">WebDAV</div>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    setShowSettings(false);
                    setShowWebdavSync(true);
                  }}
                >
                  Open
                </Button>
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowSettings(false)}>Close</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={showWebdavSync} onOpenChange={setShowWebdavSync}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>WebDAV Sync</DialogTitle>
          </DialogHeader>
          <div className="grid gap-3 py-1">
            <div className="grid gap-2">
              <div className="text-sm font-medium">WebDAV Settings</div>
              <div className="grid gap-2">
                <label className="text-xs text-muted-foreground">WebDAV URL</label>
                <Input
                  value={settings.webdav_url ?? ""}
                  onChange={(e) => setSettings((s) => ({ ...s, webdav_url: e.target.value }))}
                  placeholder="https://dav.example.com/dav/"
                />
              </div>
              <div className="grid gap-2">
                <label className="text-xs text-muted-foreground">Remote folder</label>
                <Input
                  value={settings.webdav_folder ?? "xTermius"}
                  onChange={(e) => setSettings((s) => ({ ...s, webdav_folder: e.target.value }))}
                  placeholder=""
                />
              </div>
              <div className="grid gap-2">
                <label className="text-xs text-muted-foreground">Username</label>
                <Input
                  value={settings.webdav_username ?? ""}
                  onChange={(e) => setSettings((s) => ({ ...s, webdav_username: e.target.value }))}
                  placeholder=""
                />
              </div>
              <div className="grid gap-2">
                <label className="text-xs text-muted-foreground">Password</label>
                <Input
                  type="password"
                  value={settings.webdav_password ?? ""}
                  onChange={(e) => setSettings((s) => ({ ...s, webdav_password: e.target.value }))}
                  placeholder=""
                />
              </div>
            </div>

            <div className="rounded-lg border border-border bg-muted/25 px-3 py-2 grid gap-1 text-xs text-muted-foreground">
              <div>
                Remote file:{" "}
                <code className="font-mono">
                  {resolveWebdavHostsDbUrl(settings.webdav_url ?? "", settings.webdav_folder ?? "xTermius") || "(not set)"}
                </code>
              </div>
              <div>
                Local file: <code className="font-mono">{localHostsDbPath || "(unknown)"}</code>
              </div>
            </div>
            {syncBusy ? (
              <div className="text-xs text-muted-foreground">
                {syncBusy === "save" ? "Saving..." : syncBusy === "pull" ? "Pulling..." : "Pushing..."}
              </div>
            ) : syncNotice ? (
              <div className={"text-xs " + (syncNotice.kind === "ok" ? "text-foreground/70" : "text-destructive")}>
                {syncNotice.text}
              </div>
            ) : null}
          </div>
          <DialogFooter className="sm:justify-between sm:space-x-0">
            <Button
              variant="outline"
              disabled={!isInTauri || syncBusy !== null}
              onClick={() => setShowSettings(true)}
            >
              Settings
            </Button>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                disabled={!isInTauri || syncBusy !== null}
                onClick={async () => {
                  if (!isInTauri) return;
                  setSyncBusy("save");
                  setSyncNotice(null);
                  try {
                    await invoke("settings_save", { settings });
                    setSyncNotice({ kind: "ok", text: "Saved" });
                  } catch (e) {
                    const msg = `Failed to save settings.\n\n${String(e)}`;
                    setSyncNotice({ kind: "err", text: "Save failed" });
                    try {
                      await message(msg, { title: "WebDAV", kind: "error" });
                    } catch {
                      // Ignore.
                    }
                  } finally {
                    setSyncBusy(null);
                  }
                }}
              >
                Save
              </Button>
              <Button variant="outline" disabled={!isInTauri || syncBusy !== null} onClick={doWebdavPull}>
                Pull
              </Button>
              <Button variant="default" disabled={!isInTauri || syncBusy !== null} onClick={doWebdavPush}>
                Push
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>

    </div>
  );
}

export default App;
