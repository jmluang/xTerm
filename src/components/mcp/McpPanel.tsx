import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export type McpGrantSummary = {
  connectionId: string;
  generation: number;
  observe: boolean;
  execute: boolean;
};

export type McpClientSummary = {
  clientId: string;
  label?: string | null;
  createdAtMs: number;
  grants: McpGrantSummary[];
};

export type McpConnectionView = {
  connectionId: string;
  generation: number;
  hostName: string;
  user: string;
  port: number;
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
  workingDirectory: string;
  timeoutSeconds: number;
  status: string;
  createdAtMs: number;
};

export type McpTaskAuditRecord = {
  appInstanceId: string;
  taskId: string;
  clientId: string;
  connectionId: string;
  generation: number;
  requestId: string;
  requestDigest: string;
  command: string;
  workingDirectory: string;
  timeoutSeconds: number;
  createdAtMs: number;
  approvedAtMs: number | null;
  startedAtMs: number | null;
  endedAtMs: number | null;
  status: string;
  exitCode: number | null;
  detail: string | null;
  auditError: string | null;
};

export type McpBridgeInfo = {
  executablePath: string;
  environment: "development" | "packaged" | string;
};

type McpStatus = { enabled: boolean };

const APPROVAL_WINDOW_MS = 5 * 60 * 1000;

const WARNING_TEXT =
  "Observe sends terminal data to an external model; xTermius cannot guarantee secret detection. " +
  "Execute uses this SSH account's full privileges (which may be root), and every command still waits for approval.";

function visibleControls(value: string): string {
  return value.replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]/gu, (character) => {
    const namedEscapes: Record<string, string> = {
      "\u0000": "\\0",
      "\b": "\\b",
      "\t": "\\t",
      "\n": "\\n",
      "\f": "\\f",
      "\r": "\\r",
    };
    const named = namedEscapes[character];
    if (named) return named;
    const codePoint = character.codePointAt(0) ?? 0;
    return `\\u{${codePoint.toString(16).padStart(4, "0")}}`;
  });
}

function sshTarget(connection: McpConnectionView): string {
  return `${visibleControls(connection.user)}@${visibleControls(connection.hostName)}:${connection.port}`;
}

function clientLabel(client: McpClientSummary | undefined, fallbackId: string): string {
  return visibleControls(client?.label?.trim() || `Client ${fallbackId}`);
}

function formatTimestamp(timestampMs: number | null | undefined): string {
  if (!timestampMs) return "—";
  return new Date(timestampMs).toLocaleString();
}

function formatApprovalExpiry(createdAtMs: number): string {
  return formatTimestamp(createdAtMs + APPROVAL_WINDOW_MS);
}

