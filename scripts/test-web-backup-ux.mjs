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
  assert.match(index, /app\.css\?v=tapescale-alpha-9/);
  assert.match(index, /app\.js\?v=tapescale-alpha-9/);
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
  assert.match(app, /Prepared scratch identity is intentionally pending/);
  assert.match(app, /Destructive reuse gate/);
});

test("the start flow requires a currently eligible drive and tape", () => {
  const open = section("  async function openStartBackup(", "  function updateStartBackupTape(");
  const update = section("  function updateStartBackupTape(", "  async function submitStartBackup(");
  const submit = section("  async function submitStartBackup(", "  function backupMediaConfirmation(");

  assert.match(open, /await loadBackupMediaReadiness\("profile", profileId\)/);
  assert.match(open, /const pairs = readiness\.candidates/);
  assert.match(open, /new Set\(pairs\.map/);
  assert.match(open, /readiness\.recommended_candidate/);
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

  assert.match(submit, /backupRunState\(retained\) === "waiting_for_media" \|\| backupNeedsNextMedia\(retained\)/);
  assert.match(submit, /openAssignBackupMedia\(state\.selectedBackupRunId, \{ driveId, cartridgeId, guided: true \}\)/);
  assert.match(submit, /compression_mode: backupCompressionMode/);
  assert.match(submit, /spanning_enabled: backupSpanningEnabled/);
  assert.doesNotMatch(submit, /toast\("Full backup queued"/);
});

test("backup policy exposes compression and spanning without a one-tape ceiling", () => {
  const open = section("  function openBackupProfile(", "  async function submitBackupProfile(");
  const submit = section("  async function submitBackupProfile(", "  function backupCapacityPlan(");

  assert.match(open, /name="compression_mode"/);
  assert.match(open, /value="auto" selected/);
  assert.match(open, /value="gzip"/);
  assert.match(open, /value="lzo"/);
  assert.match(open, /value="none"/);
  assert.match(open, /Automatic · LZO \(recommended\)/);
  assert.match(open, /prioritize source throughput and reduce tape starvation/);
  assert.doesNotMatch(open, /Automatic · fast GZIP/);
  assert.match(open, /name="spanning_enabled" checked/);
  assert.match(open, /spans tapes as needed/);
  assert.match(submit, /compression_mode: String\(values\.get\("compression_mode"\) \|\| "auto"\)/);
  assert.match(submit, /spanning_enabled: values\.get\("spanning_enabled"\) === "on"/);
});

test("unsupported legacy compression is shown and blocked instead of becoming auto", () => {
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
  const legacy = { compression_mode: "zstd" };
  assert.equal(evaluate[2]({ compression_mode: "auto" }), "Automatic → LZO (fastest)");
  const legacyAutomatic = Function(
    "pick",
    "backupProfileForRun",
    "backupEvidenceValue",
    '"use strict";\n' + helpers + "\nreturn backupCompressionLabel;"
  )(testPick, () => null, () => "gzip");
  assert.equal(legacyAutomatic({ compression_mode: "auto" }), "Automatic → GZIP1 (legacy)");
  assert.equal(evaluate[0](legacy), "zstd");
  assert.equal(evaluate[1](legacy), false);
  assert.equal(evaluate[2](legacy), "Unsupported (zstd)");

  const profiles = section("  function renderBackupProfiles(", "  function renderBackupRunTable(");
  assert.match(profiles, /!compressionSupported/);
  assert.match(profiles, /disabled title="Create a new backup plan with a supported compression mode\./);
  assert.match(profiles, /unsupported legacy compression mode/);
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
  assert.match(history, /supported verified Full recovery point can run an attended one- or multi-tape restore/);
  assert.match(history, /<h4>Restore run<\/h4>/);
  assert.match(history, /Attended .*multi-tape.*single-tape/);
  assert.match(history, /Load tape .* of/);
  assert.match(history, /durably paused at a read-only media boundary/);
  assert.match(history, /data-action="open-restore-media-handoff"/);
  assert.match(history, /data-action="open-cancel-restore-rehearsal"/);
  assert.match(history, /Download restore evidence/);
  assert.doesNotMatch(history, /Ready G0\.2 restore plans|Recovery rehearsals/);

  assert.match(app, /Server-normalized file or directory selection is executable only for a single-tape recovery point/);
  assert.match(app, /multi-tape selected execution stays blocked until per-path media evidence is retained/);
  assert.match(app, /Plan complete restore/);
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
  assert.match(start, /Later tapes use separate exact handoffs/);
  assert.match(start, /immutably bound to the normalized paths and selection hash/);
  assert.match(start, /local console has no application sign-in/);
  assert.match(start, /new isolated recovery destination, not live data/);
  assert.match(start, /openModal\("Start Restore run"/);
  assert.doesNotMatch(start, /supported G0\.2 recovery point|Start recovery rehearsal/);

  assert.match(handoff, /\/media-readiness/);
  assert.match(handoff, /pick\(readiness, "ready"\) !== true/);
  assert.match(handoff, /Incomplete durable tape request/);
  assert.match(handoff, /Unload and remove the previous tape, then insert this exact recorded cartridge/);
  assert.match(handoff, /name="loaded_media_confirmed" required/);
  assert.match(handoff, /name="attended_confirmed" required/);
  assert.match(handoff, /Finish later/);
  assert.match(handoff, /const readiness = await loadRestoreMediaReadiness\(runId\)/);
  assert.match(handoff, /The exact restore tape request changed while this form was open/);
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

test("writing progress explains segmented disk spooling without pretending the tape is continuous", () => {
  const cue = section("  function renderBackupSpoolingCue(", "  function backupPhaseIndex(");
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");

  assert.match(cue, /backupRunState\(run\) !== "writing"/);
  assert.match(cue, /backupEvidenceValue\(run, "data_spooling"\)/);
  assert.match(cue, /"effective_maximum_bytes", "maximum_bytes"/);
  assert.match(cue, /Measured disk buffering protects the tape stream/);
  assert.match(cue, /fills and drains a bounded disk buffer in segments/);
  assert.match(cue, /tape can pause between segments when the source cannot refill at drive speed/);
  assert.match(cue, /live panel reports the actual buffer and tape activity/);
  assert.match(cue, /This buffer does not limit the total archive size/);
  assert.doesNotMatch(cue, /%|percent|percentage/i);
  assert.match(detail, /notice \+ renderBackupSpoolingCue\(run\) \+ renderBackupPreparationProgress\(run\) \+ renderBackupLiveThroughput\(run\) \+ measuredProgress/);
});

test("backup preparation and transfer telemetry are live, directional, and honest about stale or missing samples", () => {
  const helpers = section("  function backupLiveIo(", "  function renderBackupSpoolingCue(");
  const values = Function(
    "objectValue", "pick", "backupRunState", "BACKUP_TERMINAL_STATES", "human", "date", "bytes", "esc",
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
    (value) => String(value)
  );
  const [stale, renderPreparation, render] = values;
  const now = Date.now();
  const freshSample = {
    sampled_at: new Date(now).toISOString(),
    sample_window_ms: 5000,
    phase: "tape_write",
    activity: "buffering",
    engine_state: "running",
    progress_idle_ms: 0,
    no_progress_timeout_ms: 1800000,
    pipeline_bytes_per_second: 1048576,
    tape_write_bytes_per_second: 0,
    tape_read_bytes_per_second: null,
    tape_read_counter_available: true,
    tape_write_counter_available: true,
    tape_counters_available: true,
    stale_after_seconds: 20,
    pipeline_basis: "bacula_storage_running_job_bytes_delta",
    tape_basis: "linux_scsi_tape_sysfs_counter_delta",
    spool_bytes: 16777216,
    spool_capacity_bytes: 67108864,
    spool_file_count: 2,
    spool_fill_bytes_per_second: 2097152,
    spool_drain_bytes_per_second: 0,
    spool_basis: "run_scoped_bacula_spool_regular_file_bytes"
  };

  assert.equal(stale(freshSample, now), false);
  const live = render({ state: "writing", result: { live_io: freshSample } });
  assert.match(live, /Filling the disk buffer/);
  assert.match(live, /Buffering/);
  assert.match(live, /Disk buffer/);
  assert.match(live, /16\.0 MiB \/ 64\.0 MiB/);
  assert.match(live, /25\.0% temporary occupancy/);
  assert.match(live, /not archive completion/);
  assert.match(live, /Filling<\/span><strong>2\.0 MiB\/s/);
  assert.match(live, /Source data is moving into the temporary disk buffer/);
  assert.match(live, /Bacula processed stream/);
  assert.match(live, /After Bacula hashing\/compression · not raw ZFS throughput/);
  assert.doesNotMatch(live, /Logical backup pipeline/);
  assert.match(live, /1\.0 MiB\/s/);
  assert.match(live, /Tape write/);
  assert.match(live, /0 B\/s/);
  assert.match(live, /Tape read/);
  assert.match(live, /Measuring…/);
  assert.match(live, /Rolling 5 second sample/);
  assert.match(live, /Engine progress: Bacula Storage Running Job Bytes Delta/);
  assert.match(live, /Disk buffer: Run Scoped Bacula Spool Regular File Bytes/);
  assert.match(live, /Linux Scsi Tape Sysfs Counter Delta/);
  assert.match(live, /Bacula: Running/);

  const firstSpoolSample = { ...freshSample, activity: "starting", sample_window_ms: null };
  delete firstSpoolSample.spool_fill_bytes_per_second;
  delete firstSpoolSample.spool_drain_bytes_per_second;
  const firstSpool = render({ state: "writing", result: { live_io: firstSpoolSample } });
  assert.match(firstSpool, /Net movement<\/span><strong>Measuring…/);
  assert.doesNotMatch(firstSpool, /Net movement<\/span><strong>0 B\/s/);

  const streaming = render({ state: "writing", result: { live_io: { ...freshSample, activity: "streaming", spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 4194304, tape_write_bytes_per_second: 4194304 } } });
  assert.match(streaming, /Streaming buffered data to tape/);
  assert.match(streaming, /Tape active/);
  assert.match(streaming, /Draining<\/span><strong>4\.0 MiB\/s/);
  assert.match(streaming, /physical tape-write counter is advancing/);

  const despooling = render({ state: "writing", result: { live_io: { ...freshSample, activity: "despooling", spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 3145728, tape_write_counter_available: false, tape_write_bytes_per_second: null } } });
  assert.match(despooling, /Draining the disk buffer to tape/);
  assert.match(despooling, /temporary disk buffer is shrinking/);

  const idle = render({ state: "writing", result: { live_io: { ...freshSample, activity: "idle", progress_idle_ms: 300000, pipeline_bytes_per_second: 0, spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 0 } } });
  assert.match(idle, /No data movement/);
  assert.match(idle, /worker is responding/i);
  assert.match(idle, /no measured disk-buffer, logical, or tape bytes have moved for 5m 0s/i);
  assert.match(idle, /Automatic cancellation in 25m 0s/);
  assert.match(idle, /heartbeat is not counted as progress/);

  const stalled = render({ state: "writing", result: { live_io: { ...freshSample, activity: "stalled", progress_idle_ms: 1800000, pipeline_bytes_per_second: 0, spool_fill_bytes_per_second: 0, spool_drain_bytes_per_second: 0 } } });
  assert.match(stalled, /activity-stalled/);
  assert.match(stalled, /Transfer stalled/);
  assert.match(stalled, /Action required/);
  assert.match(stalled, /no-progress safety threshold has been reached/i);

  const legacyIdle = render({ state: "writing", result: { live_io: { sampled_at: new Date(now).toISOString(), sample_window_ms: 5000, phase: "backup_write", pipeline_bytes_per_second: 0, tape_write_bytes_per_second: 0, tape_read_bytes_per_second: 0, tape_counters_available: true, stale_after_seconds: 20 } } });
  assert.match(legacyIdle, /No data movement/);
  assert.doesNotMatch(legacyIdle, /Disk buffer<\/span>/);

  const unavailable = render({ state: "writing", result: { live_io: { ...freshSample, tape_read_counter_available: false, tape_write_counter_available: false, tape_counters_available: false } } });
  assert.equal((unavailable.match(/Unavailable/g) || []).length, 2);

  const partial = render({ state: "writing", result: { live_io: { ...freshSample, tape_read_counter_available: false } } });
  assert.match(partial, /Tape write<\/span><strong>0 B\/s/);
  assert.match(partial, /Tape read<\/span><strong>Unavailable/);

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

  assert.match(renderPreparation({ state: "preparing", result: {} }), /Checking the immutable snapshot, Bacula catalog, drive, tape, and disk spool/);
  assert.match(renderPreparation({ state: "inventorying", result: {} }), /Waiting for the worker's first source-scan sample/);
  const stability = renderPreparation({ state: "verifying", result: { preparation_progress: { ...preparationSample, stage: "source_stability_check", step: "sorting" } } });
  assert.match(stability, /Sorting manifest entries/);
  assert.match(stability, /Tape readback is complete/);
  assert.equal(render({ state: "verifying", result: { live_io: freshSample, preparation_progress: preparationSample } }), "");

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

test("final authorization is explicit without asking for the cartridge label twice", () => {
  const open = section("  async function openAssignBackupMedia(", "  function updateBackupMediaForDrive(");
  const submit = section("  async function submitBackupMedia(", "  function prepareBackupTape(");

  assert.match(open, /name="backup_authority_confirmed" required/);
  assert.match(open, /I physically verified this labeled scratch tape is loaded/);
  assert.match(open, /name="confirmation"/);
  assert.match(open, /aria-describedby="backup-media-confirmation-help"/);
  assert.match(open, /id="backup-media-confirmation-help" aria-live="polite"/);
  assert.doesNotMatch(open, /backup-typed-cartridge/);
  assert.equal((open.match(/name="backup_authority_confirmed"/g) || []).length, 2, "each mutually exclusive write/readback branch has one authority checkbox");

  assert.match(submit, /await loadBackupMediaReadiness\("run", runId\)/);
  assert.match(submit, /required_confirmation/);
  assert.match(submit, /typed_cartridge_label: exactLabel/);
  assert.match(submit, /volume_sequence: volumeSequence/);
  assert.match(submit, /loaded_media_confirmed: authorized/);
  assert.match(submit, /destructive_overwrite_confirmed: !readbackMedia && authorized/);
  assert.match(submit, /encryption_deviation_confirmed: !readbackMedia && authorized/);
  assert.match(submit, /String\(values\.get\("confirmation"\) \|\| ""\) !== expectedConfirmation/);

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
  assert.match(start, /const continueGuided = backupModalSubmissionCurrent\(form, token\)/);
  assert.match(start, /if \(!continueGuided\) \{[\s\S]*Backup request saved[\s\S]*return;/);
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
  const render = section("  function renderJobs(", "  function mediaName(");
  const routeBranch = render.indexOf("if (routeId)");
  const overviewMetrics = render.indexOf("const active =");

  assert.ok(routeBranch >= 0 && routeBranch < overviewMetrics, "the dedicated route must return before dashboard composition");
  assert.match(render, /href="#jobs">← Back to Backup Center/);
  assert.match(render, /renderBackupRunDetail/);

  const progress = section("  function backupProgressPercent(", "  function backupFailureDetail(");
  assert.match(progress, /reported !== null && reported !== ""/);
  assert.match(progress, /return null/);
  assert.doesNotMatch(progress, /backupPhaseIndex\(run\).*100/);
});

test("run summaries distinguish planned, active, complete, and failed readback", () => {
  const helperSource = section("  function backupReadbackSummary(", "  function backupFailureDetail(");
  const summary = Function("backupRunState", "BACKUP_TERMINAL_STATES", "pick", "backupPreparationProgress", '"use strict";\n' + helperSource + "\nreturn backupReadbackSummary;")(
    (run) => run.state,
    new Set(["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"]),
    (object, key) => object && object[key] !== undefined ? object[key] : null,
    (run) => run.result && run.result.preparation_progress || {}
  );

  assert.equal(summary({ state: "waiting_for_media" }), "Complete readback planned");
  assert.equal(summary({ state: "verifying" }), "Readback in progress");
  assert.equal(summary({ state: "verifying", result: { preparation_progress: { stage: "source_stability_check" } } }), "Readback complete; final source check");
  assert.equal(summary({ state: "verified" }), "Readback complete");
  assert.equal(summary({ state: "failed" }), "Readback not completed");

  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  assert.doesNotMatch(detail, /Full backup · complete readback/);
  assert.match(detail, /backupReadbackSummary\(run\)/);
  assert.match(app, /\{ name: "Confirm first tape", detail: "Choose the first archive volume"/);
  assert.match(app, /\{ name: "Preflight", detail: "Validate source, catalog, drive, tape, and spool", aliases: \["queued", "media_assigned", "preparing"\] \}/);
  assert.match(app, /\{ name: "Inventory", detail: "Hash files and build the recovery manifest", aliases: \["inventorying", "manifesting", "source_manifest"\] \}/);
  assert.match(app, /\{ name: "Finalize", detail: "Confirm the source snapshot and seal evidence", aliases: \["source_stability_check", "final_source_check"\] \}/);
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

  assert.match(beforeWrite, /before any tape-motion or data-write command was dispatched/);
  assert.match(beforeWrite, /loaded tape and known drive position were preserved/);
  assert.doesNotMatch(beforeWrite, /reconcile/i);
  assert.match(legacyBeforeWrite, /older record marked the drive uncertain/);
  assert.match(legacyBeforeWrite, /reconcile it once/);
  assert.match(afterWrite, /Treat tape position as uncertain/);
  assert.match(afterWrite, /reconcile the drive/);
});

test("multi-tape runs ask for each ordered continuation volume", () => {
  const helpers = section("  function backupNeedsNextMedia(", "  function backupRunRouteId(");
  const open = section("  async function openAssignBackupMedia(", "  function updateBackupMediaForDrive(");
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");

  assert.match(helpers, /waiting_for_next_media/);
  assert.match(helpers, /next_volume_sequence/);
  assert.match(open, /objectValue\(readiness, "media_request"\)/);
  assert.match(open, /"volume_order", "volume_sequence", "media_sequence"/);
  assert.match(open, /data-volume-sequence/);
  assert.match(open, /Erase tape and continue archive/);
  assert.match(detail, /Load next tape/);
  assert.match(detail, /Unload and remove it, then insert and confirm a different prepared scratch tape/);
  assert.match(open, /Unload and remove the previous archive tape before inserting this one/);
  assert.match(detail, /Every tape in the archive was written, completely read back/);
  assert.match(detail, /pick\(allowedActions, readbackMedia \? "assign_media" : nextMedia \? "assign_next_media" : "assign_media"\) === true/);
  assert.doesNotMatch(detail, /assign_next_media" : "assign_media"\) !== false/);
  assert.match(detail, /Tape accepted; worker is rechecking it/);
  assert.match(detail, /No further tape selection is needed for this request/);
  assert.match(app, /runs\.filter\(backupWaitingForOperator\)\.length/);
  assert.match(app, /backupWaitingForOperator\(active\) \? "Continue backup" : "View progress"/);
});

test("readback media handoffs load exact ordered tapes without destructive copy", () => {
  const helpers = section("  function backupMediaRequestPhase(", "  function backupCompressionMode(");
  const phase = section("  function backupPhaseIndex(", "  function renderBackupPhaseTrack(");
  const open = section("  async function openAssignBackupMedia(", "  function updateBackupMediaForDrive(");
  const submit = section("  async function submitBackupMedia(", "  function prepareBackupTape(");
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");

  assert.match(helpers, /backupMediaRequestPhase\(source\) === "full_readback"/);
  assert.match(helpers, /if \(backupMediaIsReadback\(run\)\) return false/);
  assert.match(phase, /backupMediaIsReadback\(run\).*aliases\.includes\("readback"\)/s);
  assert.ok(
    phase.indexOf('pick(backupPreparationProgress(run), "stage") === "source_stability_check"') < phase.indexOf("if (backupMediaIsReadback(run))"),
    "the final source check must supersede a retained readback-media marker"
  );
  assert.match(open, /data-media-phase=/);
  assert.match(open, /Load recorded archive tape/);
  assert.match(open, /Unload and remove the current tape before inserting this one/);
  assert.match(open, /will not erase, relabel, or append/);
  assert.match(open, /Readback order/);
  assert.match(open, /Confirm tape and continue readback/);
  assert.match(open, /server confirmation/);
  assert.match(submit, /currentPhase !== mediaPhase \|\| currentSequence !== volumeSequence/);
  assert.match(submit, /destructive_overwrite_confirmed: !readbackMedia && authorized/);
  assert.match(submit, /encryption_deviation_confirmed: !readbackMedia && authorized/);
  assert.match(open, /Standalone SAS drives cannot report the physical cartridge label/);
  assert.match(open, /Bacula validates its on-tape volume label after rewind/);
  assert.doesNotMatch(open, /reject a different label before any tape operation/);
  assert.match(submit, /accepted your physical-label attestation/);
  assert.match(submit, /validate the on-tape volume label after rewind/);
  assert.doesNotMatch(submit, /reading the exact ordered archive tape/);
  assert.match(detail, /Load tape .* for readback/);
  assert.match(detail, /without erasing or relabeling it/);
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
