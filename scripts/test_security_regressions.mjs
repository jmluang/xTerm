import assert from "node:assert/strict";
import fs from "node:fs";

function read(path) {
  return fs.readFileSync(path, "utf8");
}

function readJson(path) {
  return JSON.parse(read(path));
}

function assertNoRendererSecretReads() {
  const app = read("src-tauri/src/app.rs");
  const frontend = [
    "src/hooks/useHostsManager.ts",
    "src/hooks/terminal/actions.ts",
    "src/hooks/terminal/ptyEvents.ts",
    "src/hooks/useWebdavSync.ts",
    "src/components/settings/SettingsPanel.tsx",
  ]
    .filter((path) => fs.existsSync(path))
    .map((path) => `${path}\n${read(path)}`)
    .join("\n");

  assert.doesNotMatch(app, /host_password_get/, "host_password_get must not be exposed as a Tauri command");
  assert.doesNotMatch(frontend, /host_password_get/, "frontend must not invoke host_password_get");
  assert.doesNotMatch(frontend, /sessionAutoPasswords|password prompt|auto password/i, "frontend must not cache or sniff SSH passwords");
}

function assertTypedPtySpawn() {
  const pty = read("src-tauri/src/pty.rs");
  const frontendActions = read("src/hooks/terminal/actions.ts");

  assert.match(pty, /pub async fn pty_spawn_ssh/, "backend must expose typed SSH session spawning");
  assert.doesNotMatch(
    pty,
    /#\[tauri::command\]\s*pub async fn pty_spawn\s*<[^>]*>\s*\([^)]*file:\s*String/s,
    "generic pty_spawn(file, args, cwd, env) must not be exposed as a Tauri command"
  );
  assert.match(frontendActions, /invoke<string>\("pty_spawn_ssh"/, "frontend must call pty_spawn_ssh");
  assert.doesNotMatch(frontendActions, /file:\s*["']\/usr\/bin\/ssh["']|args:\s*\[/, "frontend must not assemble ssh command args");
  assert.doesNotMatch(
    pty,
    /SSH_ASKPASS_REQUIRE|SSH_ASKPASS|NumberOfPasswordPrompts=1/,
    "interactive terminal SSH sessions must not force askpass or collapse keyboard-interactive MFA prompts"
  );
  assert.match(
    pty,
    /AutoPasswordState[\s\S]*AUTO_PASSWORD_ARM_SECONDS[\s\S]*AutoPasswordPromptMatcher/,
    "saved SSH passwords must be handled through bounded target-matched PTY prompt response, not askpass"
  );
  assert.match(pty, /fn disarm\(&mut self\)/, "saved SSH password prompt handling must be explicitly disarmable");
  assert.match(pty, /parse_env_vars[\s\S]*SetEnv=/, "host envVars must be parsed and sent into SSH sessions");
  assert.match(pty, /PtyOutputDecoder[\s\S]*Encoding::for_label/, "host encoding must drive PTY output decoding");
}

function assertWebviewBoundary() {
  const config = readJson("src-tauri/tauri.conf.json");
  const capabilities = read("src-tauri/capabilities/default.json");
  const cargoToml = read("src-tauri/Cargo.toml");

  assert.equal(config.app.windows[0].devtools, false, "production devtools must be disabled");
  assert.ok(config.app.security.csp && typeof config.app.security.csp === "string", "CSP must be configured");
  assert.doesNotMatch(capabilities, /"shell:allow-open"/, "shell open capability must not be granted when unused");
  assert.doesNotMatch(cargoToml, /tauri-plugin-shell/, "unused shell plugin dependency must be removed");
  assert.doesNotMatch(capabilities, /"process:default"/, "process default capability must be narrowed to restart only");
  assert.match(capabilities, /"process:allow-restart"/, "updater relaunch only needs process restart");
}

function assertWebdavPasswordNotSerialized() {
  const models = read("src-tauri/src/models.rs");
  const hostStore = read("src-tauri/src/host_store.rs");
  const webdavSync = read("src-tauri/src/webdav_sync.rs");

  assert.match(models, /has_webdav_password/, "Settings must expose has_webdav_password instead of returning the secret");
  assert.match(models, /skip_serializing[\s\S]*pub webdav_password/, "webdav_password must be skipped during settings serialization");
  assert.match(hostStore, /webdav_password_set|webdav_password_delete/, "settings_save must write WebDAV password to Keychain");
  assert.doesNotMatch(webdavSync, /settings\.webdav_password/, "WebDAV sync must read password from Keychain, not settings JSON");
}

function assertSshConfigValidation() {
  const sshConfig = read("src-tauri/src/ssh_config.rs");

  assert.match(sshConfig, /validate_host_for_ssh_config/, "ssh_config generation must validate host fields");
  assert.match(sshConfig, /contains\('\\n'\)|contains\('\\r'\)|is_control/, "ssh_config validation must reject newline/control characters");
  assert.match(sshConfig, /rejects_newline_in_alias/, "ssh_config regression tests must cover directive injection");
}

function assertHostProbeAuthBoundary() {
  const hostProbe = read("src-tauri/src/host_probe.rs");

  assert.doesNotMatch(
    hostProbe,
    /StrictHostKeyChecking=accept-new/,
    "host probes must not silently trust new SSH host keys"
  );
  assert.match(hostProbe, /create_new\(true\)/, "host probe askpass scripts must be created atomically");
  assert.match(hostProbe, /mode\(0o700\)/, "host probe askpass scripts must be executable only by the owner");
  assert.doesNotMatch(hostProbe, /fs::write\(&path,\s*script\)/, "host probe askpass scripts must not use non-atomic fs::write");
  assert.match(hostProbe, /probe_ssh_args/, "host probes must build SSH args through a testable helper");
  assert.match(hostProbe, /"-F"[\s\S]*get_ssh_config_path/, "host probes must reuse the generated ssh_config");
  assert.match(hostProbe, /ControlMaster=auto[\s\S]*ControlPersist=30s/, "host live probes must reuse SSH connections with ControlMaster");
}

function assertWebdavCanonicalDbSync() {
  const webdavSync = read("src-tauri/src/webdav_sync.rs");
  const dbIndex = webdavSync.indexOf('"hosts.db"');
  const jsonIndex = webdavSync.indexOf('"hosts.json"');
  assert.ok(dbIndex >= 0 && jsonIndex >= 0 && dbIndex < jsonIndex, "WebDAV pull must try hosts.db before legacy hosts.json");
  assert.match(webdavSync, /canonical.*hosts\.db|hosts\.db.*canonical/i, "WebDAV sync must document hosts.db as the canonical remote format");
}

assertNoRendererSecretReads();
assertTypedPtySpawn();
assertWebviewBoundary();
assertWebdavPasswordNotSerialized();
assertSshConfigValidation();
assertHostProbeAuthBoundary();
assertWebdavCanonicalDbSync();

console.log("Security regressions verified");
