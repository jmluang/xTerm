import { useEffect, useState } from "react";
import { listenToast, type AppToastPayload } from "@/lib/toast";

type ToastItem = Required<Pick<AppToastPayload, "id" | "title" | "tone" | "durationMs">> &
  Pick<AppToastPayload, "description">;

function normalizeToast(payload: AppToastPayload): ToastItem {
  return {
    id: payload.id || `${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
    title: payload.title,
    description: payload.description,
    tone: payload.tone || "info",
    durationMs: Math.max(1400, payload.durationMs ?? 2600),
  };
}

export function ToastViewport() {
  const [items, setItems] = useState<ToastItem[]>([]);

  useEffect(() => {
    const unlisten = listenToast((payload) => {
      const item = normalizeToast(payload);
      setItems((prev) => [...prev, item].slice(-4));
      window.setTimeout(() => {
        setItems((prev) => prev.filter((it) => it.id !== item.id));
      }, item.durationMs);
    });
    return unlisten;
  }, []);

  if (items.length === 0) return null;

  return (
    <div className="fixed right-4 bottom-4 z-[140] flex flex-col gap-2 pointer-events-none">
      {items.map((item) => {
        const toneClass =
          item.tone === "error"
            ? "border-rose-500/35 text-foreground bg-rose-500/12"
            : item.tone === "warning"
              ? "border-amber-500/35 text-foreground bg-amber-500/12"
              : item.tone === "success"
                ? "border-emerald-500/35 text-foreground bg-emerald-500/12"
                : "border-border/70 text-foreground bg-background/90";
        return (
          <div
            key={item.id}
            className={[
              "min-w-[220px] max-w-[360px] rounded-lg border px-3 py-2 shadow-lg backdrop-blur-sm",
              "text-[12px] leading-snug",
              toneClass,
            ].join(" ")}
          >
            <div className="font-medium">{item.title}</div>
            {item.description ? <div className="mt-0.5 opacity-90">{item.description}</div> : null}
          </div>
        );
      })}
    </div>
  );
}
