import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import ts from "typescript";
import vm from "node:vm";

const SESSION_ID = "pty-test-session";

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

function createHarness() {
  const listeners = new Map();
  const timers = new Map();
  const animationFrames = new Map();
  let nextTimerId = 1;
  let nextAnimationFrameId = 1;
  const window = {
    setTimeout(callback, delay) {
      const id = nextTimerId++;
      timers.set(id, { callback, delay });
      return id;
    },
    clearTimeout(id) {
      timers.delete(id);
    },
    requestAnimationFrame(callback) {
      const id = nextAnimationFrameId++;
      animationFrames.set(id, callback);
      return id;
    },
    cancelAnimationFrame(id) {
      animationFrames.delete(id);
    },
  };
  const runtimeRefs = Object.fromEntries([
    "sessionBuffers", "sessionConnectTimers", "sessionMeta", "sessionCloseReason",
  ].map((key) => [key, { current: new Map() }]));
  runtimeRefs.sessionHadAnyOutput = { current: new Set() };
  runtimeRefs.sessionConnectingCounted = { current: new Set() };
  runtimeRefs.sessionMeta.current.set(SESSION_ID, { hostId: "host-a" });
  const terminalRefs = {
    activeSessionIdRef: { current: null },
    sessionTerminals: { current: new Map() },
  };
  const [, setSessions] = state([]);
  const [, setConnectingHosts] = state({});
  const [, setActiveSessionId] = state(null);
  const buffer = loadModule("src/hooks/terminal/sessionBuffer.ts", {});
  const { usePtyEvents } = loadModule("src/hooks/terminal/ptyEvents.ts", {
    react: { useRef: (current) => ({ current }), useEffect: (effect) => effect() },
    "@tauri-apps/api/event": {
      listen: async (name, callback) => {
        listeners.set(name, callback);
        return () => listeners.delete(name);
      },
    },
    "@/hooks/terminal/types": { MAX_SESSION_BUFFER_CHARS: 2_000_000 },
    "@/hooks/terminal/sessionBuffer": buffer,
    "@/lib/perfMetrics": { markFirstSessionOutput: () => {} },
  }, { window });
  usePtyEvents({
    isInTauri: true,
    runtimeRefs,
    terminalRefs,
    setSessions,
    setConnectingHosts,
    setActiveSessionId,
  });

  const writes = [];
  const writeCallbacks = [];
  terminalRefs.sessionTerminals.current.set(SESSION_ID, {
    terminal: {
      write(data, callback) {
        writes.push(data);
        if (callback) writeCallbacks.push(callback);
      },
    },
  });

  return {
    runtimeRefs,
    terminalRefs,
    writes,
    writeCallbacks,
    emit(event) {
      const callback = listeners.get(event.name);
      assert.ok(callback, `No listener for ${event.name}`);
      callback({ payload: event.payload });
    },
    runNextTimer() {
      const entry = timers.entries().next();
      assert.equal(entry.done, false, "Expected a scheduled timer");
      const [id, { callback, delay }] = entry.value;
      timers.delete(id);
      assert.equal(delay, 50, "PTY fallback timer must remain 50ms");
      callback();
    },
    completeWrite() {
      const callback = writeCallbacks.shift();
      assert.ok(callback, "Expected a pending xterm write callback");
      callback();
    },
    hasScheduledAnimationFrame() {
      return animationFrames.size > 0;
    },
    hasScheduledTimer() {
      return timers.size > 0;
    },
    readBuffered() {
      return buffer.readSessionBuffer(runtimeRefs.sessionBuffers.current, SESSION_ID);
    },
  };
}

function drain(harness) {
  while (harness.writeCallbacks.length > 0 || harness.hasScheduledTimer()) {
    if (harness.writeCallbacks.length > 0) {
      harness.completeWrite();
    } else {
      harness.runNextTimer();
    }
  }
}

test("PTY writes drain in order with a 64,000-character bound", () => {
  const harness = createHarness();
  const chunks = Array.from({ length: 320 }, (_, index) => {
    const prefix = String(index).padStart(3, "0");
    return `${prefix}:${String.fromCharCode(65 + (index % 26)).repeat(300 - prefix.length - 1)}`;
  });
  chunks[100] = `${"p".repeat(63_999)}😀tail`;
  chunks[101] = `${"q".repeat(99)}\ud83d`;
  chunks[102] = `\ude00${"r".repeat(199)}`;
  const expected = chunks.join("");

  for (const data of chunks) {
    harness.emit({ name: "pty:data", payload: { session_id: SESSION_ID, data } });
  }
  assert.equal(harness.writes.length, 0, "Writes must wait for the scheduled flush");
  assert.equal(harness.hasScheduledAnimationFrame(), true, "The visible-path RAF must remain scheduled");
  harness.runNextTimer();
  drain(harness);

  assert.ok(harness.writes.length > 1, "The payload must drain in multiple writes");
  assert.ok(harness.writes.every((data) => data.length <= 64_000));
  assert.equal(harness.writes.join(""), expected);
});

test("PTY fallback timer flushes when RAF is left pending", () => {
  const harness = createHarness();
  harness.emit({ name: "pty:data", payload: { session_id: SESSION_ID, data: "timer-output" } });
  assert.equal(harness.writes.length, 0);
  harness.runNextTimer();
  assert.deepEqual(harness.writes, ["timer-output"]);
  harness.completeWrite();
});

test("Failed exit drains a queued tail after the in-flight write callback", () => {
  const harness = createHarness();
  harness.runtimeRefs.sessionMeta.current.set(SESSION_ID, { hostId: "host-a" });
  const head = "h".repeat(90_000);
  const tail = "tail-".repeat(30_000);
  harness.emit({ name: "pty:data", payload: { session_id: SESSION_ID, data: head } });
  harness.runNextTimer();
  assert.equal(harness.writes.length, 1);
  assert.ok(harness.writes[0].length <= 64_000);

  harness.emit({ name: "pty:data", payload: { session_id: SESSION_ID, data: tail } });
  harness.emit({ name: "pty:exit", payload: { session_id: SESSION_ID, code: 255 } });
  assert.equal(harness.writes.length, 1, "Exit handling must not write around an in-flight callback");

  drain(harness);
  assert.ok(harness.writes.every((data) => data.length <= 64_000));
  assert.equal(harness.writes.join(""), head + tail);
});

test("Missing terminal buffers pending chunks without joining the queue", () => {
  const harness = createHarness();
  const chunks = ["first-", "second-", "third"];
  for (const data of chunks) {
    harness.emit({ name: "pty:data", payload: { session_id: SESSION_ID, data } });
  }
  harness.terminalRefs.sessionTerminals.current.delete(SESSION_ID);
  harness.runNextTimer();
  assert.equal(harness.readBuffered(), chunks.join(""));
});
