/// <reference types="vite/client" />

interface Host {
  id: string;
  name: string;
  alias: string;
  hostname: string;
  user: string;
  port: number;
  identityFile?: string;
  proxyJump?: string;
  tags: string[];
  notes: string;
  updatedAt: string;
  deleted: boolean;
}

interface Session {
  id: string;
  hostAlias: string;
}

declare global {
  interface Window {
    __TAURI__?: {
      invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
      event: {
        listen: <T>(event: string, callback: (event: { payload: T }) => void) => Promise<() => void>;
      };
    };
  }
}
