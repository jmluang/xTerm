import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
import ts from "typescript";

// Execute the production hooks with deterministic IPC and event scheduling.
// No desktop commands, credentials, network requests, or real timers are used.
function loadModule(path, dependencies, globals = {}) {
  const source = fs.readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  });
  const exports = {};
  vm.runInNewContext(outputText, {
    exports,
    require(name) {
      assert.ok(name in dependencies, `Unexpected dependency: ${name}`);
      return dependencies[name];
    },
    console,
    ...globals,
  }, { filename: path });
  return exports;
}

function state(initial) {
  const cell = { current: initial };
  return [cell, (next) => {
    cell.current = typeof next === "function" ? next(cell.current) : next;
  }];
}

test("WebDAV push preserves hosts saved after the settings window opened", async () => {
  const oldHost = { id: "host-a", hostname: "old.example.test", deleted: false };
  const currentHosts = [
    { ...oldHost, hostname: "new.example.test" },
    { id: "host-b", hostname: "second.example.test", deleted: false },
  ];
  let database = structuredClone(currentHosts);
  let uploaded;
  const { useWebdavSync } = loadModule("src/hooks/useWebdavSync.ts", {
    react: { useState: (initial) => [initial, () => {}] },
    "@tauri-apps/api/core": {
      invoke: async (command, args) => {
        if (command === "hosts_save") database = structuredClone(args.hosts);
        if (command === "hosts_load") return structuredClone(database);
        if (command === "settings_load") return {};
        if (command === "webdav_push") uploaded = structuredClone(database);
      },
    },
    "@tauri-apps/api/path": { configDir: async () => "/test-config" },
    "@tauri-apps/plugin-dialog": { confirm: async () => true, message: async () => {} },
  });
  const sync = useWebdavSync({
    isInTauri: true,
    hostsRef: { current: [oldHost] },
    loadHosts: async () => {},
  });
  await sync.doWebdavPush();
  assert.deepEqual({ database, uploaded }, { database: currentHosts, uploaded: currentHosts });
});

