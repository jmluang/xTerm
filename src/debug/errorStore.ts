export type CapturedError = {
  kind: "error" | "unhandledrejection";
  message: string;
  stack?: string;
  time: number;
};

let lastError: CapturedError | null = null;
const subs = new Set<(e: CapturedError | null) => void>();

export function getLastError() {
  return lastError;
}

export function clearLastError() {
  lastError = null;
  for (const fn of subs) fn(lastError);
}

export function setLastError(e: CapturedError) {
  lastError = e;
  for (const fn of subs) fn(lastError);
}

export function subscribe(fn: (e: CapturedError | null) => void) {
  subs.add(fn);
  return () => subs.delete(fn);
}

