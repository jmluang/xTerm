import {
  DndContext,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import { arrayMove, SortableContext, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { ArrowUpDown, Check, FileInput, PanelLeftClose, Pencil, Search, Trash2, X } from "lucide-react";
import type { Dispatch, RefObject, SetStateAction } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SortableHostRow } from "@/components/hosts/SortableHostRow";
import type { Host } from "@/types/models";

export function HostsSidebar(props: {
  hostListRef: RefObject<HTMLDivElement>;
  hostListScrollable: boolean;
  hostSearch: string;
  setHostSearch: (value: string) => void;
  sortedHosts: Host[];
  reorderMode: boolean;
  setReorderMode: Dispatch<SetStateAction<boolean>>;
  setActiveDragHostId: Dispatch<SetStateAction<string | null>>;
  hosts: Host[];
  persistHostOrder: (nextHosts: Host[]) => Promise<void>;
  connectingHosts: Record<string, { stage: string; startedAt: number; count: number }>;
  openEditDialog: (host: Host) => void;
  deleteHost: (host: Host) => Promise<void>;
  connectToHost: (host: Host) => Promise<void>;
  openAddDialog: () => void;
  openSshImportDialog: () => Promise<void>;
  sshImportLoading: boolean;
  isInTauri: boolean;
  setSidebarOpen: Dispatch<SetStateAction<boolean>>;
}) {
  const {
    hostListRef,
    hostListScrollable,
    hostSearch,
    setHostSearch,
    sortedHosts,
    reorderMode,
    setReorderMode,
    setActiveDragHostId,
    hosts,
    persistHostOrder,
    connectingHosts,
    openEditDialog,
    deleteHost,
    connectToHost,
    openAddDialog,
    openSshImportDialog,
    sshImportLoading,
    isInTauri,
    setSidebarOpen,
  } = props;

  const dndSensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));

  function renderHostMeta(host: Host) {
    return (
      <>
        <div className="text-sm font-semibold break-words leading-snug" title={host.alias || host.hostname || "Unnamed"}>
          {host.alias || host.hostname || "Unnamed"}
        </div>
        <div className="text-[11px] text-muted-foreground break-words leading-snug">
          {host.user ? `${host.user}@` : ""}
          {host.hostname}
          {host.port && host.port !== 22 ? `:${host.port}` : ""}
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
  }

  function renderHostActions(host: Host) {
    return (
      <>
        <button
          type="button"
          className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center pointer-events-auto bg-background/70 backdrop-blur ring-1 ring-black/5"
          onClick={(e) => {
            e.stopPropagation();
            openEditDialog(host);
          }}
          title="Edit"
          aria-label="Edit host"
        >
          <Pencil size={16} />
        </button>
        <button
          type="button"
          className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center pointer-events-auto bg-background/70 backdrop-blur ring-1 ring-black/5"
          onClick={(e) => {
            e.stopPropagation();
            void deleteHost(host);
          }}
          title="Delete"
          aria-label="Delete host"
        >
          <Trash2 size={16} />
        </button>
      </>
    );
  }

  return (
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
          className="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent inline-flex items-center justify-center"
          title="Import SSH Config"
          aria-label="Import SSH Config"
          onClick={(e) => {
            e.stopPropagation();
            void openSshImportDialog();
          }}
          disabled={sshImportLoading || !isInTauri}
          data-tauri-drag-region="false"
          style={{ WebkitAppRegion: "no-drag" } as any}
        >
          <FileInput size={18} />
        </button>
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
            <div className="sticky top-0 z-20 -mx-2 px-2 pt-1 pb-1" style={{ background: "var(--app-sidebar-bg)" } as any}>
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
            reorderMode && !hostSearch.trim() ? (
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
                  {sortedHosts.map((host) => (
                    <SortableHostRow
                      key={host.id}
                      host={host}
                      reorderMode={reorderMode}
                      hostSearch={hostSearch}
                      left={renderHostMeta(host)}
                      right={renderHostActions(host)}
                      onRowClick={() => {
                        // no click in reorder mode
                      }}
                    />
                  ))}
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
                    void connectToHost(host);
                  }}
                >
                  <div className="min-w-0">
                    <div className="flex items-start gap-2">
                      <div className="flex-1 min-w-0">{renderHostMeta(host)}</div>
                    </div>
                  </div>
                  <div
                    className={[
                      "absolute right-2 top-2 flex items-center gap-1 transition-opacity pointer-events-none",
                      reorderMode ? "opacity-0" : "opacity-0 group-hover:opacity-100",
                    ].join(" ")}
                  >
                    {renderHostActions(host)}
                  </div>
                </div>
              ))
            )
          ) : (
            <div className="flex flex-col items-center justify-center h-full text-center px-4">
              <p className="text-sm text-muted-foreground mb-3">No hosts yet</p>
              <div className="flex flex-col gap-2 w-full max-w-[160px]">
                <Button onClick={openAddDialog}>Create Host</Button>
                {isInTauri ? (
                  <Button variant="outline" onClick={() => void openSshImportDialog()} disabled={sshImportLoading}>
                    Import SSH Config
                  </Button>
                ) : null}
              </div>
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
  );
}
