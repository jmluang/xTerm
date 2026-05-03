import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";

function read(path) {
  return fs.readFileSync(path, "utf8");
}

function readJson(path) {
  return JSON.parse(read(path));
}

function latestReleaseVersion() {
  const tags = execFileSync("git", ["tag", "--sort=-v:refname"], { encoding: "utf8" })
    .split(/\r?\n/)
    .map((tag) => tag.trim())
    .filter(Boolean);

  const latest = tags.find((tag) => /^v\d+\.\d+\.\d+$/.test(tag));
  assert.ok(latest, "Expected at least one stable semver release tag");
  return latest.slice(1);
}

function assertCargoTomlVersion(expectedVersion) {
  const cargoToml = read("src-tauri/Cargo.toml");
  const match = cargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  assert.ok(match, "Expected src-tauri/Cargo.toml to declare a package version");
  assert.equal(match[1], expectedVersion, "src-tauri/Cargo.toml version must match the latest release tag");
}

function assertCargoLockVersion(expectedVersion) {
  const cargoLock = read("src-tauri/Cargo.lock");
  const match = cargoLock.match(/\[\[package\]\]\s+name = "xtermius"\s+version = "([^"]+)"/);
  assert.ok(match, "Expected src-tauri/Cargo.lock to contain the xtermius package entry");
  assert.equal(match[1], expectedVersion, "src-tauri/Cargo.lock xtermius version must match the latest release tag");
}

function assertAppVersionFiles(expectedVersion) {
  const packageJson = readJson("package.json");
  const packageLock = readJson("package-lock.json");
  const tauriConfig = readJson("src-tauri/tauri.conf.json");

  assert.equal(packageJson.version, expectedVersion, "package.json version must match the latest release tag");
  assert.equal(packageLock.version, expectedVersion, "package-lock.json root version must match the latest release tag");
  assert.equal(
    packageLock.packages?.[""]?.version,
    expectedVersion,
    "package-lock.json package entry version must match the latest release tag"
  );
  assert.equal(tauriConfig.version, expectedVersion, "src-tauri/tauri.conf.json version must match the latest release tag");
  assertCargoTomlVersion(expectedVersion);
  assertCargoLockVersion(expectedVersion);
}

function assertUpdaterAutoCheckContract() {
  const hook = read("src/hooks/useUpdaterController.ts");

  assert.match(
    hook,
    /autoCheckStartedRef/,
    "useUpdaterController must guard one automatic update check per controller instance"
  );
  assert.match(
    hook,
    /useEffect\(\(\) => \{[\s\S]*autoCheckStartedRef[\s\S]*runUpdateCheck\(\)/,
    "useUpdaterController must start an automatic update check when updater actions are enabled"
  );
  assert.match(
    hook,
    /checkForUpdates:\s*\(\)\s*=>\s*runUpdateCheck\(\)/,
    "Manual update checks must reuse the same runUpdateCheck path as the automatic check"
  );
}

const expectedVersion = latestReleaseVersion();
assertAppVersionFiles(expectedVersion);
assertUpdaterAutoCheckContract();

console.log(`Updater regressions verified for release ${expectedVersion}`);
