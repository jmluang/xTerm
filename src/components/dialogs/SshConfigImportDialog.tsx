import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import type { SshConfigImportCandidate } from "@/types/models";

export function SshConfigImportDialog(props: {
  open: boolean;
  loading: boolean;
  candidates: SshConfigImportCandidate[];
  onClose: () => void;
  onImport: (aliases: string[]) => Promise<void>;
}) {
  const { open, loading, candidates, onClose, onImport } = props;
  const [selectedAliases, setSelectedAliases] = useState<Set<string>>(new Set());
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setSelectedAliases(new Set(candidates.map((item) => item.alias)));
  }, [open, candidates]);

  const sorted = useMemo(() => {
    return candidates
      .slice()
      .sort((a, b) => a.alias.toLowerCase().localeCompare(b.alias.toLowerCase()));
  }, [candidates]);

  function toggleAlias(alias: string) {
    setSelectedAliases((prev) => {
      const next = new Set(prev);
      if (next.has(alias)) next.delete(alias);
      else next.add(alias);
      return next;
    });
  }

  async function submit() {
    if (importing) return;
    const aliases = Array.from(selectedAliases);
    if (aliases.length === 0) return;
    setImporting(true);
    try {
      await onImport(aliases);
    } finally {
      setImporting(false);
    }
  }

  if (!open) return null;

  return (
    <div className="absolute inset-0 z-50" data-tauri-drag-region="false" style={{ WebkitAppRegion: "no-drag" } as any}>
      <div
        className="absolute inset-0 bg-black/20 backdrop-blur-sm"
        onMouseDown={(e) => {
          e.stopPropagation();
        }}
        onClick={onClose}
      />
      <div className="absolute inset-0 overflow-auto p-4" onMouseDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
        <div className="mx-auto max-w-3xl">
          <div className="rounded-2xl bg-background/95 backdrop-blur ring-1 ring-black/10 shadow-xl overflow-hidden">
            <div className="px-5 py-4 border-b border-border flex items-start gap-3">
              <div className="min-w-0 flex-1">
                <div className="text-lg font-semibold">Import Hosts From SSH Config</div>
                <div className="text-xs text-muted-foreground mt-1">Detected from ~/.ssh config files. Select the hosts you want to import.</div>
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

            <div className="px-5 py-4 border-b border-border flex items-center gap-2">
              <Button type="button" variant="outline" size="sm" onClick={() => setSelectedAliases(new Set(sorted.map((v) => v.alias)))}>
                Select All
              </Button>
              <Button type="button" variant="outline" size="sm" onClick={() => setSelectedAliases(new Set())}>
                Clear
              </Button>
              <div className="text-xs text-muted-foreground ml-auto">{selectedAliases.size} selected</div>
            </div>

            <div className="max-h-[60vh] overflow-auto">
              {loading ? (
                <div className="px-5 py-8 text-sm text-muted-foreground">Scanning ~/.ssh ...</div>
              ) : sorted.length === 0 ? (
                <div className="px-5 py-8 text-sm text-muted-foreground">No importable hosts found.</div>
              ) : (
                <div className="px-5 py-3 space-y-2">
                  {sorted.map((item) => {
                    const checked = selectedAliases.has(item.alias);
                    return (
                      <label
                        key={`${item.alias}-${item.sourcePath}`}
                        className="flex items-start gap-3 p-3 rounded-lg border border-border hover:bg-muted/50 cursor-pointer"
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() => toggleAlias(item.alias)}
                          className="mt-1"
                        />
                        <div className="min-w-0 flex-1">
                          <div className="text-sm font-semibold break-words">{item.alias}</div>
                          <div className="text-xs text-muted-foreground break-words mt-0.5">
                            {item.user ? `${item.user}@` : ""}
                            {item.hostname}
                            {item.port && item.port !== 22 ? `:${item.port}` : ""}
                          </div>
                          <div className="text-[11px] text-muted-foreground/80 break-all mt-1">{item.sourcePath}</div>
                        </div>
                      </label>
                    );
                  })}
                </div>
              )}
            </div>

            <div className="px-5 py-4 border-t border-border flex items-center justify-end gap-2">
              <Button variant="outline" onClick={onClose} disabled={importing}>
                Cancel
              </Button>
              <Button onClick={() => void submit()} disabled={importing || selectedAliases.size === 0 || loading}>
                {importing ? "Importing..." : "Import Selected"}
              </Button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
