import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { ThemeMode } from "@/lib/theme";

export function SettingsDialog(props: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  themeMode: ThemeMode;
  setThemeMode: (mode: ThemeMode) => void;
  onOpenWebdav: () => void;
}) {
  const { open, onOpenChange, themeMode, setThemeMode, onOpenWebdav } = props;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
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
                    themeMode === m
                      ? "bg-accent text-foreground"
                      : "text-muted-foreground hover:text-foreground",
                  ].join(" ")}
                  onClick={() => setThemeMode(m)}
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
              <Button variant="outline" size="sm" onClick={onOpenWebdav}>
                Open
              </Button>
            </div>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
