import { useState, useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { configDir } from "@tauri-apps/api/path";
import { open } from "@tauri-apps/plugin-dialog";
import "@xterm/xterm/css/xterm.css";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  Plus,
  Settings2,
  Trash2,
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
  identityFile?: string;
  proxyJump?: string;
  envVars?: string;
  encoding?: string;
  tags: string[];
  notes: string;
  updatedAt: string;
  deleted: boolean;
}

interface Session {
  id: string;
  hostAlias: string;
}

function App() {
  const [hosts, setHosts] = useState<Host[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [connectingHostId, setConnectingHostId] = useState<string | null>(null);
  const [connectingStage, setConnectingStage] = useState<string | null>(null);
  const [terminalReady, setTerminalReady] = useState(false);
  const [showDialog, setShowDialog] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [editingHost, setEditingHost] = useState<Host | null>(null);
  const [formData, setFormData] = useState<Partial<Host>>({});
  const [isInTauri] = useState(() => {
    const w = window as any;
    return !!(w.__TAURI__ || w.__TAURI_INTERNALS__);
  });
  const terminalRef = useRef<HTMLDivElement>(null);
  const terminalInstance = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);
  const sessionBuffers = useRef(new Map<string, string>());
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

  useEffect(() => {
    activeSessionIdRef.current = activeSessionId;
  }, [activeSessionId]);

  useEffect(() => {
    // Keep persisted theme in sync with UI state.
    setThemeMode(themeMode);
  }, [themeMode]);

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
      if (activeSessionIdRef.current === session_id && terminalInstance.current) {
        terminalInstance.current.write(data);
      } else {
        sessionBuffers.current.set(session_id, (sessionBuffers.current.get(session_id) ?? "") + data);
      }
    });

    const unlistenExitP = listen<{ session_id: string; code: number }>("pty:exit", (event) => {
      const { session_id } = event.payload;
      // Clear UI state for the exited session. Don't append exit text to the terminal.
      if (terminalInstance.current && activeSessionIdRef.current === session_id) {
        terminalInstance.current.clear();
      }
      sessionBuffers.current.delete(session_id);
      setSessions((prev) => prev.filter((s) => s.id !== session_id));
      setActiveSessionId((prev) => (prev === session_id ? null : prev));
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
    terminalInstance.current?.clear();
  }, [sessions.length, terminalReady]);

  useEffect(() => {
    if (terminalRef.current && !terminalInstance.current) {
      const term = new Terminal({
        cursorBlink: true,
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
      term.focus();
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
    const el = terminalRef.current;
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
    if (!activeSessionId) return;

    // Basic tab switching support: clear and replay buffer for the selected session.
    // (Long-term: give each session its own xterm instance.)
    term.clear();
    const buf = sessionBuffers.current.get(activeSessionId);
    if (buf) term.write(buf);
    requestAnimationFrame(() => fitAndResizeActivePty());
    requestAnimationFrame(() => term.focus());
  }, [activeSessionId, terminalReady]);

  async function loadHosts() {
    try {
      if (isInTauri) {
        const data = await invoke<Host[]>("hosts_load");
        setHosts(data);
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
    try {
      await invoke("hosts_save", { hosts: newHosts });
    } catch (e) {
      localStorage.setItem("xtermius_hosts", JSON.stringify(newHosts));
    }
  }

  async function connectToHost(host: Host) {
    if (!isInTauri) {
      alert("SSH only works in the desktop app (Tauri).");
      return;
    }

    try {
      console.debug("[ui] connect click", { hostAlias: host.alias, host: host.hostname });
      setConnectingHostId(host.id);
      setConnectingStage("config");
      const cd = await withTimeout(configDir().catch(() => ""), 5000, "configDir()");
      const sshConfigPath = `${cd}/xtermius/ssh_config`;
      
      setConnectingStage("save");
      await withTimeout(invoke("hosts_save", { hosts: hosts }), 5000, "hosts_save");

      const term = terminalInstance.current;
      const cols = term?.cols ?? 80;
      const rows = term?.rows ?? 24;

      setConnectingStage("spawn");
      const sessionId = await withTimeout(invoke<string>("pty_spawn", {
        file: "/usr/bin/ssh",
        args: ["-F", sshConfigPath, host.alias],
        cwd: null,
        env: {},
        cols,
        rows,
      }), 10000, "pty spawn");
      console.debug("[pty] spawned", { sessionId });
      setSessions(prev => [...prev, { id: sessionId.toString(), hostAlias: host.alias }]);
      setActiveSessionId(sessionId.toString());
      setConnectingStage("read");
      requestAnimationFrame(() => terminalInstance.current?.focus());
    } catch (e) {
      console.error("Failed to connect:", e);
      alert("Failed to connect: " + e);
    } finally {
      setConnectingHostId(null);
      setConnectingStage(null);
    }
  }

  async function closeSession(sessionId: string) {
    if (isInTauri) {
      try { await invoke("pty_kill", { sessionId }); } 
      catch (e) { console.error(e); }
    }
    sessionBuffers.current.delete(sessionId);
    setSessions(prev => prev.filter(s => s.id !== sessionId));
    if (activeSessionId === sessionId) setActiveSessionId(null);
  }

  async function selectIdentityFile() {
    if (!isInTauri) {
      alert("File selection only works in the desktop app");
      return;
    }
    try {
      const selected = await open({
        multiple: false,
        filters: [{
          name: "SSH Key",
          extensions: ["pem", "ppk", "key", "*"]
        }]
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
    
    if (editingHost) {
      newHosts = hosts.map(h => h.id === editingHost.id ? { ...h, ...formData, updatedAt: now } as Host : h);
    } else {
      const newHost: Host = {
        id: Date.now().toString(),
        name: formData.hostname || "Unnamed",
        alias: formData.alias || formData.hostname?.toLowerCase().replace(/\s+/g, "-") || "new-host",
        hostname: formData.hostname || "",
        user: formData.user || "",
        port: formData.port || 22,
        password: formData.password,
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
    }
    
    await saveHostsToBackend(newHosts);
    setHosts(newHosts);
    setShowDialog(false);
  }

  async function deleteHost(host: Host) {
    if (!confirm(`Delete "${host.hostname}"?`)) return;
    
    const newHosts = hosts.map(h => h.id === host.id ? { ...h, deleted: true, updatedAt: new Date().toISOString() } : h);
    await saveHostsToBackend(newHosts);
    setHosts(newHosts);
  }

  const activeHosts = hosts.filter(h => !h.deleted);

  return (
    <div className="h-screen text-foreground overflow-hidden" style={{ background: "var(--app-bg)" } as any}>
      {/* Unified macOS titlebar + content area (sidebar left, shell right). */}
      <div className={["grid h-full min-h-0 min-w-0", sidebarOpen ? "grid-cols-[288px_1fr]" : "grid-cols-1"].join(" ")}>
        {sidebarOpen ? (
          <div className="min-w-0 flex flex-col" style={{ background: "var(--app-sidebar-bg)" } as any}>
            <div
              data-tauri-drag-region
              className="h-12 flex items-center gap-2 pl-[88px] pr-2"
              style={{ WebkitAppRegion: "drag" } as any}
            >
              <button
                type="button"
                title="Hide Hosts"
                aria-label="Hide Hosts"
                onClick={() => setSidebarOpen(false)}
                className="h-8 w-8 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                data-tauri-drag-region="false"
                style={{ WebkitAppRegion: "no-drag" } as any}
              >
                <PanelLeftClose size={18} />
              </button>
              <div className="flex-1" />
            </div>

            <aside className="flex-1 min-h-0 overflow-hidden">
              <div className="h-full overflow-auto px-2 py-2">
                {activeHosts.length > 0 ? (
                  activeHosts.map((host) => (
                    <div
                      key={host.id}
                      className="px-3 py-2 rounded-lg cursor-pointer hover:bg-muted mb-1 group flex items-start gap-2"
                      onClick={() => connectToHost(host)}
                    >
                      <div className="min-w-0 flex-1">
                        <div className="text-sm font-semibold truncate">
                          {host.alias || host.hostname || "Unnamed"}
                        </div>
                        <div className="text-[11px] text-muted-foreground truncate">
                          {host.user ? `${host.user}@` : ""}{host.hostname}{host.port && host.port !== 22 ? `:${host.port}` : ""}
                        </div>
                        {connectingHostId === host.id ? (
                          <div className="text-[11px] text-muted-foreground mt-1">
                            Connecting{connectingStage ? ` (${connectingStage})` : ""}...
                          </div>
                        ) : null}
                      </div>
                      <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                        <button
                          type="button"
                          className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                          onClick={(e) => { e.stopPropagation(); openEditDialog(host); }}
                          title="Edit"
                          aria-label="Edit host"
                        >
                          <Pencil size={16} />
                        </button>
                        <button
                          type="button"
                          className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                          onClick={(e) => { e.stopPropagation(); deleteHost(host); }}
                          title="Delete"
                          aria-label="Delete host"
                        >
                          <Trash2 size={16} />
                        </button>
                      </div>
                    </div>
                  ))
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

        <div className="min-w-0 flex flex-col" style={{ background: "var(--app-bg)" } as any}>
          <div
            data-tauri-drag-region
            className={[
              "h-12 flex items-center gap-2 pr-3 select-none",
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
                className="h-8 w-8 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                data-tauri-drag-region="false"
                style={{ WebkitAppRegion: "no-drag" } as any}
              >
                <PanelLeftOpen size={18} />
              </button>
            ) : null}

            {/* Session list in titlebar (left-aligned) */}
            <div className="min-w-0 flex items-center gap-2 flex-1">
              {sessions.length > 0 ? (
                <div className="flex items-center gap-1 overflow-x-auto py-1 max-w-[min(720px,60vw)]">
                  {sessions.map((session) => {
                    const active = session.id === activeSessionId;
                    return (
                      <div
                        key={session.id}
                        className={[
                          "group shrink-0 inline-flex items-center gap-2 h-8 px-2 rounded-lg cursor-pointer",
                          active ? "bg-[var(--app-chip-active)] text-foreground" : "bg-[var(--app-chip)] hover:bg-[var(--app-chip-hover)] text-foreground",
                        ].join(" ")}
                        onClick={() => setActiveSessionId(session.id)}
                        title={session.hostAlias}
                        aria-label={`Switch to ${session.hostAlias}`}
                        data-tauri-drag-region="false"
                        style={{ WebkitAppRegion: "no-drag" } as any}
                      >
                        <span className="text-sm font-semibold max-w-[220px] truncate">
                          {session.hostAlias}
                        </span>
                        <button
                          type="button"
                          className="h-7 w-7 rounded-md leading-4 text-muted-foreground hover:text-foreground hover:bg-background/60"
                          onClick={(e) => { e.stopPropagation(); closeSession(session.id); }}
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
                <div className="h-8 flex items-center px-2 text-sm font-semibold text-muted-foreground/80">
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
                className="h-8 w-8 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                onClick={() => setShowSettings(true)}
                title="Settings"
                aria-label="Settings"
              >
                <Settings2 size={18} />
              </button>
              <button
                type="button"
                className="h-8 w-8 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                onClick={openAddDialog}
                title="Add Host"
                aria-label="Add Host"
              >
                <Plus size={18} />
              </button>
            </div>
          </div>

          <main className="flex-1 min-h-0 min-w-0 flex flex-col overflow-hidden">
            <div className="flex-1 min-h-0 p-3 relative overflow-hidden">
              <div
                className="h-full w-full rounded-xl p-3 overflow-hidden"
                style={{ background: "var(--app-term-bg)" } as any}
                onMouseDown={() => terminalInstance.current?.focus()}
              >
                <div ref={terminalRef} className="h-full w-full" />
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
            </div>
          </main>
        </div>
      </div>

      <Dialog open={showDialog} onOpenChange={setShowDialog}>
        <DialogContent className="max-w-md max-h-[90vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{editingHost ? "Edit Host" : "Add Host"}</DialogTitle>
          </DialogHeader>
          <div className="grid gap-4 py-4">
            <div className="grid gap-2">
              <label className="text-sm font-medium">Hostname</label>
              <Input 
                value={formData.hostname || ""} 
                onChange={e => setFormData({...formData, hostname: e.target.value, name: e.target.value})}
                placeholder="192.168.1.1 or server.example.com"
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium">Alias</label>
              <Input 
                value={formData.alias || ""} 
                onChange={e => setFormData({...formData, alias: e.target.value})}
                placeholder="my-server"
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium">User</label>
              <Input 
                value={formData.user || ""} 
                onChange={e => setFormData({...formData, user: e.target.value})}
                placeholder="root"
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium">Port</label>
              <Input 
                type="number"
                value={formData.port || 22} 
                onChange={e => setFormData({...formData, port: parseInt(e.target.value)})}
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium">Password</label>
              <Input 
                type="password"
                value={formData.password || ""} 
                onChange={e => setFormData({...formData, password: e.target.value})}
                placeholder="Leave empty to prompt"
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium">Identity File</label>
              <div className="flex gap-2">
                <Input 
                  value={formData.identityFile || ""} 
                  onChange={e => setFormData({...formData, identityFile: e.target.value})}
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
                onChange={e => setFormData({...formData, proxyJump: e.target.value})}
                placeholder="jump-host or user@jump:port"
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium">Environment Variables</label>
              <Input 
                value={formData.envVars || ""} 
                onChange={e => setFormData({...formData, envVars: e.target.value})}
                placeholder="VAR1=value1, VAR2=value2"
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium">Encoding</label>
              <select 
                className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                value={formData.encoding || "utf-8"}
                onChange={e => setFormData({...formData, encoding: e.target.value})}
              >
                <option value="utf-8">UTF-8 (Default)</option>
                <option value="gbk">GBK</option>
                <option value="gb2312">GB2312</option>
                <option value="big5">Big5</option>
                <option value="shift-jis">Shift-JIS</option>
                <option value="euc-kr">EUC-KR</option>
              </select>
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium">Notes</label>
              <Input 
                value={formData.notes || ""} 
                onChange={e => setFormData({...formData, notes: e.target.value})}
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowDialog(false)}>Cancel</Button>
            <Button onClick={handleSave}>Save</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

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
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowSettings(false)}>Close</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export default App;
