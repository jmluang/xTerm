import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export type McpClientSummary = {
  clientId: string;
  label?: string | null;
  createdAtMs: number;
  grants: string[];
};

export type McpConnectionView = {
  connectionId: string;
  generation: number;
  hostName: string;
  user: string;
  alias: string;
  state: string;
  requiresReconnect: boolean;
};

export type McpPendingTask = {
  taskId: string;
  clientId: string;
  connectionId: string;
  generation: number;
  command: string;
  status: string;
  createdAtMs: number;
};

const WARNING_TEXT =
  "Approved commands run with your SSH account's full permissions on this machine's connection. " +
  "Never approve a command you do not fully understand.";

export function McpPanel() {
  const [clients, setClients] = useState<McpClientSummary[]>([]);
  const [connections, setConnections] = useState<McpConnectionView[]>([]);
  const [pending, setPending] = useState<McpPendingTask[]>([]);
  const [pairLabel, setPairLabel] = useState("");
  const [freshToken, setFreshToken] = useState<null | { clientId: string; token: string }>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [clients, connections, pending] = await Promise.all([
        invoke<McpClientSummary[]>("mcp_list_clients"),
        invoke<McpConnectionView[]>("mcp_list_connections"),
        invoke<McpPendingTask[]>("mcp_pending_tasks"),
      ]);
      setClients(clients);
      setConnections(connections);
      setPending(pending);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  // Connections open/close and task state change while the panel is open.
  useEffect(() => {
    const unlisten = listen("pty:exit", () => void refresh());
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  async function createPairing() {
    try {
      const created = await invoke<{ clientId: string; token: string }>("mcp_pair_client", {
        label: pairLabel.trim() || null,
      });
      setFreshToken(created);
      setPairLabel("");
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  async function unpair(clientId: string) {
    try {
      await invoke("mcp_unpair_client", { clientId });
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  async function grant(clientId: string, connectionId: string) {
    try {
      await invoke("mcp_grant_connection", { clientId, connectionId });
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  async function revoke(clientId: string, connectionId: string) {
    try {
      await invoke("mcp_revoke_connection", { clientId, connectionId });
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  async function decide(taskId: string, approve: boolean) {
    try {
      await invoke(approve ? "mcp_approve_task" : "mcp_reject_task", { taskId });
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="mx-auto max-w-4xl grid gap-4">
      <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-3">
        <div className="text-lg font-semibold">MCP (Beta)</div>
        <p className="text-xs text-muted-foreground">
          Local agents pair with xTermius and can only see connections you grant, read output
          produced after the grant, and run commands one-by-one after your approval. Reconnecting
          a host invalidates previous grants automatically.
        </p>
        {error && <div className="text-xs text-red-500">{error}</div>}
      </div>

      {pending.length > 0 && (
        <div className="rounded-2xl border border-amber-500/40 bg-amber-500/5 p-5 grid gap-3">
          <div className="text-sm font-semibold text-amber-600 dark:text-amber-400">
            Pending command approvals ({pending.length})
          </div>
          <div className="text-xs text-muted-foreground">{WARNING_TEXT}</div>
          {pending.map((task) => (
            <div
              key={task.taskId}
              className="rounded-xl border border-border bg-card p-3 grid gap-2"
            >
              <div className="font-mono text-xs break-all">$ {task.command}</div>
              <div className="text-xs text-muted-foreground">
                client {task.clientId.slice(0, 8)} · connection {task.connectionId.slice(0, 8)} ·
                generation {task.generation}
              </div>
              <div className="flex gap-2">
                <Button type="button" size="sm" onClick={() => void decide(task.taskId, true)}>
                  Approve &amp; Run
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => void decide(task.taskId, false)}
                >
                  Deny
                </Button>
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-3">
        <div className="text-sm font-medium">Pair a client</div>
        <div className="flex gap-2">
          <Input
            value={pairLabel}
            onChange={(event) => setPairLabel(event.target.value)}
            placeholder="Client label (e.g. Claude Code)"
            className="max-w-xs"
          />
          <Button type="button" onClick={() => void createPairing()}>
            Generate Pairing Token
          </Button>
        </div>
        {freshToken && (
          <div className="rounded-xl border border-border p-3 grid gap-1 text-xs">
            <div className="font-medium">Token shown once — copy it now:</div>
            <code className="break-all font-mono">{freshToken.token}</code>
            <div className="text-muted-foreground">client id: {freshToken.clientId}</div>
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="w-fit"
              onClick={() => setFreshToken(null)}
            >
              I saved it
            </Button>
          </div>
        )}
        {clients.length > 0 && (
          <div className="grid gap-2">
            {clients.map((client) => (
              <div
                key={client.clientId}
                className="rounded-xl border border-border p-3 flex items-center justify-between gap-3"
              >
                <div className="min-w-0">
                  <div className="text-sm truncate">{client.label || client.clientId.slice(0, 8)}</div>
                  <div className="text-xs text-muted-foreground">
                    {client.grants.length} granted connection(s)
                  </div>
                </div>
                <Button type="button" size="sm" variant="outline" onClick={() => void unpair(client.clientId)}>
                  Unpair
                </Button>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="rounded-2xl border border-border bg-card/80 p-5 grid gap-3">
        <div className="text-sm font-medium">Connections</div>
        {connections.length === 0 && (
          <div className="text-xs text-muted-foreground">No active connections.</div>
        )}
        <div className="grid gap-2">
          {connections.map((connection) => (
            <div key={connection.connectionId} className="rounded-xl border border-border p-3 grid gap-2">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <div className="text-sm truncate">
                    {connection.alias || connection.hostName} ({connection.user}@{connection.hostName})
                  </div>
                  <div className="text-xs text-muted-foreground">
                    state: {connection.state}
                    {connection.requiresReconnect && " · reconnect to enable MCP"}
                  </div>
                </div>
              </div>
              {connection.requiresReconnect ? (
                <div className="text-xs text-muted-foreground">
                  This connection predates managed reuse — close and reconnect it to enable MCP.
                </div>
              ) : (
                <div className="grid gap-2">
                  {clients.map((client) => {
                    const granted = client.grants.includes(connection.connectionId);
                    return (
                      <div key={client.clientId} className="flex items-center justify-between gap-3 text-xs">
                        <span className="truncate">{client.label || client.clientId.slice(0, 8)}</span>
                        {granted ? (
                          <Button
                            type="button"
                            size="sm"
                            variant="outline"
                            onClick={() => void revoke(client.clientId, connection.connectionId)}
                          >
                            Revoke
                          </Button>
                        ) : (
                          <Button
                            type="button"
                            size="sm"
                            onClick={() => void grant(client.clientId, connection.connectionId)}
                          >
                            Grant
                          </Button>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
