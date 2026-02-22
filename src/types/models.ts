export interface Host {
  id: string;
  name: string;
  alias: string;
  hostname: string;
  user: string;
  port: number;
  password?: string;
  hasPassword?: boolean;
  hostInsightsEnabled?: boolean;
  hostLiveMetricsEnabled?: boolean;
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

export interface HostStaticInfo {
  systemName?: string;
  kernel?: string;
  arch?: string;
  cpuModel?: string;
  cpuCores?: number;
  memTotalKb?: number;
}

export interface HostLiveProcess {
  command: string;
  cpuPercent: number;
  memPercent: number;
}

export interface HostLiveInfo {
  cpuPercent?: number;
  cpuUserPercent?: number;
  cpuSystemPercent?: number;
  cpuIowaitPercent?: number;
  cpuIdlePercent?: number;
  cpuCores?: number;
  uptimeSeconds?: number;
  memTotalKb?: number;
  memUsedKb?: number;
  memFreeKb?: number;
  memPageCacheKb?: number;
  load1?: number;
  load5?: number;
  load15?: number;
  diskRootTotalKb?: number;
  diskRootUsedKb?: number;
  processes: HostLiveProcess[];
}

export interface Session {
  id: string;
  hostAlias: string;
  hostId: string;
  startedAt: number;
  endedAt?: number;
  status: "starting" | "running" | "exited";
  exitCode?: number;
}

export interface Settings {
  webdav_url?: string | null;
  webdav_folder?: string | null;
  webdav_username?: string | null;
  webdav_password?: string | null;
}

export interface SshConfigImportCandidate {
  alias: string;
  hostname: string;
  user: string;
  port: number;
  identityFile?: string;
  proxyJump?: string;
  sourcePath: string;
}
