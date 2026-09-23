import assert from "node:assert/strict";
import fs from "node:fs";

function read(filePath) {
  return fs.readFileSync(filePath, "utf8");
}

function exists(filePath) {
  return fs.existsSync(filePath);
}

function section(text, header) {
  const start = text.search(new RegExp(`^${header}:\\s*$`, "m"));
  assert.notEqual(start, -1, `Expected ${header} section to exist`);
  const headerEnd = text.indexOf("\n", start);
  if (headerEnd === -1) {
    return text.slice(start);
  }
  const body = text.slice(headerEnd + 1);
  const next = body.search(/^[a-zA-Z_][\w-]*:\s*(?:#.*)?$/m);
  return next === -1 ? text.slice(start) : text.slice(start, headerEnd + 1 + next);
}

function assertIncludesAll(text, expected, context) {
  for (const value of expected) {
    assert.ok(text.includes(value), `${context} must include ${value}`);
  }
}

function releaseBuildJob(workflow) {
  const start = workflow.indexOf("\n  build:\n");
  const end = workflow.indexOf("\n  finalize_release:\n", start);
  assert.notEqual(start, -1, "Expected release build job to exist");
  assert.notEqual(end, -1, "Expected release finalizer to follow the build job");
  return workflow.slice(start, end);
}

function namedStep(workflow, name) {
  const marker = `      - name: ${name}`;
  const start = workflow.indexOf(marker);
  assert.notEqual(start, -1, `Expected workflow step ${name} to exist`);
  const next = workflow.indexOf("\n      - name:", start + marker.length);
  return workflow.slice(start, next === -1 ? undefined : next);
}

const failures = [];

function check(name, fn) {
  try {
    fn();
    console.log(`ok - ${name}`);
  } catch (error) {
    failures.push({ name, error });
    console.error(`not ok - ${name}`);
    console.error(`  ${error.message}`);
  }
}

check("README release docs no longer reference the removed build-dmg workflow", () => {
  const readme = read("README.md");
  assert.doesNotMatch(readme, /build-dmg\.ya?ml/, "README.md must not reference build-dmg.yml");
  assert.match(readme, /\.github\/workflows\/release\.yml/, "README.md must document the current release workflow");
  assert.match(readme, /latest\.json/, "README.md must document the updater latest.json asset");
  assert.match(readme, /aarch64|Apple Silicon/, "README.md must document the Apple Silicon release target");
  assert.match(readme, /x64|Intel/, "README.md must document the Intel release target");
});

check("CI workflow runs frontend, Rust, and updater regression checks", () => {
  const workflowPath = ".github/workflows/ci.yml";
  assert.ok(exists(workflowPath), "Expected .github/workflows/ci.yml to exist");
  const workflow = read(workflowPath);

  assertIncludesAll(
    workflow,
    [
      "npm ci",
      "npm run build",
      "npm run test:security",
      "npm run test:frontend-regressions",
      "cargo test",
      "npm run tauri build -- --no-bundle",
      "npm run test:updater",
    ],
    ".github/workflows/ci.yml"
  );
  assert.match(
    workflow,
    /EXPECTED_RELEASE_VERSION="\$\(node -p "require\('\.\/package\.json'\)\.version"\)"\s+npm run test:updater/,
    "CI updater regressions must use the checked-out package version instead of relying on fetched git tags"
  );
});

check("Tauri JavaScript API is pinned to the locked Rust minor release", () => {
  const packageJson = JSON.parse(read("package.json"));
  const packageLock = JSON.parse(read("package-lock.json"));
  const apiVersion = packageJson.dependencies?.["@tauri-apps/api"];
  const lockedApiVersion = packageLock.packages?.["node_modules/@tauri-apps/api"]?.version;
  const cargoLock = read("src-tauri/Cargo.lock");
  const tauriVersion = cargoLock.match(/\[\[package\]\]\nname = "tauri"\nversion = "([^"]+)"/)?.[1];

  assert.match(
    apiVersion ?? "",
    /^\d+\.\d+\.\d+$/,
    "@tauri-apps/api must use an exact version to prevent lockfile-only minor upgrades"
  );
  assert.equal(apiVersion, lockedApiVersion, "package.json and package-lock.json must agree on @tauri-apps/api");
  assert.ok(tauriVersion, "Cargo.lock must contain the resolved tauri package version");
  assert.equal(
    apiVersion.split(".").slice(0, 2).join("."),
    tauriVersion.split(".").slice(0, 2).join("."),
    "@tauri-apps/api and Rust tauri must use the same major/minor version"
  );
});

check("workflow_dispatch is either removed or has an explicit tag input", () => {
  const workflow = read(".github/workflows/release.yml");
  if (!/workflow_dispatch\s*:/.test(workflow)) {
    return;
  }

  const onBlock = section(workflow, "on");
  assert.match(onBlock, /workflow_dispatch:\s*\n\s+inputs:\s*\n\s+tag:/, "workflow_dispatch must define a tag input");
});

check("release workflow derives tag and version for push and manual dispatch", () => {
  const workflow = read(".github/workflows/release.yml");
  assert.match(workflow, /github\.event\.inputs\.tag/, "Release workflow must read manual dispatch tag input");
  assert.match(workflow, /GITHUB_REF_NAME/, "Release workflow must still support tag push refs");
  assert.match(workflow, /RELEASE_VERSION="\$\{RELEASE_TAG#v\}"/, "Release workflow must derive version from RELEASE_TAG");
});

check("release workflow checks out the selected release tag", () => {
  const workflow = read(".github/workflows/release.yml");
  assert.match(
    workflow,
    /ref:\s*\$\{\{\s*github\.event\.inputs\.tag\s*\|\|\s*github\.ref\s*\}\}/,
    "Release workflow must check out the manual dispatch tag or pushed tag ref"
  );
});

check("release workflow creates the draft release before tauri-action uploads assets", () => {
  const workflow = read(".github/workflows/release.yml");
  assert.match(
    workflow,
    /gh release create "\$\{RELEASE_TAG\}"[\s\S]*--draft[\s\S]*--verify-tag/,
    "Release workflow must create the draft release without passing target_commitish to the create-release API"
  );
  assert.doesNotMatch(
    workflow,
    /releaseCommitish:/,
    "tauri-action must not pass target_commitish when creating or finding the release"
  );
});

check("release workflow runs static release tests after syncing versions", () => {
  const workflow = read(".github/workflows/release.yml");
  assert.match(workflow, /name:\s*Run release regression tests/, "Release workflow must have a post-sync regression test step");
  assert.match(workflow, /npm run test:security/, "Release workflow must run security regressions after version sync");
  assert.match(
    workflow,
    /npm run test:frontend-regressions/,
    "Release workflow must run frontend regressions after version sync"
  );
  assert.match(
    workflow,
    /EXPECTED_RELEASE_VERSION="\$\{RELEASE_VERSION\}"\s+npm run test:updater/,
    "Release workflow must run updater tests with EXPECTED_RELEASE_VERSION after version sync"
  );
  assert.match(
    workflow,
    /EXPECTED_RELEASE_VERSION="\$\{RELEASE_VERSION\}"\s+npm run test:release-workflow/,
    "Release workflow must run release workflow tests with EXPECTED_RELEASE_VERSION after version sync"
  );
});

check("release runs Rust tests, warning-free clippy, and the frontend production build before tauri-action", () => {
  const workflow = read(".github/workflows/release.yml");
  const buildJob = releaseBuildJob(workflow);
  const regressionStep = namedStep(buildJob, "Run release regression tests");
  const publishStep = namedStep(buildJob, "Build and publish Tauri release");

  assert.ok(
    buildJob.indexOf("      - name: Run release regression tests") <
      buildJob.indexOf("      - name: Build and publish Tauri release"),
    "All release checks must finish before tauri-action can publish assets"
  );
  assert.match(
    regressionStep,
    /cargo test --locked --manifest-path src-tauri\/Cargo\.toml --lib --bins --tests/,
    "Release workflow must run Rust library, binary, and integration tests"
  );
  assert.match(
    regressionStep,
    /cargo clippy --locked --manifest-path src-tauri\/Cargo\.toml --lib --bins -- -D warnings/,
    "Release workflow must fail on Rust clippy warnings"
  );
  assert.match(regressionStep, /npm run build/, "Release workflow must run the TypeScript and frontend production build");
  assert.match(publishStep, /uses:\s*tauri-apps\/tauri-action@/, "Expected tauri-action to remain the publishing step");
});

check("each release architecture forwards its bridge target to staging and tauri-action", () => {
  const workflow = read(".github/workflows/release.yml");
  const buildJob = releaseBuildJob(workflow);
  const stageStep = namedStep(buildJob, "Stage MCP bridge sidecar");
  const publishStep = namedStep(buildJob, "Build and publish Tauri release");
  const tauriConfig = JSON.parse(read("src-tauri/tauri.conf.json"));
  const bridgeScript = read("scripts/build_bridge_sidecar.mjs");
  const matrixTarget = /XTERM_BRIDGE_TARGET:\s*\$\{\{\s*matrix\.target\s*\}\}/;

  assert.match(stageStep, matrixTarget, "Sidecar staging must use the current matrix target");
  assert.match(
    publishStep,
    matrixTarget,
    "tauri-action beforeBuildCommand must receive the current matrix target when it rebuilds the bridge"
  );
  assert.match(
    buildJob,
    /- os:\s*macos-14\s*\n\s+target:\s*aarch64-apple-darwin[\s\S]*?- os:\s*macos-15-intel\s*\n\s+target:\s*x86_64-apple-darwin/,
    "Release matrix must continue to build Apple Silicon and Intel targets"
  );
  assert.match(
    tauriConfig.build?.beforeBuildCommand ?? "",
    /npm run build:bridge/,
    "tauri-action beforeBuildCommand must rebuild the sidecar that consumes XTERM_BRIDGE_TARGET"
  );
  assert.match(
    bridgeScript,
    /process\.env\.XTERM_BRIDGE_TARGET/,
    "Sidecar build script must use XTERM_BRIDGE_TARGET passed by the release matrix"
  );
});

check("local Playwright outputs are ignored and generated snapshots are absent", () => {
  const gitignore = read(".gitignore");
  assert.match(gitignore, /^\.playwright-mcp\/$/m, ".gitignore must exclude the Playwright MCP artifact directory");
  assert.match(gitignore, /^output\/playwright\/$/m, ".gitignore must exclude generated Playwright output");
  assert.doesNotMatch(gitignore, /^!docs\/COMPILATION\.md$/m, ".gitignore must not unignore the removed compilation notes");
  assert.equal(
    exists(".playwright-mcp/page-2026-04-19T15-43-03-823Z.yml"),
    false,
    "Tracked Playwright MCP page snapshot must be removed"
  );
  assert.equal(
    exists("output/playwright/release-page-snapshot.md"),
    false,
    "Tracked Playwright release snapshot must be removed"
  );
});

check("obsolete compilation notes are removed without removing superpowers documentation", () => {
  assert.equal(exists("docs/COMPILATION.md"), false, "Obsolete compilation notes must be removed");
  assert.equal(exists("docs/superpowers"), true, "Superpowers documentation must remain available");
});

check("release finalizer validates latest.json with jq against version, platform signatures, and release assets", () => {
  const workflow = read(".github/workflows/release.yml");
  const finalizer = section(workflow, "jobs").match(/finalize_release:[\s\S]*$/)?.[0] ?? "";
  assert.ok(finalizer, "Expected finalize_release job to exist");

  assert.match(finalizer, /\bjq\b/, "Release finalizer must parse latest.json with jq");
  assert.match(finalizer, /\.version/, "Release finalizer must validate latest.json version");
  assertIncludesAll(
    finalizer,
    ["darwin-aarch64", "darwin-aarch64-app", "darwin-x86_64", "darwin-x86_64-app"],
    "Release finalizer latest.json validation"
  );
  assert.match(finalizer, /\.platforms/, "Release finalizer must validate latest.json platforms");
  assert.match(finalizer, /\.signature/, "Release finalizer must validate non-empty platform signatures");
  assert.match(finalizer, /basename/, "Release finalizer must compare latest.json URL basenames with release assets");
  assert.match(finalizer, /url/, "Release finalizer must validate platform URLs");
  assert.match(finalizer, /GITHUB_REF_NAME|RELEASE_TAG/, "Release finalizer must validate URLs against the current tag");
});

check("package.json exposes test:release-workflow", () => {
  const packageJson = JSON.parse(read("package.json"));
  assert.equal(
    packageJson.scripts?.["test:release-workflow"],
    "node scripts/test_release_workflow.mjs",
    "package.json must expose the release workflow regression test"
  );
});

if (failures.length > 0) {
  console.error(`\n${failures.length} release workflow regression check(s) failed.`);
  process.exit(1);
}

console.log("Release workflow regressions verified");
