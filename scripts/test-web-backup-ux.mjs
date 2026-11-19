import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const app = readFileSync(new URL("../web/app.js", import.meta.url), "utf8");
const index = readFileSync(new URL("../web/index.html", import.meta.url), "utf8");
const css = readFileSync(new URL("../web/app.css", import.meta.url), "utf8");

function section(start, end) {
  const from = app.indexOf(start);
  assert.notEqual(from, -1, `missing ${start}`);
  const to = app.indexOf(end, from + start.length);
  assert.notEqual(to, -1, `missing ${end} after ${start}`);
  return app.slice(from, to);
}

test("the console presents an explicit alpha identity and truthful local access boundary", () => {
  assert.match(index, /<title>TapeScale Alpha Operations<\/title>/);
  assert.match(index, /class="release-badge">Alpha<\/span>/);
  assert.match(index, /class="version-channel">Alpha<\/span>/);
  assert.match(index, /id="app-version">—<\/span>/);
  assert.match(index, /app\.css\?v=tapescale-alpha-16/);
  assert.match(index, /app\.js\?v=tapescale-alpha-16/);
  assert.match(css, /\.release-badge/);
  assert.match(app, /document\.title = routeTitle \+ " · TapeScale Alpha"/);
  assert.match(index, /<strong>Local console<\/strong><small>No application sign-in<\/small>/);
  assert.doesNotMatch(index, /Authentication pending/);
  assert.match(app, /TapeScale Alpha does not provide application authentication for this local console/);
  assert.match(app, /trusted management network/);
  assert.match(app, /authenticating reverse proxy/);
});

