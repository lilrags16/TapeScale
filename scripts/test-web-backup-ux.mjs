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
  assert.match(index, /app\.css\?v=tapescale-alpha-3/);
  assert.match(index, /app\.js\?v=tapescale-alpha-3/);
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
  assert.match(retained, /Only this browse-only directory view is bounded/);
  assert.match(retained, /retained manifest and tape archive are complete/);
  assert.match(retained, /backup itself was not limited/);
});

test("the Recovery Vault offers attended one- and multi-tape Restore runs only under the server contract", () => {
  const phases = section("  const RESTORE_REHEARSAL_PHASES = [", "  function restoreRehearsalDetailRecord(");
  const detailContract = section("  function restoreRehearsalVolumes(", "  function restoreRehearsalPhase(");
  const history = section("  function renderRestoreRehearsal(", "  function renderRecoveryPointDossier(");
  const dossier = section("  function renderRecoveryPointDossier(", "  function renderRestore(");
  const planDialog = section("  function openRestorePlan(", "  function optionalFormNumber(");
  const mediaContract = section("  function restorePlanTapes(", "  function restorePlanDrives(");
  const result = section("  function renderRestorePlanResult(", "  async function submitRestorePlan(");
  const start = section("  function openRestoreRehearsal(", "  async function submitRestoreRehearsal(");
  const handoff = section("  function restoreMediaReadinessValue(", "  function openCancelRestoreRehearsal(");

  assert.match(phases, /name: "Restore files", detail: "Read the complete backup across its ordered tapes"/);
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

  assert.match(dossier, /recovers the complete point across its exact ordered tape set into a new isolated destination/);
  assert.match(dossier, /File or directory selection, encrypted recovery points, and clean-catalog recovery are not available yet/);
  assert.match(planDialog, /server-returned ordered tape set/);

  assert.match(mediaContract, /planArray\(restorePlanExecution\(plan\), "required_tape_ids", "required_tapes", "tapes"\)/);
  assert.match(mediaContract, /pick\(execution, "multi_volume"\) === true/);
  assert.match(mediaContract, /declaredCount === tapes\.length/);
  assert.match(mediaContract, /tapes\.length === 1 \|\| restorePlanMultiVolumeSupported\(plan\)/);
  assert.match(result, /executionSupported = ready && executionOffered && volumeContractReady/);
  assert.match(result, /matching attended execution contract for this ordered media set/);
  assert.match(result, /Restore this complete recovery point/);
  assert.match(result, /Start Restore run/);
  assert.match(result, /pause durably at each media boundary and name the exact next cartridge/);
  assert.match(result, /Full-point-only and does not support file selection, encrypted recovery points, or clean-catalog recovery/);
  assert.match(result, /Every later tape requires its own exact attended handoff/);

  assert.match(start, /if \(!restorePlanVolumeContractReady\(plan\)\)/);
  assert.doesNotMatch(start, /if \(tapes\.length !== 1\)/);
  assert.match(start, /pick\(restorePlanExecution\(plan\), "destination_subpath", "destination"\)/);
  assert.match(start, /restores the complete recovery point across .* exact ordered cartridges/);
  assert.match(start, /Load only tape 1 now/);
  assert.match(start, /Later tapes use separate exact handoffs/);
  assert.match(start, /File or directory selection, encrypted recovery points, and clean-catalog recovery are not supported/);
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

test("writing progress explains bounded disk spooling without inventing a percentage", () => {
  const cue = section("  function renderBackupSpoolingCue(", "  function backupPhaseIndex(");
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");

  assert.match(cue, /backupRunState\(run\) !== "writing"/);
  assert.match(cue, /backupEvidenceValue\(run, "data_spooling"\)/);
  assert.match(cue, /"effective_maximum_bytes", "maximum_bytes"/);
  assert.match(cue, /Buffering on disk, then streaming to tape/);
  assert.match(cue, /drive may remain idle while the buffer initially fills/);
  assert.match(cue, /once despooling begins, it should stream continuously for that buffered segment/);
  assert.doesNotMatch(cue, /pause normally|switches between filling and writing/);
  assert.match(cue, /This buffer does not limit the total archive size/);
  assert.doesNotMatch(cue, /%|percent|percentage/i);
  assert.match(detail, /notice \+ renderBackupSpoolingCue\(run\) \+ measuredProgress/);
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
  const summary = Function("backupRunState", "BACKUP_TERMINAL_STATES", '"use strict";\n' + helperSource + "\nreturn backupReadbackSummary;")(
    (run) => run.state,
    new Set(["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"])
  );

  assert.equal(summary({ state: "waiting_for_media" }), "Complete readback planned");
  assert.equal(summary({ state: "verifying" }), "Readback in progress");
  assert.equal(summary({ state: "verified" }), "Readback complete");
  assert.equal(summary({ state: "failed" }), "Readback not completed");

  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  assert.doesNotMatch(detail, /Full backup · complete readback/);
  assert.match(detail, /backupReadbackSummary\(run\)/);
  assert.match(app, /\{ name: "Confirm first tape", detail: "Choose the first archive volume"/);
  assert.match(app, /\{ name: "Starting", detail: "Waiting for the worker", aliases: \["queued", "media_assigned", "preparing"\] \}/);
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
