import { useEffect, useState } from "react";
import { clearLastError, getLastError, subscribe, type CapturedError } from "./errorStore";

function isOverlayEnabled() {
  if (import.meta.env.DEV) return true;
  try {
    const url = new URL(window.location.href);
    if (url.searchParams.get("debug") === "1") return true;
  } catch {
    // ignore
  }
  return localStorage.getItem("xtermius_debug_overlay") === "1";
}

export function GlobalErrorOverlay() {
  const [enabled] = useState(isOverlayEnabled);
  const [err, setErr] = useState<CapturedError | null>(() => (enabled ? getLastError() : null));

  useEffect(() => {
    if (!enabled) return;
    return subscribe(setErr);
  }, [enabled]);

  if (!enabled || !err) return null;

  return (
    <div
      style={{
        position: "fixed",
        left: 12,
        right: 12,
        bottom: 12,
        zIndex: 999999,
        background: "rgba(2, 6, 23, 0.92)",
        color: "#e2e8f0",
        border: "1px solid rgba(148, 163, 184, 0.35)",
        borderRadius: 10,
        padding: 12,
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
        boxShadow: "0 10px 30px rgba(0,0,0,0.35)",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
        <div style={{ fontWeight: 700, color: "#fca5a5" }}>
          {err.kind} at {new Date(err.time).toLocaleTimeString()}
        </div>
        <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
          <button
            style={{
              cursor: "pointer",
              borderRadius: 8,
              padding: "6px 10px",
              border: "1px solid rgba(148, 163, 184, 0.35)",
              background: "rgba(15, 23, 42, 0.9)",
              color: "#e2e8f0",
            }}
            onClick={() => {
              const text = `${err.message}\n${err.stack ?? ""}`.trim();
              navigator.clipboard?.writeText(text).catch(() => {});
            }}
          >
            copy
          </button>
          <button
            style={{
              cursor: "pointer",
              borderRadius: 8,
              padding: "6px 10px",
              border: "1px solid rgba(148, 163, 184, 0.35)",
              background: "rgba(15, 23, 42, 0.9)",
              color: "#e2e8f0",
            }}
            onClick={() => clearLastError()}
          >
            dismiss
          </button>
        </div>
      </div>
      <div style={{ marginTop: 8, whiteSpace: "pre-wrap", color: "#fecaca" }}>{err.message}</div>
      {err.stack ? (
        <pre style={{ marginTop: 8, whiteSpace: "pre-wrap", opacity: 0.85 }}>{err.stack}</pre>
      ) : null}
    </div>
  );
}

