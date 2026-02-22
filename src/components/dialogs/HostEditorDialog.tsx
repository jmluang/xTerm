import { ChevronDown } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { Host } from "@/types/models";

export function HostEditorDialog(props: {
  open: boolean;
  onClose: () => void;
  editingHost: Host | null;
  formData: Partial<Host>;
  setFormData: Dispatch<SetStateAction<Partial<Host>>>;
  selectIdentityFile: () => Promise<void>;
  onSave: () => Promise<void>;
}) {
  const { open, onClose, editingHost, formData, setFormData, selectIdentityFile, onSave } = props;
  if (!open) return null;

  return (
    <div
      className="absolute inset-0 z-50"
      data-tauri-drag-region="false"
      style={{ WebkitAppRegion: "no-drag" } as any}
    >
      <div
        className="absolute inset-0 bg-black/20 backdrop-blur-sm"
        onMouseDown={(e) => {
          e.stopPropagation();
        }}
        onClick={onClose}
      />
      <div
        className="absolute inset-0 overflow-auto p-4"
        onMouseDown={(e) => {
          e.stopPropagation();
        }}
        onClick={(e) => {
          e.stopPropagation();
        }}
      >
        <div className="mx-auto max-w-4xl">
          <div className="rounded-2xl bg-background/95 backdrop-blur ring-1 ring-black/10 shadow-xl overflow-hidden">
            <div className="px-5 py-4 border-b border-border flex items-start gap-3">
              <div className="min-w-0 flex-1">
                <div className="text-lg font-semibold">{editingHost ? "Edit Host" : "Add Host"}</div>
              </div>
              <button
                type="button"
                className="h-8 w-8 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
                onClick={onClose}
                title="Close"
                aria-label="Close"
              >
                x
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
                        onChange={(e) =>
                          setFormData({ ...formData, hostname: e.target.value, name: e.target.value })
                        }
                        placeholder="192.168.1.1 or server.example.com"
                      />
                    </div>
                    <div className="grid gap-2">
                      <label className="text-sm font-medium">Alias</label>
                      <Input
                        value={formData.alias || ""}
                        onChange={(e) => setFormData({ ...formData, alias: e.target.value })}
                        placeholder="my-server"
                      />
                    </div>
                    <div className="grid gap-3 grid-cols-2">
                      <div className="grid gap-2">
                        <label className="text-sm font-medium">User</label>
                        <Input
                          value={formData.user || ""}
                          onChange={(e) => setFormData({ ...formData, user: e.target.value })}
                          placeholder="root"
                        />
                      </div>
                      <div className="grid gap-2">
                        <label className="text-sm font-medium">Port</label>
                        <Input
                          type="number"
                          value={formData.port || 22}
                          onChange={(e) =>
                            setFormData({ ...formData, port: parseInt(e.target.value || "22", 10) })
                          }
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
                        {editingHost?.hasPassword || formData.hasPassword ? (
                          <span className="text-[11px] px-2 py-0.5 rounded-full bg-muted/40 text-muted-foreground">
                            Saved
                          </span>
                        ) : null}
                      </div>
                      <div className="flex items-center gap-2">
                        <Input
                          type="password"
                          value={formData.password ?? ""}
                          onChange={(e) => setFormData({ ...formData, password: e.target.value })}
                          placeholder={
                            editingHost?.hasPassword || formData.hasPassword
                              ? "Edit saved password"
                              : "Leave empty to prompt"
                          }
                          className="flex-1"
                        />
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
                          onChange={(e) => setFormData({ ...formData, identityFile: e.target.value })}
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
                        onChange={(e) => setFormData({ ...formData, proxyJump: e.target.value })}
                        placeholder="jump-host or user@jump:port"
                      />
                    </div>
                  </div>
                </div>
              </div>

              <details className="rounded-xl border border-border bg-card/40 p-4">
                <summary className="cursor-pointer select-none text-sm font-semibold">Advanced</summary>
                <div className="grid gap-4 mt-4">
                  <div className="grid gap-2">
                    <label className="text-sm font-medium">Environment Variables</label>
                    <Input
                      value={formData.envVars || ""}
                      onChange={(e) => setFormData({ ...formData, envVars: e.target.value })}
                      placeholder="VAR1=value1, VAR2=value2"
                    />
                  </div>
                  <div className="grid gap-2">
                    <label className="text-sm font-medium">Encoding</label>
                    <div className="relative">
                      <select
                        className="h-10 w-full appearance-none rounded-lg border border-border bg-background/70 px-3 pr-9 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        value={formData.encoding || "utf-8"}
                        onChange={(e) => setFormData({ ...formData, encoding: e.target.value })}
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
              <Button variant="outline" onClick={onClose}>
                Cancel
              </Button>
              <Button onClick={onSave}>Save</Button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
