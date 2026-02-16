import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { resolveWebdavHostsDbUrl } from "@/lib/webdav";
import type { Settings } from "@/types/models";
import type { Dispatch, SetStateAction } from "react";

export function WebdavSyncDialog(props: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  settings: Settings;
  setSettings: Dispatch<SetStateAction<Settings>>;
  localHostsDbPath: string;
  syncBusy: null | "pull" | "push" | "save";
  syncNotice: null | { kind: "ok" | "err"; text: string };
  isInTauri: boolean;
  onOpenSettings: () => void;
  onSaveSettings: () => Promise<void>;
  onPull: () => Promise<void>;
  onPush: () => Promise<void>;
}) {
  const {
    open,
    onOpenChange,
    settings,
    setSettings,
    localHostsDbPath,
    syncBusy,
    syncNotice,
    isInTauri,
    onOpenSettings,
    onSaveSettings,
    onPull,
    onPush,
  } = props;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
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
                {resolveWebdavHostsDbUrl(settings.webdav_url ?? "", settings.webdav_folder ?? "xTermius") ||
                  "(not set)"}
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
          <Button variant="outline" disabled={!isInTauri || syncBusy !== null} onClick={onOpenSettings}>
            Settings
          </Button>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              disabled={!isInTauri || syncBusy !== null}
              onClick={() => {
                void onSaveSettings();
              }}
            >
              Save
            </Button>
            <Button
              variant="outline"
              disabled={!isInTauri || syncBusy !== null}
              onClick={() => {
                void onPull();
              }}
            >
              Pull
            </Button>
            <Button
              variant="default"
              disabled={!isInTauri || syncBusy !== null}
              onClick={() => {
                void onPush();
              }}
            >
              Push
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
