import assert from "node:assert/strict";
import fs from "node:fs";

function read(path) {
  return fs.readFileSync(path, "utf8");
}

function readJson(path) {
  return JSON.parse(read(path));
}

function assertMatch(source, pattern, message) {
  assert.match(source, pattern, message);
}

function assertOrdered(source, first, second, message) {
  const firstIndex = source.indexOf(first);
  const secondIndex = source.indexOf(second);
  assert.ok(firstIndex >= 0, `${message}: missing "${first}"`);
  assert.ok(secondIndex >= 0, `${message}: missing "${second}"`);
  assert.ok(firstIndex < secondIndex, message);
}

function assertPackageScript() {
  const packageJson = readJson("package.json");
  assert.equal(
    packageJson.scripts?.["test:frontend-regressions"],
    "node scripts/test_frontend_regressions.mjs",
    "package.json must expose the frontend regression script"
  );
}

function assertBellStyleReachesXterm() {
  const runtime = read("src/hooks/terminal/runtime.ts");
  assertMatch(
    runtime,
    /term\.onBell\(\(\) => \{[\s\S]*terminalOptionsRef\.current\.bellStyle\s*!==\s*"sound"/,
    "xterm bell events must be gated by terminalOptions.bellStyle"
  );
  assertMatch(
    runtime,
    /sessionBellDisposables/,
    "xterm bell subscriptions must be disposed with their session terminal"
  );
}

function assertMetricsDockGatesLivePolling() {
  const appController = read("src/hooks/useAppController.ts");
  const hostInsights = read("src/hooks/useHostInsights.ts");

  assertMatch(
    appController,
    /useHostInsights\(\{[\s\S]*metricsDockEnabled[\s\S]*\}\)/,
    "useAppController must pass metricsDockEnabled into useHostInsights"
  );
  assertMatch(
    hostInsights,
    /metricsDockEnabled:\s*boolean/,
    "useHostInsights must accept metricsDockEnabled as an explicit parameter"
  );
  assertOrdered(
    hostInsights,
    "if (!metricsDockEnabled)",
    'invoke<HostLiveInfo>("host_probe_live"',
    "Live host polling must be gated before invoking host_probe_live"
  );
  assertMatch(
    hostInsights,
    /activeSession\.status\s*!==\s*"running"/,
    "Live host polling must only run for running sessions"
  );
  assertMatch(
    hostInsights,
    /livePollInFlight/,
    "Live host polling must use a single-flight in-flight guard"
  );
  assertMatch(
    hostInsights,
    /\[\s*isInTauri[\s\S]*metricsDockEnabled[\s\S]*activeSessionId[\s\S]*\]/,
    "metricsDockEnabled must be included in the live polling effect dependencies"
  );
}

function assertAllSessionResizePath() {
  const runtime = read("src/hooks/terminal/runtime.ts");
  assertMatch(
    runtime,
    /lastSentPtySizeBySessionRef/,
    "PTY resize dedupe state must be tracked per session"
  );
  assertMatch(
    runtime,
    /function\s+fitAndResizeMountedPtys/,
    "Runtime must expose an all-mounted-session fit/resize path"
  );
  assertMatch(
    runtime,
    /for\s*\(\s*const\s+\[\s*sessionId\s*,\s*handle\s*\]\s+of\s+terminalRefs\.sessionTerminals\.current\s*\)/,
    "All-session resize must iterate every mounted session terminal"
  );
  assertMatch(
    runtime,
    /invoke\("pty_resize"[\s\S]*sessionId[\s\S]*cols[\s\S]*rows/,
    "All-session resize path must resize each PTY with deduped cols/rows"
  );
}

function assertPtyDataQueueBackpressure() {
  const ptyEvents = read("src/hooks/terminal/ptyEvents.ts");
  assertMatch(
    ptyEvents,
    /type\s+PtyDataQueueState/,
    "PTY data writes must use an explicit per-session queue state"
  );
  assertMatch(
    ptyEvents,
    /PTY_DATA_BACKPRESSURE_CHARS/,
    "PTY data queue must define a backpressure threshold"
  );
  assertMatch(
    ptyEvents,
    /ptyDataQueues\.current\.get\(sessionId\)/,
    "PTY data queue must be keyed by session id"
  );
  assertMatch(
    ptyEvents,
    /requestAnimationFrame/,
    "PTY data queue must schedule batched xterm writes with RAF"
  );
  assertMatch(
    ptyEvents,
    /handle\.terminal\.write\([\s\S]*\(\)\s*=>/,
    "PTY data queue must continue flushing through xterm write callbacks"
  );
  assertMatch(
    ptyEvents,
    /queuedChars/,
    "PTY data queue must track queued characters for backpressure"
  );
}

function assertToastA11y() {
  const toastViewport = read("src/components/ui/ToastViewport.tsx");
  assertMatch(toastViewport, /role="status"/, "Toast items must use status semantics");
  assertMatch(toastViewport, /aria-live="polite"/, "Toast items must be announced politely");
  assertMatch(toastViewport, /aria-atomic="true"/, "Toast announcements must be atomic");
}

function assertSessionTabsA11y() {
  const mainPane = read("src/components/layout/MainPane.tsx");
  assertMatch(mainPane, /role="tablist"/, "Session tabs container must expose tablist semantics");
  assertMatch(mainPane, /<button[\s\S]*role="tab"/, "Each session tab must be a button with tab semantics");
  assert.doesNotMatch(
    mainPane,
    /<div[^>]*onClick=\{\(\) => setActiveSessionId\(session\.id\)\}[^>]*>/,
    "Session tab switching must not be handled by a clickable div wrapper"
  );
  assertMatch(mainPane, /aria-selected=\{active\}/, "Session tabs must expose selected state");
  assertMatch(mainPane, /onKeyDown=\{\(e\) => \{[\s\S]*ArrowRight[\s\S]*ArrowLeft/, "Session tabs must support arrow-key navigation");
}

assertPackageScript();
assertBellStyleReachesXterm();
assertMetricsDockGatesLivePolling();
assertAllSessionResizePath();
assertPtyDataQueueBackpressure();
assertToastA11y();
assertSessionTabsA11y();

console.log("Frontend regressions verified");
