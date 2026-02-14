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
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";

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

  useEffect(() => {
    activeSessionIdRef.current = activeSessionId;
  }, [activeSessionId]);

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
      const { session_id, code } = event.payload;
      if (terminalInstance.current && activeSessionIdRef.current === session_id) {
        terminalInstance.current.write(`\r\n[Session exited: ${code}]\r\n`);
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
    if (terminalRef.current && !terminalInstance.current) {
      const term = new Terminal({
        cursorBlink: true,
        fontSize: 14,
        fontFamily: "Menlo, Monaco, 'Courier New', monospace",
        theme: { background: "#1e1e1e" },
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(terminalRef.current);
      fit.fit();
      term.focus();
      terminalInstance.current = term;
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
      <div className="flex h-screen">
      <div className="w-64 border-r bg-slate-50 flex flex-col">
        <div className="p-4 border-b flex items-center justify-between gap-2">
          <h2 className="font-semibold">Hosts</h2>
          <Button size="sm" onClick={openAddDialog}>Add</Button>
        </div>
        <div className="flex-1 overflow-auto p-2">
          {activeHosts.length > 0 ? (
            activeHosts.map((host) => (
              <div
                key={host.id}
                className="p-3 rounded-md cursor-pointer hover:bg-slate-100 mb-1 group"
                onClick={() => connectToHost(host)}
              >
                <div className="font-medium">{host.hostname || "Unnamed"}</div>
                <div className="text-sm text-slate-500">{host.user}@{host.hostname}</div>
                {connectingHostId === host.id ? (
                  <div className="text-xs text-slate-400 mt-1">
                    Connecting{connectingStage ? ` (${connectingStage})` : ""}...
                  </div>
                ) : null}
                <div className="flex gap-2 mt-2 opacity-0 group-hover:opacity-100 transition-opacity">
                  <Button variant="outline" size="sm" onClick={(e) => { e.stopPropagation(); openEditDialog(host); }}>
                    Edit
                  </Button>
                  <Button variant="destructive" size="sm" onClick={(e) => { e.stopPropagation(); deleteHost(host); }}>
                    Del
                  </Button>
                </div>
              </div>
            ))
          ) : (
            <div className="flex flex-col items-center justify-center h-full text-center">
              <p className="text-slate-500 mb-4">No hosts yet</p>
              <Button onClick={openAddDialog}>Create Host</Button>
            </div>
          )}
        </div>
        {!isInTauri && (
          <div className="p-3 text-xs text-slate-500 text-center border-t">
            Web Mode - SSH requires desktop app
          </div>
        )}
      </div>
      
      <div className="flex-1 flex flex-col min-h-0">
        {sessions.length > 0 ? (
          <Tabs value={activeSessionId || ""} onValueChange={setActiveSessionId} className="shrink-0">
            <TabsList className="justify-start rounded-none h-9 bg-slate-100 border-b w-full">
              {sessions.map((session) => (
                <TabsTrigger key={session.id} value={session.id} asChild>
                  <div className="inline-flex items-center gap-2 px-3">
                    <span>{session.hostAlias}</span>
                    <button
                      type="button"
                      className="h-4 w-4 p-0 leading-4 text-slate-600 hover:text-slate-900"
                      onClick={(e) => { e.stopPropagation(); closeSession(session.id); }}
                      aria-label="Close tab"
                      title="Close"
                    >
                      ×
                    </button>
                  </div>
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
        ) : (
          <div className="h-9 bg-slate-100 border-b flex items-center px-3 text-sm text-slate-500">
            No active session
          </div>
        )}

        <div className="flex-1 bg-slate-900 p-2 min-h-0 relative">
          <div
            ref={terminalRef}
            className="h-full w-full"
            onMouseDown={() => terminalInstance.current?.focus()}
          />
          {sessions.length === 0 ? (
            <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
              <div className="text-center text-slate-400">
                <p className="text-lg mb-2">Select a host to connect</p>
                <p className="text-sm">Or click a host from the sidebar</p>
              </div>
            </div>
          ) : null}
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
    </div>
  );
}

export default App;
