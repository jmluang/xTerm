import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, message, open } from "@tauri-apps/plugin-dialog";
import type { Host } from "@/types/models";

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
    setFormData({ name: "", alias: "", hostname: "", user: "", port: 22, tags: [], notes: "" });
    setShowDialog(true);
  }

  function openEditDialog(host: Host) {
    setEditingHost(host);
    setFormData({ ...host });
    setShowDialog(true);
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
    selectIdentityFile,
    handleSave,
    deleteHost,
  };
}
