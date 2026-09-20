// Build the xtermius-mcp-bridge sidecar and stage it for Tauri's
// `bundle.externalBin`. Tauri expects `<name>-<target-triple>` next to
// tauri.conf.json; this script compiles it in release mode for the host (or
// an explicitly requested target) and copies it into place.

import { execSync } from "node:child_process";
import { copyFileSync, chmodSync, existsSync, mkdirSync } from "node:fs";
import path from "node:path";

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const tauriDir = path.join(root, "src-tauri");
// Precedence: explicit argv > env (CI matrix) > host. The release workflow
// exports XTERM_BRIDGE_TARGET=${{ matrix.target }} so the sidecar matches
// the --target passed to `tauri build`.
const target =
  process.argv[2] || process.env.XTERM_BRIDGE_TARGET || hostTargetTriple();

function hostTargetTriple() {
  const verbose = execSync("rustc -vV", { encoding: "utf8" });
  const host = verbose.match(/^host:\s*(\S+)$/m)?.[1];
  if (!host) throw new Error("cannot determine host target triple");
  return host;
}

console.log(`[bridge-sidecar] building xtermius-mcp-bridge for ${target}`);
// tauri-build validates that every bundle.externalBin exists whenever ANY
// binary of the crate is built — including the bridge itself, before it has
// been staged. Bypass that chicken-and-egg validation for this build only.
execSync(
  `cargo build --release --locked --bin xtermius-mcp-bridge --target ${target}`,
  {
    cwd: tauriDir,
    stdio: "inherit",
    env: {
      ...process.env,
      TAURI_CONFIG: JSON.stringify({ bundle: { externalBin: [] } }),
    },
  }
);

const built = path.join(tauriDir, "target", target, "release", "xtermius-mcp-bridge");
if (!existsSync(built)) {
  throw new Error(`bridge binary not found at ${built}`);
}
const staged = path.join(tauriDir, `xtermius-mcp-bridge-${target}`);
copyFileSync(built, staged);
chmodSync(staged, 0o755);
console.log(`[bridge-sidecar] staged ${path.relative(root, staged)}`);
