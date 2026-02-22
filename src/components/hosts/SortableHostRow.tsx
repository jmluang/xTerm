import type { ReactNode } from "react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical } from "lucide-react";
import type { Host } from "@/types/models";

export function SortableHostRow(props: {
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
        reorderMode
          ? "cursor-grab active:cursor-grabbing bg-black/5 hover:bg-black/10"
          : "cursor-pointer hover:bg-[var(--app-sidebar-row-hover)]",
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
