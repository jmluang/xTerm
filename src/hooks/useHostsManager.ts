import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, message, open } from "@tauri-apps/plugin-dialog";
import type { Host, SshConfigImportCandidate } from "@/types/models";

export function useHostsManager(params: { isInTauri: boolean; sidebarOpen: boolean }) {
  const { isInTauri, sidebarOpen } = params;
  const [hosts, setHosts] = useState<Host[]>([]);
  const hostsRef = useRef<Host[]>([]);
  const [hostSearch, setHostSearch] = useState("");
  const hostListRef = useRef<HTMLDivElement>(null);
  const [hostListScrollable, setHostListScrollable] = useState(false);
  const [reorderMode, setReorderMode] = useState(false);
  const [showDialog, setShowDialog] = useState(false);
  const [editingHost, setEditingHost] = useState<Host | null>(null);
  const [formData, setFormData] = useState<Partial<Host>>({});
  const [showSshImportDialog, setShowSshImportDialog] = useState(false);
  const [sshImportLoading, setSshImportLoading] = useState(false);
  const [sshImportCandidates, setSshImportCandidates] = useState<SshConfigImportCandidate[]>([]);

  useEffect(() => {
    hostsRef.current = hosts;
  }, [hosts]);

  async function saveHostsToBackend(newHosts: Host[]) {
    if (isInTauri) {
      await invoke("hosts_save", { hosts: newHosts });
      return;
    }
    localStorage.setItem("xtermius_hosts", JSON.stringify(newHosts));
  }

  function normalizeHostOrder(list: Host[]): Host[] {
    const alive = list.filter((h) => !h.deleted);
    alive.sort((a, b) => {
      const ao = typeof a.sortOrder === "number" ? a.sortOrder : Number.MAX_SAFE_INTEGER;
      const bo = typeof b.sortOrder === "number" ? b.sortOrder : Number.MAX_SAFE_INTEGER;
      if (ao !== bo) return ao - bo;
      const at = Date.parse(a.updatedAt || "") || 0;
      const bt = Date.parse(b.updatedAt || "") || 0;
      return bt - at;
    });
    const dense = new Map<string, number>();
    for (let i = 0; i < alive.length; i += 1) dense.set(alive[i].id, i);
    return list.map((h) => (h.deleted ? h : { ...h, sortOrder: dense.get(h.id) ?? h.sortOrder }));
  }

  async function loadHosts() {
    try {
      if (isInTauri) {
        const data = await invoke<Host[]>("hosts_load");
        const alive = data.filter((h) => !h.deleted);
        const orders = alive
          .map((h) => h.sortOrder)
          .filter((v): v is number => typeof v === "number");
        const unique = new Set(orders);
        const needsNormalize = alive.length > 0 && (orders.length !== alive.length || unique.size !== alive.length);
        if (needsNormalize) {
          const normalized = normalizeHostOrder(data);
          setHosts(normalized);
          await invoke("hosts_save", { hosts: normalized });
        } else {
          setHosts(data);
        }
      } else {
        const saved = localStorage.getItem("xtermius_hosts");
        if (saved) setHosts(JSON.parse(saved));
        else setHosts([]);
      }
    } catch (e) {
      console.error("Failed to load hosts:", e);
      const saved = localStorage.getItem("xtermius_hosts");
      if (saved) setHosts(JSON.parse(saved));
      else setHosts([]);
    }
  }

  async function selectIdentityFile() {
    if (!isInTauri) {
      alert("File selection only works in the desktop app");
      return;
    }
    try {
      const selected = await open({
        title: "Select Identity File",
        multiple: false,
        directory: false,
      });
      if (selected) {
        setFormData({ ...formData, identityFile: selected as string });
      }
    } catch (e) {
      console.error("Failed to select file:", e);
    }
  }

  function openAddDialog() {
    setEditingHost(null);
    setFormData({
      name: "",
      alias: "",
      hostname: "",
      user: "",
      port: 22,
      tags: [],
      notes: "",
      hostInsightsEnabled: true,
      hostLiveMetricsEnabled: true,
    });
    setShowDialog(true);
  }

  async function openSshImportDialog() {
    if (!isInTauri) {
      alert("SSH config import only works in the desktop app.");
      return;
    }

    setSshImportLoading(true);
    try {
      const candidates = await invoke<SshConfigImportCandidate[]>("ssh_config_scan_importable_hosts");
      if (candidates.length === 0) {
        await message("No importable hosts found in ~/.ssh config files.", {
          title: "SSH Config Import",
          kind: "info",
        });
        return;
      }
      setSshImportCandidates(candidates);
      setShowSshImportDialog(true);
    } catch (error) {
      await message(`Failed to scan ~/.ssh config.\n\n${String(error)}`, {
        title: "SSH Config Import",
        kind: "error",
      });
    } finally {
      setSshImportLoading(false);
    }
  }

  async function importSshConfigHosts(selectedAliases: string[]) {
    if (selectedAliases.length === 0) return;

    const selected = sshImportCandidates.filter((item) => selectedAliases.includes(item.alias));
    if (selected.length === 0) return;

    const now = new Date().toISOString();
    const aliveHosts = hosts.filter((h) => !h.deleted);
    const maxSortOrder = aliveHosts.reduce((max, host, index) => {
      const value = typeof host.sortOrder === "number" ? host.sortOrder : index;
      return Math.max(max, value);
    }, -1);

    const existingAlias = new Set(aliveHosts.map((h) => (h.alias || "").trim().toLowerCase()).filter(Boolean));
    const existingEndpoint = new Set(
      aliveHosts
        .map((h) => `${(h.user || "").trim().toLowerCase()}@${(h.hostname || "").trim().toLowerCase()}:${h.port || 22}`)
        .filter(Boolean)
    );

    let imported = 0;
    let skipped = 0;
    const append: Host[] = [];
    for (const [index, item] of selected.entries()) {
      const alias = (item.alias || "").trim();
      const hostname = (item.hostname || alias).trim();
      const user = (item.user || "").trim();
      const port = item.port || 22;
      const aliasKey = alias.toLowerCase();
      const endpointKey = `${user.toLowerCase()}@${hostname.toLowerCase()}:${port}`;

      if (!alias || !hostname) {
        skipped += 1;
        continue;
      }
      if (existingAlias.has(aliasKey) || existingEndpoint.has(endpointKey)) {
        skipped += 1;
        continue;
      }

      append.push({
        id: `${Date.now()}-${index}-${Math.random().toString(36).slice(2, 8)}`,
        name: hostname,
        alias,
        hostname,
        user,
        port,
        hasPassword: false,
        hostInsightsEnabled: true,
        hostLiveMetricsEnabled: true,
        identityFile: item.identityFile,
        proxyJump: item.proxyJump,
        envVars: "",
        encoding: "utf-8",
        sortOrder: maxSortOrder + imported + 1,
        tags: [],
        notes: `Imported from ${item.sourcePath}`,
        updatedAt: now,
        deleted: false,
      });
      existingAlias.add(aliasKey);
      existingEndpoint.add(endpointKey);
      imported += 1;
    }

    if (append.length === 0) {
      await message("No new hosts were imported. Selected hosts may already exist.", {
        title: "SSH Config Import",
        kind: "info",
      });
      return;
    }

    const nextHosts = [...hosts, ...append];
    try {
      await saveHostsToBackend(nextHosts);
      setHosts(nextHosts);
      setShowSshImportDialog(false);
      await message(`Imported ${imported} host(s).${skipped > 0 ? ` Skipped ${skipped} duplicate(s).` : ""}`, {
        title: "SSH Config Import",
        kind: "info",
      });
    } catch (error) {
      await message(`Failed to import hosts.\n\n${String(error)}`, {
        title: "SSH Config Import",
        kind: "error",
      });
    }
  }

  async function openEditDialog(host: Host) {
    setEditingHost(host);
    setFormData({ ...host });
    setShowDialog(true);
    if (!isInTauri || !host.hasPassword) return;
    try {
      const password = await invoke<string | null>("host_password_get", { hostId: host.id });
      setFormData((prev) => ({ ...prev, password: password ?? "", hasPassword: !!(password && password.trim()) }));
    } catch (e) {
      console.debug("[keychain] preload host password failed", e);
    }
  }

  async function handleSave() {
    const now = new Date().toISOString();
    let newHosts: Host[];
    let targetId: string | null = null;
    let pwAction: "keep" | "set" | "clear" = "keep";
    let pwValue = "";

    if (editingHost) {
      const patch: any = { ...formData, updatedAt: now };
      if (typeof formData.password === "string") {
        patch.hasPassword = formData.password.trim().length > 0;
      }
      newHosts = hosts.map((h) => (h.id === editingHost.id ? ({ ...h, ...patch } as Host) : h));
      targetId = editingHost.id;
      if (typeof formData.password === "string") {
        const t = formData.password.trim();
        if (!t) {
          pwAction = "clear";
        } else {
          pwAction = "set";
          pwValue = formData.password;
        }
      }
    } else {
      const newHost: Host = {
        id: Date.now().toString(),
        name: formData.hostname || "Unnamed",
        alias: formData.alias || formData.hostname?.toLowerCase().replace(/\s+/g, "-") || "new-host",
        hostname: formData.hostname || "",
        user: formData.user || "",
        port: formData.port || 22,
        password: formData.password,
        hasPassword: !!(formData.password && String(formData.password).trim()),
        hostInsightsEnabled: formData.hostInsightsEnabled !== false,
        hostLiveMetricsEnabled: formData.hostLiveMetricsEnabled !== false,
        identityFile: formData.identityFile,
        proxyJump: formData.proxyJump,
        envVars: formData.envVars,
        encoding: formData.encoding || "utf-8",
        tags: formData.tags || [],
        notes: formData.notes || "",
        updatedAt: now,
        deleted: false,
      };
      newHosts = [...hosts, newHost];
      targetId = newHost.id;
      if (typeof formData.password === "string") {
        const t = formData.password.trim();
        if (!t) {
          pwAction = "keep";
        } else {
          pwAction = "set";
          pwValue = formData.password;
        }
      }
    }

    newHosts = newHosts.map((h) => (h.password ? { ...h, password: undefined } : h));

    try {
      await saveHostsToBackend(newHosts);

      if (isInTauri && targetId) {
        if (pwAction === "set") {
          await invoke("host_password_set", { hostId: targetId, password: pwValue });
        } else if (pwAction === "clear") {
          await invoke("host_password_delete", { hostId: targetId });
        }
      }
    } catch (e) {
      try {
        await message(`Failed to save host.\n\n${String(e)}`, { title: "Save Failed", kind: "error" });
      } catch {
        // Ignore.
      }
      return;
    }
    setShowDialog(false);
    await loadHosts();
  }

  async function deleteHost(host: Host) {
    const ok = await confirm(`Delete "${host.hostname}"?`, {
      title: "Delete Host",
      kind: "warning",
    });
    if (!ok) return;

    const newHosts = hosts.map((h) =>
      h.id === host.id ? { ...h, deleted: true, updatedAt: new Date().toISOString() } : h
    );
    await saveHostsToBackend(newHosts);
    if (isInTauri) {
      try {
        await invoke("host_password_delete", { hostId: host.id });
      } catch {
        // Ignore.
      }
    }
    setHosts(newHosts);
  }

  async function persistHostOrder(nextHosts: Host[]) {
    const normalized = normalizeHostOrder(nextHosts);
    setHosts(normalized);
    await saveHostsToBackend(normalized);
  }

  const activeHosts = hosts.filter((h) => !h.deleted);
  const filteredHosts = useMemo(() => {
    const q = hostSearch.trim().toLowerCase();
    if (!q) return activeHosts;
    return activeHosts.filter((h) => {
      const hay = [
        h.alias,
        h.hostname,
        h.user,
        `${h.user || ""}@${h.hostname || ""}`,
        (h.tags || []).join(" "),
        h.notes,
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      return hay.includes(q);
    });
  }, [activeHosts, hostSearch]);

  const sortedHosts = useMemo(() => {
    const arr = filteredHosts.slice();
    arr.sort((a, b) => {
      const ao = typeof a.sortOrder === "number" ? a.sortOrder : Number.MAX_SAFE_INTEGER;
      const bo = typeof b.sortOrder === "number" ? b.sortOrder : Number.MAX_SAFE_INTEGER;
      if (ao !== bo) return ao - bo;
      const at = Date.parse(a.updatedAt || "") || 0;
      const bt = Date.parse(b.updatedAt || "") || 0;
      return bt - at;
    });
    return arr;
  }, [filteredHosts]);

  useEffect(() => {
    const el = hostListRef.current;
    if (!el) return;
    const check = () => setHostListScrollable(el.scrollHeight > el.clientHeight + 1);
    check();
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => ro.disconnect();
  }, [sortedHosts.length, sidebarOpen]);

  return {
    hosts,
    setHosts,
    hostsRef,
    loadHosts,
    hostSearch,
    setHostSearch,
    hostListRef,
    hostListScrollable,
    reorderMode,
    setReorderMode,
    showDialog,
    setShowDialog,
    editingHost,
    formData,
    setFormData,
    sortedHosts,
    persistHostOrder,
    openEditDialog,
    openAddDialog,
    openSshImportDialog,
    importSshConfigHosts,
    showSshImportDialog,
    setShowSshImportDialog,
    sshImportCandidates,
    sshImportLoading,
    selectIdentityFile,
    handleSave,
    deleteHost,
  };
}
