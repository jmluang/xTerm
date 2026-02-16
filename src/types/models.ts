export interface Host {
  id: string;
  name: string;
  alias: string;
  hostname: string;
  user: string;
  port: number;
  password?: string;
  hasPassword?: boolean;
  identityFile?: string;
  proxyJump?: string;
  envVars?: string;
  encoding?: string;
  sortOrder?: number;
  tags: string[];
  notes: string;
  updatedAt: string;
  deleted: boolean;
}

export interface Session {
  id: string;
  hostAlias: string;
  hostId: string;
  startedAt: number;
  endedAt?: number;
  status: "running" | "exited";
  exitCode?: number;
}

export interface Settings {
  webdav_url?: string | null;
  webdav_folder?: string | null;
  webdav_username?: string | null;
  webdav_password?: string | null;
}
