#!/usr/bin/env bash
set -euo pipefail

# Cold-start benchmark for packaged xTermius.app on macOS.
# It measures: open app -> first window appears.
#
# Usage:
#   ./scripts/bench_startup.sh
# Optional env:
#   RUNS=10 WARMUP=3 WAIT_TIMEOUT_SEC=20 APP_PATH=... APP_NAME=... PROCESS_NAME=...

RUNS="${RUNS:-8}"
WARMUP="${WARMUP:-3}"
WAIT_TIMEOUT_SEC="${WAIT_TIMEOUT_SEC:-20}"
APP_NAME="${APP_NAME:-xTermius}"
PROCESS_NAME="${PROCESS_NAME:-xtermius}"
APP_PATH="${APP_PATH:-src-tauri/target/release/bundle/macos/${APP_NAME}.app}"

if [[ "${OSTYPE:-}" != darwin* ]]; then
  echo "This benchmark is macOS-only." >&2
  exit 1
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "hyperfine is required. Install with: brew install hyperfine" >&2
  exit 1
fi

if [[ ! -d "$APP_PATH" ]]; then
  echo "App bundle not found: $APP_PATH" >&2
  echo "Build first: npm run tauri build" >&2
  exit 1
fi

quit_app() {
  osascript -e "tell application \"$APP_NAME\" to quit" >/dev/null 2>&1 || true
  pkill -9 -x "$PROCESS_NAME" >/dev/null 2>&1 || true
  for _ in {1..200}; do
    if ! pgrep -x "$PROCESS_NAME" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.05
  done
}

wait_first_window() {
  local ui_name="$1"
  local timeout_sec="$2"

  osascript <<OSA
set timeoutSeconds to ${timeout_sec}
set startAt to (current date)
tell application "System Events"
  repeat
    if exists process "${ui_name}" then
      tell process "${ui_name}"
        if (count of windows) > 0 then
          return
        end if
      end tell
    end if
    if ((current date) - startAt) > timeoutSeconds then
      error "timeout waiting first window for ${ui_name}" number 124
    end if
    delay 0.01
  end repeat
end tell
OSA
}

cold_start_once() {
  quit_app
  sleep 1.0
  sync

  open -na "$APP_PATH"
  wait_first_window "$APP_NAME" "$WAIT_TIMEOUT_SEC"
  quit_app
}

export APP_NAME PROCESS_NAME APP_PATH WAIT_TIMEOUT_SEC
export -f quit_app wait_first_window cold_start_once

quit_app

echo "Benchmark config: runs=${RUNS} warmup=${WARMUP} timeout=${WAIT_TIMEOUT_SEC}s"
hyperfine \
  --warmup "$WARMUP" \
  --runs "$RUNS" \
  --style full \
  --command-name "${APP_NAME} cold start" \
  "bash -lc 'cold_start_once'"
