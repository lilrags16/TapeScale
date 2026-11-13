import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const app = readFileSync(new URL("../web/app.js", import.meta.url), "utf8");

function section(start, end) {
  const from = app.indexOf(start);
  assert.notEqual(from, -1, `missing ${start}`);
  const to = app.indexOf(end, from + start.length);
  assert.notEqual(to, -1, `missing ${end} after ${start}`);
  return app.slice(from, to);
}

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

  assert.match(submit, /backupRunState\(retained\) === "waiting_for_media"/);
  assert.match(submit, /openAssignBackupMedia\(state\.selectedBackupRunId, \{ driveId, cartridgeId, guided: true \}\)/);
  assert.doesNotMatch(submit, /toast\("Full backup queued"/);
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
  assert.equal((open.match(/name="backup_authority_confirmed"/g) || []).length, 1);

  assert.match(submit, /await loadBackupMediaReadiness\("run", runId\)/);
  assert.match(submit, /required_confirmation/);
  assert.match(submit, /typed_cartridge_label: exactLabel/);
  assert.match(submit, /loaded_media_confirmed: authorized/);
  assert.match(submit, /destructive_overwrite_confirmed: authorized/);
  assert.match(submit, /encryption_deviation_confirmed: authorized/);
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
  assert.match(app, /\{ name: "Confirm tape", detail: "Choose drive and tape"/);
  assert.match(app, /\{ name: "Starting", detail: "Waiting for the worker", aliases: \["queued", "media_assigned", "preparing"\] \}/);
});

test("verified backups lead directly to the retained file browser", () => {
  const detail = section("  function renderBackupRunDetail(", "  function renderJobs(");
  assert.match(detail, /data-action="browse-backup-files"/);
  assert.match(detail, /Browse backed-up files/);
  assert.match(detail, /<details class="backup-technical">/);
});
