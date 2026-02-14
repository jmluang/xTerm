import { setLastError } from "./errorStore";

let installed = false;

function toMessage(err: unknown) {
  if (err instanceof Error) return `${err.name}: ${err.message}`;
  try {
    return typeof err === "string" ? err : JSON.stringify(err);
  } catch {
    return String(err);
  }
}

function toStack(err: unknown) {
  return err instanceof Error ? err.stack : undefined;
}

export function installGlobalErrorHandlers() {
  if (installed) return;
  installed = true;

  window.addEventListener("error", (event) => {
    const err = (event as ErrorEvent).error ?? (event as any).message;
    setLastError({
      kind: "error",
      message: toMessage(err),
      stack: toStack(err),
      time: Date.now(),
    });
  });

  window.addEventListener("unhandledrejection", (event) => {
    const reason = (event as PromiseRejectionEvent).reason;
    setLastError({
      kind: "unhandledrejection",
      message: toMessage(reason),
      stack: toStack(reason),
      time: Date.now(),
    });
  });
}

