import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
import ts from "typescript";

function loadWebdavSync(invoke, dialog = {}) {
  const source = fs.readFileSync(new URL("../src/hooks/useWebdavSync.ts", import.meta.url), "utf8");
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  });
  const exports = {};
  vm.runInNewContext(outputText, {
    exports,
    require(name) {
      if (name === "react") return { useState: (initial) => [initial, () => {}] };
      if (name === "@tauri-apps/api/core") return { invoke };
      if (name === "@tauri-apps/api/path") return { configDir: async () => "/test-config" };
      if (name === "@tauri-apps/plugin-dialog") {
        return {
          confirm: dialog.confirm ?? (async () => true),
          message: dialog.message ?? (async () => {}),
        };
      }
      throw new Error(`Unexpected dependency: ${name}`);
    },
    console,
  }, { filename: "src/hooks/useWebdavSync.ts" });
  return exports.useWebdavSync;
}

test("push reads current backend hosts instead of a stale empty snapshot", async () => {
  const currentHosts = [
    { id: "host-a", hostname: "new.example.test", deleted: false },
  ];
  let uploaded;
  const calls = [];
  let confirms = 0;
  const useWebdavSync = loadWebdavSync(async (command) => {
    calls.push(command);
    if (command === "hosts_load") return structuredClone(currentHosts);
    if (command === "settings_load") return {};
    if (command === "webdav_push") uploaded = structuredClone(currentHosts);
  }, {
    confirm: async () => { confirms += 1; return true; },
  });

  const sync = useWebdavSync({
    isInTauri: true,
    hostsRef: { current: [] },
    loadHosts: async () => {},
  });
  await sync.doWebdavPush();

  assert.equal(confirms, 0);
  assert.deepEqual(uploaded, currentHosts);
  assert.deepEqual(calls, ["hosts_load", "settings_save", "settings_load", "webdav_push"]);
});

test("empty backend requires confirmation and cancellation does not push", async () => {
  const calls = [];
  let confirms = 0;
  let pushed = false;
  const useWebdavSync = loadWebdavSync(async (command) => {
    calls.push(command);
    if (command === "hosts_load") return [];
    if (command === "webdav_push") pushed = true;
  }, {
    confirm: async () => { confirms += 1; return false; },
  });

  const sync = useWebdavSync({ isInTauri: true, loadHosts: async () => {} });
  await sync.doWebdavPush();

  assert.equal(confirms, 1);
  assert.equal(pushed, false);
  assert.deepEqual(calls, ["hosts_load"]);
});

test("failed backend host read aborts before settings save or push", async () => {
  const calls = [];
  let confirms = 0;
  let messages = 0;
  const useWebdavSync = loadWebdavSync(async (command) => {
    calls.push(command);
    if (command === "hosts_load") throw new Error("database unavailable");
  }, {
    confirm: async () => { confirms += 1; return true; },
    message: async () => { messages += 1; },
  });

  const sync = useWebdavSync({ isInTauri: true, loadHosts: async () => {} });
  await sync.doWebdavPush();

  assert.equal(confirms, 0);
  assert.equal(messages, 1);
  assert.deepEqual(calls, ["hosts_load"]);
});
