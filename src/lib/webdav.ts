export function resolveWebdavHostsDbUrl(input: string, folder: string): string {
  const raw = (input ?? "").trim();
  if (!raw) return "";

  // UI hint only. Backend does the real normalization.
  const last = raw.split("/").pop() ?? "";
  const looksLikeDbOrJson = /\.((db|json|sqlite))$/i.test(last);
  if (looksLikeDbOrJson) return `${raw.split("/").slice(0, -1).join("/")}/hosts.db`;

  const base = raw.endsWith("/") ? raw.slice(0, -1) : raw;
  const f = (folder ?? "").trim().replace(/^\/+|\/+$/g, "");
  if (!f) return `${base}/hosts.db`;
  return `${base}/${f}/hosts.db`;
}