export function McpPanel() {
  const isInTauri = Boolean(
    (window as Window & { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown }).__TAURI__ ||
      (window as Window & { __TAURI__?: unknown; __TAURI_INTERNALS__?: unknown })
        .__TAURI_INTERNALS__,
  );
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [clients, setClients] = useState<McpClientSummary[]>([]);
  const [connections, setConnections] = useState<McpConnectionView[]>([]);
  const [pending, setPending] = useState<McpPendingTask[]>([]);
  const [bridgeInfo, setBridgeInfo] = useState<McpBridgeInfo | null>(null);
  const [pairLabel, setPairLabel] = useState("");
  const [freshToken, setFreshToken] = useState<null | { clientId: string; token: string }>(null);
  const [error, setError] = useState<string | null>(null);
  const [toggling, setToggling] = useState(false);
  const [updatingGrant, setUpdatingGrant] = useState<string | null>(null);
  const [copied, setCopied] = useState<"token" | "claude" | "opencode" | null>(null);

  const refresh = useCallback(async () => {
    if (!isInTauri) {
      setStatus(null);
      setError("MCP settings are available in the xTermius desktop app.");
      return;
    }
    try {
      const nextStatus = await invoke<McpStatus>("mcp_status");
      const [nextClients, nextConnections, nextPending, nextBridgeInfo] = await Promise.all([
        invoke<McpClientSummary[]>("mcp_list_clients"),
        invoke<McpConnectionView[]>("mcp_list_connections"),
        invoke<McpPendingTask[]>("mcp_pending_tasks"),
        nextStatus.enabled
          ? invoke<McpBridgeInfo>("mcp_bridge_info")
          : Promise.resolve(null),
      ]);
      setStatus(nextStatus);
      setClients(nextClients);
      setConnections(nextConnections);
      setPending(nextPending);
      setBridgeInfo(nextBridgeInfo);
      setError(null);
    } catch (err) {
      setStatus(null);
      setError(String(err));
    }
  }, [isInTauri]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    if (!isInTauri) return;
    const unlisten = listen("pty:exit", () => void refresh());
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [isInTauri, refresh]);

  async function setEnabled(enabled: boolean) {
    setToggling(true);
    try {
      const next = await invoke<McpStatus>("mcp_set_enabled", { enabled });
      setStatus(next);
      if (!enabled) setFreshToken(null);
      await refresh();
    } catch (err) {
      const message = String(err);
      await refresh();
      setError(message);
    } finally {
      setToggling(false);
    }
  }

  async function createPairing() {
    try {
      const created = await invoke<{ clientId: string; token: string }>("mcp_pair_client", {
        label: pairLabel.trim() || null,
      });
      const nextBridgeInfo = await invoke<McpBridgeInfo>("mcp_bridge_info");
      setFreshToken(created);
      setBridgeInfo(nextBridgeInfo);
      setCopied(null);
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

  async function setPermissions(
    client: McpClientSummary,
    connection: McpConnectionView,
    update: Partial<Pick<McpGrantSummary, "observe" | "execute">>,
  ) {
    const key = `${client.clientId}:${connection.connectionId}`;
    const existing = client.grants.find((grant) => grant.connectionId === connection.connectionId);
    const observe = update.observe ?? existing?.observe ?? false;
    const execute = update.execute ?? existing?.execute ?? false;
    setUpdatingGrant(key);
    try {
      await invoke("mcp_set_grant_permissions", {
        clientId: client.clientId,
        connectionId: connection.connectionId,
        observe,
        execute,
      });
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setUpdatingGrant(null);
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

  async function copyText(kind: "token" | "claude" | "opencode", value: string) {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(kind);
      window.setTimeout(() => setCopied((current) => (current === kind ? null : current)), 1800);
    } catch (err) {
      setError(`Could not copy configuration: ${String(err)}`);
    }
  }

  const enabled = status?.enabled === true;
  const claudeConfig = freshToken && bridgeInfo
    ? JSON.stringify(
        {
          mcpServers: {
            xtermius: {
              command: bridgeInfo.executablePath,
              args: [],
              env: {
                XTERMIUS_MCP_CLIENT_ID: freshToken.clientId,
                XTERMIUS_MCP_TOKEN: freshToken.token,
              },
            },
          },
        },
        null,
        2,
      )
    : null;
  const openCodeConfig = freshToken && bridgeInfo
    ? JSON.stringify(
        {
          $schema: "https://opencode.ai/config.json",
          mcp: {
            xtermius: {
              type: "local",
              command: [bridgeInfo.executablePath],
              environment: {
                XTERMIUS_MCP_CLIENT_ID: freshToken.clientId,
                XTERMIUS_MCP_TOKEN: freshToken.token,
              },
            },
          },
        },
        null,
        2,
      )
    : null;

  return (
    <div className="mx-auto grid max-w-4xl gap-4">
      <section className="grid gap-3 rounded-2xl border border-border bg-card/80 p-5">
        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <h2 className="text-lg font-semibold">MCP access</h2>
            <p className="mt-1 text-xs text-muted-foreground">
              Local MCP clients can only use connections and permissions you explicitly enable.
              Command requests always wait for approval.
            </p>
            <p className="mt-2 text-xs text-amber-600 dark:text-amber-400">
              {WARNING_TEXT} Never enable Observe for a session containing data you cannot send to an external model.
            </p>
          </div>
          <label className="flex shrink-0 items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={enabled}
              disabled={status === null || toggling}
              aria-label="Enable MCP access"
              onChange={(event) => void setEnabled(event.target.checked)}
            />
            <span className="font-medium">
              {status === null ? "Unavailable" : enabled ? "Enabled" : "Disabled"}
            </span>
          </label>
        </div>

        {!enabled && status !== null && (
          <p className="border-l-2 border-border pl-3 text-xs text-muted-foreground">
            MCP is off by default. No agent tools are available while disabled. Enable it before
            pairing clients or assigning connection permissions.
          </p>
        )}
        {error && (
          <div role="alert" className="text-xs text-red-500">
            {error}
          </div>
        )}
      </section>

      {enabled && pending.length > 0 && (
        <section className="grid gap-3 rounded-2xl border border-amber-500/40 bg-amber-500/5 p-5">
          <div className="text-sm font-semibold text-amber-600 dark:text-amber-400">
            Pending command approvals ({pending.length})
          </div>
          <div className="text-xs text-muted-foreground">{WARNING_TEXT}</div>
          {pending.map((task) => {
            const client = clients.find((item) => item.clientId === task.clientId);
            const connection = connections.find((item) => item.connectionId === task.connectionId);
            return (
              <div key={task.taskId} className="grid gap-2 rounded-xl border border-border bg-card p-3">
                <div className="break-all whitespace-pre-wrap font-mono text-xs">
                  $ {visibleControls(task.command)}
                </div>
                <div className="text-xs text-muted-foreground">
                  Working directory: {task.workingDirectory === "login"
                    ? "Login directory"
                    : visibleControls(task.workingDirectory)}{" · "}
                  Timeout: {task.timeoutSeconds} seconds{" · "}
                  Approval expires: {formatApprovalExpiry(task.createdAtMs)}
                </div>
                <div className="break-words text-xs text-muted-foreground">
                  {clientLabel(client, task.clientId)} · {connection ? sshTarget(connection) : `Connection ${task.connectionId}`} · generation {task.generation}
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
            );
          })}
        </section>
      )}

      {enabled && (
        <>
          <section className="grid gap-3 rounded-2xl border border-border bg-card/80 p-5">
            <div>
              <h2 className="text-sm font-medium">Pair a client</h2>
              <p className="mt-1 text-xs text-muted-foreground">
                Pairing identifies a local client. It grants no connection access by itself.
              </p>
            </div>
            <div className="flex flex-wrap gap-2">
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
              <div className="grid gap-3 rounded-xl border border-border p-3 text-xs">
                <div>
                  <div className="font-medium">Token shown once — copy a JSON config now:</div>
                  <div className="mt-1 text-muted-foreground">
                    The token is not placed in a shell command, so it will not enter shell history.
                  </div>
                </div>
                <div className="grid gap-1">
                  <div className="font-medium">Client ID</div>
                  <code className="break-all font-mono">{freshToken.clientId}</code>
                </div>
                <div className="grid gap-1">
                  <div className="font-medium">Pairing token</div>
                  <code className="break-all font-mono">{freshToken.token}</code>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    className="w-fit"
                    onClick={() => void copyText("token", freshToken.token)}
                  >
                    {copied === "token" ? "Copied" : "Copy token"}
                  </Button>
                </div>
                <div className="text-muted-foreground">
                  Bridge ({bridgeInfo?.environment ?? "unknown"}): {bridgeInfo?.executablePath ?? "Unavailable"}
                </div>
                {claudeConfig && openCodeConfig ? (
                  <div className="grid gap-3 md:grid-cols-2">
                    <div className="grid min-w-0 gap-2">
                      <div className="flex items-center justify-between gap-2 font-medium">
                        <span>Claude Code (.mcp.json)</span>
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          onClick={() => void copyText("claude", claudeConfig)}
                        >
                          {copied === "claude" ? "Copied" : "Copy JSON"}
                        </Button>
                      </div>
                      <pre className="max-h-64 overflow-auto rounded-lg bg-muted/50 p-2 text-[11px] leading-5">
                        {claudeConfig}
                      </pre>
                    </div>
                    <div className="grid min-w-0 gap-2">
                      <div className="flex items-center justify-between gap-2 font-medium">
                        <span>OpenCode (opencode.jsonc)</span>
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          onClick={() => void copyText("opencode", openCodeConfig)}
                        >
                          {copied === "opencode" ? "Copied" : "Copy JSONC"}
                        </Button>
                      </div>
                      <pre className="max-h-64 overflow-auto rounded-lg bg-muted/50 p-2 text-[11px] leading-5">
                        {openCodeConfig}
                      </pre>
                    </div>
                  </div>
                ) : (
                  <div className="text-amber-600 dark:text-amber-400">
                    Bridge path is unavailable; keep this token visible until the app can provide a local bridge path.
                  </div>
                )}
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className="w-fit"
                  onClick={() => {
                    setFreshToken(null);
                    setCopied(null);
                  }}
                >
                  Dismiss token
                </Button>
              </div>
            )}
            {clients.length === 0 ? (
              <div className="text-xs text-muted-foreground">No clients are paired.</div>
            ) : (
              <div className="grid gap-2">
                {clients.map((client) => (
                  <div
                    key={client.clientId}
                    className="flex items-center justify-between gap-3 rounded-xl border border-border p-3"
                  >
                    <div className="min-w-0">
                      <div className="break-words text-sm">
                        {visibleControls(client.label?.trim() || client.clientId)}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {client.grants.length} connection permission set(s)
                      </div>
                    </div>
                    <Button type="button" size="sm" variant="outline" onClick={() => void unpair(client.clientId)}>
                      Unpair
                    </Button>
                  </div>
                ))}
              </div>
            )}
          </section>

          <section className="grid gap-3 rounded-2xl border border-border bg-card/80 p-5">
            <div>
              <h2 className="text-sm font-medium">Connection permissions</h2>
              <p className="mt-1 text-xs text-muted-foreground">
                Observe reads new connection output. Execute allows approved command requests and task controls.
              </p>
            </div>
            {connections.length === 0 ? (
              <div className="text-xs text-muted-foreground">No active connections.</div>
            ) : (
              <div className="grid gap-2">
                {connections.map((connection) => (
                  <div key={connection.connectionId} className="grid gap-3 rounded-xl border border-border p-3">
                    <div className="min-w-0">
                      <div className="break-words text-sm font-medium">{sshTarget(connection)}</div>
                      <div className="break-words text-xs text-muted-foreground">
                        {connection.alias && connection.alias !== connection.hostName
                          ? `SSH alias ${visibleControls(connection.alias)} · `
                          : ""}
                        State: {connection.state}
                      </div>
                    </div>
                    {connection.state !== "ready" ? (
                      <div className="text-xs text-muted-foreground">
                        Wait for authentication to complete before assigning permissions.
                      </div>
                    ) : connection.requiresReconnect ? (
                      <div className="text-xs text-muted-foreground">
                        This connection predates managed reuse. Close and reconnect it before assigning permissions.
                      </div>
                    ) : clients.length === 0 ? (
                      <div className="text-xs text-muted-foreground">Pair a client to assign permissions.</div>
                    ) : (
                      <div className="grid gap-2">
                        {clients.map((client) => {
                          const grant = client.grants.find(
                            (item) => item.connectionId === connection.connectionId,
                          );
                          const key = `${client.clientId}:${connection.connectionId}`;
                          const updating = updatingGrant === key;
                          return (
                            <div
                              key={client.clientId}
                              className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-t border-border/70 pt-2 text-xs"
                            >
                              <span className="min-w-0 break-words">
                                {visibleControls(client.label?.trim() || client.clientId)}
                              </span>
                              <div className="flex items-center gap-4">
                                <label className="flex items-center gap-1.5">
                                  <input
                                    type="checkbox"
                                    checked={grant?.observe ?? false}
                                    disabled={connection.state !== "ready" || updating || updatingGrant !== null}
                                    aria-label={`Observe ${sshTarget(connection)} for ${clientLabel(client, client.clientId)}`}
                                    onChange={(event) =>
                                      void setPermissions(client, connection, { observe: event.target.checked })
                                    }
                                  />
                                  <span>Observe</span>
                                </label>
                                <label className="flex items-center gap-1.5">
                                  <input
                                    type="checkbox"
                                    checked={grant?.execute ?? false}
                                    disabled={connection.state !== "ready" || updating || updatingGrant !== null}
                                    aria-label={`Execute on ${sshTarget(connection)} for ${clientLabel(client, client.clientId)}`}
                                    onChange={(event) =>
                                      void setPermissions(client, connection, { execute: event.target.checked })
                                    }
                                  />
                                  <span>Execute</span>
                                </label>
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </section>
        </>
      )}
    </div>
  );
}