function sessionHarness(earlyEvent, options = {}) {
  const listeners = new Map();
  const timers = new Map();
  const timerDelays = new Map();
  const calls = [];
  const alerts = [];
  let nextTimer = 1;
  let nextSession = 1;
  const crypto = { randomUUID: () => `session-${nextSession++}` };
  const window = {
    crypto,
    setTimeout: (callback, delay) => {
      const id = nextTimer++;
      timers.set(id, callback);
      timerDelays.set(id, delay);
      return id;
    },
    clearTimeout: (id) => { timers.delete(id); timerDelays.delete(id); },
    requestAnimationFrame: () => 0,
    cancelAnimationFrame: () => {},
  };
  const runtimeRefs = Object.fromEntries([
    "sessionBuffers", "sessionConnectTimers", "sessionMeta", "sessionCloseReason",
  ].map((key) => [key, { current: new Map() }]));
  runtimeRefs.sessionHadAnyOutput = { current: new Set() };
  runtimeRefs.sessionConnectingCounted = { current: new Set() };
  const terminalRefs = {
    activeSessionIdRef: { current: null },
    sessionTerminals: { current: new Map() },
    terminalInstance: { current: null },
  };
  const [sessions, setSessions] = state([]);
  const [connecting, setConnectingHosts] = state({});
  const [active, setActive] = state(null);
  const setActiveSessionId = (next) => {
    setActive(next);
    if (!options.deferActiveRef) terminalRefs.activeSessionIdRef.current = active.current;
  };
  const params = { isInTauri: true, runtimeRefs, terminalRefs, setSessions, setConnectingHosts, setActiveSessionId };
  const buffer = loadModule("src/hooks/terminal/sessionBuffer.ts", {});
  const { usePtyEvents } = loadModule("src/hooks/terminal/ptyEvents.ts", {
    react: { useRef: (current) => ({ current }), useEffect: (effect) => effect() },
    "@tauri-apps/api/event": {
      listen: async (name, callback) => { listeners.set(name, callback); return () => {}; },
    },
    "@/hooks/terminal/types": { MAX_SESSION_BUFFER_CHARS: 2_000_000 },
    "@/hooks/terminal/sessionBuffer": buffer,
    "@/lib/perfMetrics": { markFirstSessionOutput: () => {} },
  }, { window });
  usePtyEvents(params);
  const emit = (event) => listeners.get(event.name)({ payload: event.payload });
  const { useSessionActions } = loadModule("src/hooks/terminal/actions.ts", {
    "@tauri-apps/api/core": {
      invoke: async (command, args) => {
        calls.push({ command, args });
        if (command === "pty_kill" && options.kill) return options.kill(args);
        if (command === "pty_spawn_ssh") {
          // The Rust backend starts its emitter threads before returning the ID.
          for (const event of Array.isArray(earlyEvent) ? earlyEvent : earlyEvent ? [earlyEvent] : []) emit(event);
          if (options.spawn) return options.spawn(args);
          return args.sessionId ?? "session-1";
        }
      },
    },
    "@tauri-apps/plugin-dialog": { confirm: async () => false },
  }, {
    window,
    crypto,
    requestAnimationFrame: window.requestAnimationFrame,
    alert: options.allowAlerts ? (message) => alerts.push(message) : assert.fail,
  });
  const host = { id: "host-a", alias: "test-host", hostname: "example.test" };
  const actions = useSessionActions({ ...params, hosts: [host], activeSessionId: null });
  return {
    connect: () => actions.connectToHost(host), close: actions.closeSession,
    sessions, connecting, emit, active, calls, alerts,
    terminalRefs, runtimeRefs, timers, timerDelays,
    fireTimeout: async (delay) => {
      const entry = [...timerDelays].find(([, value]) => value === delay);
      assert.ok(entry, `Expected a ${delay}ms timer`);
      const [id] = entry;
      const callback = timers.get(id);
      window.clearTimeout(id);
      await callback();
    },
    flush: () => {
      const callback = [...timers.values()].at(-1);
      assert.ok(callback, "Expected a scheduled flush");
      callback();
    },
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((onResolve, onReject) => { resolve = onResolve; reject = onReject; });
  return { promise, resolve, reject };
}

async function until(condition) {
  for (let i = 0; i < 50; i += 1) {
    if (condition()) return;
    await Promise.resolve();
  }
  assert.fail("Expected asynchronous action did not occur");
}

const firstOutput = { name: "pty:data", payload: { session_id: "session-1", data: "Ready\r\n" } };
const failedExit = { name: "pty:exit", payload: { session_id: "session-1", code: 255 } };

for (const early of [false, true]) {
  test(`First output ${early ? "before" : "after"} spawn response marks the session running`, async () => {
    const harness = sessionHarness(early ? firstOutput : null);
    await harness.connect();
    if (!early) harness.emit(firstOutput);
    assert.equal(harness.sessions.current[0]?.status, "running");
    assert.equal(Object.keys(harness.connecting.current).length, 0);
    assert.equal(harness.runtimeRefs.sessionConnectTimers.current.size, 0);
  });

  test(`Failed exit ${early ? "before" : "after"} spawn response marks the session exited`, async () => {
    const harness = sessionHarness(early ? failedExit : null);
    await harness.connect();
    if (!early) harness.emit(failedExit);
    assert.equal(harness.sessions.current[0]?.status, "exited");
    assert.equal(harness.sessions.current[0]?.exitCode, 255);
    assert.equal(Object.keys(harness.connecting.current).length, 0);
    assert.equal(harness.runtimeRefs.sessionConnectTimers.current.size, 0);
  });
}

for (const code of [0, 255]) {
  test(`Early output followed by exit ${code} is reconciled without a connection timer`, async () => {
    const harness = sessionHarness([
      firstOutput,
      { name: "pty:exit", payload: { session_id: "session-1", code } },
    ]);
    await harness.connect();
    assert.equal(harness.sessions.current.length, code === 0 ? 0 : 1);
    if (code !== 0) {
      assert.equal(harness.sessions.current[0].status, "exited");
      assert.equal(harness.sessions.current[0].exitCode, code);
    }
    assert.equal(Object.keys(harness.connecting.current).length, 0);
    assert.equal(harness.runtimeRefs.sessionConnectTimers.current.size, 0);
  });
}

test("Early successful exit clears selection before the active ref effect runs", async () => {
  const harness = sessionHarness(
    { name: "pty:exit", payload: { session_id: "session-1", code: 0 } },
    { deferActiveRef: true },
  );
  await harness.connect();
  assert.equal(harness.sessions.current.length, 0);
  assert.equal(harness.active.current, null);
});

test("Timed-out spawn is removed and its late backend session is killed", async () => {
  const spawn = deferred();
  const harness = sessionHarness(null, { spawn: () => spawn.promise, allowAlerts: true });
  const connecting = harness.connect();
  await until(() => [...harness.timerDelays.values()].includes(10_000));
  await harness.fireTimeout(10_000);
  await connecting;
  assert.equal(harness.sessions.current.length, 0);
  assert.equal(Object.keys(harness.connecting.current).length, 0);
  assert.equal(harness.runtimeRefs.sessionMeta.current.size, 0);
  harness.emit(firstOutput);
  harness.emit(failedExit);
  spawn.resolve("session-1");
  await until(() => harness.calls.some((call) => call.command === "pty_kill"));
  assert.equal(harness.sessions.current.length, 0);
  assert.equal(harness.runtimeRefs.sessionBuffers.current.size, 0);
  assert.equal(harness.runtimeRefs.sessionConnectTimers.current.size, 0);
});

test("Closing during spawn never resurrects the tab and kills the late process", async () => {
  const spawn = deferred();
  let backendRunning = false;
  const harness = sessionHarness(null, {
    spawn: () => spawn.promise,
    kill: () => { backendRunning = false; },
  });
  const connecting = harness.connect();
  await until(() => harness.calls.some((call) => call.command === "pty_spawn_ssh"));
  await harness.close("session-1");
  backendRunning = true;
  spawn.resolve("session-1");
  await connecting;
  assert.equal(harness.sessions.current.length, 0);
  assert.equal(Object.keys(harness.connecting.current).length, 0);
  assert.equal(backendRunning, false, "The late backend process must not remain running");
  assert.equal(harness.runtimeRefs.sessionConnectTimers.current.size, 0);
  assert.equal(harness.runtimeRefs.sessionCloseReason.current.size, 0);
});

test("Closing an already failed tab leaves no lifecycle entries", async () => {
  const harness = sessionHarness(failedExit);
  await harness.connect();
  await harness.close("session-1");
  assert.equal(harness.sessions.current.length, 0);
  for (const ref of Object.values(harness.runtimeRefs)) assert.equal(ref.current.size, 0);
});

test("Exit delivered during user close releases the connecting count exactly once", async () => {
  const harness = sessionHarness(null, { kill: () => harness.emit(failedExit) });
  await harness.connect();
  await harness.close("session-1");
  assert.equal(harness.sessions.current.length, 0);
  assert.equal(Object.keys(harness.connecting.current).length, 0);
  for (const ref of Object.values(harness.runtimeRefs)) assert.equal(ref.current.size, 0);
});

test("Spawn rejection after user close leaves no lifecycle entries", async () => {
  const spawn = deferred();
  const harness = sessionHarness(null, { spawn: () => spawn.promise, allowAlerts: true });
  const connecting = harness.connect();
  await until(() => harness.calls.some((call) => call.command === "pty_spawn_ssh"));
  await harness.close("session-1");
  spawn.reject(new Error("spawn unavailable"));
  await connecting;
  assert.equal(harness.sessions.current.length, 0);
  for (const ref of Object.values(harness.runtimeRefs)) assert.equal(ref.current.size, 0);
});

test("Spawn failure after output does not consume another connection's host count", async () => {
  const pending = new Map();
  const harness = sessionHarness(null, {
    allowAlerts: true,
    spawn: (args) => {
      const item = deferred();
      pending.set(args.sessionId, item);
      return item.promise;
    },
  });
  const first = harness.connect();
  const second = harness.connect();
  await until(() => pending.size === 2);
  assert.equal(harness.connecting.current["host-a"].count, 2);
  harness.emit(firstOutput);
  assert.equal(harness.connecting.current["host-a"].count, 1);
  pending.get("session-1").reject(new Error("spawn response failed"));
  await first;
  assert.equal(harness.connecting.current["host-a"].count, 1);
  pending.get("session-2").resolve("session-2");
  await second;
  harness.emit({ name: "pty:data", payload: { session_id: "session-2", data: "Ready" } });
  assert.equal(Object.keys(harness.connecting.current).length, 0);
});

test("Output backlog stays within the 64,000-character write batch budget", async () => {
  const harness = sessionHarness(null);
  await harness.connect();
  const writes = [];
  let completeWrite;
  harness.terminalRefs.sessionTerminals.current.set("session-1", {
    terminal: {
      write(data, callback) { writes.push(data); completeWrite = callback; },
    },
  });
  harness.emit(firstOutput);
  harness.flush();
  for (let i = 0; i < 10; i += 1) {
    harness.emit({ name: "pty:data", payload: { session_id: "session-1", data: "x".repeat(64_000) } });
  }
  completeWrite();
  harness.flush();
  assert.ok(writes[1].length <= 64_000, `A single write received ${writes[1].length} characters`);
});

test("Closing a failed tab discards its remaining output queue", async () => {
  const harness = sessionHarness(null);
  await harness.connect();
  let completeWrite;
  harness.terminalRefs.sessionTerminals.current.set("session-1", {
    terminal: { write(_data, callback) { completeWrite = callback; } },
  });
  harness.emit({ name: "pty:data", payload: { session_id: "session-1", data: "x".repeat(90_000) } });
  harness.flush();
  harness.emit(failedExit);
  await harness.close("session-1");
  harness.terminalRefs.sessionTerminals.current.delete("session-1");
  completeWrite();
  if (harness.timers.size) harness.flush();
  assert.equal(harness.runtimeRefs.sessionBuffers.current.size, 0);
  assert.equal(harness.timers.size, 0);
});