test("operational chrome uses recovery exercise and manual motion language", () => {
  assert.match(index, />Recovery exercise<\/span>/);
  assert.doesNotMatch(index, /Recovery test/);
  assert.doesNotMatch(app, /Recovery test/);
  assert.match(app, /pageHead\("Recovery exercise"/);
  assert.match(app, /Manual scratch-media motion enabled/);
  assert.match(app, /Manual scratch-media motion is disabled/);
  assert.match(app, /"Manual motion"/);
  assert.match(app, /attended scratch-media motion request/);
  assert.doesNotMatch(app, /Lab tape motion|lab-only motion|"Lab motion"/);
  assert.match(app, /G0\.2 laboratory path—not a production backup policy/);
  assert.match(app, /G0\.2 · full · single tape · no encryption/);
});

test("cartridge headlines do not confuse the destructive reuse gate with operational state", () => {
  const helpers = section("  function mediaReuseState(", "  function mediaPoolId(");
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const testCount = (value) => Array.isArray(value) ? value.length : Number(value) || 0;
  const [operationalState, reuseAssessment, attentionRequired] = Function(
    "pick", "count",
    '"use strict";\n' + helpers + "\nreturn [mediaOperationalState, mediaReuseAssessment, mediaAttentionRequired];"
  )(testPick, testCount);

  const registered = { reuse_state: "needs_reconciliation", identity_status: "unknown", health_status: "unknown", has_content: false };
  const prepared = { ...registered, health_status: "qualified_scratch" };
  const verified = { ...registered, identity_status: "reconciled", health_status: "written_verified", has_content: true, confirmed_location_id: "location-1" };

  assert.equal(operationalState(registered), "registered");
  assert.equal(operationalState(prepared), "prepared_scratch");
  assert.equal(operationalState(verified), "verified_archive");
  assert.equal(operationalState({ ...verified, reuse_state: "protected", retained_recovery_points: 1 }), "protected");
  assert.equal(operationalState({ ...prepared, health_status: "write_unverified" }), "needs_attention");
  assert.equal(operationalState({ ...registered, identity_status: "mismatch" }), "identity_conflict");
  assert.equal(operationalState({ ...verified, catalog_reconciled: false }), "catalog_review");
  assert.equal(operationalState({ ...verified, identity_status: "unknown" }), "identity_review");
  assert.equal(operationalState({ ...verified, confirmed_location_id: null }), "custody_review");
  assert.equal(attentionRequired(prepared), false);
  assert.equal(attentionRequired({ ...verified, confirmed_location_id: null }), true);
  assert.equal(reuseAssessment(prepared), "Reuse evidence incomplete");
  assert.match(app, /Automatic tape ID is missing/);
  assert.match(app, /blank or legacy tape predates automatic identification/);
  assert.match(app, /Destructive reuse gate/);
});

test("the drive inventory shows the automatically detected loaded tape identity", () => {
  const drives = section("  function renderDrives()", "  function renderVerification()");
  assert.match(drives, /loaded_tape_identity_status/);
  assert.match(drives, /loaded_tape_identity_verified/);
  assert.match(drives, /loaded_cartridge_label/);
  assert.match(drives, /loaded_tape_volume_name/);
  assert.match(drives, /status\("identified"\)/);
  assert.match(drives, /status\("reading ID"\)/);
  assert.match(drives, /"Loaded tape"/);
  assert.match(drives, /Prepare this blank or legacy tape once/);
});

test("the start flow requires a currently eligible drive and tape", () => {
  const open = section("  async function openStartBackup(", "  function updateStartBackupTape(");
  const update = section("  function updateStartBackupTape(", "  async function submitStartBackup(");
  const submit = section("  async function submitStartBackup(", "  function backupMediaConfirmation(");

  assert.match(open, /await loadBackupMediaReadiness\("profile", profileId\)/);
  assert.match(open, /const pairs = readiness\.candidates/);
  assert.match(open, /new Set\(pairs\.map/);
  assert.doesNotMatch(open, /readiness\.recommended_candidate/, "an arbitrary inventory tape must not be presented as physically loaded");
  assert.match(open, /drives\.length === 1/);
  assert.match(open, /initialCandidates\.length === 1/);
  assert.match(open, /name="drive_id" required hidden/);
  assert.match(open, /name="cartridge_id" required hidden/);
  assert.match(open, /The button below is the destructive confirmation/);
  assert.match(open, /Erase .* and start backup/);
  assert.match(open, /const blocked = pairs\.length === 0/);
  assert.match(open, /backupReadinessRemediation\(readiness\)/);
  assert.match(open, /backupReadinessAction\(remediation, "profile", profileId\)/);
  assert.match(open, /data-action="refresh-backup-readiness"/);
  assert.match(open, /blocked \? " disabled" : ""/);

  assert.match(update, /backupReadinessCandidates\(form\.dataset\.readinessKey\)/);
  assert.match(update, /submit\.disabled = !drive \|\| !selected/);

  const validation = submit.indexOf('await loadBackupMediaReadiness("profile", profileId)');
  const currentModal = submit.indexOf("if (!backupModalSubmissionCurrent(form, token)) return;");
  const createRequest = submit.indexOf("await request(endpoints.backupProfiles");
  assert.ok(validation >= 0 && validation < currentModal && currentModal < createRequest, "readiness and modal cancellation must be checked before the durable request is created");
});

test("creating a request continues directly into final tape authorization", () => {
  const submit = section("  async function submitStartBackup(", "  function backupMediaConfirmation(");

  assert.match(submit, /await loadBackupMediaReadiness\("run", state\.selectedBackupRunId\)/);
  assert.match(submit, /await request\(endpoints\.backupRuns \+ "\/" \+ encodeURIComponent\(state\.selectedBackupRunId\) \+ "\/media"/);
  assert.match(submit, /typed_cartridge_label: exactLabel/);
  assert.match(submit, /loaded_media_confirmed: true/);
  assert.match(submit, /destructive_overwrite_confirmed: true/);
  assert.match(submit, /encryption_deviation_confirmed: true/);
  assert.match(submit, /confirmation: expectedConfirmation/);
  assert.doesNotMatch(submit, /openAssignBackupMedia\(/, "initial backup must remain in one modal and one operator action");
  assert.match(submit, /compression_mode: backupCompressionMode/);
  assert.match(submit, /spanning_enabled: backupSpanningEnabled/);
  assert.doesNotMatch(submit, /toast\("Full backup queued"/);
});

test("new backup policy uses bounded native DAR packs before sequential tape streaming", () => {
  const open = section("  function openBackupProfile(", "  async function submitBackupProfile(");
  const submit = section("  async function submitBackupProfile(", "  function backupCapacityPlan(");

  assert.match(open, /name="compression_mode"/);
  assert.match(open, /value="auto" selected/);
  assert.match(open, /value="zstd"/);
  assert.match(open, /value="lz4"/);
  assert.match(open, /value="gzip"/);
  assert.match(open, /value="none"/);
  assert.match(open, /Automatic · Zstandard fast \(recommended\)/);
  assert.match(open, /Automatic uses Zstandard level 1/);
  assert.match(open, /name="spanning_enabled" checked/);
  assert.match(open, /spans tapes as needed/);
  assert.match(open, /type="hidden" name="data_delivery" value="disk_spool"/);
  assert.doesNotMatch(open, /name="data_delivery" value="direct"/);
  assert.match(open, /Bounded disk buffer, any archive size/);
  assert.match(open, /independently validated archive packs/);
  assert.match(open, /reclaims disk space as the verified set advances/);
  assert.match(submit, /compression_mode: String\(values\.get\("compression_mode"\) \|\| "auto"\)/);
  assert.match(submit, /data_delivery: String\(values\.get\("data_delivery"\) \|\| "disk_spool"\)/);
  assert.match(submit, /spanning_enabled: values\.get\("spanning_enabled"\) === "on"/);
});

test("delivery labels distinguish advanced direct, bounded rolling, legacy, and unsupported plans", () => {
  const helpers = section("  function backupDataDeliveryExplicit(", "  function backupSpanningEnabled(");
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const values = Function(
    "pick", "backupProfileForRun", "backupEvidenceValue",
    '"use strict";\n' + helpers + "\nreturn [backupDataDelivery, backupDataDeliverySupported, backupDataDeliveryLabel];"
  )(testPick, () => null, testPick);
  assert.equal(values[0]({ data_delivery: "direct" }), "direct");
  assert.equal(values[2]({ data_delivery: "direct" }), "Direct streaming (advanced)");
  assert.equal(values[2]({ data_delivery: "disk_spool" }), "Bounded rolling staging");
  assert.equal(values[2]({ data_delivery: "disk_spool", engine_name: "TapeScale Native", engine_version: "1", format_version: "tapescale-native-v1" }), "Legacy whole-archive staging");
  assert.equal(values[2]({
    data_delivery: "disk_spool",
    data_run: { engine_name: "TapeScale Native", engine_version: "1", adapter_version: "native-v1", format_version: "tapescale-native-v1" }
  }), "Legacy whole-archive staging");
  assert.equal(values[2]({}), "Staged delivery (legacy)");
  assert.equal(values[1]({ data_delivery: "ram_ring" }), false);
  assert.equal(values[2]({ data_delivery: "ram_ring" }), "Unsupported (ram_ring)");
});

test("native compression choices and legacy aliases retain explicit labels", () => {
  const helpers = section("  function backupCompressionMode(", "  function backupSpanningEnabled(");
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const evaluate = Function(
    "pick",
    "backupProfileForRun",
    "backupEvidenceValue",
    '"use strict";\n' + helpers + "\nreturn [backupCompressionMode, backupCompressionSupported, backupCompressionLabel];"
  )(testPick, () => null, () => null);
  const unsupported = { compression_mode: "bzip2" };
  assert.equal(evaluate[2]({ compression_mode: "auto" }), "Automatic → Zstandard level 1 (fast)");
  const legacyAutomatic = Function(
    "pick",
    "backupProfileForRun",
    "backupEvidenceValue",
    '"use strict";\n' + helpers + "\nreturn backupCompressionLabel;"
  )(testPick, () => null, () => "gzip");
  assert.equal(legacyAutomatic({ compression_mode: "auto" }), "Automatic → GZIP1 (legacy)");
  assert.equal(evaluate[2]({ compression_mode: "zstd" }), "Zstandard level 1 (fast)");
  assert.equal(evaluate[2]({ compression_mode: "lz4" }), "LZ4 (fastest)");
  assert.equal(evaluate[2]({ compression_mode: "lzo" }), "LZO (legacy)");
  assert.equal(evaluate[0](unsupported), "bzip2");
  assert.equal(evaluate[1](unsupported), false);
  assert.equal(evaluate[2](unsupported), "Unsupported (bzip2)");

  const profiles = section("  function renderBackupProfiles(", "  function renderBackupRunTable(");
  assert.match(profiles, /!compressionSupported/);
  assert.match(profiles, /disabled title="Choose a new snapshot with supported compression and delivery settings\./);
  assert.match(profiles, /unsupported legacy setting/);
});

test("management work uses only the latest attempt per saved snapshot", () => {
  const helpers = section("  function backupRunSuccessful(", "  function backupRunMatches(");
  const terminal = new Set(["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"]);
  let runs = [];
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const functions = Function(
    "BACKUP_TERMINAL_STATES", "pick", "recordId", "orderedBackupRuns", "backupRunState", "backupWaitingForOperator", "backupRunActive",
    "backupMediaIsReadback", "backupNeedsNextMedia", "backupRunProfileName", "backupRunStateLabel", "backupTapeSummary", "label",
    "backupProfileForRun", "backupTableProgressLabel", "backupOperatorFailure", "esc", "date", "status",
    '"use strict";\n' + helpers + "\nreturn { latestBackupAttempts, backupWorkItems, backupSemanticState, backupFilterMatches };"
  )(
    terminal,
    testPick,
    (run) => String(run.id),
    () => runs,
    (run) => run.state,
    (run) => run.state === "waiting_for_media",
    (run) => !terminal.has(run.state),
    () => false,
    () => false,
    (run) => run.name,
    (run) => run.state,
    () => "Not assigned",
    String,
    () => null,
    () => null,
    () => "Recorded failure",
    String,
    String,
    String
  );

  runs = [
    { id: "new", profile_id: "profile-a", state: "verified", name: "Archive A" },
    { id: "old", profile_id: "profile-a", state: "failed", name: "Archive A" }
  ];
  assert.deepEqual(functions.latestBackupAttempts().map((run) => run.id), ["new"]);
  assert.equal(functions.backupWorkItems().length, 0, "a protected retry must clear the older failed attempt");

  runs = [
    { id: "wait", profile_id: "profile-w", state: "waiting_for_media", name: "Needs tape" },
    { id: "active", profile_id: "profile-b", state: "writing", name: "Streaming" },
    { id: "failed-new", profile_id: "profile-c", state: "failed", name: "Archive C" },
    { id: "failed-old", profile_id: "profile-c", state: "failed", name: "Archive C" }
  ];
  assert.deepEqual(functions.backupWorkItems().map((item) => item.kind), ["waiting", "failed", "active"]);
  assert.equal(functions.backupWorkItems().filter((item) => item.kind === "failed").length, 1);
});

test("the command center prioritizes and deduplicates unified work without hiding shared routes", () => {
  const helpers = section("  function managementActions()", "  function renderOverviewPrimaryAction(");
  const state = {
    data: {
      overview: {
        summary: { required_actions: 3 },
        actions: [
          { id: "same-backup-server", title: "Server duplicate", state: "waiting_for_media", route: "jobs/wait" },
          { id: "stale-active-server", title: "Stale tape wait", state: "waiting_for_media", route: "jobs/active" },
          { id: "tape-a", title: "Tape A conflict", state: "blocked", route: "tapes" },
          { id: "tape-b", title: "Tape B conflict", state: "blocked", route: "tapes" }
        ]
      }
    }
  };
  const work = [
    { kind: "waiting", run: { id: "wait", created_at: "2026-01-01" }, title: "Tape needed", detail: "Load tape" },
    { kind: "failed", run: { id: "failed", created_at: "2026-01-01" }, title: "Backup failed", detail: "Review" },
    { kind: "active", run: { id: "active", created_at: "2026-01-01" }, title: "Backup active", detail: "Writing" }
  ];
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const api = Function(
    "state", "backupWorkItems", "recordId", "pick", "count",
    '"use strict";\n' + helpers + "\nreturn { managementActions, managementNeedsCount };"
  )(state, () => work, (run) => run.id, testPick, (value) => Number(value) || 0);
  const actions = api.managementActions();
  assert.deepEqual(actions.map((action) => action.title), ["Tape needed", "Tape A conflict", "Tape B conflict", "Backup failed", "Backup active"]);
  assert.equal(actions.filter((action) => action.route === "jobs/wait").length, 1, "the same deep-linked backup must appear once");
  assert.equal(actions.find((action) => action.route === "jobs/active").title, "Backup active", "fresh polled backup state must replace a stale overview duplicate");
  assert.equal(actions.filter((action) => action.route === "tapes").length, 2, "distinct system actions may share a top-level route");
  assert.equal(api.managementNeedsCount(), 4, "healthy active work is excluded from Needs you");

  work.splice(0, work.length, { kind: "active", run: { id: "active", created_at: "2026-01-01" }, title: "Backup active", detail: "Writing" });
  state.data.overview.summary.required_actions = 1;
  state.data.overview.actions = [{ id: "stale-active-server", title: "Stale tape wait", state: "waiting_for_media", route: "jobs/active" }];
  assert.equal(api.managementNeedsCount(), 0, "a listed stale server duplicate must not keep the alert count raised after the run resumes");
});

test("backup history filters describe operator outcomes instead of raw engine states", () => {
  const helpers = section("  function backupRunSuccessful(", "  function backupProfileKey(");
  const terminal = new Set(["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"]);
  const semantic = Function(
    "BACKUP_TERMINAL_STATES", "backupRunState", "backupWaitingForOperator", "backupRunActive",
    '"use strict";\n' + helpers + "\nreturn backupSemanticState;"
  )(
    terminal,
    (run) => run.state,
    (run) => ["waiting_for_media", "waiting_for_next_media", "next_media", "media_required"].includes(run.state),
    (run) => !terminal.has(run.state)
  );
  for (const state of ["verified", "passed", "completed", "complete"]) assert.equal(semantic({ state }), "protected");
  for (const state of ["failed", "blocked", "interrupted"]) assert.equal(semantic({ state }), "unsuccessful");
  for (const state of ["cancelled", "canceled"]) assert.equal(semantic({ state }), "cancelled");
  for (const state of ["waiting_for_media", "waiting_for_next_media", "next_media", "media_required"]) assert.equal(semantic({ state }), "needs_tape");
  assert.equal(semantic({ state: "writing" }), "active");

  const table = section("  function renderBackupRunTable(", "  function renderBackupEvidence(");
  assert.match(table, /\["active", "In progress"\]/);
  assert.match(table, /\["needs_tape", "Waiting on me"\]/);
  assert.match(table, /\["unsuccessful", "Needs attention"\]/);
  assert.match(table, /\["protected", "Protected"\]/);
  assert.doesNotMatch(table, /All states/);
});

test("the operator home and Backup Center lead with current work and honest snapshot actions", () => {
  const overview = section("  function renderOverview()", "  function backupRunState(");
  const hero = section("  function renderOverviewPrimaryAction(", "  function renderOverview()");
  const profiles = section("  function renderBackupProfiles(", "  function renderBackupRunTable(");
  const jobs = section("  function renderBackupWorkQueue(", "  function mediaName(");
  const detail = section("  function renderBackupRunDetail(", "  function renderBackupWorkQueue(");

  assert.match(overview, /What needs attention now/);
  assert.match(overview, /Backup work/);
  assert.match(overview, /Prepared tapes/);
  assert.match(hero, /System action needed/);
  assert.match(hero, /safeActionRoute\(first\)/);
  assert.match(jobs, /Needs you/);
  assert.match(jobs, /In progress/);
  assert.match(profiles, /Saved snapshot selections/);
  assert.match(profiles, /Back up same snapshot/);
  assert.match(profiles, /Choose newer snapshot/);
  assert.match(profiles, /manual only/);
  assert.match(profiles, /scheduled exact snapshot/);
  assert.match(profiles, /Schedule repeats this exact snapshot/);
  assert.doesNotMatch(profiles, /Back up again/);
  assert.match(detail, /Retry same snapshot/);
  assert.match(detail, /Choose newer snapshot/);
  assert.match(detail, /Reload details/);
});

test("refresh and polling load only the data needed for current work", () => {
  const refresh = section("  function routeRefreshKeys(", "  async function refreshCurrentRoute(");
  const routeRefreshKeys = Function('"use strict";\n' + refresh + "\nreturn routeRefreshKeys;")();
  assert.deepEqual(routeRefreshKeys("jobs"), ["backupProfiles", "backupRuns", "media", "drives", "recoveryPoints"]);
  assert.deepEqual(routeRefreshKeys("movements"), ["movements", "media", "locations"]);
  assert.ok(!routeRefreshKeys("jobs").includes("qualificationRuns"));

  const poll = section("  function scheduleBackupRunPoll(", "  function openAddMedia(");
  assert.match(poll, /await loadOne\("backupRuns"\)/);
  assert.match(poll, /currentActive\.some\(backupWaitingForOperator\)/);
  assert.match(poll, /if \(completed\).*loadOne\("overview"\).*loadOne\("recoveryPoints"\)/s);
  assert.doesNotMatch(poll, /loadOne\("jobs"\)/);
  assert.doesNotMatch(poll, /Promise\.all\(\[loadOne\("backupRuns"\), loadOne\("jobs"\), loadOne\("overview"\)/);

  const clicks = section('  document.addEventListener("click"', '  document.addEventListener("input"');
  assert.match(clicks, /if \(action === "refresh"\) refreshCurrentRoute\(\)/);
});

test("mobile More opens complete navigation and exposes live work badges", () => {
  assert.match(index, /<button type="button" data-action="open-nav"[^>]*>[\s\S]*<small>More<\/small><\/button>/);
  assert.doesNotMatch(index, /data-route="settings" href="#settings">[\s\S]{0,120}<small>More<\/small>/);
  assert.match(index, /id="mobile-backup-count"/);
  assert.match(index, /id="mobile-action-count"/);
  assert.match(css, /\.mobile-nav-count/);
  const chrome = section("  function updateChrome()", "  function pageHead(");
  assert.match(chrome, /Backups, " \+ backupWork\.length \+ " current item/);
  assert.match(chrome, /Open all navigation, " \+ actionCount \+ " current item/);
});

test("the separate G0.2 phase track remains single-tape and uncompressed", () => {
  const phases = section("  const RECOVERY_PHASES = [", "  const RECOVERY_TERMINAL_STATES");
  assert.match(phases, /name: "Write tape", detail: "Full single-tape backup"/);
  assert.doesNotMatch(phases, /Compressed full backup|as many authorized tapes/);
});

test("bounded source and retained browsers disclose partial directory views", () => {
  const source = section("  function renderSourceBrowser(", "  async function loadSourceBrowser(");
  const retained = section("  function renderRecoveryPointFiles(", "  const RESTORE_REHEARSAL_PHASES");

  assert.match(source, /payload\.truncated === true/);
  assert.match(source, /This is a partial browser view/);
  assert.match(source, /type a known snapshot path/);
  assert.match(source, /no TapeScale total file-count limit/);
  assert.match(retained, /payload\.truncated === true/);
  assert.match(retained, /Only this directory view is bounded/);
  assert.match(retained, /retained manifest and tape archive are complete/);
  assert.match(retained, /backup itself was not limited/);
  assert.match(retained, /selected folder still includes descendants that are not displayed here/);
});

test("retained manifests provide an accessible, capability-gated selective restore planner", () => {
  const helpers = section("  function retainedRestorePath(", "  function recoveryFileBreadcrumbs(");
  const entry = section("  function recoveryFileEntry(", "  async function loadRecoveryPointFiles(");
  const browser = section("  function renderRecoveryPointFiles(", "  const RESTORE_REHEARSAL_PHASES");
  const openPlan = section("  function openRestorePlan(", "  function optionalFormNumber(");
  const planContract = section("  function restorePlanExecution(", "  function restorePlanTapes(");
  const submitPlan = section("  async function submitRestorePlan(", "  function openRestoreRehearsal(");
  const queue = section("  async function submitRestoreRehearsal(", "  function restoreMediaReadinessValue(");

  assert.match(helpers, /path_selective_restore_supported/);
  assert.match(helpers, /selection_capabilities/);
  assert.match(helpers, /"maximum_paths"/);
  assert.match(helpers, /maximum : 256/);
  assert.match(helpers, /"recursive_directories"/);
  assert.match(entry, /entry\.selectable === true/);
  assert.match(entry, /data-action="toggle-recovery-restore-path"/);
  assert.match(entry, /already included by a selected parent folder/);
  assert.match(browser, /Choose files and folders to restore/);
  assert.match(browser, /does not advertise path-selective restore/);
  assert.match(browser, /renderRecoveryRestoreSelectionTray/);
  assert.match(helpers, /data-action="plan-selected-restore"/);
  assert.match(helpers, /role="status"|aria-live="polite"/);
  assert.match(helpers, /including entries not shown in this partial directory view/);

  const state = {
    recoveryRestoreSelections: { point: ["folder/child.txt", "other.txt"] },
    recoveryFileBrowserCapabilities: {
      point: { path_selective_restore_supported: true, selection_capabilities: { maximum_paths: 256, recursive_directories: true } }
    }
  };
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const api = Function(
    "state", "pick", "toast", "render", "live", "window", "CSS", "esc",
    '"use strict";\n' + helpers + "\nreturn { toggleRecoveryRestorePath, recoveryRestorePathRelation, recoveryRestoreSelectionContract };"
  )(state, testPick, () => {}, () => {}, { textContent: "" }, { requestAnimationFrame: () => {} }, { escape: String }, String);
  api.toggleRecoveryRestorePath("point", "folder");
  assert.deepEqual(state.recoveryRestoreSelections.point, ["folder", "other.txt"], "a selected parent must replace redundant descendants");
  assert.equal(api.recoveryRestorePathRelation("point", "folder/deeper/file.txt"), "included");
  assert.deepEqual(api.recoveryRestoreSelectionContract("point"), { kind: "selected_paths", paths: ["folder", "other.txt"] });
  state.recoveryRestoreSelections.point = [];
  assert.deepEqual(api.recoveryRestoreSelectionContract("point"), { kind: "complete_point", paths: [] });

  assert.match(openPlan, /selection \|\| \{ kind: "complete_point", paths: \[\] \}/);
  assert.match(openPlan, /Plan selected restore/);
  assert.match(openPlan, /Folders recursively include everything beneath them/);
  assert.match(submitPlan, /selection,/);
  assert.match(submitPlan, /kind: "selected_paths"/);
  assert.match(submitPlan, /kind: "complete_point", paths: \[\]/);
  assert.match(planContract, /"normalized_paths", "paths", "requested_paths"/);
  assert.match(planContract, /"selection_hash"/);
  assert.match(planContract, /"selected_manifest_digest"/);
  assert.match(queue, /plan_id: planId/);
  assert.match(queue, /preview_hash:/);
  assert.doesNotMatch(queue, /selection:/, "execution must stay bound to the immutable server plan instead of resending mutable paths");

  assert.match(css, /\.recovery-selection-tray/);
  assert.match(css, /\.recovery-entry-selection/);
  assert.match(css, /\.restore-plan-scope/);
  assert.match(css, /@media \(max-width: 680px\)[\s\S]*\.recovery-selection-actions/);
});

test("the Recovery Vault keeps multi-tape full restore and single-tape selected restore inside the server contract", () => {
  const phases = section("  const RESTORE_REHEARSAL_PHASES = [", "  function restoreRehearsalDetailRecord(");
  const detailContract = section("  function restoreRehearsalVolumes(", "  function restoreRehearsalPhase(");
  const history = section("  function renderRestoreRehearsal(", "  function renderRecoveryPointDossier(");
  const dossier = section("  function renderRecoveryPointDossier(", "  function renderRestore(");
  const planDialog = section("  function openRestorePlan(", "  function optionalFormNumber(");
  const mediaContract = section("  function restorePlanTapes(", "  function restorePlanDrives(");
  const result = section("  function renderRestorePlanResult(", "  async function submitRestorePlan(");
  const start = section("  function openRestoreRehearsal(", "  async function submitRestoreRehearsal(");
  const queue = section("  async function submitRestoreRehearsal(", "  function restoreMediaReadinessValue(");
  const handoff = section("  function restoreMediaReadinessValue(", "  function openCancelRestoreRehearsal(");

  assert.match(phases, /name: "Restore files", detail: "Read the planned scope across its ordered tapes"/);
  assert.match(phases, /"waiting_for_media", "media_required"/);
  assert.match(phases, /name: "Verify files", detail: "Hash and compare restored content"/);
  assert.match(phases, /name: "Complete", detail: "Retain restore evidence"/);
  assert.doesNotMatch(phases, /Retain rehearsal evidence/);

  assert.match(detailContract, /arrayValue\(detail, "volumes", "required_volumes"\)/);
  assert.match(app, /"expected_cartridge_label", "expected_volume_label", "internal_label"/);
  assert.match(detailContract, /sort\(\(left, right\) => left\.sequence - right\.sequence\)/);
  assert.match(detailContract, /"media_request", "current_media_request"/);
  assert.match(detailContract, /pick\(detail, "media_handoff"\)/);
  assert.match(detailContract, /required.*waiting_for_media/s);
  assert.match(detailContract, /Exact ordered restore media/);
  assert.match(detailContract, /Required next/);

  assert.match(history, /<h3>Restore runs<\/h3>/);
  assert.match(history, /Offline recovery is available with the independent tapescale-recovery utility/);
  assert.match(history, /compatible legacy recovery point can run an attended restore only when the server offers that exact execution contract/);
  assert.match(history, /unsuccessful or retired outcome is audit history, not a recovery source/);
  assert.match(history, /Recovery planning is unavailable because this point is not verified/);
  assert.match(history, /<h4>Restore run<\/h4>/);
  assert.match(history, /Attended .*multi-tape.*single-tape/);
  assert.match(history, /Load tape .* of/);
  assert.match(history, /durably paused at a read-only media boundary/);
  assert.match(history, /data-action="open-restore-media-handoff"/);
  assert.match(history, /data-action="open-cancel-restore-rehearsal"/);
  assert.match(history, /Download restore evidence/);
  assert.doesNotMatch(history, /Ready G0\.2 restore plans|Recovery rehearsals/);

  assert.match(app, /Offline recovery available · in-console restore pending/);
  assert.match(app, /This DAR recovery point is cataloged, browseable, and physically read back/);
  assert.match(dossier, /pointVerified = pointState === "verified"/);
  assert.match(dossier, /legacy_format_retired/);
  assert.match(dossier, /Not a valid recovery point/);
  assert.match(dossier, /records are audit history only/);
  assert.match(dossier, /const planAction = pointVerified/);
  assert.match(app, /this Alpha restores the server-normalized selected paths only when this is a single-tape recovery point/);
  assert.match(app, /Plan a complete restore/);
  assert.match(planDialog, /server-returned ordered tape set/);

  assert.match(mediaContract, /planArray\(restorePlanExecution\(plan\), "required_tape_ids", "required_tapes", "tapes"\)/);
  assert.match(mediaContract, /pick\(execution, "multi_volume"\) === true/);
  assert.match(mediaContract, /declaredCount === tapes\.length/);
  assert.match(mediaContract, /tapes\.length === 1 \|\| restorePlanMultiVolumeSupported\(plan\)/);
  assert.match(result, /selectedMultiVolume = selectedScope && multiVolume/);
  assert.match(result, /executionSupported = ready && executionOffered && volumeContractReady && !selectedMultiVolume/);
  assert.match(result, /Selected-path execution requires a single-volume recovery point/);
  assert.match(result, /matching attended execution contract for this ordered media set/);
  assert.match(result, /Restore this complete recovery point/);
  assert.match(result, /Restore the selected content/);
  assert.match(result, /server-normalized selected scope/);
  assert.match(result, /Start Restore run/);
  assert.match(result, /pause durably at each media boundary and name the exact next cartridge/);
  assert.match(result, /Encrypted recovery points and clean-catalog recovery remain unavailable/);
  assert.match(result, /Every later tape requires its own exact attended handoff/);

  assert.match(start, /if \(!restorePlanVolumeContractReady\(plan\)\)/);
  assert.doesNotMatch(start, /if \(tapes\.length !== 1\)/);
  assert.match(start, /if \(selectedScope && tapes\.length > 1\)/);
  assert.match(start, /Generate a complete-point plan for this multi-tape backup/);
  assert.match(start, /pick\(restorePlanExecution\(plan\), "destination_subpath", "destination"\)/);
  assert.match(start, /planned selected scope.*complete recovery point/);
  assert.match(start, /Load only tape 1 now/);
  assert.match(start, /pauses durably and names each next tape/);
  assert.match(start, /immutably bound to the normalized paths and selection hash/);
  assert.match(start, /local console has no application sign-in/);
  assert.match(start, /new isolated destination shown above/);
  assert.match(start, /const onlyDrive = drives\.length === 1/);
  assert.match(start, /name="drive_id" required hidden/);
  assert.match(start, /reads its internal ID and blocks a mismatch/);
  assert.doesNotMatch(start, /name="tape_loaded_confirmed"/);
  assert.doesNotMatch(start, /name="attended_confirmed"/);
  assert.doesNotMatch(start, /name="confirmation"/);
  assert.match(queue, /confirmation: phrase/);
  assert.match(queue, /tape_loaded_confirmed: true/);
  assert.match(queue, /attended_confirmed: true/);
  assert.doesNotMatch(queue, /values\.get\("confirmation"\)/);
  assert.match(start, /openModal\("Start Restore run"/);
  assert.doesNotMatch(start, /supported G0\.2 recovery point|Start recovery rehearsal/);

  assert.match(handoff, /\/media-readiness/);
  assert.match(handoff, /pick\(readiness, "ready"\) !== true/);
  assert.match(handoff, /Incomplete durable tape request/);
  assert.match(handoff, /Unload and remove the previous tape, then insert this cartridge/);
  assert.match(handoff, /const selectedDrive = drives\.length === 1/);
  assert.match(handoff, /name="drive_id" required hidden/);
  assert.match(handoff, /TapeScale reads the internal tape ID and blocks a mismatch/);
  assert.match(handoff, /no typed phrase or repeated tape attestation is needed/);
  assert.doesNotMatch(handoff, /name="loaded_media_confirmed"/);
  assert.doesNotMatch(handoff, /name="attended_confirmed"/);
  assert.doesNotMatch(handoff, /name="confirmation"/);
  assert.match(handoff, /Finish later/);
  assert.match(handoff, /const readiness = await loadRestoreMediaReadiness\(runId\)/);
  assert.match(handoff, /The exact restore tape request changed while this form was open/);
  assert.match(handoff, /loaded_media_confirmed: true/);
  assert.match(handoff, /attended_confirmed: true/);
  assert.doesNotMatch(handoff, /values\.get\("loaded_media_confirmed"\)/);
  assert.doesNotMatch(handoff, /values\.get\("attended_confirmed"\)/);
  assert.doesNotMatch(handoff, /values\.get\("confirmation"\)/);
  assert.match(handoff, /endpoints\.restoreRehearsals \+ "\/" \+ encodeURIComponent\(runId\) \+ "\/media"/);
  for (const field of ["request_id", "drive_id", "expected_stable_id", "cartridge_id", "typed_cartridge_label", "volume_order", "loaded_media_confirmed", "attended_confirmed", "confirmation"]) {
    assert.match(handoff, new RegExp(field + ":"), `media handoff must send ${field}`);
  }
  assert.match(app, /action === "open-restore-media-handoff"/);
  assert.match(app, /event\.target\.id === "restore-media-handoff-form"/);
  assert.match(css, /\.restore-media-handoff/);
  assert.match(css, /\.restore-volume-plan li\.done/);
});

test("capacity planning is explicitly estimated and supports unknown archive size", () => {
  const capacity = section("  function backupCapacityPlan(", "  async function openStartBackup(");

  assert.match(capacity, /Estimated tapes/);
  assert.match(capacity, /Determined while writing/);
  assert.match(capacity, /planning guidance, not a guaranteed fit/);
  assert.match(capacity, /If another tape is needed/);
  assert.match(capacity, /create a new backup plan for this snapshot with media spanning enabled/);
  assert.doesNotMatch(app, /will estimate the tape count/);
  assert.doesNotMatch(capacity, /will fit/);
});

test("running backup detail is organized around one phase-specific operation", () => {
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");

  assert.match(detail, /renderBackupCurrentOperation\(run/);
  assert.doesNotMatch(detail, /renderBackupSpoolingCue\(run\)/);
  assert.doesNotMatch(detail, /renderBackupLiveThroughput\(run\)/);
  assert.doesNotMatch(detail, /measuredProgress|Measured progress/);
  assert.doesNotMatch(detail, /<h3>Backup result<\/h3>.*renderBackupOutcome\(run\)/s);
  assert.doesNotMatch(app, /function renderBackupSpoolingCue\(/);
  assert.doesNotMatch(app, /function backupProgressPercent\(/);
  assert.match(detail, /backupRunActive\(run\) && !pick\(run, "cancel_requested_at"\)/, "a retained cancellation request must hide the duplicate action");
  assert.match(css, /\.backup-current-file/);
  assert.match(css, /\.backup-current-file code\s*\{[^}]*overflow-wrap:\s*anywhere;[^}]*white-space:\s*normal;/s);
  assert.doesNotMatch(css, /\.backup-current-file code\s*\{[^}]*white-space:\s*nowrap;/s);
  assert.match(css, /\.backup-current-operation\.cancellation-requested/);
  assert.match(css, /\.backup-current-operation\.activity-waiting/);
});

test("transfer progress uses the current phase's cumulative counters without inventing completion", () => {
  const helpers = section("  function backupTransferPhase(", "  function backupDurationLabel(");
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const objectValue = (object, key) => object && object[key] && typeof object[key] === "object" && !Array.isArray(object[key]) ? object[key] : {};
  const evidenceValue = (run, ...keys) => {
    for (const source of [run, run && run.result, run && run.manifest]) {
      const value = testPick(source, ...keys);
      if (value !== null) return value;
    }
    return null;
  };
  const progress = Function(
    "pick", "objectValue", "backupPreparationProgress", "backupMediaRequestPhase", "backupRunState", "backupNeedsNextMedia", "backupEvidenceValue", "BACKUP_RECORD_PROGRESS_BASES", "backupRunActive",
    '"use strict";\n' + helpers + "\nreturn backupTransferProgress;"
  )(
    testPick,
    objectValue,
    (run) => objectValue(objectValue(run, "result"), "preparation_progress"),
    (run) => String(testPick(objectValue(run, "media_request"), "phase") || "").toLowerCase(),
    (run) => String(testPick(run, "state") || "queued").toLowerCase(),
    (run) => Boolean(run && run.needsNextMedia),
    evidenceValue,
    new Set([
      "bacula_job_files_over_manifest_entries",
      "bacula_job_files_over_backup_job_files",
      "bacula_job_files_over_selected_manifest_entries"
    ]),
    (run) => !["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"].includes(run.state)
  );

  const now = Date.parse("2026-11-14T12:00:00Z");
  const sample = (overrides = {}) => ({
    phase: "backup_write",
    sampled_at: "2026-11-14T11:59:58Z",
    sample_window_ms: 5000,
    stale_after_seconds: 20,
    progress_basis: "bacula_job_files_over_manifest_entries",
    pipeline_bytes_processed: 400,
    files_processed: 4,
    files_total: 10,
    pipeline_bytes_per_second: 100,
    archive_entries_per_second: 2,
    ...overrides
  });

  const fresh = progress({
    state: "writing",
    result: {
      live_io: sample(),
      transfer_progress: sample({ pipeline_bytes_processed: 350, files_processed: 3 })
    },
    manifest: { logical_bytes: 800, file_count: 8 }
  }, now);
  assert.equal(fresh.phase, "backup_write");
  assert.equal(fresh.processedBytes, 400, "fresh live I/O must beat an older retained sample");
  assert.equal(fresh.totalBytes, null, "pipeline bytes are never treated as a completion denominator");
  assert.equal(fresh.processedFiles, 4);
  assert.equal(fresh.totalFiles, 10);
  assert.equal(fresh.percent, 40);
  assert.equal(fresh.percentBasis, "records");
  assert.equal(fresh.rate, 100);
  assert.equal(fresh.entryRate, 2);
  assert.equal(fresh.etaSeconds, null, "uneven archive entries must not produce a countdown");
  assert.equal(fresh.sampleFresh, true);
  assert.equal(fresh.paused, false);

  const paused = progress({
    state: "waiting_for_media",
    media_request: { phase: "backup_write", volume_order: 2 },
    result: {
      live_io: sample({ pipeline_bytes_processed: 650, files_processed: 7 }),
      transfer_progress: sample({ pipeline_bytes_processed: 600, files_processed: 6 })
    }
  }, now);
  assert.equal(paused.paused, true);
  assert.equal(paused.processedBytes, 600, "media waits must retain the durable cumulative position");
  assert.equal(paused.percent, 60);
  assert.equal(paused.sampleFresh, false);
  assert.equal(paused.rate, null, "media waits must never present a live rate");
  assert.equal(paused.etaSeconds, null, "media waits must never present a countdown");

  const stale = progress({
    state: "writing",
    result: {
      live_io: sample({ sampled_at: "2026-11-14T11:00:00Z", pipeline_bytes_processed: 450, files_processed: 4, pipeline_bytes_per_second: 999999 }),
      transfer_progress: sample({ sampled_at: "2026-11-14T11:59:40Z", pipeline_bytes_processed: 500, files_processed: 5, pipeline_bytes_per_second: 999999 })
    }
  }, now);
  assert.equal(stale.processedBytes, 500);
  assert.equal(stale.sampleFresh, false);
  assert.equal(stale.stale, false, "a retained cumulative position is not mislabeled as a live stale rate");
  assert.equal(stale.rate, null);
  assert.equal(stale.etaSeconds, null);

  const readback = progress({
    state: "verifying",
    result: {
      live_io: sample({ phase: "full_readback", progress_basis: "bacula_job_files_over_backup_job_files", pipeline_bytes_processed: 100, files_processed: 1 }),
      transfer_progress: sample({ phase: "backup_write", pipeline_bytes_processed: 1000, files_processed: 10 })
    }
  }, now);
  assert.equal(readback.phase, "full_readback");
  assert.equal(readback.processedBytes, 100, "write counters must not leak across the readback reset");
  assert.equal(readback.percent, 10);

  const inventory = progress({
    state: "inventorying",
    result: { preparation_progress: sample({ phase: undefined, stage: "source_inventory", bytes_hashed: 250, files_hashed: 3, logical_bytes_discovered: 1000, hash_bytes_per_second: 50 }) },
    manifest: { logical_bytes: 1000, file_count: 10 }
  }, now);
  assert.equal(inventory.phase, "source_inventory");
  assert.equal(inventory.processedBytes, 250);
  assert.equal(inventory.totalBytes, null, "the growing inventory total is not a completion denominator");
  assert.equal(inventory.percent, null);
  assert.equal(inventory.etaSeconds, null);

  const stability = progress({
    state: "verifying",
    result: { preparation_progress: sample({ phase: undefined, stage: "source_stability_check", bytes_hashed: 250, files_hashed: 3, hash_bytes_per_second: 50 }) },
    manifest: { logical_bytes: 1000, file_count: 10 }
  }, now);
  assert.equal(stability.phase, "source_stability_check");
  assert.equal(stability.percent, 25);
  assert.equal(stability.rate, 50);
  assert.equal(stability.etaSeconds, 15);

  const unknown = progress({ state: "writing", result: { live_io: sample({ progress_basis: "pipeline_bytes_only" }) } }, now);
  assert.equal(unknown.totalBytes, null);
  assert.equal(unknown.percent, null);
  assert.equal(unknown.etaSeconds, null);
  assert.equal(unknown.processedBytes, 400, "pipeline bytes remain useful as a stream position");

  const exceeded = progress({ state: "writing", result: { live_io: sample({ files_processed: 12 }) } }, now);
  assert.equal(exceeded.rawPercent, 100, "counter overshoot must be clamped");
  assert.equal(exceeded.percent, 99, "active phase finalization must not look complete");
  assert.equal(exceeded.phaseComplete, true);
  assert.equal(exceeded.etaSeconds, null);

  const zero = progress({ state: "writing", result: { live_io: sample({ pipeline_bytes_processed: 0, files_processed: 0, files_total: 0 }) } }, now);
  assert.equal(zero.rawPercent, 100, "a manifest-proven empty source is explicitly complete");
  assert.equal(zero.percent, 99, "the active phase still waits for finalization");
  assert.equal(zero.etaSeconds, null);

  const inconsistentZero = progress({ state: "writing", result: { live_io: sample({ pipeline_bytes_processed: 10, files_processed: 1, files_total: 0 }) } }, now);
  assert.equal(inconsistentZero.percent, null, "a nonzero counter cannot be presented as complete against an empty entry set");
  assert.equal(inconsistentZero.etaSeconds, null);

  const failed = progress({ state: "failed", result: { transfer_progress: sample({ pipeline_bytes_processed: 300, files_processed: 3 }) } }, now);
  assert.equal(failed.percent, 30, "failure preserves its last confirmed phase position rather than becoming complete");

  const terminalComplete = progress({ state: "failed", result: { transfer_progress: sample({ files_processed: 10 }) } }, now);
  assert.equal(terminalComplete.percent, 100, "a terminal run may show the complete retained phase position");

  for (const progressBasis of [
    "bacula_job_files_over_manifest_entries",
    "bacula_job_files_over_backup_job_files",
    "bacula_job_files_over_selected_manifest_entries"
  ]) {
    const recognized = progress({ state: "writing", result: { live_io: sample({ progress_basis: progressBasis }) } }, now);
    assert.equal(recognized.percentBasis, "records", `${progressBasis} must use its record denominator`);
  }
});

test("the operation hero renders phase-specific progress, rates, and tape actions", () => {
  const helpers = section("  function backupApproximateDuration(", "  function backupPhaseIndex(");
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const objectValue = (object, key) => object && object[key] && typeof object[key] === "object" && !Array.isArray(object[key]) ? object[key] : {};
  const [operation, render] = Function(
    "pick", "objectValue", "backupRunState", "backupTransferProgress", "BACKUP_TERMINAL_STATES", "backupMediaRequestState", "backupDataDelivery", "backupTransferPhase", "backupLiveActivity", "backupNativeEngineState", "backupPhaseIndex", "backupRunStateLabel", "backupNeedsNextMedia", "backupMediaIsReadback", "backupNextVolumeSequence", "recordId", "esc", "backupRunVolumes", "bytes", "backupDurationLabel", "date", "BACKUP_PHASES",
    '"use strict";\n' + helpers + "\nreturn [backupCurrentOperation, renderBackupCurrentOperation];"
  )(
    testPick,
    objectValue,
    (run) => run.state,
    (run) => run._progress,
    new Set(["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"]),
    (run) => String(testPick(objectValue(run, "media_request"), "state") || "").toLowerCase(),
    (run) => run.data_delivery || "disk_spool",
    (value) => ({ tape_write: "backup_write", writing: "backup_write", tape_read: "full_readback", verifying: "full_readback" })[String(value || "")] || String(value || ""),
    (liveIo) => liveIo.activity || "starting",
    (liveIo) => String(liveIo && liveIo.engine_state || "").toLowerCase(),
    (run) => Number.isInteger(run.step) ? run.step : 3,
    (run) => ({ writing: "Writing tape", verifying: "Reading tape back", failed: "Failed", queued: "Waiting to start" })[run.state] || run.state,
    (run) => Boolean(run.needsNextMedia),
    (run) => testPick(objectValue(run, "media_request"), "phase") === "full_readback",
    (run) => Number(testPick(objectValue(run, "media_request"), "volume_order") || 1),
    (run) => run.id || "run-1",
    (value) => String(value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;"),
    (run) => run.volumes || [],
    (value) => Number(value).toLocaleString() + " B",
    (milliseconds) => Math.floor(Number(milliseconds) / 1000) + "s",
    String,
    Array.from({ length: 8 }, (_, index) => ({ name: "Phase " + (index + 1) }))
  );

  const progress = (overrides = {}) => ({
    phase: "backup_write",
    processedBytes: 400,
    totalBytes: null,
    processedFiles: 4,
    totalFiles: 10,
    percent: 40,
    rawPercent: 40,
    percentBasis: "records",
    progressBasis: "bacula_job_files_over_manifest_entries",
    phaseComplete: false,
    sampleFresh: true,
    stale: false,
    paused: false,
    rate: 100,
    entryRate: 2,
    etaSeconds: null,
    sampledAt: "2026-11-14T12:00:00Z",
    sample: {},
    liveIo: {
      phase: "backup_write",
      activity: "streaming",
      current_file: 'Radio Stuff/session <final>&".wav',
      file_daemon_read_bytes_per_second: 80,
      tape_write_counter_available: true,
      tape_write_bytes_per_second: 200,
      tape_read_counter_available: true,
      tape_read_bytes_per_second: 300
    },
    preparation: {},
    ...overrides
  });

  const directRun = {
    id: "run-1",
    state: "writing",
    data_delivery: "direct",
    expected_cartridge_label: "AA0315",
    volumes: [{ sequence: 1, label: "AA0315" }],
    _progress: progress()
  };
  const direct = render(directRun);
  assert.match(direct, /Step 4 of 8/);
  assert.match(direct, /Streaming an archive pack to tape/);
  assert.match(direct, /Archive entries reached/);
  assert.match(direct, /4 of 10 entries/);
  assert.match(direct, /400 B in the current archive stage/);
  assert.match(direct, /40%/);
  assert.match(direct, /Archive write stream/);
  assert.match(direct, /100 B\/s/);
  assert.match(direct, /Source read/);
  assert.match(direct, /80 B\/s/);
  assert.match(direct, /Tape write/);
  assert.match(direct, /200 B\/s/);
  assert.match(direct, /Archive entry rate/);
  assert.match(direct, /2 entries\/s/);
  assert.doesNotMatch(direct, /Tape read/);
  assert.doesNotMatch(direct, /Estimated remaining/);
  assert.match(direct, /Current file/);
  assert.match(direct, /Radio Stuff\/session &lt;final&gt;&amp;&quot;\.wav/);
  assert.doesNotMatch(direct, /<final>/, "the source-relative filename must be HTML escaped");
  assert.match(direct, /not retained after this phase/);
  assert.match(direct, /Legacy direct streaming record/);
  assert.match(direct, /New TapeScale Native backups use bounded, independently validated disk packs/);

  const readbackRun = {
    ...directRun,
    state: "verifying",
    step: 5,
    _progress: progress({
      phase: "full_readback",
      processedBytes: 100,
      processedFiles: 1,
      percent: 10,
      rawPercent: 10,
      progressBasis: "bacula_job_files_over_backup_job_files",
      liveIo: { phase: "full_readback", activity: "reading", tape_read_counter_available: true, tape_read_bytes_per_second: 175 }
    })
  };
  const readback = render(readbackRun);
  assert.match(readback, /Step 6 of 8/);
  assert.match(readback, /Reading archive back/);
  assert.match(readback, /Archive entries read back/);
  assert.match(readback, /1 of 10 entries/);
  assert.match(readback, /Tape read/);
  assert.match(readback, /175 B\/s/);
  assert.doesNotMatch(readback, /Tape write/);

  const stagedRun = {
    ...directRun,
    data_delivery: "disk_spool",
    _progress: progress({
      liveIo: {
        phase: "backup_write",
        activity: "buffering",
        engine_state: "packing",
        tape_write_counter_available: true,
        tape_write_bytes_per_second: 0,
        spool_bytes: 250,
        spool_target_bytes: 1000,
        spool_capacity_bytes: 1200
      }
    })
  };
  const staged = render(stagedRun);
  assert.match(staged, /Packing the rolling archive buffer/);
  assert.match(staged, /Rolling spool/);
  assert.match(staged, /250 B \/ 1,000 B/);
  assert.match(staged, /25% of high-water mark · hard limit 1,200 B/);
  assert.match(staged, /Concurrent bounded pack producer/);
  assert.doesNotMatch(staged, /Tape read/);

  const legacyStaged = render({
    ...stagedRun,
    _progress: progress({ liveIo: { phase: "backup_write", activity: "buffering", spool_bytes: 250, spool_capacity_bytes: 1000 } })
  });
  assert.match(legacyStaged, /25% of reported spool capacity \(legacy sample\)/);

  const pausedRun = {
    ...directRun,
    state: "waiting_for_media",
    needsNextMedia: true,
    media_request: { phase: "backup_write", state: "pending", volume_order: 2 },
    allowed_actions: { assign_next_media: true },
    _progress: progress({ processedBytes: 600, processedFiles: 6, percent: 60, rawPercent: 60, paused: true, sampleFresh: false, rate: 999, entryRate: null, etaSeconds: null, liveIo: {} })
  };
  const paused = render(pausedRun);
  assert.match(paused, /Load tape 2 to continue/);
  assert.match(paused, /6 of 10 entries/);
  assert.match(paused, /600 B in the current archive stage/);
  assert.match(paused, /60%/);
  assert.match(paused, /Paused at the last confirmed position/);
  assert.match(paused, /Load next tape/);
  assert.doesNotMatch(paused, /999 B\/s|Estimated remaining/);

  const stale = render({
    ...directRun,
    _progress: progress({ processedBytes: 500, processedFiles: 5, percent: 50, rawPercent: 50, sampleFresh: false, stale: true, rate: null, entryRate: null, etaSeconds: null, liveIo: {} })
  });
  assert.match(stale, /Update delayed/);
  assert.match(stale, /5 of 10 entries/);
  assert.doesNotMatch(stale, /Estimated remaining|100 B\/s|200 B\/s/);

  const inventory = render({
    ...directRun,
    state: "inventorying",
    step: 2,
    _progress: progress({
      phase: "source_inventory",
      processedBytes: 250,
      totalBytes: null,
      processedFiles: 3,
      totalFiles: null,
      percent: null,
      rawPercent: null,
      percentBasis: null,
      rate: 50,
      entryRate: null,
      etaSeconds: null,
      preparation: { files_discovered: 5, logical_bytes_discovered: 700, elapsed_ms: 5000 }
    })
  });
  assert.match(inventory, /Scanning and hashing source/);
  assert.match(inventory, /Total grows while scanning/);
  assert.match(inventory, /Bytes hashed/);
  assert.doesNotMatch(inventory, /Tape write|Tape read|aria-valuenow/);

  const finalCheck = render({
    ...readbackRun,
    step: 6,
    _progress: progress({ phase: "source_stability_check", processedBytes: 250, totalBytes: 1000, percent: 25, rawPercent: 25, percentBasis: "bytes", progressBasis: "sha256_source_bytes_over_manifest_logical_bytes", rate: 50, entryRate: null, etaSeconds: 15, liveIo: {}, preparation: { stage: "source_stability_check" } })
  });
  assert.match(finalCheck, /Confirming source snapshot/);
  assert.match(finalCheck, /Source snapshot checked/);
  assert.match(finalCheck, /Source check/);
  assert.doesNotMatch(finalCheck, /<span>Tape write<\/span>|<span>Tape read<\/span>/);

  const unknownTotal = render({ ...directRun, _progress: progress({ totalBytes: null, totalFiles: null, percent: null, rawPercent: null, percentBasis: null, progressBasis: "pipeline_bytes_only", entryRate: null, etaSeconds: null }) });
  assert.match(unknownTotal, /Engine stream position · not archive completion/);
  assert.doesNotMatch(unknownTotal, /aria-valuenow/);

  const empty = render({ ...directRun, _progress: progress({ processedBytes: 0, processedFiles: 0, totalFiles: 0, percent: 99, rawPercent: 100, phaseComplete: true, entryRate: 0, etaSeconds: null }) });
  assert.match(empty, /Empty source confirmed/);
  assert.match(empty, /Phase finalization is still in progress/);

  const inconsistentZero = render({ ...directRun, _progress: progress({ processedBytes: 10, processedFiles: 1, totalFiles: 0, percent: null, rawPercent: null, entryRate: null, etaSeconds: null }) });
  assert.match(inconsistentZero, /10 B/);
  assert.match(inconsistentZero, /Engine stream position · not archive completion/);
  assert.doesNotMatch(inconsistentZero, /100%|Empty source confirmed/);

  const activeComplete = render({ ...directRun, _progress: progress({ processedFiles: 10, percent: 99, rawPercent: 100, phaseComplete: true }) });
  assert.match(activeComplete, /aria-valuenow="99\.0"/);
  assert.match(activeComplete, /All entries reached · phase finalization pending/);
  assert.doesNotMatch(activeComplete, />100%<|aria-valuenow="100/);

  const overshoot = render({ ...directRun, _progress: progress({ processedFiles: 12, percent: 99, rawPercent: 100, phaseComplete: true }) });
  assert.match(overshoot, /10 of 10 entries/);
  assert.doesNotMatch(overshoot, /12 of 10 entries/);

  const waitingForFirstSample = render({
    ...directRun,
    state: "waiting_for_media",
    media_request: { phase: "backup_write", state: "pending", volume_order: 1 },
    _progress: progress({ processedBytes: null, processedFiles: null, totalFiles: null, percent: null, rawPercent: null, percentBasis: null, sampleFresh: false, paused: true, rate: null, entryRate: null, sampledAt: null, liveIo: {} })
  });
  assert.match(waitingForFirstSample, /No transfer is active until the requested tape is confirmed/);
  assert.doesNotMatch(waitingForFirstSample, /backup-operation-progress|backup-operation-position|B\/s|entries\/s/);

  const cancellation = render({ ...directRun, cancel_requested_at: "2026-11-14T12:01:00Z", allowed_actions: { cancel: true } });
  assert.match(cancellation, /Cancellation requested/);
  assert.match(cancellation, /Stopping safely/);
  assert.match(cancellation, /cancellation-requested/);
  assert.doesNotMatch(cancellation, /backup-operation-action/);

  const legacyIdle = render({
    ...directRun,
    _progress: progress({ liveIo: { phase: "backup_write", activity: "idle" } })
  });
  assert.match(legacyIdle, /No data movement/);
  assert.doesNotMatch(legacyIdle, /safety threshold|Automatic cancellation|for 0s/);

  const failedRun = { ...directRun, state: "failed", _progress: progress({ processedBytes: 300, processedFiles: 3, percent: 30, rawPercent: 30, sampleFresh: false, rate: null, entryRate: null, etaSeconds: null, liveIo: {} }) };
  const failed = render(failedRun);
  assert.match(failed, /Last confirmed progress/);
  assert.match(failed, /3 of 10 entries/);
  assert.match(failed, /30%/);
  assert.doesNotMatch(failed, /Estimated remaining/);

  const failedBeforeTransfer = render({ ...directRun, state: "failed", _progress: progress({ processedBytes: null, totalBytes: null, processedFiles: null, totalFiles: null, percent: null, rawPercent: null, percentBasis: null, sampleFresh: false, rate: null, entryRate: null, etaSeconds: null, liveIo: {} }) });
  assert.match(failedBeforeTransfer, /No transfer progress was recorded before this attempt stopped/);
  assert.doesNotMatch(failedBeforeTransfer, /Last confirmed progress/);

  assert.equal(render({ ...directRun, state: "verified" }), "", "terminal success uses the verified outcome instead of a live operation hero");
  assert.equal(operation(pausedRun).action.heading, "Load tape 2 to continue");
});

test("shared preparation and engine telemetry remain honest about stale or missing samples", () => {
  const helpers = section("  function backupLiveIo(", "  function backupApproximateDuration(");
  const values = Function(
    "objectValue", "pick", "backupRunState", "BACKUP_TERMINAL_STATES", "human", "date", "bytes", "esc", "backupDataDelivery",
    '"use strict";\n' + helpers + "\nreturn [backupLiveIoStale, renderBackupPreparationProgress, renderBackupLiveThroughput];"
  )(
    (object, key) => object && object[key] && typeof object[key] === "object" && !Array.isArray(object[key]) ? object[key] : {},
    (object, ...keys) => {
      if (!object || typeof object !== "object") return null;
      for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
      return null;
    },
    (run) => run.state,
    new Set(["verified", "failed", "blocked", "interrupted", "cancelled"]),
    (value) => String(value).replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase()),
    (value) => String(value),
    (value) => {
      const amount = Number(value);
      if (amount === 0) return "0 B";
      return (amount / 1048576).toFixed(1) + " MiB";
    },
    (value) => String(value),
    (run) => run && run.data_delivery || "disk_spool"
  );
  const [stale, renderPreparation, render] = values;
  const now = Date.now();
  const freshSample = {
    sampled_at: new Date(now).toISOString(),
    sample_window_ms: 5000,
    phase: "tape_write",
    activity: "buffering",
    engine_state: "",
    progress_idle_ms: 0,
    no_progress_timeout_ms: 1800000,
    pipeline_bytes_per_second: 1048576,
    tape_write_bytes_per_second: 0,
    tape_read_bytes_per_second: null,
    tape_read_counter_available: true,
    tape_write_counter_available: true,
    tape_counters_available: true,
    stale_after_seconds: 20,
    pipeline_basis: "native_dar_archive_bytes_delta",
    tape_basis: "linux_scsi_tape_sysfs_counter_delta",
    spool_bytes: 16777216,
    spool_high_water_bytes: 67108864,
    spool_low_water_bytes: 8388608,
    spool_capacity_bytes: 83886080,
    spool_pack_count: 2,
    spool_reclaimed_bytes: 134217728,
    spool_fill_bytes_per_second: 2097152,
    spool_drain_bytes_per_second: 0,
    spool_basis: "native_run_scoped_archive_slice_bytes"
  };

  assert.equal(stale(freshSample, now), false);
  const live = render({ state: "writing", result: { live_io: freshSample } });
  assert.match(live, /Filling the bounded archive buffer/);
  assert.match(live, /Buffering/);
  assert.match(live, /Rolling spool/);
  assert.match(live, /16\.0 MiB \/ 64\.0 MiB/);
  assert.match(live, /25\.0% of high-water mark/);
  assert.match(live, /refill at\/below 8\.0 MiB/);
  assert.match(live, /128\.0 MiB reclaimed/);
  assert.match(live, /hard limit 80\.0 MiB/);
  assert.match(live, /Filling<\/span><strong>2\.0 MiB\/s/);
  assert.match(live, /DAR is filling a bounded disk window with independently validated archive packs/);
  assert.match(live, /Archive write stream/);
  assert.match(live, /Engine stream bytes · not source logical bytes/);
  assert.match(live, /1\.0 MiB\/s/);
  assert.match(live, /Tape write/);
  assert.match(live, /0 B\/s/);
  assert.doesNotMatch(live, /Tape read/);
  assert.match(live, /Rolling 5 second sample/);
  assert.match(live, /Engine progress: Native Dar Archive Bytes Delta/);
  assert.match(live, /Rolling spool: Native Run Scoped Archive Slice Bytes/);
  assert.match(live, /Linux Scsi Tape Sysfs Counter Delta/);
  assert.doesNotMatch(live, /Bacula/);

  const packing = render({ state: "writing", result: { live_io: { ...freshSample, engine_state: "packing", activity: "buffering" } } });
  assert.match(packing, /Packing the archive buffer/);
  assert.match(packing, /DAR pack production/);
  assert.match(packing, /Tape<\/span><strong>Idle/);
  assert.match(packing, /validated reserve reaches its resume threshold/i);

  const refilling = render({ state: "writing", result: { live_io: { ...freshSample, engine_state: "refilling_spool", activity: "refilling" } } });
  assert.match(refilling, /Refilling the archive buffer/);
  assert.match(refilling, /safe tape-file boundary/);

  const packStreaming = render({ state: "writing", result: { live_io: { ...freshSample, engine_state: "writing_native_pack", activity: "streaming", spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 4194304, tape_write_bytes_per_second: 4194304 } } });
  assert.match(packStreaming, /Streaming an archive pack to tape/);
  assert.match(packStreaming, /Native pack stream/);
  assert.match(packStreaming, /reclaimed only after the complete tape files and filemarks close/);

  const firstSpoolSample = { ...freshSample, activity: "starting", sample_window_ms: null };
  delete firstSpoolSample.spool_fill_bytes_per_second;
  delete firstSpoolSample.spool_drain_bytes_per_second;
  const firstSpool = render({ state: "writing", result: { live_io: firstSpoolSample } });
  assert.match(firstSpool, /Net movement<\/span><strong>Measuring…/);
  assert.doesNotMatch(firstSpool, /Net movement<\/span><strong>0 B\/s/);

  const streaming = render({ state: "writing", result: { live_io: { ...freshSample, engine_state: "writing_native_volume", activity: "streaming", spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 4194304, tape_write_bytes_per_second: 4194304 } } });
  assert.match(streaming, /Streaming a native volume to tape/);
  assert.match(streaming, /Tape active/);
  assert.match(streaming, /Draining<\/span><strong>4\.0 MiB\/s/);
  assert.match(streaming, /staged TapeScale Native volume is streaming sequentially/);

  const despooling = render({ state: "writing", result: { live_io: { ...freshSample, engine_state: "writing_native_volume", activity: "despooling", spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 3145728, tape_write_counter_available: false, tape_write_bytes_per_second: null } } });
  assert.match(despooling, /Streaming a native volume to tape/);
  assert.match(despooling, /Native pack stream/);
  assert.match(despooling, /Tape write<\/span><strong>Unavailable/);

  const idle = render({ state: "writing", result: { live_io: { ...freshSample, activity: "idle", progress_idle_ms: 300000, pipeline_bytes_per_second: 0, spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 0 } } });
  assert.match(idle, /No data movement/);
  assert.match(idle, /worker is responding/i);
  assert.match(idle, /no measured disk-buffer, engine, or tape bytes have moved for 5m 0s/i);
  assert.match(idle, /Automatic cancellation in 25m 0s/);
  assert.match(idle, /heartbeat is not counted as progress/);

  const stalled = render({ state: "writing", result: { live_io: { ...freshSample, activity: "stalled", progress_idle_ms: 1800000, pipeline_bytes_per_second: 0, spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 0 } } });
  assert.match(stalled, /activity-stalled/);
  assert.match(stalled, /Transfer stalled/);
  assert.match(stalled, /Action required/);
  assert.match(stalled, /no-progress safety threshold has been reached/i);

  const legacyIdle = render({ state: "writing", result: { live_io: { sampled_at: new Date(now).toISOString(), sample_window_ms: 5000, phase: "backup_write", pipeline_bytes_per_second: 0, tape_write_bytes_per_second: 0, tape_read_bytes_per_second: 0, tape_counters_available: true, stale_after_seconds: 20 } } });
  assert.match(legacyIdle, /No data movement/);
  assert.doesNotMatch(legacyIdle, /Rolling spool<\/span>/);

  const unavailable = render({ state: "writing", result: { live_io: { ...freshSample, tape_read_counter_available: false, tape_write_counter_available: false, tape_counters_available: false } } });
  assert.equal((unavailable.match(/Unavailable/g) || []).length, 1);

  const partial = render({ state: "writing", result: { live_io: { ...freshSample, tape_read_counter_available: false } } });
  assert.match(partial, /Tape write<\/span><strong>0 B\/s/);
  assert.doesNotMatch(partial, /Tape read/);

  const verifying = render({ state: "verifying", result: { live_io: { ...freshSample, engine_state: "reading_native_volume", phase: "full_readback", activity: "reading", tape_read_bytes_per_second: 1048576 } } });
  assert.match(verifying, /Reading the volume back now/);
  assert.match(verifying, /Physical readback/);
  assert.match(verifying, /Immediate readback of the just-written physical volume/);

  const restoring = render({ state: "restoring", result: { live_io: { ...freshSample, phase: "restore", activity: "restoring", tape_read_bytes_per_second: 1048576 } } });
  assert.match(restoring, /Archive read stream/);
  assert.match(restoring, /Legacy recovered data is moving through the restore stream/);

  const oldSample = { ...freshSample, sampled_at: "2000-01-01T00:00:00Z", pipeline_bytes_per_second: 999999999 };
  assert.equal(stale(oldSample, now), true);
  const delayed = render({ state: "writing", result: { live_io: oldSample } });
  assert.match(delayed, /Transfer sample is stale/);
  assert.match(delayed, /Rates are hidden/);
  assert.doesNotMatch(delayed, /953\.7 MiB\/s/);

  assert.equal(render({ state: "inventorying", result: {} }), "");
  assert.match(render({ state: "waiting_for_media", result: { live_io: freshSample } }), /Waiting for tape/);
  assert.doesNotMatch(render({ state: "waiting_for_media", result: { live_io: freshSample } }), /1\.0 MiB\/s/);
  assert.equal(render({ state: "verified", result: { live_io: freshSample } }), "");

  const preparationSample = {
    stage: "source_inventory",
    step: "discovering_and_hashing",
    started_at: new Date(now - 65000).toISOString(),
    sampled_at: new Date(now).toISOString(),
    elapsed_ms: 65000,
    sample_window_ms: 5000,
    stale_after_seconds: 20,
    entries_discovered: 42,
    files_discovered: 40,
    files_hashed: 25,
    logical_bytes_discovered: 4194304,
    bytes_hashed: 2097152,
    hash_bytes_per_second: 1048576,
    basis: "sha256_source_bytes_read"
  };
  const preparation = renderPreparation({ state: "inventorying", result: { preparation_progress: preparationSample } });
  assert.match(preparation, /Scanning and hashing the source/);
  assert.match(preparation, /SHA-256 bytes read/);
  assert.match(preparation, /2\.0 MiB/);
  assert.match(preparation, /1\.0 MiB\/s/);
  assert.match(preparation, /25/);
  assert.match(preparation, /40 discovered/);
  assert.match(preparation, /1m 5s/);
  assert.match(preparation, /Tape writing has not started/);
  assert.match(preparation, /no completion percentage is estimated/);
  assert.doesNotMatch(preparation, /style="width:/);

  const firstPreparation = renderPreparation({ state: "inventorying", result: { preparation_progress: { ...preparationSample, hash_bytes_per_second: null } } });
  assert.match(firstPreparation, /Measuring rate…/);
  assert.doesNotMatch(firstPreparation, /0 B\/s/);

  const stalePreparation = renderPreparation({ state: "inventorying", result: { preparation_progress: { ...preparationSample, sampled_at: "2000-01-01T00:00:00Z", hash_bytes_per_second: 999999999 } } });
  assert.match(stalePreparation, /Update delayed/);
  assert.match(stalePreparation, /2\.0 MiB/);
  assert.match(stalePreparation, /Rate hidden until next update/);
  assert.doesNotMatch(stalePreparation, /953\.7 MiB\/s/);

  assert.match(renderPreparation({ state: "preparing", result: {} }), /Checking the immutable snapshot, native catalog, drive, tape, and bounded-spool watermarks/);
  assert.match(renderPreparation({ state: "inventorying", result: {} }), /Waiting for the worker's first source-scan sample/);
  const stability = renderPreparation({ state: "verifying", result: { preparation_progress: { ...preparationSample, stage: "source_stability_check", step: "sorting" } } });
  assert.match(stability, /Sorting manifest entries/);
  assert.match(stability, /Tape readback is complete/);
  assert.match(render({ state: "writing", result: { live_io: freshSample, preparation_progress: preparationSample } }), /Filling the bounded archive buffer/, "a retained inventory sample must not hide active write telemetry");

  const direct = render({ state: "writing", data_delivery: "direct", result: { live_io: { ...freshSample, activity: "streaming", tape_write_bytes_per_second: 4194304 } } });
  assert.match(direct, /Streaming directly to tape/);
  assert.doesNotMatch(direct, /Rolling spool|bounded archive buffer|staged archive/);
  const directStarting = render({ state: "writing", data_delivery: "direct", result: { live_io: { ...freshSample, activity: "buffering" } } });
  assert.match(directStarting, /Starting the tape stream/);
  assert.match(directStarting, /legacy direct source-to-tape stream is starting/);
  assert.doesNotMatch(directStarting, /staged archive/);

  const evidence = section("  function renderBackupEvidence(", "  function renderBackupOutcome(");
  assert.match(evidence, /\["Recorded average", throughput == null \? null/);
  assert.doesNotMatch(evidence, /\["Throughput",/);
  assert.match(css, /\.backup-live-rate-grid/);
  assert.match(css, /\.backup-preparation-grid/);
  assert.match(css, /\.backup-live-io\.stale/);
  assert.match(css, /\.backup-live-io\.activity-stalled/);
  assert.match(css, /\.backup-live-io\.activity-idle/);
  assert.match(app, /outcome \+ liveTransfer \+ renderRestoreVolumeSequence/);
  assert.match(app, /outcome \+ renderBackupPreparationProgress\(\{ state: stateValue/);
});

test("one media action synthesizes the legacy authorization only after fresh readiness", () => {
  const open = section("  async function openAssignBackupMedia(", "  function updateBackupMediaForDrive(");
  const submit = section("  async function submitBackupMedia(", "  function prepareBackupTape(");

  assert.doesNotMatch(open, /backup_authority_confirmed/);
  assert.doesNotMatch(open, /name="confirmation"/);
  assert.match(open, /const fixedDrive = readyDrives\.length === 1/);
  assert.match(open, /const fixedMedia = selectedMedia/);
  assert.match(open, /name="drive_id" required hidden/);
  assert.match(open, /name="cartridge_id" required hidden/);
  assert.match(open, /The destructive button below is the confirmation/);

  assert.match(submit, /await loadBackupMediaReadiness\("run", runId\)/);
  assert.match(submit, /required_confirmation/);
  assert.match(submit, /typed_cartridge_label: exactLabel/);
  assert.match(submit, /volume_sequence: volumeSequence/);
  assert.match(submit, /loaded_media_confirmed: true/);
  assert.match(submit, /destructive_overwrite_confirmed: !readbackMedia/);
  assert.match(submit, /encryption_deviation_confirmed: !readbackMedia/);
  assert.match(submit, /confirmation: expectedConfirmation/);
  assert.doesNotMatch(submit, /values\.get\("confirmation"\)/);

  const validation = submit.indexOf('await loadBackupMediaReadiness("run", runId)');
  const currentModal = submit.indexOf("if (!backupModalSubmissionCurrent(form, token)) return;");
  const assignment = submit.indexOf("await request(endpoints.backupRuns");
  assert.ok(validation >= 0 && validation < currentModal && currentModal < assignment, "closing the modal during recheck must stop the destructive assignment");
  assert.match(submit, /if \(continueGuided\) \{[\s\S]*closeModal\(\);[\s\S]*location\.hash = target/);
  assert.match(submit, /Backup started in background/);
});

test("modal cancellation is a real pre-mutation boundary", () => {
  const helperSource = section("  function backupModalSubmissionCurrent(", "  async function submitStartBackup(");
  const compile = Function("state", "modalLayer", '"use strict";\n' + helperSource + "\nreturn backupModalSubmissionCurrent;");
  const state = { backupReadinessModalToken: 7 };
  const modalLayer = { hidden: false };
  const current = compile(state, modalLayer);

  assert.equal(current({ isConnected: true }, 7), true);
  assert.equal(current({ isConnected: true }, 6), false);
  assert.equal(current({ isConnected: false }, 7), false);
  modalLayer.hidden = true;
  assert.equal(current({ isConnected: true }, 7), false);

  const start = section("  async function submitStartBackup(", "  function backupMediaConfirmation(");
  const requestCheck = start.indexOf('await loadBackupMediaReadiness("run", state.selectedBackupRunId)');
  const currentCheck = start.indexOf("if (!backupModalSubmissionCurrent(form, token)) return;", requestCheck);
  const assignment = start.indexOf("await request(endpoints.backupRuns", currentCheck);
  assert.ok(requestCheck >= 0 && requestCheck < currentCheck && currentCheck < assignment, "closing the modal during the second readiness check must stop media authorization");
  assert.match(start, /Backup started in background/);
});

test("tape preparation uses one destructive action and automatically reconciles the sole drive", () => {
  const open = section("  function openTapeInitialization(", "  function tapeInitializeLabel(");
  const submit = section("  async function submitTapeInitialization(", "  function qualificationRunDetail(");
  const legacy = section("  function openQualification(", "  function scheduleQualificationPoll(");

  assert.match(open, /safeDrives\.length === 1/);
  assert.match(open, /name="drive_id" required hidden/);
  assert.match(open, /data-tape-init-media-fact/);
  assert.match(open, /The final button is the only destructive confirmation/);
  assert.match(open, /writes a durable internal tape ID/);
  assert.match(open, /replaces the preparation ID with its permanent archive-volume ID automatically/);
  assert.match(open, /Future loads are identified automatically from the tape itself/);
  assert.doesNotMatch(open, /trust_drive_identity/);
  assert.doesNotMatch(open, /name="scratch_media_confirmed"/);
  assert.doesNotMatch(open, /name="destructive_overwrite_confirmed"/);
  assert.doesNotMatch(open, /name="confirmation"/);
  assert.match(submit, /"DESTROY " \+ labelText \+ " WITH TAPESCALE NATIVE TEST"/);
  assert.match(submit, /Reconciling the only loaded drive/);
  assert.match(submit, /scratch_media_confirmed: true/);
  assert.match(submit, /destructive_overwrite_confirmed: true/);
  assert.match(legacy, /openTapeInitialization\(\{ dataset: \{ driveId:/);
  assert.doesNotMatch(legacy, /qualification-form/);
});

test("server blocker actions lead to the correct recovery surface", () => {
  const helperSource = section("  function backupPrimaryReadinessBlocker(", "  function backupReadinessAction(");
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const remediation = Function("pick", '"use strict";\n' + helperSource + "\nreturn backupReadinessRemediation;")(testPick);
  const ready = (code, action) => remediation({ blockers: [{ code, action, message: code }] });

  assert.deepEqual(ready("snapshot_unavailable", "choose_snapshot"), { kind: "choose_snapshot", heading: "Choose an available snapshot", label: "Choose another snapshot" });
  assert.deepEqual(ready("no_media_in_pool", "open_media"), { kind: "media", heading: "Choose an eligible scratch tape", label: "Open tape inventory", href: "#tapes" });
  assert.deepEqual(ready("media_wrong_pool", "open_media"), { kind: "media", heading: "Choose an eligible scratch tape", label: "Open tape inventory", href: "#tapes" });
  assert.deepEqual(ready("setup_test_required", "prepare_tape"), { kind: "prepare", heading: "Prepare a scratch tape", label: "Prepare loaded tape", driveId: "", mediaId: "" });
  assert.deepEqual(ready("drive_inaccessible", "check_device_access"), { kind: "drive", heading: "The tape drive needs attention", label: "Open drive status", href: "#drives" });
  assert.deepEqual(ready("resource_busy", "wait"), { kind: "refresh", heading: "Tape resources are busy", label: "Check again" });
  assert.equal(remediation({ blockers: [
    { code: "setup_test_required", action: "prepare_tape" },
    { code: "snapshot_unavailable", action: "choose_snapshot" }
  ] }).kind, "choose_snapshot", "profile blockers must take precedence over tape preparation");
  assert.equal(remediation({ blockers: [
    { code: "media_wrong_pool", action: "open_media" },
    { code: "setup_test_required", action: "prepare_tape" }
  ] }).kind, "prepare", "an exact pair that only needs setup must beat blockers for unrelated tapes");
});

test("a run URL renders a dedicated progress page", () => {
  const load = section("  async function loadAll(", "  function setBadge(");
  const render = section("  function renderJobs(", "  function mediaName(");
  const routeBranch = render.indexOf("if (routeId)");
  const overviewMetrics = render.indexOf("const active =");

  assert.match(load, /const requestedId = backupRunRouteId\(\)/);
  assert.match(load, /const id = requestedId \|\| recordId\(run\)/);
  assert.match(load, /if \(id\) await loadBackupRunDetail\(id, !initial\)/);
  assert.ok(routeBranch >= 0 && routeBranch < overviewMetrics, "the dedicated route must return before dashboard composition");
  assert.match(render, /href="#jobs">← Back to Backup Center/);
  assert.match(render, /renderBackupRunDetail/);
  assert.match(app, /function backupTransferProgress\(/);
  assert.doesNotMatch(section("  function backupTransferPhase(", "  function backupFailureDetail("), /progress_percent|percent_complete/);
});

test("backup polling preserves disclosure, focus, and scroll state while refreshing detail once", () => {
  const uiStateHelpers = section("  function captureBackupRouteUiState(", "  function scheduleBackupRunPoll(");
  const technicalBefore = { open: true };
  const phasesBefore = { open: true };
  const focusedBefore = { id: "cancel-control", dataset: { action: "cancel-backup-run", id: "run-1" }, matches: () => false, selectionStart: 2, selectionEnd: 6 };
  const technicalAfter = { open: false, querySelector: () => null };
  const phasesAfter = { open: false };
  const focusCalls = [];
  const selectionCalls = [];
  const focusedAfter = { focus: (options) => focusCalls.push(options), setSelectionRange: (...range) => selectionCalls.push(range) };
  let restored = false;
  const main = {
    contains: (element) => element === focusedBefore,
    querySelector: (selector) => restored
      ? selector === ".backup-technical" ? technicalAfter : selector === ".backup-phase-mobile" ? phasesAfter : null
      : selector === ".backup-technical" ? technicalBefore : selector === ".backup-phase-mobile" ? phasesBefore : null,
    querySelectorAll: () => []
  };
  const document = { activeElement: focusedBefore, getElementById: (id) => id === "cancel-control" ? focusedAfter : null };
  const scrollCalls = [];
  const window = { scrollX: 12, scrollY: 345, scrollTo: (...coordinates) => scrollCalls.push(coordinates) };
  const [capture, restore] = Function(
    "state", "backupRunRouteId", "main", "document", "window",
    '"use strict";\n' + uiStateHelpers + "\nreturn [captureBackupRouteUiState, restoreBackupRouteUiState];"
  )({ route: "jobs" }, () => "run-1", main, document, window);

  const snapshot = capture();
  assert.deepEqual(snapshot, {
    technicalOpen: true,
    phasesOpen: true,
    focusId: "cancel-control",
    focusAction: "cancel-backup-run",
    focusRecordId: "run-1",
    focusTechnical: false,
    selectionStart: 2,
    selectionEnd: 6,
    scrollX: 12,
    scrollY: 345
  });
  restored = true;
  restore(snapshot);
  assert.equal(technicalAfter.open, true);
  assert.equal(phasesAfter.open, true);
  assert.deepEqual(focusCalls, [{ preventScroll: true }]);
  assert.deepEqual(selectionCalls, [[2, 6]]);
  assert.deepEqual(scrollCalls, [[12, 345]]);

  const [captureElsewhere] = Function(
    "state", "backupRunRouteId", "main", "document", "window",
    '"use strict";\n' + uiStateHelpers + "\nreturn [captureBackupRouteUiState, restoreBackupRouteUiState];"
  )({ route: "restore" }, () => null, main, document, window);
  assert.equal(captureElsewhere(), null, "unrelated routes are not treated as backup polling surfaces");

  const poll = section("  function scheduleBackupRunPoll(", "  function openAddMedia(");
  assert.match(poll, /const routeUi = captureBackupRouteUiState\(\)/);
  assert.match(poll, /await loadBackupRunDetail\(recordId\(selected\), true, false\)/);
  const captureIndex = poll.indexOf("captureBackupRouteUiState()");
  const renderIndex = poll.indexOf("render();");
  const restoreIndex = poll.indexOf("restoreBackupRouteUiState(routeUi)");
  assert.ok(captureIndex >= 0 && captureIndex < renderIndex && renderIndex < restoreIndex, "route UI state must wrap the poll-driven render");
  assert.match(poll, /if \(\["jobs", "overview"\]\.includes\(state\.route\)\) \{\s*render\(\);/s, "polling must not destroy focus on an unrelated route");
});

test("a late active detail response cannot overwrite a newer terminal list record", async () => {
  const helper = section("  async function loadBackupRunDetail(", "  async function selectBackupRun(");
  const terminal = { id: "run1", state: "failed", result: { transfer_progress: { files_processed: 7 } } };
  const state = {
    route: "jobs",
    errors: {},
    backupRunDetailLoading: new Set(),
    data: { backupRuns: [terminal], backupRunDetails: {} }
  };
  const load = Function(
    "state", "request", "endpoints", "recordId", "rememberBackupRun", "selectedBackupRun", "render",
    '"use strict";\n' + helper + "\nreturn loadBackupRunDetail;"
  )(
    state,
    async () => ({ run: { id: "run1", state: "writing", result: { transfer_progress: { files_processed: 6 } } } }),
    { backupRuns: "/api/v1/backup-runs" },
    (run) => run && run.id || "",
    (run) => { state.data.backupRuns = [run]; },
    () => terminal,
    () => {}
  );

  await load("run1", true, false);
  assert.equal(state.data.backupRuns[0], terminal, "detail caching must not replace the authoritative list snapshot");
  assert.equal(state.data.backupRunDetails.run1.run.state, "writing", "the response remains available for guarded detail merging");
});

test("fresh selected detail wins without resurrecting deleted result fields", () => {
  const helper = section("  function backupRunRecord(", "  function orderedBackupRuns(");
  const state = {
    selectedBackupRunId: "run1",
    errors: {},
    data: {
      backupRunDetails: {
        run1: {
          run: {
            id: "run1",
            state: "verifying",
            result: {
              live_io: { phase: "full_readback", pipeline_bytes_processed: 600 },
              transfer_progress: { phase: "full_readback", pipeline_bytes_processed: 600 }
            }
          }
        }
      }
    }
  };
  const record = Function(
    "state", "recordId", "objectValue", "BACKUP_TERMINAL_STATES", "backupRunState",
    '"use strict";\n' + helper + "\nreturn backupRunRecord;"
  )(
    state,
    (value) => value && value.id || "",
    (object, key) => object && object[key] && typeof object[key] === "object" && !Array.isArray(object[key]) ? object[key] : {},
    new Set(["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"]),
    (run) => run.state
  );
  const merged = record({
    id: "run1",
    state: "writing",
    result: {
      logical_bytes: 1000,
      preparation_progress: { stage: "source_inventory" },
      live_io: { phase: "backup_write", pipeline_bytes_processed: 100 }
    }
  });

  assert.equal(merged.state, "verifying");
  assert.equal(merged.result.logical_bytes, undefined, "the detailed result replaces a stale summary result as one coherent snapshot");
  assert.equal(merged.result.live_io.phase, "full_readback");
  assert.equal(merged.result.preparation_progress, undefined, "keys deliberately absent from detail must stay absent");
  assert.equal(merged.result.live_io.pipeline_bytes_processed, 600);
  assert.equal(merged.result.transfer_progress.pipeline_bytes_processed, 600);

  const terminalList = record({ id: "run1", state: "failed", result: { transfer_progress: { phase: "backup_write", files_processed: 3 } } });
  assert.equal(terminalList.state, "failed", "a terminal list refresh must beat an older active detail");
  assert.equal(terminalList.result.transfer_progress.phase, "backup_write");

  state.errors["backupRun:run1"] = new Error("detail refresh failed");
  const listAfterDetailError = record({ id: "run1", state: "writing", result: { transfer_progress: { files_processed: 5 } } });
  assert.equal(listAfterDetailError.state, "writing");
  assert.equal(listAfterDetailError.result.transfer_progress.files_processed, 5);

  delete state.errors["backupRun:run1"];
  state.selectedBackupRunId = "another-run";
  const unselected = record({ id: "run1", state: "writing", result: { transfer_progress: { files_processed: 7 } } });
  assert.equal(unselected.result.transfer_progress.files_processed, 7, "cached detail must not overwrite a fresh unselected list row");
});

test("backup detail distinguishes the requested time from the actual start time", () => {
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  assert.match(detail, /const requestedAt = pick\(run, "created_at"\)/);
  assert.match(detail, /const startedAt = pick\(run, "started_at"\)/);
  assert.match(detail, /const timing = startedAt \? "Started " \+ date\(startedAt\) : "Requested " \+ date\(requestedAt\)/);
  assert.match(detail, /<dt>Requested<\/dt>/);
  assert.match(detail, /startedAt \? '<dt>Started<\/dt>/);
  assert.doesNotMatch(detail, /Started .*pick\(run, "created_at", "started_at"\)/s);
});

test("backup history labels percentages with their phase", () => {
  const helper = section("  function backupTableProgressLabel(", "  function backupReadbackSummary(");
  const label = Function(
    "BACKUP_TERMINAL_STATES", "backupRunState", "backupTransferProgress", "backupPercentLabel", "backupNativeEngineState", "backupLiveIo",
    '"use strict";\n' + helper + "\nreturn backupTableProgressLabel;"
  )(
    new Set(["verified", "failed", "cancelled"]),
    (run) => run.state,
    (run) => run.progress,
    (value) => Math.round(value) + "%",
    (liveIo) => String(liveIo && liveIo.engine_state || "").toLowerCase(),
    (run) => run.result && run.result.live_io || {}
  );

  assert.equal(label({ state: "writing", progress: { phase: "backup_write", percent: 42.4 } }), "Write 42%");
  assert.equal(label({ state: "verifying", progress: { phase: "full_readback", percent: 17.2 } }), "Readback 17%");
  assert.equal(label({ state: "verifying", progress: { phase: "source_stability_check", percent: 9.8 } }), "Final check 10%");
  assert.equal(label({ state: "writing", progress: { phase: "backup_write", percent: null } }), null);
  assert.equal(label({ state: "failed", progress: { phase: "backup_write", percent: 42.4 } }), null);
});

test("run summaries distinguish planned, active, complete, and failed readback", () => {
  const helperSource = section("  function backupReadbackSummary(", "  function backupFailureDetail(");
  const summary = Function("backupRunState", "BACKUP_TERMINAL_STATES", "pick", "backupPreparationProgress", "backupMediaIsReadback", "backupNativeEngineState", "backupLiveIo", '"use strict";\n' + helperSource + "\nreturn backupReadbackSummary;")(
    (run) => run.state,
    new Set(["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"]),
    (object, key) => object && object[key] !== undefined ? object[key] : null,
    (run) => run.result && run.result.preparation_progress || {},
    (run) => run.media_request && run.media_request.phase === "full_readback",
    (liveIo) => String(liveIo && liveIo.engine_state || "").toLowerCase(),
    (run) => run.result && run.result.live_io || {}
  );

  assert.equal(summary({ state: "waiting_for_media" }), "Complete readback planned");
  assert.equal(summary({ state: "waiting_for_media", media_request: { phase: "full_readback" } }), "Readback tape needed");
  assert.equal(summary({ state: "verifying" }), "Readback in progress");
  assert.equal(summary({ state: "verifying", result: { preparation_progress: { stage: "source_stability_check" } } }), "Readback complete; final source check");
  assert.equal(summary({ state: "verified" }), "Readback complete");
  assert.equal(summary({ state: "failed" }), "Readback not completed");

  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  assert.doesNotMatch(detail, /Full backup · complete readback/);
  assert.match(detail, /backupReadbackSummary\(run\)/);
  assert.match(app, /\{ name: "Confirm first tape", detail: "Choose the first archive volume"/);
  assert.match(app, /\{ name: "Preflight", detail: "Validate source, catalog, drive, tape, and staging path", aliases: \["queued", "media_assigned", "preparing"\] \}/);
  assert.match(app, /\{ name: "Inventory", detail: "Hash files and build the recovery manifest", aliases: \["inventorying", "manifesting", "source_manifest", "source_inventory"\] \}/);
  assert.match(app, /\{ name: "Build archive packs", detail: "Build and test bounded DAR packs", aliases: \[[^\]]*"native_packing"[^\]]*"refilling_spool"/);
  assert.match(app, /\{ name: "Validate pack", detail: "Check each completed pack before tape", aliases: \[[^\]]*"native_validating"[^\]]*"validating_pack"/);
  assert.match(app, /\{ name: "Write & reclaim", detail: "Stream validated packs; reclaim after durable filemarks", aliases: \[[^\]]*"native_volume_write"[^\]]*"native_volume_readback"/);
  assert.match(app, /\{ name: "Finalize", detail: "Seal the pack set and confirm the source", aliases: \[[^\]]*"native_archive_seal"[^\]]*"source_stability_check"/);
});

test("the eight-stage journey is complete on desktop and compact on mobile", () => {
  const phases = section("  const BACKUP_PHASES = [", "  const BACKUP_TERMINAL_STATES");
  const phaseList = Function('"use strict";\n' + phases + "\nreturn BACKUP_PHASES;")();
  const legacyPhases = section("  const LEGACY_BACKUP_PHASES = [", "  const BACKUP_RECORD_PROGRESS_BASES");
  const legacyPhaseList = Function('"use strict";\n' + legacyPhases + "\nreturn LEGACY_BACKUP_PHASES;")();
  const phaseIndexSource = section("  function backupPhaseIndex(", "  function renderBackupPhaseTrack(");
  const phaseIndex = Function(
    "BACKUP_PHASES", "LEGACY_BACKUP_PHASES", "backupUsesLegacyWholeArchive", "backupRunState", "backupPreparationProgress", "pick", "backupMediaIsReadback", "backupNeedsNextMedia", "backupEvidenceValue", "backupRunEvents", "backupTransferPhase", "backupExpectedTransferPhase", "objectValue", "backupNativeEngineState", "backupLiveIo",
    '"use strict";\n' + phaseIndexSource + "\nreturn backupPhaseIndex;"
  )(
    phaseList,
    legacyPhaseList,
    (run) => Boolean(run.legacy),
    (run) => run.state,
    (run) => run.preparation || {},
    (object, ...keys) => {
      if (!object) return null;
      for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
      return null;
    },
    (run) => Boolean(run.readback_media),
    (run) => Boolean(run.next_media),
    (run, ...keys) => {
      for (const key of keys) if (run[key] !== undefined && run[key] !== null && run[key] !== "") return run[key];
      return null;
    },
    (run) => run.events || [],
    (value) => {
      const phase = String(value || "").toLowerCase();
      if (["backup_write", "tape_write", "writing", "spooling"].includes(phase)) return "backup_write";
      if (["full_readback", "tape_read", "tape_readback", "verifying", "reading"].includes(phase)) return "full_readback";
      if (["source_stability_check", "final_source_check"].includes(phase)) return "source_stability_check";
      if (["source_inventory", "inventory", "inventorying"].includes(phase)) return "source_inventory";
      return phase;
    },
    (run) => run.expected_phase || run.result && run.result.transfer_progress && run.result.transfer_progress.phase || "",
    (object, key) => object && object[key] && typeof object[key] === "object" && !Array.isArray(object[key]) ? object[key] : {},
    (liveIo) => String(liveIo && liveIo.engine_state || "").toLowerCase(),
    (run) => run.result && run.result.live_io || {}
  );
  const renderer = section("  function renderBackupPhaseTrack(", "  function backupTableProgressLabel(");
  assert.equal((phases.match(/\{ name:/g) || []).length, 8);
  assert.equal(phaseIndex({ state: "preparing" }), 1);
  assert.equal(phaseIndex({ state: "inventorying" }), 2);
  assert.equal(phaseIndex({ state: "writing" }), 5);
  assert.equal(phaseIndex({ state: "writing", result: { live_io: { engine_state: "packing" } } }), 3);
  assert.equal(phaseIndex({ state: "writing", result: { live_io: { engine_state: "validating" } } }), 4);
  assert.equal(phaseIndex({ state: "written" }), 6);
  assert.equal(phaseIndex({ state: "verifying" }), 5);
  assert.equal(phaseIndex({ state: "verifying", readback_media: true, preparation: { stage: "final_source_check" } }), 6, "the final-check alias supersedes retained readback media state");
  assert.equal(phaseIndex({ state: "failed", result: { transfer_progress: { phase: "full_readback" } } }), 5, "a terminal detail stays on its retained canonical transfer phase");
  assert.equal(phaseIndex({ state: "failed", events: [{ event_type: "engine_dispatched", evidence: { phase: "full_readback" } }] }), 5, "canonical event evidence drives the journey");
  assert.equal(phaseIndex({ state: "failed", events: [{ event_type: "engine_dispatched", evidence: { phase: "source_inventory" } }] }), 2);
  assert.equal(phaseIndex({ state: "writing", phase_index: 0 }), 0, "an explicit phase index remains authoritative");
  assert.equal(phaseIndex({ state: "writing", legacy: true }), 5, "legacy tape write remains the write stage");
  assert.equal(phaseIndex({ state: "verifying", legacy: true }), 6, "legacy full readback uses the separate readback stage");
  assert.equal(phaseIndex({ state: "verifying", legacy: true, result: { live_io: { engine_state: "reading_native_volume" } } }), 6, "legacy native read telemetry stays on readback");
  assert.match(css, /\.phase-track\.backup-phase-track\s*\{[^}]*grid-template-columns:\s*repeat\(8,/s);
  assert.match(renderer, /class="phase-track backup-phase-track backup-phase-desktop"/);
  assert.match(renderer, /<details class="backup-phase-mobile">/);
  assert.match(renderer, /Step .* of .*phaseDefinitions\.length/);
  assert.match(css, /\.backup-phase-mobile\s*\{\s*display:\s*none;/s);
  assert.match(css, /\.backup-phase-desktop\s*\{\s*display:\s*none !important;/s);
  assert.match(css, /\.backup-phase-mobile\s*\{\s*display:\s*block;/s);
  assert.match(css, /\.job-kicker\s*\{[^}]*flex-wrap:\s*wrap;/s);
});

test("lease failures explain tape state without exposing raw JSON", () => {
  const testPick = (object, ...keys) => {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    return null;
  };
  const evidenceValue = (run, ...keys) => {
    for (const source of [run, run.result, run.result && run.result.evidence]) {
      const value = testPick(source, ...keys);
      if (value !== null) return value;
    }
    return null;
  };
  const detailSource = section("  function backupFailureDetail(", "  function backupRunMatches(");
  const failureDetail = Function("pick", "backupEvidenceValue", '"use strict";\n' + detailSource + "\nreturn backupFailureDetail;")(testPick, evidenceValue);
  const recorded = {
    detail: "a drive or cartridge lease expired during the physical backup",
    media_write_started: false,
    position_uncertain: false
  };

  assert.equal(failureDetail({ failure_detail: recorded }), recorded.detail);
  assert.equal(failureDetail({ failure_detail: JSON.stringify(recorded) }), recorded.detail);
  assert.doesNotMatch(failureDetail({ failure_detail: JSON.stringify(recorded) }), /media_write_started|position_uncertain|\{/);

  const operatorSource = section("  function backupOperatorFailure(", "  function renderBackupRunDetail(");
  const operatorFailure = Function("backupFailureDetail", "backupEvidenceValue", '"use strict";\n' + operatorSource + "\nreturn backupOperatorFailure;")(failureDetail, evidenceValue);
  const beforeWrite = operatorFailure({ failure_detail: JSON.stringify(recorded), result: { media_write_started: false, position_uncertain: false } });
  const legacyBeforeWrite = operatorFailure({ failure_detail: JSON.stringify(recorded), result: { media_write_started: false, position_uncertain: true } });
  const afterWrite = operatorFailure({ failure_detail: recorded.detail, result: { media_write_started: true } });
  const legacyCapacity = operatorFailure({
    failure_detail: "complete archive staging needs approximately 2112779023008 bytes before tape motion, but only 956971745280 bytes are available after the configured reserve"
  });

  assert.match(beforeWrite, /before any tape-motion or data-write command was dispatched/);
  assert.match(beforeWrite, /loaded tape and known drive position were preserved/);
  assert.doesNotMatch(beforeWrite, /reconcile/i);
  assert.match(legacyBeforeWrite, /older record marked the drive uncertain/);
  assert.match(legacyBeforeWrite, /reconcile it once/);
  assert.match(afterWrite, /Treat tape position as uncertain/);
  assert.match(afterWrite, /reconcile the drive/);
  assert.match(legacyCapacity, /complete archive to fit on the staging disk/);
  assert.match(legacyCapacity, /tape was not changed/i);
  assert.match(legacyCapacity, /Retry with Native v2/);
  assert.doesNotMatch(legacyCapacity, /drive or tape changed/i);
});

test("historical Native v1 attempts remain explicit while retry advances to Native v2", () => {
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  const phase = section("  function renderBackupPhaseTrack(", "  function backupTableProgressLabel(");

  assert.match(app, /const LEGACY_BACKUP_PHASES =/);
  assert.match(app, /Stage complete archive/);
  assert.match(app, /Build the entire DAR archive on disk; tape remains idle/);
  assert.match(phase, /backupUsesLegacyWholeArchive\(run\) \? LEGACY_BACKUP_PHASES : BACKUP_PHASES/);
  assert.match(detail, /This is a retained Native v1 attempt/);
  assert.match(detail, /A new retry uses Native v2 bounded rolling packs/);
  assert.match(detail, /Retry with Native v2/);
});

test("multi-tape runs ask for each ordered continuation volume", () => {
  const helpers = section("  function backupNeedsNextMedia(", "  function backupRunRouteId(");
  const open = section("  async function openAssignBackupMedia(", "  function updateBackupMediaForDrive(");
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  const action = section("  function backupOperatorAction(", "  function backupCurrentOperation(");

  assert.match(helpers, /waiting_for_next_media/);
  assert.match(helpers, /next_volume_sequence/);
  assert.match(open, /objectValue\(readiness, "media_request"\)/);
  assert.match(open, /"volume_order", "volume_sequence", "media_sequence"/);
  assert.match(open, /data-volume-sequence/);
  assert.match(open, /and continue archive/);
  assert.doesNotMatch(open, /backup_authority_confirmed/);
  assert.doesNotMatch(open, /name="confirmation"/);
  assert.match(action, /Load next tape/);
  assert.match(action, /Unload and remove it, then insert a different prepared scratch tape/);
  assert.match(open, /Unload and remove the previous archive tape before inserting this one/);
  assert.match(detail, /Every tape in the archive was written, completely read back/);
  assert.match(action, /pick\(allowed, readback \? "assign_media" : next \? "assign_next_media" : "assign_media"\) === true/);
  assert.doesNotMatch(action, /assign_next_media" : "assign_media"\) !== false/);
  assert.match(app, /Tape accepted; resuming/);
  assert.match(app, /This tape handoff is complete\. The worker is rechecking the drive and may request another archive tape later if the job requires one\./);
  assert.doesNotMatch(app, /No further tape selection is needed for this request/);
  assert.match(app, /runs\.filter\(backupWaitingForOperator\)\.length/);
  assert.match(app, /backupWaitingForOperator\(active\) \? "Continue backup" : "View progress"/);
});

test("readback media handoffs load exact ordered tapes without destructive copy", () => {
  const helpers = section("  function backupMediaRequestPhase(", "  function backupCompressionMode(");
  const phase = section("  function backupPhaseIndex(", "  function renderBackupPhaseTrack(");
  const open = section("  async function openAssignBackupMedia(", "  function updateBackupMediaForDrive(");
  const submit = section("  async function submitBackupMedia(", "  function prepareBackupTape(");
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  const action = section("  function backupOperatorAction(", "  function backupCurrentOperation(");

  assert.match(helpers, /backupMediaRequestPhase\(source\) === "full_readback"/);
  assert.match(helpers, /if \(backupMediaIsReadback\(run\)\) return false/);
  assert.match(phase, /backupMediaIsReadback\(run\).*aliases\.includes\("readback"\)/s);
  assert.ok(
    phase.indexOf('backupTransferPhase(pick(backupPreparationProgress(run), "stage")) === "source_stability_check"') < phase.indexOf("if (backupMediaIsReadback(run))"),
    "the final source check must supersede a retained readback-media marker"
  );
  assert.match(open, /data-media-phase=/);
  assert.match(open, /Load recorded archive tape/);
  assert.match(open, /Unload and remove the current tape before inserting this one/);
  assert.match(open, /rejects a different tape before this read-only handoff continues/);
  assert.match(open, /Readback order/);
  assert.match(open, /Read detected tape/);
  assert.doesNotMatch(open, /backup_authority_confirmed/);
  assert.doesNotMatch(open, /name="confirmation"/);
  assert.match(submit, /currentPhase !== mediaPhase \|\| currentSequence !== volumeSequence/);
  assert.match(submit, /destructive_overwrite_confirmed: !readbackMedia/);
  assert.match(submit, /encryption_deviation_confirmed: !readbackMedia/);
  assert.match(submit, /confirmation: expectedConfirmation/);
  assert.match(open, /const fixedMedia = selectedMedia/);
  assert.match(open, /reads the on-tape ID after rewind and blocks a mismatch/);
  assert.doesNotMatch(submit, /physical-label attestation/);
  assert.match(submit, /read the on-tape ID again before reading backup data/);
  assert.doesNotMatch(submit, /reading the exact ordered archive tape/);
  assert.match(action, /Load tape .* for readback/);
  assert.match(action, /will not erase or relabel it/);
});

test("ordered archive evidence uses durable volume order and cartridge labels", () => {
  const volumes = section("  function backupRunVolumes(", "  function backupTapeSummary(");
  assert.match(volumes, /"expected_cartridge_label"/);
  assert.match(volumes, /"engine_volume_name"/);
  assert.match(volumes, /"volume_order", "sequence"/);
  assert.match(volumes, /sort\(\(left, right\) => left\.sequence - right\.sequence\)/);
});

test("verified backups lead directly to the retained file browser", () => {
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  assert.match(detail, /data-action="browse-backup-files"/);
  assert.match(detail, /Browse backed-up files/);
  assert.match(detail, /<details class="backup-technical">/);
});
