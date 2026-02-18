export type AppToastTone = "info" | "success" | "warning" | "error";

export type AppToastPayload = {
  id?: string;
  title: string;
  description?: string;
  tone?: AppToastTone;
  durationMs?: number;
};

const APP_TOAST_EVENT = "xtermius:toast";

export function showToast(payload: AppToastPayload) {
  window.dispatchEvent(new CustomEvent<AppToastPayload>(APP_TOAST_EVENT, { detail: payload }));
}

export function listenToast(listener: (payload: AppToastPayload) => void) {
  const onEvent = (event: Event) => {
    const custom = event as CustomEvent<AppToastPayload>;
    if (!custom.detail?.title) return;
    listener(custom.detail);
  };
  window.addEventListener(APP_TOAST_EVENT, onEvent);
  return () => window.removeEventListener(APP_TOAST_EVENT, onEvent);
}
