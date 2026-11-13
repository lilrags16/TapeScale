(() => {
  "use strict";

  const API = "/api/v1";
  const sourceBrowserEndpoint = "/source-browser";
  const endpoints = {
    system: "/system",
    overview: "/overview",
    jobs: "/jobs",
    backupProfiles: "/backup-profiles",
    backupRuns: "/backup-runs",
    media: "/media",
    mediaPools: "/media-pools",
    drives: "/drives",
    locations: "/locations",
    movements: "/movements",
    verification: "/verification",
    recoveryPoints: "/recovery-points",
    preflight: "/preflight",
    qualification: "/qualification",
    qualificationRuns: "/qualification/runs",
    recoveryProfiles: "/recovery-exercises/profiles",
    recoveryExercises: "/recovery-exercises",
    restoreRehearsals: "/restore-rehearsals"
  };
  const routes = new Set(["overview", "recovery-lab", "jobs", "restore", "movements", "tapes", "drives", "verification", "settings"]);
  const RECOVERY_PHASES = [
    { name: "Preflight", detail: "Bind source, drive, and tape", aliases: ["queued", "started", "preparing", "preflight"] },
    { name: "Inventory", detail: "Hash the immutable fixture", aliases: ["inventorying", "manifesting", "manifest", "source_manifest"] },
    { name: "Write tape", detail: "Full single-tape backup", aliases: ["writing", "backup", "engine_write"] },
    { name: "Write complete", detail: "Close the Bacula backup job", aliases: ["written", "finalizing"] },
    { name: "Read tape", detail: "Decode from physical media", aliases: ["verifying", "reading", "readback"] },
    { name: "Restore", detail: "Extract to isolated destination", aliases: ["restoring", "restored", "restored_manifest"] },
    { name: "Compare", detail: "Match source and restore", aliases: ["validating", "comparing", "comparison"] },
    { name: "Proven", detail: "Retain recovery evidence", aliases: ["passed", "verified", "completed", "complete"] }
  ];
  const RECOVERY_TERMINAL_STATES = new Set(["passed", "verified", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"]);
  const BACKUP_PHASES = [
    { name: "Confirm first tape", detail: "Choose the first archive volume", aliases: ["waiting_for_media"] },
    { name: "Starting", detail: "Waiting for the worker", aliases: ["queued", "media_assigned", "preparing"] },
    { name: "Scan files", detail: "Read the snapshot", aliases: ["inventorying", "manifesting", "source_manifest"] },
    { name: "Write archive", detail: "Copy across tapes as needed", aliases: ["writing", "media_write_started", "engine_dispatched", "waiting_for_next_media", "next_media", "media_required"] },
    { name: "Close archive", detail: "Finish the complete backup set", aliases: ["written", "finalizing"] },
    { name: "Read back", detail: "Verify every physical tape", aliases: ["verifying", "reading", "readback"] },
    { name: "Protected", detail: "Retain recovery evidence", aliases: ["verified", "passed", "completed", "complete"] }
  ];
  const BACKUP_TERMINAL_STATES = new Set(["verified", "passed", "completed", "complete", "failed", "blocked", "interrupted", "cancelled", "canceled"]);
  const state = {
    route: "overview",
    loading: true,
    refreshing: false,
    settingsTab: "preflight",
    data: {
      system: null,
      overview: { summary: { protected_sources: 0, active_jobs: 0, required_actions: 0 }, sources: [], actions: [], actions_total: 0, actions_returned: 0, actions_truncated: false },
      jobs: [],
      backupProfiles: [],
      backupRuns: [],
      backupRunDetails: {},
      backupMediaReadiness: {},
      media: [],
      mediaPools: [],
      mediaDetails: {},
      mediaReuseEvaluations: {},
      drives: [],
      slots: [],
      driveOperations: [],
      locations: [],
      movements: [],
      movementDetails: {},
      verification: [],
      recoveryPoints: [],
      preflight: null,
      qualification: null,
      qualificationRuns: [],
      recoveryProfiles: [],
      recoveryExercises: [],
      recoveryExerciseDetails: {},
      recoveryPointDetails: {},
      restoreRehearsals: [],
      restoreRehearsalDetails: {},
      restoreMediaReadiness: {}
    },
    received: new Set(),
    errors: {},
    qualificationPollTimer: null,
    recoveryExercisePollTimer: null,
    restoreRehearsalPollTimer: null,
    restoreMediaModalToken: 0,
    backupRunPollTimer: null,
    backupReadinessModalToken: 0,
    pendingBackupSetup: null,
    selectedBackupRunId: null,
    backupRunDetailLoading: new Set(),
    backupQuery: "",
    backupStateFilter: "all",
    selectedRecoveryExerciseId: null,
    selectedRecoveryPointId: null,
    selectedRestoreRehearsalId: null,
    selectedMovementId: null,
    selectedMediaId: null,
    mediaQuery: "",
    mediaPoolFilter: "all",
    mediaStatusFilter: "all",
    mediaLocationFilter: "all",
    mediaIdentityFilter: "all",
    mediaHealthFilter: "all",
    mediaProtectionFilter: "all",
    mediaDetailLoading: new Set(),
    mediaReuseLoading: new Set(),
    mediaReuseDriveByMedia: {},
    mediaReuseEvaluatedAt: {},
    recoveryPointQuery: "",
    recoveryPointStatus: "all",
    recoveryPointRepresentation: "all",
    recoveryPointDetailLoading: new Set(),
    recoveryFileBrowsers: {},
    recoveryFileBrowserOpen: new Set(),
    recoveryFileBrowserLoading: new Set(),
    recoveryFileBrowserErrors: {},
    recoveryFileBrowserRequests: {},
    restoreRehearsalDetailLoading: new Set(),
    movementDetailLoading: new Set(),
    restorePlanDrafts: {},
    restorePlans: {}
  };

  const main = document.querySelector("#main-content");
  const modalLayer = document.querySelector("#modal-layer");
  const drawerLayer = document.querySelector("#drawer-layer");
  const shell = document.querySelector("#app-shell");
  const live = document.querySelector("#live-region");
  let modalReturnFocus = null;
  let sourceBrowserRequest = 0;
  let sourceBrowserPath = "";
  let sourceBrowserReadOnly = null;
  let sourceBrowserSelectable = false;
  let sourceBrowserTarget = {
    inputId: "backup-source-subpath",
    panelId: "backup-source-browser",
    summaryId: "backup-source-selection",
    nameId: "backup-profile-name"
  };

  function esc(value) {
    return String(value == null ? "" : value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#039;");
  }

  function pick(object, ...keys) {
    if (!object || typeof object !== "object") return null;
    for (const key of keys) {
      if (object[key] !== undefined && object[key] !== null && object[key] !== "") return object[key];
    }
    return null;
  }

  function list(payload, key) {
    return payload && Array.isArray(payload[key]) ? payload[key] : [];
  }

  function count(value) {
    const parsed = Number(value);
    return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0;
  }

  function label(value) {
    return value == null || value === "" ? "Not reported" : String(value);
  }

  function human(value) {
    return String(value).replace(/([a-z0-9])([A-Z])/g, "$1 $2").replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
  }

  function date(value) {
    if (!value) return "Not reported";
    const parsed = new Date(value);
    return Number.isNaN(parsed.getTime()) ? String(value) : parsed.toLocaleString();
  }

  function bytes(value) {
    const amount = Number(value);
    if (!Number.isFinite(amount) || amount < 0) return "Not reported";
    if (amount === 0) return "0 B";
    const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    const unit = Math.min(Math.floor(Math.log(amount) / Math.log(1024)), units.length - 1);
    return (amount / Math.pow(1024, unit)).toFixed(unit ? 1 : 0) + " " + units[unit];
  }

  function status(value) {
    const text = label(value);
    const tone = String(value || "").toLowerCase().replace(/[^a-z]+/g, "-");
    return '<span class="status ' + esc(tone) + '">' + esc(text) + "</span>";
  }

  function recordId(record) {
    return String(pick(record, "id", "uuid", "job_id", "external_id", "stable_id", "internal_label", "label", "name") || "");
  }

  function normalize(key, payload) {
    if (key === "overview") {
      const raw = payload && typeof payload === "object" ? payload : {};
      const summary = raw.summary && typeof raw.summary === "object" ? raw.summary : {};
      const actions = list(raw, "actions");
      const requiredActions = Math.max(count(pick(summary, "required_actions", "requiredActions")), actions.length);
      const actionsTotal = Math.max(count(pick(raw, "actions_total", "actionsTotal")), requiredActions);
      return Object.assign({}, raw, {
        summary: {
          protected_sources: count(pick(summary, "protected_sources", "protectedSources")),
          active_jobs: count(pick(summary, "active_jobs", "activeJobs")),
          required_actions: actionsTotal
        },
        sources: list(raw, "sources"),
        actions,
        actions_total: actionsTotal,
        actions_returned: actions.length,
        actions_truncated: pick(raw, "actions_truncated", "actionsTruncated") === true || actions.length < actionsTotal
      });
    }
    if (key === "jobs") return list(payload, "jobs");
    if (key === "backupProfiles") return list(payload, "profiles");
    if (key === "backupRuns") return list(payload, "runs");
    if (key === "media") return list(payload, "media");
    if (key === "mediaPools") {
      const pools = list(payload, "pools");
      return pools.length ? pools : list(payload, "media_pools");
    }
    if (key === "locations") return list(payload, "locations");
    if (key === "movements") return list(payload, "movements");
    if (key === "verification") return list(payload, "verification");
    if (key === "recoveryPoints") return list(payload, "recovery_points");
    if (key === "qualificationRuns") return list(payload, "runs");
    if (key === "recoveryProfiles") return list(payload, "profiles");
    if (key === "recoveryExercises") {
      const exercises = list(payload, "exercises");
      return exercises.length ? exercises : list(payload, "runs");
    }
    if (key === "restoreRehearsals") {
      const rehearsals = list(payload, "rehearsals");
      return rehearsals.length ? rehearsals : list(payload, "runs");
    }
    if (key === "drives") {
      return { drives: list(payload, "drives"), slots: list(payload, "slots"), operations: list(payload, "operations") };
    }
    return payload && typeof payload === "object" ? payload : null;
  }

  function errorMessage(error) {
    const payload = error && error.payload;
    return label(
      pick(payload, "message", "detail") ||
      (payload && typeof payload.error === "string" ? payload.error : null) ||
      pick(payload && payload.error, "message", "detail") ||
      (error && error.message)
    );
  }

  async function request(path, options) {
    const response = await fetch(API + path, Object.assign({
      headers: { Accept: "application/json" },
      cache: "no-store"
    }, options || {}));
    let payload = null;
    try {
      payload = await response.json();
    } catch (_) {
      payload = null;
    }
    if (!response.ok) {
      const error = new Error("Request failed with HTTP " + response.status);
      error.payload = payload;
      error.status = response.status;
      throw error;
    }
    return payload;
  }

  async function loadOne(key) {
    try {
      const normalized = normalize(key, await request(endpoints[key]));
      if (key === "media" || key === "drives") invalidateMediaReuseEvaluations();
      if (key === "drives") {
        state.data.drives = normalized.drives;
        state.data.slots = normalized.slots;
        state.data.driveOperations = normalized.operations;
      } else {
        state.data[key] = normalized;
      }
      state.received.add(key);
      delete state.errors[key];
    } catch (error) {
      state.errors[key] = error;
    }
  }

  async function loadAll(initial) {
    if (initial) state.loading = true;
    else state.refreshing = true;
    updateChrome();
    render();
    await Promise.all(Object.keys(endpoints).map(loadOne));
    if (state.route === "recovery-lab") {
      const exercise = selectedRecoveryExercise();
      if (exercise && recordId(exercise)) await loadRecoveryExerciseDetail(recordId(exercise));
    }
    if (state.route === "restore") {
      const point = selectedRecoveryPoint();
      if (point && recordId(point)) await loadRecoveryPointDetail(recordId(point), !initial);
      const rehearsal = selectedRestoreRehearsal();
      if (rehearsal && recordId(rehearsal)) await loadRestoreRehearsalDetail(recordId(rehearsal), !initial);
    }
    if (state.route === "movements") {
      const movement = selectedMovement();
      if (movement && recordId(movement)) await loadMovementDetail(recordId(movement), !initial);
    }
    if (state.route === "tapes") {
      const media = selectedMedia();
      if (media && recordId(media)) await loadMediaDetail(recordId(media), !initial);
    }
    if (state.route === "jobs") {
      const run = selectedBackupRun();
      if (run && recordId(run)) await loadBackupRunDetail(recordId(run), !initial);
    }
    state.loading = false;
    state.refreshing = false;
    updateChrome();
    render();
    scheduleQualificationPoll();
    scheduleRecoveryExercisePoll();
    scheduleRestoreRehearsalPoll();
    scheduleBackupRunPoll();
  }

  function setBadge(id, value) {
    const element = document.querySelector(id);
    if (!element) return;
    const visible = Number(value) > 0;
    element.textContent = visible ? String(value) : "0";
    element.hidden = !visible;
  }

  function updateChrome() {
    const failures = Object.keys(state.errors).length;
    const successes = state.received.size;
    const health = document.querySelector("#service-health");
    const dot = health && health.querySelector(".health-dot");
    const strong = health && health.querySelector("strong");
    const small = health && health.querySelector("small");
    if (dot) dot.className = "health-dot" + (failures ? (successes ? " warning" : " failed") : "");
    if (strong) strong.textContent = state.loading ? "Connecting" : failures ? (successes ? "Partially connected" : "API unavailable") : "Connected";
    const mode = pick(state.data.overview, "mode");
    if (small) small.textContent = state.loading ? "Waiting for API" : mode === "unconfigured" ? "Not configured" : failures ? failures + " endpoint" + (failures === 1 ? "" : "s") + " unavailable" : "API ready";
    const version = document.querySelector("#app-version");
    if (version) version.textContent = label(pick(state.data.system, "version"));
    const site = document.querySelector("#site-name");
    const siteObject = pick(state.data.overview, "site");
    if (site) site.textContent = String(pick(state.data.overview, "site_name", "siteName") || pick(siteObject, "name") || "Local site");
    const checks = state.data.preflight;
    const preflight = document.querySelector("#preflight-status");
    if (preflight) preflight.textContent = label(pick(checks, "status") || "Setup pending");
    const preflightDot = document.querySelector(".system-pill .health-dot");
    const preflightState = String(pick(checks, "status") || "").toLowerCase();
    if (preflightDot) preflightDot.className = "health-dot" + (/fail|block|error/.test(preflightState) ? " failed" : /warn|attention|pending/.test(preflightState) ? " warning" : "");
    const summary = state.data.overview.summary || {};
    setBadge("#nav-job-count", state.data.backupRuns.filter(backupRunActive).length);
    setBadge("#nav-move-count", state.data.movements.filter((movement) => !movementTerminal(movement)).length);
    setBadge("#nav-recovery-count", state.data.recoveryExercises.filter(recoveryExerciseActive).length);
    const actionCount = Math.max(count(pick(summary, "required_actions")), state.data.overview.actions.length);
    setBadge("#notification-badge", actionCount);
    const notification = document.querySelector(".notification-button");
    if (notification) notification.setAttribute("aria-label", actionCount ? "Open notifications, " + actionCount + " unresolved" : "Open notifications");
  }

  function pageHead(title, description, actions) {
    return '<div class="page-head"><div class="page-title-block"><h1>' + esc(title) + '</h1><p>' + esc(description) + '</p></div><div class="page-actions">' + (actions || "") + "</div></div>";
  }

  function refreshButton() {
    return '<button class="button button-quiet" type="button" data-action="refresh"' + (state.refreshing ? " disabled" : "") + ">" + (state.refreshing ? "Refreshing…" : "Refresh") + "</button>";
  }

  function noticeFor(key) {
    const error = state.errors[key];
    if (!error) return state.refreshing ? '<div class="notice info"><span class="notice-icon" aria-hidden="true">↻</span><div class="notice-copy"><strong>Refreshing</strong><span>Requesting current data from TapeScale.</span></div></div>' : "";
    return '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not load ' + esc(human(key)) + '</strong><span>' + esc(errorMessage(error)) + '</span></div><button class="button button-small" type="button" data-action="refresh">Retry</button></div>';
  }

  function empty(title, message, action) {
    return '<div class="panel empty-state"><div class="empty-state-inner"><span class="empty-symbol" aria-hidden="true">○</span><h2>' + esc(title) + '</h2><p>' + esc(message) + '</p>' + (action || "") + "</div></div>";
  }

  function table(headers, rows) {
    return '<div class="panel table-panel"><div class="table-scroll"><table class="data-table"><thead><tr>' +
      headers.map((header) => "<th>" + esc(header) + "</th>").join("") +
      "</tr></thead><tbody>" + rows.join("") + "</tbody></table></div><div class=\"table-foot\">Showing " + rows.length + " record" + (rows.length === 1 ? "" : "s") + "</div></div>";
  }

  function cell(value) {
    return "<td>" + esc(label(value)) + "</td>";
  }

  function loading() {
    return '<div class="route-loading" role="status"><div class="loading-header"></div><div class="loading-grid"><span></span><span></span><span></span></div><div class="loading-table"></div></div>';
  }

  function safeActionRoute(action) {
    let value = String(pick(action, "route") || "").trim();
    if (!value) return null;
    value = value.replace(/^#?\/?/, "");
    if (!routes.has(value.split("/")[0])) return null;
    return "#" + value;
  }

  function renderActionItem(action) {
    const route = safeActionRoute(action);
    const stateValue = String(pick(action, "state", "status") || "attention");
    const body = '<span class="action-symbol ' + esc(stateValue.toLowerCase()) + '" aria-hidden="true">!</span><span class="action-copy"><strong>' + esc(label(pick(action, "title", "label", "name"))) + '</strong><span>' + esc(label(pick(action, "detail", "message", "state"))) + '</span></span>' + (pick(action, "due_at", "due") ? '<time class="action-time">' + esc(date(pick(action, "due_at", "due"))) + "</time>" : "");
    return route ? '<li class="action-item action-item-routed"><a class="action-item-link" href="' + esc(route) + '">' + body + "</a></li>" : '<li class="action-item">' + body + "</li>";
  }

  function renderOverview() {
    const overview = state.data.overview;
    const summary = overview.summary || {};
    const ready = state.received.has("overview");
    const metric = (name, value) => '<div class="metric"><span>' + esc(name) + '</span><strong>' + esc(ready ? value : "—") + "</strong></div>";
    let html = pageHead("Overview", "Current protection state reported by TapeScale.", refreshButton()) + noticeFor("overview");
    html += renderRecoveryOverviewCallout();
    html += '<section class="panel metric-strip">' +
      metric("Protected sources", summary.protected_sources) +
      metric("Active jobs", summary.active_jobs) +
      metric("Required actions", summary.required_actions) +
      metric("Cartridges", state.received.has("media") ? state.data.media.length : "—") +
      "</section>";
    html += '<div class="grid-2 section"><section class="panel"><div class="panel-head"><div><h2>Protected sources</h2><p>Sources returned by the overview API.</p></div></div><div class="panel-body">';
    if (overview.sources.length) {
      html += '<ul class="detail-list">' + overview.sources.map((source) => "<li><span>" + esc(label(pick(source, "dataset_name", "name", "id"))) + "</span><strong>" + esc(label(pick(source, "recovery_status", "logical_root", "path", "dataset", "state", "status"))) + "</strong></li>").join("") + "</ul>";
    } else {
      html += '<p>No protected sources have been reported.</p>';
    }
    html += '</div></section><section class="panel"><div class="panel-head"><div><h2>Required actions</h2><p>Actions returned by the overview API.</p></div></div><div class="panel-body">';
    if (overview.actions.length) {
      html += '<ul class="action-list">' + overview.actions.map(renderActionItem).join("") + "</ul>";
      if (overview.actions_truncated) {
        html += '<div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Showing the first ' + esc(overview.actions.length) + ' of ' + esc(overview.actions_total) + ' required actions</strong><span>Resolve visible items or refresh as conditions change. The total includes additional actions not shown in this bounded overview.</span></div></div>';
      }
    } else {
      html += "<p>No actions are currently reported.</p>";
    }
    return html + "</div></section></div>";
  }

  function backupRunState(run) {
    return String(pick(run, "state", "status") || "queued").toLowerCase();
  }

  function backupMediaRequestPhase(source) {
    const request = objectValue(source, "media_request");
    return String(pick(request, "phase") || "").toLowerCase();
  }

  function backupMediaRequestState(source) {
    const request = objectValue(source, "media_request");
    return String(pick(request, "state") || "").toLowerCase();
  }

  function backupMediaIsReadback(source) {
    return backupMediaRequestPhase(source) === "full_readback";
  }

  function backupWaitingForOperator(run) {
    const waiting = backupRunState(run) === "waiting_for_media" || backupNeedsNextMedia(run);
    const requestState = backupMediaRequestState(run);
    return waiting && !["authorized", "loaded"].includes(requestState);
  }

  function backupRunStateLabel(run) {
    if (backupRunState(run) === "waiting_for_media" && backupMediaRequestState(run) === "authorized") return "Tape accepted";
    if (backupRunState(run) === "waiting_for_media" && backupMediaRequestState(run) === "loaded") return "Resuming";
    if (backupRunState(run) === "waiting_for_media" && backupMediaIsReadback(run)) return "Tape needed for readback";
    if (backupRunState(run) === "waiting_for_media" && backupNeedsNextMedia(run)) return "Next tape needed";
    const labels = {
      queued: "Waiting to start",
      waiting_for_media: "Tape needed",
      waiting_for_next_media: "Next tape needed",
      next_media: "Next tape needed",
      media_required: "Next tape needed",
      preparing: "Starting backup",
      inventorying: "Scanning files",
      writing: "Writing tape",
      written: "Write complete",
      finalizing: "Closing tape",
      verifying: "Reading tape back",
      verified: "Protected",
      passed: "Protected",
      completed: "Protected",
      complete: "Protected",
      blocked: "Needs attention",
      interrupted: "Interrupted",
      failed: "Failed",
      cancelled: "Cancelled",
      canceled: "Cancelled"
    };
    return labels[backupRunState(run)] || human(backupRunState(run));
  }

  function backupRunActive(run) {
    return !BACKUP_TERMINAL_STATES.has(backupRunState(run));
  }

  function backupNeedsNextMedia(run) {
    const runState = backupRunState(run);
    if (backupMediaIsReadback(run)) return false;
    if (["waiting_for_next_media", "next_media", "media_required"].includes(runState)) return true;
    const mediaRequest = objectValue(run, "media_request");
    const phase = String(pick(mediaRequest, "phase") || "").toLowerCase();
    if (phase === "next_media") return true;
    if (runState !== "waiting_for_media") return false;
    const sequence = Number(pick(mediaRequest, "volume_order", "volume_sequence", "media_sequence") ?? backupEvidenceValue(run, "next_volume_sequence", "volume_sequence", "media_sequence"));
    return Number.isInteger(sequence) && sequence > 1;
  }

  function backupCompressionMode(source) {
    const profile = source && pick(source, "profile_id", "backup_profile_id") ? backupProfileForRun(source) : null;
    const value = String(pick(source, "compression_mode", "compression") || pick(profile, "compression_mode", "compression") || "auto").toLowerCase();
    return value || "auto";
  }

  function backupCompressionSupported(source) {
    return ["auto", "none", "gzip", "lzo"].includes(backupCompressionMode(source));
  }

  function backupCompressionLabel(source) {
    const mode = backupCompressionMode(source);
    const resolved = String(backupEvidenceValue(source, "resolved_compression") || "").toLowerCase();
    if (!backupCompressionSupported(source)) return "Unsupported (" + mode + ")";
    if (mode === "auto") return resolved && resolved !== "gzip" ? "Automatic → " + resolved.toUpperCase() : "Automatic → GZIP1 (fast)";
    if (mode === "gzip") return "GZIP6 (balanced)";
    return mode === "none" ? "No compression" : "LZO (fastest)";
  }

  function backupSpanningEnabled(source) {
    const profile = source && pick(source, "profile_id", "backup_profile_id") ? backupProfileForRun(source) : null;
    const explicit = pick(source, "spanning_enabled", "allow_media_spanning", "multi_volume") ?? pick(profile, "spanning_enabled", "allow_media_spanning", "multi_volume");
    return explicit === null ? true : explicit === true;
  }

  function backupRunVolumes(run) {
    const detail = backupRunDetail(run);
    const result = objectValue(run, "result");
    const evidence = objectValue(result, "evidence");
    const sources = [run, result, evidence, detail || {}];
    let values = [];
    for (const source of sources) {
      values = arrayValue(source, "volumes", "archive_volumes", "media_set", "volume_set", "media", "volume_labels");
      if (values.length) break;
    }
    const normalized = values.map((volume, index) => {
      if (typeof volume === "string") return { label: volume, sequence: index + 1 };
      return {
        label: String(pick(volume, "expected_cartridge_label", "cartridge_label", "engine_volume_name", "volume_name", "internal_label", "label") || "Tape " + (index + 1)),
        sequence: Number(pick(volume, "volume_order", "sequence", "volume_sequence", "media_sequence")) || index + 1,
        state: String(pick(volume, "state", "status") || "recorded")
      };
    }).sort((left, right) => left.sequence - right.sequence);
    const current = pick(run, "expected_cartridge_label", "cartridge_label") || backupEvidenceValue(run, "engine_volume_name", "volume_name", "expected_cartridge_label");
    if (current && !normalized.some((volume) => volume.label === String(current))) normalized.push({ label: String(current), sequence: normalized.length + 1, state: "current" });
    return normalized;
  }

  function backupTapeSummary(run) {
    const volumes = backupRunVolumes(run);
    if (volumes.length > 1) return volumes.length + " tapes";
    if (volumes.length === 1) return volumes[0].label;
    return backupNeedsNextMedia(run) || backupRunState(run) === "waiting_for_media" ? "Choose tape" : "Not assigned";
  }

  function backupNextVolumeSequence(run) {
    const mediaRequest = objectValue(run, "media_request");
    const explicit = Number(pick(mediaRequest, "volume_order", "volume_sequence", "media_sequence") ?? backupEvidenceValue(run, "next_volume_sequence", "volume_sequence", "media_sequence"));
    if (Number.isInteger(explicit) && explicit > 0) return explicit;
    return backupRunVolumes(run).length + 1;
  }

  function backupRunRouteId() {
    const parts = location.hash.slice(1).split("/");
    if (parts[0] !== "jobs" || !parts[1]) return null;
    try {
      return decodeURIComponent(parts.slice(1).join("/"));
    } catch (_) {
      return null;
    }
  }

  function backupRunRecord(run) {
    if (!run) return null;
    const detail = state.data.backupRunDetails[recordId(run)];
    const detailedRun = detail && (detail.run || detail.backup_run || detail.request || (recordId(detail) === recordId(run) ? detail : null));
    return detailedRun && typeof detailedRun === "object" ? Object.assign({}, detailedRun, run) : run;
  }

  function orderedBackupRuns() {
    return state.data.backupRuns.map(backupRunRecord).sort((left, right) => {
      const timeDifference = new Date(pick(right, "created_at", "started_at") || 0) - new Date(pick(left, "created_at", "started_at") || 0);
      return timeDifference || recordId(right).localeCompare(recordId(left));
    });
  }

  function selectedBackupRun() {
    const runs = orderedBackupRuns();
    const requested = backupRunRouteId() || state.selectedBackupRunId;
    const selected = requested && runs.find((run) => recordId(run) === requested);
    const run = selected || runs.find(backupRunActive) || runs[0] || null;
    if (run) state.selectedBackupRunId = recordId(run);
    return run;
  }

  async function loadBackupRunDetail(id, force) {
    if (!id || state.backupRunDetailLoading.has(id) || (!force && state.data.backupRunDetails[id])) return;
    const errorKey = "backupRun:" + id;
    state.backupRunDetailLoading.add(id);
    delete state.errors[errorKey];
    try {
      const detail = await request(endpoints.backupRuns + "/" + encodeURIComponent(id));
      state.data.backupRunDetails[id] = detail && typeof detail === "object" ? detail : {};
      const detailedRun = detail && (detail.run || detail.backup_run || detail.request || (recordId(detail) === id ? detail : null));
      if (detailedRun) rememberBackupRun(detailedRun);
      delete state.errors[errorKey];
    } catch (error) {
      state.errors[errorKey] = error;
    } finally {
      state.backupRunDetailLoading.delete(id);
    }
    if (state.route === "jobs" && recordId(selectedBackupRun()) === id) render();
  }

  async function selectBackupRun(id) {
    if (!id) return;
    state.selectedBackupRunId = id;
    const target = "#jobs/" + encodeURIComponent(id);
    if (location.hash !== target) location.hash = target;
    else render();
    await loadBackupRunDetail(id);
  }

  function rememberBackupProfile(profile) {
    if (!profile || typeof profile !== "object" || !recordId(profile)) return;
    state.data.backupProfiles = [profile].concat(state.data.backupProfiles.filter((item) => recordId(item) !== recordId(profile)));
    state.received.add("backupProfiles");
    delete state.errors.backupProfiles;
  }

  function rememberBackupRun(run) {
    if (!run || typeof run !== "object" || !recordId(run)) return;
    state.data.backupRuns = [run].concat(state.data.backupRuns.filter((item) => recordId(item) !== recordId(run)));
    state.received.add("backupRuns");
    delete state.errors.backupRuns;
  }

  function backupProfileForRun(run) {
    const id = String(pick(run, "profile_id", "backup_profile_id") || "");
    return state.data.backupProfiles.find((profile) => recordId(profile) === id) || null;
  }

  function backupProfileName(profile) {
    return String(pick(profile, "name", "profile_name") || "Unnamed snapshot backup");
  }

  function backupRunProfileName(run) {
    return String(pick(run, "profile_name", "name") || (backupProfileForRun(run) && backupProfileName(backupProfileForRun(run))) || "Backup run");
  }

  function backupRunDetail(run) {
    return run ? state.data.backupRunDetails[recordId(run)] || null : null;
  }

  function backupRunEvents(run) {
    const detail = backupRunDetail(run);
    return detail ? list(detail, "events") : [];
  }

  function backupRunArtifacts(run) {
    const detail = backupRunDetail(run);
    return detail ? list(detail, "artifacts") : [];
  }

  function backupEvidenceValue(run, ...keys) {
    const result = objectValue(run, "result");
    const resultEvidence = objectValue(result, "evidence");
    const sources = [run, result, resultEvidence];
    for (const source of sources) {
      const value = pick(source, ...keys);
      if (value !== null) return value;
    }
    for (const event of backupRunEvents(run).slice().reverse()) {
      const value = pick(objectValue(event, "evidence"), ...keys);
      if (value !== null) return value;
    }
    for (const artifact of backupRunArtifacts(run).slice().reverse()) {
      const value = pick(objectValue(artifact, "metadata"), ...keys) || pick(artifact, ...keys);
      if (value !== null) return value;
    }
    return null;
  }

  function renderBackupSpoolingCue(run) {
    if (backupRunState(run) !== "writing") return "";
    const spooling = backupEvidenceValue(run, "data_spooling");
    const details = spooling && typeof spooling === "object" && !Array.isArray(spooling) ? spooling : {};
    const effectiveMaximum = Number(pick(details, "effective_maximum_bytes", "maximum_bytes"));
    const limit = Number.isFinite(effectiveMaximum) && effectiveMaximum > 0
      ? " Effective disk-buffer maximum for this run: " + bytes(effectiveMaximum) + "."
      : "";
    return '<div class="notice info backup-spooling-cue"><span class="notice-icon" aria-hidden="true">⇄</span><div class="notice-copy"><strong>Buffering on disk, then streaming to tape</strong><span>Bacula is using a bounded disk buffer to sustain the tape stream. The drive may remain idle while the buffer initially fills; once despooling begins, it should stream continuously for that buffered segment.' + esc(limit) + ' This buffer does not limit the total archive size.</span></div></div>';
  }

  function backupPhaseIndex(run) {
    const runState = backupRunState(run);
    if (["verified", "passed", "completed", "complete"].includes(runState)) return BACKUP_PHASES.length - 1;
    if (backupMediaIsReadback(run)) return BACKUP_PHASES.findIndex((phase) => phase.aliases.includes("readback"));
    if (backupNeedsNextMedia(run)) return BACKUP_PHASES.findIndex((phase) => phase.aliases.includes("next_media"));
    const explicit = Number(backupEvidenceValue(run, "phase_index"));
    if (Number.isInteger(explicit) && explicit >= 0 && explicit < BACKUP_PHASES.length) return explicit;
    const evidencePhase = String(backupEvidenceValue(run, "phase", "event_type") || runState).toLowerCase();
    const fromEvidence = BACKUP_PHASES.findIndex((phase) => phase.aliases.includes(evidencePhase));
    if (fromEvidence >= 0) return fromEvidence;
    const events = backupRunEvents(run).slice().reverse();
    for (const event of events) {
      const eventName = String(pick(event, "event_type", "to_state", "phase") || "").toLowerCase();
      const eventIndex = BACKUP_PHASES.findIndex((phase) => phase.aliases.includes(eventName));
      if (eventIndex >= 0) return eventIndex;
    }
    return 0;
  }

  function renderBackupPhaseTrack(run) {
    const current = backupPhaseIndex(run);
    const completed = ["verified", "passed", "completed", "complete"].includes(backupRunState(run));
    const failed = BACKUP_TERMINAL_STATES.has(backupRunState(run)) && !completed;
    return '<div class="phase-track backup-phase-track" aria-label="Backup phases">' + BACKUP_PHASES.map((phase, index) => {
      const phaseClass = completed || index < current ? " done" : index === current ? failed ? " failed" : " current" : "";
      const marker = completed || index < current ? "✓" : index + 1;
      return '<div class="phase' + phaseClass + '"' + (index === current && !completed ? ' aria-current="step"' : "") + '><span class="phase-marker">' + esc(marker) + '</span><strong>' + esc(phase.name) + '</strong><small>' + esc(phase.detail) + "</small></div>";
    }).join("") + "</div>";
  }

  function backupProgressPercent(run) {
    const reported = backupEvidenceValue(run, "progress_percent", "percent_complete");
    const explicit = Number(reported);
    if (reported !== null && reported !== "" && Number.isFinite(explicit) && explicit >= 0 && explicit <= 100) return explicit;
    if (["verified", "passed", "completed", "complete"].includes(backupRunState(run))) return 100;
    return null;
  }

  function backupReadbackSummary(run) {
    const runState = backupRunState(run);
    if (["verified", "passed", "completed", "complete"].includes(runState)) return "Readback complete";
    if (runState === "verifying") return "Readback in progress";
    if (BACKUP_TERMINAL_STATES.has(runState)) return "Readback not completed";
    return "Complete readback planned";
  }

  function backupFailureDetail(run) {
    const failure = pick(run, "failure_detail", "failure_class", "detail", "message") || backupEvidenceValue(run, "failure_detail", "detail", "message", "reason");
    return failure && typeof failure === "object" ? JSON.stringify(failure) : failure ? String(failure) : null;
  }

  function backupRunMatches(run) {
    const runState = backupRunState(run);
    if (state.backupStateFilter !== "all" && runState !== state.backupStateFilter) return false;
    const query = state.backupQuery.trim().toLowerCase();
    if (!query) return true;
    return [
      recordId(run), backupRunProfileName(run), pick(run, "source_subpath", "source_path", "logical_root"),
      pick(run, "expected_cartridge_label", "cartridge_label"), pick(run, "expected_stable_id", "drive_stable_id"),
      runState, pick(run, "recovery_point_id")
    ].filter(Boolean).join(" ").toLowerCase().includes(query);
  }

  function applyBackupFilters() {
    const tableBody = document.querySelector("#backup-run-rows");
    if (!tableBody) return;
    let visible = 0;
    tableBody.querySelectorAll("tr[data-backup-run]").forEach((row) => {
      const queryMatches = !state.backupQuery.trim() || String(row.dataset.search || "").includes(state.backupQuery.trim().toLowerCase());
      const stateMatches = state.backupStateFilter === "all" || row.dataset.state === state.backupStateFilter;
      row.hidden = !(queryMatches && stateMatches);
      if (!row.hidden) visible += 1;
    });
    const countNode = document.querySelector("#backup-filter-count");
    if (countNode) countNode.textContent = visible + " of " + state.data.backupRuns.length + " runs";
    const emptyNode = document.querySelector("#backup-filter-empty");
    if (emptyNode) emptyNode.hidden = visible !== 0;
  }

  function renderBackupProfiles() {
    const profiles = state.data.backupProfiles;
    const cards = profiles.map((profile) => {
      const id = recordId(profile);
      const latest = orderedBackupRuns().find((run) => String(pick(run, "profile_id", "backup_profile_id") || "") === id);
      const active = orderedBackupRuns().find((run) => String(pick(run, "profile_id", "backup_profile_id") || "") === id && backupRunActive(run));
      const source = pick(profile, "source_subpath", "source_path", "logical_root") || "Source not reported";
      const retention = count(pick(profile, "retention_days")) || 28;
      const pool = pick(profile, "media_pool_name") || "any eligible scratch pool";
      const compressionSupported = backupCompressionSupported(profile);
      const archivePolicy = backupCompressionLabel(profile) + " · " + (backupSpanningEnabled(profile) ? "spans tapes as needed" : "one tape maximum");
      const action = active
        ? '<button class="button button-small button-primary" type="button" data-action="select-backup-run" data-id="' + esc(recordId(active)) + '">' + (backupWaitingForOperator(active) ? "Continue backup" : "View progress") + '</button>'
        : '<button class="button button-small button-primary" type="button" data-action="start-backup" data-profile-id="' + esc(id) + '"' + (pick(profile, "enabled") === false || !compressionSupported ? ' disabled title="Create a new backup plan with a supported compression mode."' : "") + '>' + (latest ? "Back up again" : "Start backup") + '</button>';
      const compatibility = compressionSupported ? "" : '<small class="backup-profile-warning">This saved plan uses an unsupported legacy compression mode. Create a new snapshot backup plan before starting it.</small>';
      return '<article class="backup-profile-card"><div class="backup-profile-mark" aria-hidden="true">◆</div><div class="backup-profile-copy"><span><strong>' + esc(backupProfileName(profile)) + '</strong>' + status(human(pick(profile, "enabled") === false ? "disabled" : compressionSupported ? "snapshot saved" : "needs new plan")) + '</span><code>' + esc(source) + '</code><small>Keep verified backup ' + retention + " days · " + esc(pool) + '</small><small>' + esc(archivePolicy) + '</small>' + compatibility + '</div><div class="backup-profile-actions"><span>' + (latest ? esc(backupRunStateLabel(latest)) + " · " + esc(date(pick(latest, "finished_at", "created_at", "started_at"))) : "Not backed up yet") + "</span>" + action + "</div></article>";
    }).join("");
    return '<section class="panel backup-profiles"><div class="panel-head"><div><h2>Saved snapshot backups</h2><p>Availability and read-only state are checked again before every new request.</p></div><button class="button button-small" type="button" data-action="create-backup-profile">Choose snapshot</button></div><div class="backup-profile-list">' + (cards || '<div class="media-inline-empty">No snapshot has been selected yet. Choose a named ZFS snapshot folder to begin.</div>') + "</div></section>";
  }

  function renderBackupRunTable(runs) {
    const rows = runs.map((run) => {
      const id = recordId(run);
      const source = pick(run, "source_subpath", "source_path", "logical_root") || pick(backupProfileForRun(run), "source_subpath", "source_path", "logical_root");
      const tape = backupTapeSummary(run);
      const logical = backupEvidenceValue(run, "logical_bytes", "source_bytes");
      const progress = backupProgressPercent(run);
      const searchable = [id, backupRunProfileName(run), source, tape, backupRunState(run)].filter(Boolean).join(" ").toLowerCase();
      return '<tr class="row-action" tabindex="0" data-action="select-backup-run" data-backup-run data-id="' + esc(id) + '" data-state="' + esc(backupRunState(run)) + '" data-search="' + esc(searchable) + '"' + (backupRunMatches(run) ? "" : " hidden") + '><td class="primary-cell" data-label="Backup"><strong>' + esc(backupRunProfileName(run)) + '</strong><small>' + esc(label(source)) + '</small></td><td data-label="Status">' + status(backupRunStateLabel(run)) + (progress == null ? "" : '<small class="backup-table-progress">' + progress + "%</small>") + '</td><td data-label="Tape">' + esc(tape) + '</td><td data-label="Data">' + esc(logical == null ? "Pending" : bytes(logical)) + '</td><td data-label="Activity">' + esc(date(pick(run, "finished_at", "started_at", "created_at"))) + "</td></tr>";
    }).join("");
    const filterOptions = ["queued", "waiting_for_media", "waiting_for_next_media", "preparing", "inventorying", "writing", "written", "verifying", "verified", "failed", "blocked", "interrupted", "cancelled"].map((value) => '<option value="' + value + '"' + (state.backupStateFilter === value ? " selected" : "") + '>' + esc(backupRunStateLabel({ state: value })) + "</option>").join("");
    const visible = runs.filter(backupRunMatches).length;
    return '<section class="panel backup-run-table">' +
      '<div class="table-toolbar"><div class="toolbar-start">' +
      '<label class="search-field"><span class="sr-only">Search backup history</span><input id="backup-search" type="search" placeholder="Search backup, snapshot, or tape" value="' + esc(state.backupQuery) + '"></label>' +
      '<select class="select compact" id="backup-state-filter" aria-label="Filter backup runs by state"><option value="all">All states</option>' + filterOptions + "</select>" +
      '</div><div class="toolbar-end"><span id="backup-filter-count">' + visible + " of " + runs.length + " runs</span></div></div>" +
      '<div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Backup</th><th>Status</th><th>Tape</th><th>Data</th><th>Activity</th></tr></thead><tbody id="backup-run-rows">' + rows + "</tbody></table>" +
      '<div class="media-filter-empty" id="backup-filter-empty"' + (visible ? " hidden" : "") + ">No runs match the current filters.</div></div></section>";
  }

  function renderBackupEvidence(run) {
    const logical = backupEvidenceValue(run, "logical_bytes", "source_bytes");
    const encoded = backupEvidenceValue(run, "encoded_bytes", "media_bytes", "bytes_written");
    const throughput = backupEvidenceValue(run, "throughput_bytes_per_second", "bytes_per_second");
    const volumes = backupRunVolumes(run);
    const values = [
      ["Source entries", backupEvidenceValue(run, "source_entry_count", "entry_count", "file_count")],
      ["Logical bytes", logical == null ? null : bytes(logical)],
      ["Encoded bytes", encoded == null ? null : bytes(encoded)],
      ["Throughput", throughput == null ? null : bytes(throughput) + "/s"],
      ["Source manifest", backupEvidenceValue(run, "source_manifest_sha256", "source_manifest_digest", "manifest_sha256")],
      ["Bacula JobId", backupEvidenceValue(run, "backup_job_id", "engine_job_id", "bacula_job_id", "engine_operation_id")],
      ["Compression", backupCompressionLabel(run)],
      ["Tapes used", volumes.length || null],
      ["Archive volumes", volumes.length ? volumes.map((volume) => volume.sequence + ". " + volume.label).join(" · ") : null],
      ["Recovery point", backupEvidenceValue(run, "recovery_point_id") || pick(run, "recovery_point_id")]
    ].filter((item) => item[1] !== null && item[1] !== undefined && item[1] !== "");
    if (!values.length) return '<p class="recovery-evidence-pending">Evidence will appear as the source inventory, tape write, and readback complete.</p>';
    return '<div class="backup-evidence-grid">' + values.map(([name, value]) => '<div><span>' + esc(name) + '</span><strong' + (/manifest/i.test(name) ? ' class="digest-value"' : "") + '>' + esc(value) + "</strong></div>").join("") + "</div>";
  }

  function renderBackupOutcome(run) {
    const entries = backupEvidenceValue(run, "source_entry_count", "entry_count", "file_count");
    const logical = backupEvidenceValue(run, "logical_bytes", "source_bytes");
    const volumes = backupRunVolumes(run);
    const tape = volumes.length ? volumes.map((volume) => volume.label).join(", ") : backupTapeSummary(run);
    const runState = backupRunState(run);
    const successful = ["verified", "passed", "completed", "complete"].includes(runState);
    const unsuccessful = BACKUP_TERMINAL_STATES.has(runState) && !successful;
    const measurement = (value, formatter) => value == null ? unsuccessful ? "Not measured" : successful ? "Not reported" : ["queued", "waiting_for_media", "preparing"].includes(runState) ? "Pending" : "Measuring…" : formatter(value);
    const readback = successful ? "Complete" : runState === "verifying" ? "In progress" : unsuccessful ? "Not completed" : "Pending";
    const values = [
      ["Files and folders", measurement(entries, (value) => value)],
      ["Data", measurement(logical, bytes)],
      [volumes.length === 1 ? "Tape" : "Archive tapes", tape || "Not selected"],
      ["Compression", backupCompressionLabel(run)],
      ["Physical readback", readback]
    ];
    return '<div class="backup-evidence-grid backup-outcome-grid">' + values.map(([name, value]) => '<div><span>' + esc(name) + '</span><strong>' + esc(value) + "</strong></div>").join("") + "</div>";
  }

  function renderBackupTimeline(run) {
    const events = backupRunEvents(run);
    if (!events.length) return '<div class="media-inline-empty">No detailed activity has been returned yet.</div>';
    return '<ol class="timeline backup-timeline">' + events.map((event) => {
      const evidence = objectValue(event, "evidence");
      const detail = pick(evidence, "detail", "message", "reason", "cartridge_label") || pick(event, "reason", "detail") || "Evidence recorded";
      return '<li class="timeline-item"><time class="timeline-time">' + esc(date(pick(event, "occurred_at", "created_at"))) + '</time><span class="timeline-marker" aria-hidden="true"></span><div class="timeline-copy"><strong>' + esc(human(pick(event, "event_type", "to_state", "phase") || "event")) + '</strong><span>' + esc(detail) + "</span></div></li>";
    }).join("") + "</ol>";
  }

  function renderBackupArtifacts(run) {
    const artifacts = backupRunArtifacts(run);
    if (!artifacts.length) return "";
    return '<section class="backup-detail-section"><div class="backup-section-head"><div><span>Retained evidence</span><h3>Artifacts</h3></div></div><ul class="detail-list recovery-artifact-list">' + artifacts.map((artifact) => {
      const digest = pick(artifact, "sha256", "digest_sha256", "digest");
      const size = pick(artifact, "size_bytes");
      return '<li><span><strong>' + esc(human(pick(artifact, "artifact_kind", "kind") || "artifact")) + '</strong><small>' + esc(size == null ? label(pick(artifact, "storage_ref", "path")) : bytes(size)) + '</small></span><strong class="digest-value">' + esc(label(digest)) + "</strong></li>";
    }).join("") + "</ul></section>";
  }

  function backupOperatorFailure(run) {
    const raw = String(backupFailureDetail(run) || "");
    if (!raw) return "TapeScale stopped before it could create a verified recovery point. Open technical details for the recorded cause.";
    if (/snapshot|source|read.?only|mount/i.test(raw)) return "The selected snapshot is no longer available exactly as saved. Restore its read-only mount, then create a new backup request.";
    if (/qualif|btape/i.test(raw)) return "The loaded drive and tape no longer have a current setup test. Prepare the tape again before retrying.";
    if (/permission|access denied|os error 13/i.test(raw)) return "TapeScale could not read the source or write its persistent work area. Check the mounted paths and container permissions.";
    if (/drive|device|medium|cartridge|volume|tape/i.test(raw)) return "The drive or tape changed while the backup was running. Check the loaded tape and drive status before retrying.";
    if (/bacula|catalog|postgres|sql/i.test(raw)) return "The backup engine or its catalog rejected this run. No verified recovery point was created.";
    return raw.length > 280 ? raw.slice(0, 277) + "…" : raw;
  }

  function renderBackupRunDetail(run) {
    if (!run) return '<section class="panel backup-run-detail backup-run-detail-empty"><span aria-hidden="true">▶</span><h2>Backup not found</h2><p>Return to the Backup Center and choose a retained backup.</p></section>';
    const id = recordId(run);
    const runState = backupRunState(run);
    const progress = backupProgressPercent(run);
    const detailError = state.errors["backupRun:" + id];
    const waiting = runState === "waiting_for_media" || backupNeedsNextMedia(run);
    const readbackMedia = waiting && backupMediaIsReadback(run);
    const nextMedia = backupNeedsNextMedia(run);
    const mediaRequestState = backupMediaRequestState(run);
    const mediaAccepted = waiting && ["authorized", "loaded"].includes(mediaRequestState);
    const allowedActions = objectValue(run, "allowed_actions");
    const canAssign = waiting && pick(allowedActions, readbackMedia ? "assign_media" : nextMedia ? "assign_next_media" : "assign_media") === true;
    const canCancel = backupRunActive(run) && pick(allowedActions, "cancel") === true;
    const successful = ["verified", "passed", "completed", "complete"].includes(runState);
    const failed = BACKUP_TERMINAL_STATES.has(runState) && !successful && !["cancelled", "canceled"].includes(runState);
    const failure = backupOperatorFailure(run);
    const recoveryPointId = String(backupEvidenceValue(run, "recovery_point_id") || pick(run, "recovery_point_id") || "");
    let notice = "";
    if (mediaAccepted) notice = '<div class="notice info backup-run-notice"><span class="notice-icon" aria-hidden="true">✓</span><div class="notice-copy"><strong>' + (mediaRequestState === "loaded" ? "Tape loaded; resuming job" : "Tape accepted; worker is rechecking it") + '</strong><span>No further tape selection is needed for this request. Keep the drive and loaded tape available while the same Bacula job resumes.</span></div></div>';
    else if (waiting && readbackMedia) notice = '<div class="notice warning backup-run-notice"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Load tape ' + esc(backupNextVolumeSequence(run)) + ' for readback</strong><span>The archive write is complete. Unload and remove the current tape, then insert the exact recorded tape shown next; TapeScale will read it without erasing or relabeling it.</span></div>' + (canAssign ? '<button class="button button-primary" type="button" data-action="assign-backup-media" data-id="' + esc(id) + '">Load tape for readback</button>' : "") + "</div>";
    else if (waiting) notice = '<div class="notice warning backup-run-notice"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>' + (nextMedia ? "Load tape " + esc(backupNextVolumeSequence(run)) : "Choose the first tape for this backup") + '</strong><span>' + (nextMedia ? "The archive filled its previous tape. Unload and remove it, then insert and confirm a different prepared scratch tape to continue the same backup." : "Confirm the loaded scratch tape and authorize its overwrite. Nothing has been written yet.") + '</span></div>' + (canAssign ? '<button class="button button-primary" type="button" data-action="assign-backup-media" data-id="' + esc(id) + '">' + (nextMedia ? "Load next tape" : "Continue with tape") + '</button>' : "") + "</div>";
    else if (successful) notice = '<div class="notice success backup-run-notice"><span class="notice-icon" aria-hidden="true">✓</span><div class="notice-copy"><strong>Backup verified and cataloged for recovery</strong><span>Every tape in the archive was written, completely read back, and retained as one recovery point. Restore testing remains a separate assurance step.</span></div>' + (recoveryPointId ? '<div class="backup-complete-actions"><button class="button button-primary button-small" type="button" data-action="browse-backup-files" data-id="' + esc(recoveryPointId) + '">Browse backed-up files</button><button class="button button-small" type="button" data-action="select-recovery-point" data-id="' + esc(recoveryPointId) + '">Recovery options</button></div>' : "") + "</div>";
    else if (failed) notice = '<div class="notice danger backup-run-notice"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>' + esc(backupRunStateLabel(run)) + '</strong><span>' + esc(failure) + "</span></div></div>";
    else if (["cancelled", "canceled"].includes(runState)) notice = '<div class="notice warning backup-run-notice"><span class="notice-icon" aria-hidden="true">×</span><div class="notice-copy"><strong>Backup cancelled</strong><span>' + esc(failure || "The attempt did not establish a recovery point. Previously valid backups remain unchanged.") + "</span></div></div>";
    else notice = '<div class="notice info backup-run-notice"><span class="notice-icon" aria-hidden="true">▶</span><div class="notice-copy"><strong>' + esc(backupRunStateLabel(run)) + '</strong><span>Keep the snapshot, worker, drive, and requested archive tapes available while this stage finishes.</span></div></div>';
    const cancel = canCancel ? '<button class="button button-danger button-small" type="button" data-action="cancel-backup-run" data-id="' + esc(id) + '">Cancel run</button>' : "";
    const volumes = backupRunVolumes(run);
    const assignment = volumes.length
      ? '<span><strong>' + esc(volumes.length + " tape" + (volumes.length === 1 ? "" : "s")) + '</strong><small class="backup-volume-summary">' + volumes.map((volume) => esc(volume.sequence + ". " + volume.label)).join(" · ") + "</small></span>"
      : '<span><strong>Not selected</strong><small>Choose a prepared scratch tape</small></span>';
    const detailFailure = detailError ? '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not load retained run evidence</strong><span>' + esc(errorMessage(detailError)) + '</span></div><button class="button button-small" type="button" data-action="retry-backup-run" data-id="' + esc(id) + '">Retry</button></div>' : "";
    const measuredProgress = progress == null ? "" : '<div class="progress backup-progress"><div class="progress-track"><div class="progress-value" style="width:' + progress + '%"></div></div><div class="progress-meta"><span>Measured progress</span><span>' + progress + "%</span></div></div>";
    const technical = '<details class="backup-technical"><summary><span><strong>Technical details</strong><small>Request identity, engine evidence, and complete activity history</small></span></summary><div class="backup-technical-body"><dl class="vault-facts"><dt>Request ID</dt><dd class="mono">' + esc(id) + '</dd><dt>Created</dt><dd>' + esc(date(pick(run, "created_at", "started_at"))) + '</dd><dt>Raw state</dt><dd>' + esc(runState) + '</dd>' + (backupFailureDetail(run) ? '<dt>Recorded failure</dt><dd>' + esc(backupFailureDetail(run)) + "</dd>" : "") + '</dl><section class="backup-detail-section"><div class="backup-section-head"><div><span>Engine evidence</span><h3>Recorded facts</h3></div></div>' + renderBackupEvidence(run) + '</section><section class="backup-detail-section"><div class="backup-section-head"><div><span>Complete history</span><h3>Activity</h3></div></div>' + renderBackupTimeline(run) + "</section>" + renderBackupArtifacts(run) + "</div></details>";
    return '<article class="panel backup-run-detail"><header class="job-header backup-job-header"><div><div class="job-kicker"><span class="backup-kind-badge">Full backup</span><span class="backup-kind-badge secondary">' + esc(backupCompressionLabel(run)) + '</span>' + status(backupRunStateLabel(run)) + '</div><h1>' + esc(backupRunProfileName(run)) + '</h1><p>Started ' + esc(date(pick(run, "created_at", "started_at"))) + '</p></div><div class="job-summary"><div><span>Source snapshot</span><strong>' + esc(label(pick(run, "source_subpath", "source_path", "logical_root") || pick(backupProfileForRun(run), "source_subpath", "source_path", "logical_root"))) + '</strong></div><div><span>Archive tapes</span>' + assignment + '</div><div><span>Media policy</span><strong>' + esc(backupSpanningEnabled(run) ? "Span as needed" : "Single tape") + '</strong></div><div><span>Verification</span><strong>' + esc(backupReadbackSummary(run)) + '</strong></div></div></header>' + renderBackupPhaseTrack(run) + '<div class="backup-run-actions">' + cancel + '<button class="button button-quiet button-small" type="button" data-action="refresh">Refresh status</button></div><div class="panel-body">' + detailFailure + notice + renderBackupSpoolingCue(run) + measuredProgress + '<section class="backup-detail-section backup-result-section"><div class="backup-section-head"><div><span>At a glance</span><h3>Backup result</h3></div></div>' + renderBackupOutcome(run) + "</section>" + technical + "</div></article>";
  }

  function renderJobs() {
    const profiles = state.data.backupProfiles;
    const runs = orderedBackupRuns();
    const selected = selectedBackupRun();
    const routeId = backupRunRouteId();
    if (routeId) {
      const back = '<a class="button button-quiet" href="#jobs">← Back to Backup Center</a>';
      const heading = selected && recordId(selected) === routeId ? backupRunProfileName(selected) : "Backup progress";
      return pageHead(heading, "Live status, next action, and verified result for this backup.", back + refreshButton()) + noticeFor("backupRuns") + '<section class="section backup-run-route">' + renderBackupRunDetail(selected && recordId(selected) === routeId ? selected : null) + "</section>";
    }
    const active = runs.filter(backupRunActive).length;
    const waiting = runs.filter(backupWaitingForOperator).length;
    const failed = runs.filter((run) => ["failed", "blocked", "interrupted"].includes(backupRunState(run))).length;
    const latestVerified = runs.find((run) => ["verified", "passed", "completed", "complete"].includes(backupRunState(run)));
    const create = '<button class="button button-primary" type="button" data-action="create-backup-profile">Choose snapshot</button>';
    let html = pageHead("Backup Center", "Choose an existing ZFS snapshot, select compression, and follow a one- or multi-tape archive through complete readback.", refreshButton() + create);
    html += noticeFor("backupProfiles") + noticeFor("backupRuns");
    html += '<div class="notice info backup-scope-notice"><span class="notice-icon" aria-hidden="true">i</span><div class="notice-copy"><strong>One exact snapshot per saved item</strong><span>This build copies an existing read-only ZFS snapshot; it does not create a fresh snapshot on each run yet. Full backups may be compressed and span as many prepared tapes as the archive requires.</span></div></div>';
    html += '<section class="panel metric-strip backup-metrics"><div class="metric"><span>Saved snapshots</span><strong>' + profiles.length + '</strong></div><div class="metric"><span>Backups running</span><strong>' + active + '</strong></div><div class="metric"><span>Tape needed</span><strong>' + waiting + '</strong></div><div class="metric"><span>Latest protected</span><strong class="backup-metric-date">' + esc(latestVerified ? date(pick(latestVerified, "finished_at", "created_at")) : "None") + "</strong></div></section>";
    html += renderBackupProfiles();
    if (!runs.length) return html + empty("No backups yet", state.errors.backupRuns ? "Backup history could not be loaded." : "Choose a snapshot, then continue through tape selection to start the first backup.", create);
    html += '<section class="section"><div class="section-head"><div><h2>Backup history</h2><p>Running backups and every retained result.</p></div><span class="status ' + (failed ? "failed" : active ? "running" : "ready") + '">' + (failed ? failed + " need review" : active ? active + " active" : "No active work") + "</span></div>" + renderBackupRunTable(runs) + "</section>";
    return html;
  }

  function mediaName(item) {
    return pick(item, "internal_label", "internalLabel", "label", "id");
  }

  function objectValue(source, ...keys) {
    const value = pick(source, ...keys);
    return value && typeof value === "object" && !Array.isArray(value) ? value : {};
  }

  function arrayValue(source, ...keys) {
    if (!source || typeof source !== "object") return [];
    for (const key of keys) {
      if (Array.isArray(source[key])) return source[key];
    }
    return [];
  }

  function mediaRouteId() {
    const parts = location.hash.slice(1).split("/");
    if (parts[0] !== "tapes" || !parts[1]) return null;
    try {
      return decodeURIComponent(parts.slice(1).join("/"));
    } catch (_) {
      return null;
    }
  }

  function mediaOperationalState(item) {
    const explicit = pick(item, "reuse_state", "operational_state", "state", "health_status", "status");
    if (explicit) return String(explicit).toLowerCase();
    if (count(pick(item, "active_holds", "hold_count")) > 0) return "held";
    if (mediaRecoveryPointCount(item) > 0) return "protected";
    if (pick(item, "has_content") === true) return "needs_evaluation";
    return "available";
  }

  function mediaRecoveryPointCount(item) {
    const retained = count(pick(item, "retained_recovery_points", "retained_recovery_point_count"));
    const dependent = count(pick(item, "dependent_recovery_points", "dependent_recovery_point_count"));
    return retained + dependent;
  }

  function mediaProtectionState(item) {
    if (count(pick(item, "active_holds", "hold_count")) > 0) return "held";
    if (pick(item, "dependency_graph_truncated") === true || mediaRecoveryPointCount(item) > 0 || count(pick(item, "indefinite_archive_claims", "indefinite_claims")) > 0 || mediaOperationalState(item) === "protected") return "protected";
    return "unprotected";
  }

  function mediaPoolId(item) {
    const pool = objectValue(item, "pool");
    return String(pick(item, "pool_id") || pick(pool, "id", "uuid") || "unassigned");
  }

  function mediaPoolName(item) {
    const pool = objectValue(item, "pool");
    return pick(item, "pool_name") || pick(pool, "name") || "Unassigned";
  }

  function mediaLocationId(item) {
    const locationRecord = objectValue(item, "location", "confirmed_location");
    return String(pick(item, "confirmed_location_id", "location_id") || pick(locationRecord, "id", "uuid") || "unknown");
  }

  function mediaLocationName(item) {
    const locationRecord = objectValue(item, "location", "confirmed_location");
    return pick(item, "confirmed_location_name", "location_name") || pick(locationRecord, "name") || "Unknown / unlocated";
  }

  function orderedMedia() {
    return state.data.media.slice().sort((left, right) => {
      const leftAttention = ["held", "physically_blocked", "blocked"].includes(mediaOperationalState(left)) ? 0 : 1;
      const rightAttention = ["held", "physically_blocked", "blocked"].includes(mediaOperationalState(right)) ? 0 : 1;
      return leftAttention - rightAttention || String(mediaName(left) || "").localeCompare(String(mediaName(right) || ""));
    });
  }

  function selectedMedia() {
    const inventory = orderedMedia();
    const requested = mediaRouteId() || state.selectedMediaId;
    const selected = requested && inventory.find((item) => recordId(item) === requested || String(mediaName(item)) === requested);
    const item = selected || inventory[0] || null;
    if (item) state.selectedMediaId = recordId(item);
    return item;
  }

  function mediaDetail(id) {
    return id ? state.data.mediaDetails[id] || null : null;
  }

  async function loadMediaDetail(id, force) {
    if (!id || state.mediaDetailLoading.has(id) || (!force && mediaDetail(id))) return;
    const errorKey = "mediaDetail:" + id;
    state.mediaDetailLoading.add(id);
    delete state.errors[errorKey];
    try {
      const payload = await request(endpoints.media + "/" + encodeURIComponent(id));
      const detail = payload && payload.media && typeof payload.media === "object" ? payload.media : payload;
      state.data.mediaDetails[id] = detail && typeof detail === "object" ? detail : {};
      delete state.errors[errorKey];
    } catch (error) {
      state.errors[errorKey] = error;
    } finally {
      state.mediaDetailLoading.delete(id);
    }
    if (state.route === "tapes" && recordId(selectedMedia()) === id) render();
  }

  async function selectMedia(id) {
    if (!id) return;
    state.selectedMediaId = id;
    const target = "#tapes/" + encodeURIComponent(id);
    if (location.hash !== target) location.hash = target;
    else render();
    await loadMediaDetail(id);
  }

  function mediaFilterValue(value) {
    return String(value == null ? "" : value).toLowerCase().replace(/[^a-z0-9._:-]+/g, "-");
  }

  function mediaSearchText(item) {
    const latestVerification = objectValue(item, "latest_verification");
    return [
      recordId(item), mediaName(item), pick(item, "barcode", "serial_number"),
      pick(item, "media_generation", "generation"), pick(item, "media_format", "format"),
      mediaPoolName(item), mediaLocationName(item), mediaOperationalState(item),
      pick(item, "identity_status"), pick(item, "health_status", "health"), pick(item, "on_tape_identity"),
      pick(item, "catalog_reconciled"), pick(item, "dependency_graph_truncated"),
      pick(item, "latest_verification_outcome", "verification_outcome") || pick(latestVerification, "outcome", "status")
    ].filter(Boolean).join(" ").toLowerCase();
  }

  function applyMediaFilters() {
    const catalog = document.querySelector("#media-inventory-list");
    if (!catalog) return;
    const query = state.mediaQuery.trim().toLowerCase();
    let visible = 0;
    catalog.querySelectorAll(".media-inventory-item").forEach((button) => {
      const matches = (!query || String(button.dataset.search || "").includes(query)) &&
        (state.mediaPoolFilter === "all" || button.dataset.pool === state.mediaPoolFilter) &&
        (state.mediaStatusFilter === "all" || button.dataset.status === state.mediaStatusFilter) &&
        (state.mediaLocationFilter === "all" || button.dataset.location === state.mediaLocationFilter) &&
        (state.mediaIdentityFilter === "all" || button.dataset.identity === state.mediaIdentityFilter) &&
        (state.mediaHealthFilter === "all" || button.dataset.health === state.mediaHealthFilter) &&
        (state.mediaProtectionFilter === "all" || button.dataset.protection === state.mediaProtectionFilter);
      button.hidden = !matches;
      if (matches) visible += 1;
    });
    const node = document.querySelector("#media-filter-count");
    if (node) node.textContent = visible + " of " + state.data.media.length + " cartridges";
    const noResults = document.querySelector("#media-filter-empty");
    if (noResults) noResults.hidden = visible !== 0;
  }

  function renderMediaPools() {
    const pools = state.data.mediaPools;
    const cards = pools.map((pool) => {
      const id = recordId(pool);
      const cartridges = pick(pool, "cartridge_count", "media_count");
      const inferred = state.data.media.filter((item) => mediaPoolId(item) === id).length;
      const purpose = pick(pool, "purpose") || "routine";
      const indefinite = pick(pool, "indefinite_retention") === true;
      return '<article class="media-pool-card"><div><span class="media-pool-mark" aria-hidden="true">' + (indefinite ? "∞" : "P") + '</span><span><small>' + esc(human(purpose)) + '</small><strong>' + esc(label(pick(pool, "name"))) + '</strong></span></div><div><strong>' + esc(cartridges == null ? inferred : count(cartridges)) + '</strong><small>cartridges</small></div></article>';
    }).join("");
    return '<section class="panel media-pools"><div class="panel-head"><div><h2>Retention pools</h2><p>Administrative grouping and retention intent. Assignment never changes tape content.</p></div><button class="button button-small" type="button" data-action="create-media-pool">Create pool</button></div><div class="media-pool-grid">' + (cards || '<div class="media-inline-empty">No pools yet. Create one to group cartridges by retention purpose.</div>') + "</div></section>";
  }

  function renderMediaInventoryItem(item, selectedId) {
    const id = recordId(item);
    const active = id === selectedId;
    const stateValue = mediaOperationalState(item);
    const protectionCount = mediaRecoveryPointCount(item);
    const holds = count(pick(item, "active_holds", "hold_count"));
    const format = pick(item, "media_format", "format") || (pick(item, "media_generation", "generation") ? "LTO-" + pick(item, "media_generation", "generation") : null);
    return '<button class="media-inventory-item' + (active ? " active" : "") + '" type="button" aria-pressed="' + (active ? "true" : "false") + '" data-action="select-media" data-id="' + esc(id) + '" data-search="' + esc(mediaSearchText(item)) + '" data-pool="' + esc(mediaFilterValue(mediaPoolId(item))) + '" data-status="' + esc(mediaFilterValue(stateValue)) + '" data-location="' + esc(mediaFilterValue(mediaLocationId(item))) + '" data-identity="' + esc(mediaFilterValue(pick(item, "identity_status") || "unknown")) + '" data-health="' + esc(mediaFilterValue(pick(item, "health_status", "health") || "unknown")) + '" data-protection="' + esc(mediaProtectionState(item)) + '"><span class="media-tape-glyph" aria-hidden="true"><i></i><i></i></span><span class="media-inventory-copy"><span><strong>' + esc(label(mediaName(item))) + '</strong>' + status(human(stateValue)) + '</span><small>' + esc(label(format)) + " · " + esc(mediaPoolName(item)) + " · " + esc(mediaLocationName(item)) + '</small><span class="media-item-signals"><em>' + protectionCount + " recovery point" + (protectionCount === 1 ? "" : "s") + '</em><em>' + holds + " active hold" + (holds === 1 ? "" : "s") + '</em></span></span><span class="media-item-chevron" aria-hidden="true">›</span></button>';
  }

  function renderMediaCatalog(inventory, selectedId) {
    const pools = [{ id: "unassigned", name: "Unassigned" }].concat(state.data.mediaPools.map((pool) => ({ id: recordId(pool), name: label(pick(pool, "name")) })));
    const locations = [];
    const states = [];
    const identities = [];
    const healthStates = [];
    inventory.forEach((item) => {
      if (!locations.some((entry) => entry.id === mediaLocationId(item))) locations.push({ id: mediaLocationId(item), name: mediaLocationName(item) });
      const itemState = mediaOperationalState(item);
      const identity = String(pick(item, "identity_status") || "unknown").toLowerCase();
      const health = String(pick(item, "health_status", "health") || "unknown").toLowerCase();
      if (!states.includes(itemState)) states.push(itemState);
      if (!identities.includes(identity)) identities.push(identity);
      if (!healthStates.includes(health)) healthStates.push(health);
    });
    const option = (value, name, selected) => '<option value="' + esc(mediaFilterValue(value)) + '"' + (selected === mediaFilterValue(value) ? " selected" : "") + '>' + esc(name) + "</option>";
    const poolFilter = '<select class="select compact" id="media-pool-filter" aria-label="Filter by pool"><option value="all">All pools</option>' + pools.map((pool) => option(pool.id, pool.name, state.mediaPoolFilter)).join("") + "</select>";
    const stateFilter = '<select class="select compact" id="media-status-filter" aria-label="Filter by operational state"><option value="all">All states</option>' + states.sort().map((value) => option(value, human(value), state.mediaStatusFilter)).join("") + "</select>";
    const locationFilter = '<select class="select compact" id="media-location-filter" aria-label="Filter by location"><option value="all">All locations</option>' + locations.sort((a, b) => a.name.localeCompare(b.name)).map((locationRecord) => option(locationRecord.id, locationRecord.name, state.mediaLocationFilter)).join("") + "</select>";
    const identityFilter = '<select class="select compact" id="media-identity-filter" aria-label="Filter by identity"><option value="all">All identities</option>' + identities.sort().map((value) => option(value, human(value), state.mediaIdentityFilter)).join("") + "</select>";
    const healthFilter = '<select class="select compact" id="media-health-filter" aria-label="Filter by health"><option value="all">All health states</option>' + healthStates.sort().map((value) => option(value, human(value), state.mediaHealthFilter)).join("") + "</select>";
    const protectionFilter = '<select class="select compact" id="media-protection-filter" aria-label="Filter by protection"><option value="all">All protection</option>' + [["protected", "Protected"], ["held", "Held"], ["unprotected", "Unprotected"]].map(([value, name]) => option(value, name, state.mediaProtectionFilter)).join("") + "</select>";
    return '<aside class="panel media-catalog"><div class="panel-head"><div><h2>Cartridge inventory</h2><p id="media-filter-count">' + inventory.length + " of " + inventory.length + ' cartridges</p></div></div><div class="media-catalog-tools"><label class="search-field"><span class="sr-only">Search cartridge inventory</span><input id="media-search" type="search" value="' + esc(state.mediaQuery) + '" placeholder="Label, identity, barcode, pool…" autocomplete="off"></label><div class="media-filter-row">' + poolFilter + stateFilter + locationFilter + identityFilter + healthFilter + protectionFilter + '</div></div><div class="media-inventory-list" id="media-inventory-list">' + inventory.map((item) => renderMediaInventoryItem(item, selectedId)).join("") + '</div><div class="media-inline-empty" id="media-filter-empty" hidden>No cartridges match these filters.</div></aside>';
  }

  function mediaDetailParts(summary, detail) {
    const cartridge = Object.assign({}, summary || {}, objectValue(detail, "cartridge"));
    return {
      cartridge,
      pool: Object.assign({}, objectValue(cartridge, "pool"), objectValue(detail, "pool")),
      location: Object.assign({}, objectValue(cartridge, "confirmed_location", "location"), objectValue(detail, "confirmed_location", "location")),
      protection: objectValue(detail, "protection", "retention"),
      volumes: arrayValue(detail, "volumes", "content_volumes"),
      recoveryPoints: arrayValue(detail, "dependent_recovery_points").concat(arrayValue(detail, "recovery_points")),
      holds: arrayValue(detail, "holds"),
      verifications: arrayValue(detail, "verifications", "verification_history"),
      events: arrayValue(detail, "custody_events").concat(arrayValue(detail, "operations"), arrayValue(detail, "audit_events")),
      truncation: objectValue(detail, "truncation"),
      caps: objectValue(detail, "caps", "capabilities")
    };
  }

  function uniqueByIdentity(records) {
    const seen = new Set();
    return records.filter((record, index) => {
      const identity = recordId(record) || String(pick(record, "digest", "occurred_at", "created_at") || index);
      if (seen.has(identity)) return false;
      seen.add(identity);
      return true;
    });
  }

  function mediaStateVersion(parts) {
    return Number(pick(parts.cartridge, "state_version") || 0);
  }

  function invalidateMediaReuseEvaluations(cartridgeId) {
    const prefix = cartridgeId ? String(cartridgeId) + ":" : null;
    Object.keys(state.data.mediaReuseEvaluations).forEach((key) => {
      if (!prefix || key.startsWith(prefix)) delete state.data.mediaReuseEvaluations[key];
    });
    Object.keys(state.mediaReuseEvaluatedAt).forEach((key) => {
      if (!prefix || key.startsWith(prefix)) delete state.mediaReuseEvaluatedAt[key];
    });
  }

  function mediaDetailFact(name, value, mono) {
    return '<div><small>' + esc(name) + '</small><strong' + (mono ? ' class="mono"' : "") + '>' + esc(label(value)) + "</strong></div>";
  }

  function renderMediaReuseEvaluation(id) {
    const driveId = state.mediaReuseDriveByMedia[id];
    const key = driveId ? id + ":" + driveId : null;
    const loadingEval = key && state.mediaReuseLoading.has(key);
    const payload = key && state.data.mediaReuseEvaluations[key];
    const evaluation = payload && payload.evaluation && typeof payload.evaluation === "object" ? payload.evaluation : payload;
    if (loadingEval) return '<section class="media-dossier-section media-reuse"><div class="media-section-head"><div><span class="media-section-kicker">Decision evidence</span><h3>Exact-drive reuse evaluation</h3></div></div><div class="media-evaluation-loading"><span></span><div><strong>Evaluating cartridge and drive</strong><small>TapeScale is resolving physical, retention, and dependency blockers.</small></div></div></section>';
    const evaluatedAt = key ? Number(state.mediaReuseEvaluatedAt[key]) : 0;
    const evaluationVersion = Number(pick(evaluation, "state_version"));
    const currentVersion = currentMediaStateVersion(id);
    const evaluationCurrent = evaluation && typeof evaluation === "object" && Number.isFinite(evaluationVersion) && evaluationVersion === currentVersion && evaluatedAt > 0 && Date.now() - evaluatedAt <= 90000;
    if (evaluation && !evaluationCurrent) {
      return '<section class="media-dossier-section media-reuse"><div class="media-section-head"><div><span class="media-section-kicker">Decision evidence</span><h3>Exact-drive reuse evaluation</h3></div><button class="button button-small" type="button" data-action="open-media-reuse" data-id="' + esc(id) + '">Run fresh evaluation</button></div><div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Previous decision expired</strong><span>Cartridge or drive state may have changed. TapeScale will not present the cached result as current.</span></div></div></section>';
    }
    if (!evaluation || typeof evaluation !== "object") {
      return '<section class="media-dossier-section media-reuse"><div class="media-section-head"><div><span class="media-section-kicker">Decision evidence</span><h3>Exact-drive reuse evaluation</h3></div><button class="button button-small" type="button" data-action="open-media-reuse" data-id="' + esc(id) + '">Evaluate with drive</button></div><div class="media-safety-boundary"><span aria-hidden="true">✓</span><div><strong>Read-only safety decision</strong><p>Select the exact physical drive to test compatibility, active holds, retained recovery points, and content dependencies. This evaluation does not change the cartridge.</p></div></div></section>';
    }
    const allowed = pick(evaluation, "allowed", "eligible", "reuse_allowed") === true;
    const blockers = arrayValue(evaluation, "blockers", "reasons");
    const affected = arrayValue(evaluation, "affected_recovery_points", "recovery_points");
    const drive = objectValue(evaluation, "selected_drive", "drive");
    const decision = allowed ? "Eligible for a future separately authorized write" : "Reuse is blocked";
    const blockerList = blockers.length ? '<ul class="media-blocker-list">' + blockers.map((blocker) => '<li><span aria-hidden="true">!</span><div><strong>' + esc(label(typeof blocker === "string" ? blocker : pick(blocker, "code", "title") || "Blocker")) + '</strong><p>' + esc(label(typeof blocker === "string" ? blocker : pick(blocker, "message", "detail", "reason"))) + "</p></div></li>").join("") + "</ul>" : '<p class="media-inline-empty compact">No blockers were returned for this exact cartridge and drive pairing.</p>';
    const affectedList = affected.length ? '<div class="media-affected-list"><h4>Affected recovery points</h4>' + affected.map((point) => '<div><strong>' + esc(label(pick(point, "dataset_name", "source", "name", "id"))) + '</strong><span>' + esc(date(pick(point, "recovery_time", "created_at"))) + " · " + esc(label(pick(point, "relation", "dependency_kind", "dependency", "reason", "status"))) + "</span></div>").join("") + "</div>" : "";
    return '<section class="media-dossier-section media-reuse"><div class="media-section-head"><div><span class="media-section-kicker">Decision evidence</span><h3>Exact-drive reuse evaluation</h3></div><button class="button button-small" type="button" data-action="open-media-reuse" data-id="' + esc(id) + '">Evaluate another drive</button></div><div class="media-decision ' + (allowed ? "allowed" : "blocked") + '"><span aria-hidden="true">' + (allowed ? "✓" : "!") + '</span><div><small>Decision for ' + esc(label(pick(drive, "stable_id", "name", "device") || driveId)) + '</small><strong>' + esc(decision) + '</strong><p>' + (allowed ? "No reported policy or compatibility blocker. A separate write workflow must still authorize any media mutation." : "TapeScale will not present a write path while any blocker remains.") + "</p></div></div>" + blockerList + affectedList + "</section>";
  }

  function renderMediaHolds(id, parts) {
    const holds = parts.holds.slice().sort((left, right) => new Date(pick(right, "created_at") || 0) - new Date(pick(left, "created_at") || 0));
    const partial = pick(parts.truncation, "holds") === true;
    const rows = holds.map((hold) => {
      const holdState = String(pick(hold, "state", "status") || (pick(hold, "released_at") ? "released" : pick(hold, "active") === false ? "expired" : "active")).toLowerCase();
      const active = pick(hold, "active") != null
        ? pick(hold, "active") === true
        : !pick(hold, "released_at") && !["released", "expired", "inactive"].includes(holdState);
      const kind = String(pick(hold, "kind", "type") || "manual").toLowerCase();
      const directCartridgeHold = String(pick(hold, "cartridge_id") || "") === String(id);
      const inherited = !directCartridgeHold && Boolean(pick(hold, "recovery_point_id"));
      const releasable = active && directCartridgeHold && ["manual", "legal", "archive"].includes(kind);
      const origin = inherited ? '<em class="media-hold-origin">Dependency-propagated · system-managed</em>' : '<em class="media-hold-origin">Cartridge-level</em>';
      const releasedAt = pick(hold, "released_at");
      const lifecycle = releasedAt
        ? " · released " + esc(date(releasedAt)) + " by " + esc(label(pick(hold, "released_by_label") || "unknown operator"))
        : pick(hold, "expires_at") ? " · expires " + esc(date(pick(hold, "expires_at"))) : " · no expiry";
      const releaseReason = releasedAt && pick(hold, "release_reason")
        ? '<small>Release reason: ' + esc(pick(hold, "release_reason")) + "</small>"
        : "";
      return '<li><span class="media-hold-icon ' + (active ? "active" : "") + '" aria-hidden="true">' + (active ? "!" : "✓") + '</span><div><span><strong>' + esc(human(kind)) + " hold</strong>" + status(human(active ? "active" : holdState)) + '</span>' + origin + '<p>' + esc(label(pick(hold, "reason", "detail"))) + '</p><small>Created ' + esc(date(pick(hold, "created_at"))) + " by " + esc(label(pick(hold, "created_by_label", "actor"))) + lifecycle + (inherited ? " · recovery point " + esc(label(pick(hold, "recovery_point_id"))) : "") + "</small>" + releaseReason + "</div>" + (releasable ? '<button class="button button-small button-quiet" type="button" data-action="release-media-hold" data-media-id="' + esc(id) + '" data-hold-id="' + esc(recordId(hold)) + '">Release</button>' : "") + "</li>";
    }).join("");
    return '<section class="media-dossier-section"><div class="media-section-head"><div><span class="media-section-kicker">Retention control</span><h3>Holds</h3>' + (partial ? '<p>' + holds.length + ' records shown; this list is partial.</p>' : "") + '</div><button class="button button-small" type="button" data-action="add-media-hold" data-id="' + esc(id) + '">Add hold</button></div><ul class="media-hold-list">' + (rows || '<li class="media-inline-empty">No manual, legal, or archive holds are recorded.</li>') + "</ul></section>";
  }

  function renderMediaProtection(parts) {
    const points = uniqueByIdentity(parts.recoveryPoints);
    const pointsPartial = pick(parts.truncation, "recovery_points") === true || pick(parts.truncation, "dependent_recovery_points") === true;
    const volumesPartial = pick(parts.truncation, "volumes") === true;
    const pointRows = points.map((point) => '<li><span class="media-recovery-symbol" aria-hidden="true">↶</span><div><span><strong>' + esc(label(pick(point, "dataset_name", "source", "name", "id"))) + "</strong>" + status(human(pick(point, "status", "state") || "retained")) + '</span><p>' + esc(date(pick(point, "recovery_time", "created_at"))) + " · " + esc(label(pick(point, "dependency_kind", "representation", "level", "dependency") || "Recovery dependency")) + '</p><code>' + esc(label(pick(point, "manifest_digest", "id"))) + "</code></div></li>").join("");
    const volumeRows = parts.volumes.map((volume) => {
      const firstRecord = pick(volume, "first_record");
      const lastRecord = pick(volume, "last_record");
      const recordRange = firstRecord ? firstRecord + (lastRecord ? " – " + lastRecord : " onward") : "Not reported";
      return '<tr><td><strong>' + esc(label(pick(volume, "name", "volume_name", "label", "engine_record_id", "id"))) + '</strong><small>Volume order ' + esc(label(pick(volume, "volume_order"))) + '</small></td><td>' + esc(bytes(pick(volume, "encoded_bytes", "logical_bytes", "bytes", "size"))) + '</td><td class="mono">' + esc(recordRange) + '</td><td>' + status(human(pick(volume, "finalized") === false ? "open" : pick(volume, "status", "state") || "finalized")) + "</td></tr>";
    }).join("");
    return '<section class="media-dossier-section"><div class="media-section-head"><div><span class="media-section-kicker">Protected content</span><h3>Recovery dependencies</h3></div><span>' + points.length + (pointsPartial ? " shown · partial" : " affected") + "</span></div>" + (pointRows ? '<ul class="media-recovery-list">' + pointRows + "</ul>" : '<div class="media-inline-empty">No recovery-point dependency was returned.</div>') + (volumeRows ? '<div class="media-subsection"><h4>Recorded volumes' + (volumesPartial ? " · " + parts.volumes.length + " shown · partial" : "") + '</h4><div class="table-scroll"><table class="data-table media-volume-table"><thead><tr><th>Volume / engine record</th><th>Encoded</th><th>Record range</th><th>State</th></tr></thead><tbody>' + volumeRows + "</tbody></table></div></div>" : "") + "</section>";
  }

  function renderMediaVerification(parts) {
    const checks = parts.verifications.slice().sort((left, right) => new Date(pick(right, "completed_at", "created_at") || 0) - new Date(pick(left, "completed_at", "created_at") || 0));
    const rows = checks.map((check) => '<li><span>' + status(human(pick(check, "outcome", "status", "state") || "unknown")) + '</span><div><strong>' + esc(label(pick(check, "kind", "method", "verification_type") || "Media verification")) + '</strong><p>Recovery point ' + esc(label(pick(check, "recovery_point_id"))) + " · drive " + esc(label(pick(check, "drive_stable_id", "drive_id"))) + '</p></div><time>' + esc(date(pick(check, "finished_at", "completed_at", "created_at", "started_at"))) + "</time></li>").join("");
    return '<section class="media-dossier-section"><div class="media-section-head"><div><span class="media-section-kicker">Assurance</span><h3>Verification history</h3></div></div><ul class="media-verification-list">' + (rows || '<li class="media-inline-empty">No verification result is recorded for this cartridge.</li>') + "</ul></section>";
  }

  function renderMediaHistory(parts) {
    const events = uniqueByIdentity(parts.events).sort((left, right) => new Date(pick(right, "occurred_at", "created_at", "updated_at") || 0) - new Date(pick(left, "occurred_at", "created_at", "updated_at") || 0));
    const rows = events.map((event) => {
      const evidence = objectValue(event, "evidence", "details");
      const custody = [pick(event, "source_location_name"), pick(event, "destination_location_name", "destination_label")].filter(Boolean).join(" → ");
      const detail = [pick(event, "note", "detail", "message", "reason", "result") || pick(evidence, "detail", "path", "reason"), custody].filter(Boolean).join(" · ");
      return '<li><time>' + esc(date(pick(event, "occurred_at", "created_at", "updated_at"))) + '</time><span class="media-event-dot" aria-hidden="true"></span><div><strong>' + esc(human(pick(event, "event_type", "operation_kind", "operation_type", "action", "kind") || "Recorded event")) + '</strong><p>' + esc(label(detail)) + '</p><small>' + esc(label(pick(event, "actor_label", "actor", "created_by_label", "created_by", "source") || "TapeScale")) + "</small></div></li>";
    }).join("");
    return '<section class="media-dossier-section"><div class="media-section-head"><div><span class="media-section-kicker">Chain of evidence</span><h3>Cartridge history</h3></div></div><ol class="media-history">' + (rows || '<li class="media-inline-empty">No custody or audit history was returned.</li>') + "</ol></section>";
  }

  function renderMediaDetail(summary) {
    if (!summary) return '<section class="panel media-dossier media-dossier-empty"><span aria-hidden="true">▣</span><h2>Select a cartridge</h2><p>Choose an inventory record to inspect its protection, retention, verification, and custody evidence.</p></section>';
    const id = recordId(summary);
    const detail = mediaDetail(id);
    const detailError = state.errors["mediaDetail:" + id];
    if (!detail && state.mediaDetailLoading.has(id)) return '<section class="panel media-dossier"><div class="media-dossier-loading"><span></span><span></span><span></span></div></section>';
    if (!detail && detailError) return '<section class="panel media-dossier media-dossier-empty"><span aria-hidden="true">!</span><h2>Could not load cartridge dossier</h2><p>' + esc(errorMessage(detailError)) + '</p><button class="button" type="button" data-action="retry-media-detail" data-id="' + esc(id) + '">Retry</button></section>';
    if (!detail) return '<section class="panel media-dossier media-dossier-empty"><span aria-hidden="true">…</span><h2>Loading cartridge dossier</h2><p>Requesting the retained media record.</p></section>';
    const parts = mediaDetailParts(summary, detail);
    const cartridge = parts.cartridge;
    const poolName = pick(parts.pool, "name") || mediaPoolName(cartridge);
    const locationName = pick(parts.location, "name") || mediaLocationName(cartridge);
    const stateValue = mediaOperationalState(cartridge);
    const version = mediaStateVersion(parts);
    const reportedActiveHolds = pick(cartridge, "active_holds", "hold_count");
    const activeHolds = reportedActiveHolds == null
      ? parts.holds.filter((hold) => pick(hold, "active") != null ? pick(hold, "active") === true : !pick(hold, "released_at") && !["released", "expired", "inactive"].includes(String(pick(hold, "state", "status") || "active").toLowerCase())).length
      : count(reportedActiveHolds);
    const hasReportedProtectionCounts = pick(cartridge, "retained_recovery_points", "retained_recovery_point_count") != null || pick(cartridge, "dependent_recovery_points", "dependent_recovery_point_count") != null;
    const protectedRecoveryPoints = hasReportedProtectionCounts ? mediaRecoveryPointCount(cartridge) : uniqueByIdentity(parts.recoveryPoints).length;
    const reportedVolumes = pick(cartridge, "content_volume_count");
    const recordedVolumes = reportedVolumes == null ? parts.volumes.length : count(reportedVolumes);
    const generation = pick(cartridge, "media_generation", "generation");
    const latestVerification = objectValue(cartridge, "latest_verification");
    const catalogReconciled = pick(cartridge, "catalog_reconciled", "catalogReconciled");
    const latestVerificationAt = pick(cartridge, "latest_verification_at") || pick(latestVerification, "occurred_at", "finished_at", "completed_at", "created_at");
    const latestVerificationOutcome = pick(cartridge, "latest_verification_outcome") || pick(latestVerification, "outcome", "status");
    const latestVerificationText = latestVerificationAt
      ? (latestVerificationOutcome ? human(latestVerificationOutcome) + " · " : "") + date(latestVerificationAt)
      : "Not reported";
    const truncatedSections = Object.entries(parts.truncation).filter(([, truncated]) => truncated === true).map(([name]) => human(name));
    const truncationWarning = truncatedSections.length
      ? '<div class="notice warning media-truncation-warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>This is a bounded dossier</strong><span>Additional records exist in: ' + esc(truncatedSections.join(", ")) + ". Those sections are partial; do not infer that an omitted hold, dependency, verification, or event is absent.</span></div></div>"
      : "";
    const hero = '<header class="media-dossier-hero"><div class="media-dossier-identity"><span class="media-hero-tape" aria-hidden="true"><i></i><i></i></span><div><span class="media-section-kicker">Cartridge dossier · state v' + version + '</span><div class="media-dossier-title"><h2>' + esc(label(mediaName(cartridge))) + "</h2>" + status(human(stateValue)) + '</div><p>' + esc(label(pick(cartridge, "barcode"))) + " · " + esc(generation ? "LTO-" + generation : pick(cartridge, "media_format", "format")) + " · " + esc(locationName) + '</p></div></div><div class="media-dossier-actions"><button class="button button-quiet" type="button" data-action="assign-media-pool" data-id="' + esc(id) + '">Assign pool</button><button class="button button-quiet" type="button" data-action="add-media-hold" data-id="' + esc(id) + '">Add hold</button><button class="button button-primary" type="button" data-action="open-media-reuse" data-id="' + esc(id) + '">Evaluate reuse</button></div></header>';
    const ribbon = '<div class="media-protection-ribbon"><span><strong>' + activeHolds + '</strong> active hold' + (activeHolds === 1 ? "" : "s") + '</span><span><strong>' + protectedRecoveryPoints + '</strong> protected recovery point' + (protectedRecoveryPoints === 1 ? "" : "s") + '</span><span><strong>' + recordedVolumes + '</strong> recorded volume' + (recordedVolumes === 1 ? "" : "s") + '</span><span><strong>' + (pick(parts.protection, "indefinite") === true || pick(parts.pool, "indefinite_retention") === true ? "Indefinite" : "Policy") + '</strong> retention</span></div>';
    const facts = mediaDetailFact("Internal label", mediaName(cartridge), true) +
      mediaDetailFact("Barcode", pick(cartridge, "barcode"), true) +
      mediaDetailFact("On-tape identity", pick(cartridge, "on_tape_identity") || "Not observed", true) +
      mediaDetailFact("Identity status", human(pick(cartridge, "identity_status") || "unknown")) +
      mediaDetailFact("Health", human(pick(cartridge, "health_status", "health") || "unknown")) +
      mediaDetailFact("Catalog reconciled", catalogReconciled == null ? "Not reported" : catalogReconciled ? "Yes" : "No") +
      mediaDetailFact("Dependency graph", pick(cartridge, "dependency_graph_truncated") == null ? "Not reported" : pick(cartridge, "dependency_graph_truncated") === true ? "Truncated — review required" : "Complete within catalog") +
      mediaDetailFact("Reserved by", pick(cartridge, "reserved_by") || "Not reserved") +
      mediaDetailFact("Format", pick(cartridge, "media_format", "format")) +
      mediaDetailFact("Generation", generation ? "LTO-" + generation : null) +
      mediaDetailFact("WORM", pick(cartridge, "worm") === true ? "Yes" : "No") +
      mediaDetailFact("Pool", poolName) +
      mediaDetailFact("Pool purpose", human(pick(parts.pool, "purpose") || "unknown")) +
      mediaDetailFact("Confirmed location", locationName) +
      mediaDetailFact("Location type", human(pick(parts.location, "kind") || pick(cartridge, "confirmed_location_kind", "location_kind") || "unknown")) +
      mediaDetailFact("Content recorded", pick(cartridge, "has_content") === true ? "Yes" : pick(cartridge, "has_content") === false ? "No" : null) +
      mediaDetailFact("Latest verification", latestVerificationText) +
      mediaDetailFact("State version", "v" + version, true);
    const identitySection = '<section class="media-dossier-section"><div class="media-section-head"><div><span class="media-section-kicker">Registered identity</span><h3>Identity, location, and retention</h3></div></div><div class="media-facts-grid">' + facts + "</div></section>";
    return '<article class="panel media-dossier">' + hero + ribbon + truncationWarning + identitySection + renderMediaReuseEvaluation(id) + renderMediaHolds(id, parts) + renderMediaProtection(parts) + renderMediaVerification(parts) + renderMediaHistory(parts) + "</article>";
  }

  function renderMedia() {
    const inventory = orderedMedia();
    const selected = selectedMedia();
    const initialize = '<button class="button button-danger" type="button" data-action="initialize-tape"' + (selected ? ' data-media-id="' + esc(recordId(selected)) + '"' : "") + '>Prepare tape</button>';
    const add = '<button class="button button-quiet" type="button" data-action="add-media">Register cartridge</button>';
    const createPool = '<button class="button button-quiet" type="button" data-action="create-media-pool">Create pool</button>';
    let html = pageHead("Media Control Center", "Operational cartridge inventory, retention controls, and read-only reuse decisions.", refreshButton() + createPool + add + initialize) + noticeFor("media") + noticeFor("mediaPools");
    if (!inventory.length) return html + renderMediaPools() + empty("No cartridges", state.errors.media ? "Cartridge inventory could not be loaded." : "Register a cartridge to begin building the operational inventory.", add);
    const protectedCount = inventory.filter((item) => pick(item, "dependency_graph_truncated") === true || mediaRecoveryPointCount(item) > 0 || count(pick(item, "indefinite_archive_claims", "indefinite_claims")) > 0 || mediaOperationalState(item) === "protected").length;
    const heldCount = inventory.reduce((total, item) => total + count(pick(item, "active_holds", "hold_count")), 0);
    const attentionCount = inventory.filter((item) => count(pick(item, "active_holds", "hold_count")) > 0 || ["held", "blocked", "physically_blocked", "needs_reconciliation", "foreign", "mismatch"].includes(mediaOperationalState(item))).length;
    html += '<section class="panel metric-strip media-metrics"><div class="metric"><span>Total cartridges</span><strong>' + inventory.length + '</strong></div><div class="metric"><span>Recovery protected</span><strong>' + protectedCount + '</strong></div><div class="metric"><span>Active holds</span><strong>' + heldCount + '</strong></div><div class="metric"><span>Needs attention</span><strong>' + attentionCount + "</strong></div></section>";
    html += renderMediaPools();
    html += '<div class="media-control-layout section">' + renderMediaCatalog(inventory, selected && recordId(selected)) + renderMediaDetail(selected) + "</div>";
    return html;
  }

  function recoveryPointState(point) {
    return String(pick(point, "status", "state", "verification") || "unknown").toLowerCase();
  }

  function recoveryPointRepresentation(point) {
    return String(pick(point, "representation", "kind", "mode") || "unknown").toLowerCase();
  }

  function recoveryPointMediaLabels(point) {
    const labels = pick(point, "media_labels", "tape_labels", "cartridges");
    return Array.isArray(labels) ? labels.filter(Boolean).map(String) : [];
  }

  function recoveryPointRouteId() {
    const parts = location.hash.slice(1).split("/");
    if (parts[0] !== "restore" || !parts[1]) return null;
    try {
      return decodeURIComponent(parts.slice(1).join("/"));
    } catch (_) {
      return null;
    }
  }

  function orderedRecoveryPoints() {
    return state.data.recoveryPoints.slice().sort((left, right) => {
      const timeDifference = new Date(pick(right, "recovery_time", "created_at") || 0) - new Date(pick(left, "recovery_time", "created_at") || 0);
      return timeDifference || recordId(right).localeCompare(recordId(left));
    });
  }

  function selectedRecoveryPoint() {
    const points = orderedRecoveryPoints();
    const requestedId = recoveryPointRouteId() || state.selectedRecoveryPointId;
    const selected = requestedId && points.find((point) => recordId(point) === requestedId);
    const point = selected || points[0] || null;
    if (point) state.selectedRecoveryPointId = recordId(point);
    return point;
  }

  function recoveryPointDetail(id) {
    return id ? state.data.recoveryPointDetails[id] || null : null;
  }

  async function loadRecoveryPointDetail(id, force) {
    if (!id || state.recoveryPointDetailLoading.has(id) || (!force && recoveryPointDetail(id))) return;
    const errorKey = "recoveryPoint:" + id;
    state.recoveryPointDetailLoading.add(id);
    delete state.errors[errorKey];
    try {
      const detail = await request(endpoints.recoveryPoints + "/" + encodeURIComponent(id));
      state.data.recoveryPointDetails[id] = detail && typeof detail === "object" ? detail : {};
      delete state.errors[errorKey];
    } catch (error) {
      state.errors[errorKey] = error;
    } finally {
      state.recoveryPointDetailLoading.delete(id);
    }
    if (state.route === "restore" && recordId(selectedRecoveryPoint()) === id) render();
  }

  async function selectRecoveryPoint(id) {
    if (!id) return;
    state.selectedRecoveryPointId = id;
    const targetHash = "#restore/" + encodeURIComponent(id);
    if (location.hash !== targetHash) location.hash = targetHash;
    else render();
    await loadRecoveryPointDetail(id);
  }

  async function browseBackupFiles(id) {
    if (!id) return;
    await selectRecoveryPoint(id);
    await openRecoveryPointFiles(id);
  }

  function restoreRehearsalState(rehearsal) {
    return String(pick(rehearsal, "state", "status") || "queued").toLowerCase();
  }

  function restoreRehearsalActive(rehearsal) {
    return !RECOVERY_TERMINAL_STATES.has(restoreRehearsalState(rehearsal));
  }

  function restoreRehearsalPointId(rehearsal) {
    return String(pick(rehearsal, "recovery_point_id", "recoveryPointId") || "");
  }

  function restoreRehearsalsForPoint(pointId) {
    return state.data.restoreRehearsals
      .filter((rehearsal) => restoreRehearsalPointId(rehearsal) === pointId)
      .sort((left, right) => new Date(pick(right, "created_at", "started_at") || 0) - new Date(pick(left, "created_at", "started_at") || 0));
  }

  function selectedRestoreRehearsal() {
    const point = selectedRecoveryPoint();
    const runs = restoreRehearsalsForPoint(recordId(point));
    const selected = state.selectedRestoreRehearsalId && runs.find((run) => recordId(run) === state.selectedRestoreRehearsalId);
    const run = selected || runs[0] || null;
    if (run) state.selectedRestoreRehearsalId = recordId(run);
    return run;
  }

  async function loadRestoreRehearsalDetail(id, force) {
    if (!id || state.restoreRehearsalDetailLoading.has(id) || (!force && state.data.restoreRehearsalDetails[id])) return;
    const errorKey = "restoreRehearsal:" + id;
    state.restoreRehearsalDetailLoading.add(id);
    delete state.errors[errorKey];
    try {
      const detail = await request(endpoints.restoreRehearsals + "/" + encodeURIComponent(id));
      state.data.restoreRehearsalDetails[id] = detail && typeof detail === "object" ? detail : {};
      delete state.errors[errorKey];
    } catch (error) {
      state.errors[errorKey] = error;
    } finally {
      state.restoreRehearsalDetailLoading.delete(id);
    }
    if (state.route === "restore" && recordId(selectedRestoreRehearsal()) === id) render();
  }

  async function selectRestoreRehearsal(id) {
    if (!id) return;
    state.selectedRestoreRehearsalId = id;
    await loadRestoreRehearsalDetail(id);
    render();
  }

  function recoveryPointMatches(point) {
    const query = state.recoveryPointQuery.trim().toLowerCase();
    const pointState = recoveryPointState(point);
    const representation = recoveryPointRepresentation(point);
    if (state.recoveryPointStatus !== "all" && pointState !== state.recoveryPointStatus) return false;
    if (state.recoveryPointRepresentation !== "all" && representation !== state.recoveryPointRepresentation) return false;
    if (!query) return true;
    const searchable = [
      recordId(point),
      pick(point, "dataset_name", "source", "dataset", "logical_root"),
      pick(point, "level"),
      representation,
      pointState,
      pick(point, "manifest_digest"),
      ...recoveryPointMediaLabels(point)
    ].filter(Boolean).join(" ").toLowerCase();
    return searchable.includes(query);
  }

  function applyRecoveryPointFilters() {
    const catalog = document.querySelector("#recovery-vault-catalog");
    if (!catalog) return;
    let visible = 0;
    catalog.querySelectorAll(".vault-point").forEach((button) => {
      const matchesQuery = !state.recoveryPointQuery.trim() || String(button.dataset.search || "").includes(state.recoveryPointQuery.trim().toLowerCase());
      const matchesStatus = state.recoveryPointStatus === "all" || button.dataset.status === state.recoveryPointStatus;
      const matchesRepresentation = state.recoveryPointRepresentation === "all" || button.dataset.representation === state.recoveryPointRepresentation;
      button.hidden = !(matchesQuery && matchesStatus && matchesRepresentation);
      if (!button.hidden) visible += 1;
    });
    const countNode = document.querySelector("#vault-filter-count");
    if (countNode) countNode.textContent = visible + " of " + state.data.recoveryPoints.length + " recovery point" + (state.data.recoveryPoints.length === 1 ? "" : "s");
    const emptyNode = document.querySelector("#vault-filter-empty");
    if (emptyNode) emptyNode.hidden = visible !== 0;
  }

  function recoveryPointLogicalBytes(point) {
    const coverage = point && point.coverage && typeof point.coverage === "object" ? point.coverage : {};
    return pick(point, "logical_bytes", "source_bytes") ?? pick(coverage, "logical_bytes", "source_bytes", "bytes");
  }

  function recoveryPointVolumes(detail, point) {
    const volumes = detail ? list(detail, "volumes") : [];
    if (volumes.length) {
      return volumes.slice().sort((left, right) => Number(pick(left, "volume_order", "order") || 0) - Number(pick(right, "volume_order", "order") || 0));
    }
    return recoveryPointMediaLabels(point).map((mediaLabel, index) => ({ volume_order: index + 1, media_label: mediaLabel }));
  }

  function volumeCartridge(volume) {
    return volume && volume.cartridge && typeof volume.cartridge === "object" ? volume.cartridge : {};
  }

  function volumeLabel(volume) {
    const cartridge = volumeCartridge(volume);
    return pick(volume, "media_label", "cartridge_label", "expected_cartridge_label", "expected_volume_label", "internal_label", "barcode") || pick(cartridge, "internal_label", "label", "barcode", "id") || "Unidentified cartridge";
  }

  function renderRecoveryPointCatalog(points, selected) {
    const statuses = Array.from(new Set(points.map(recoveryPointState))).sort();
    const representations = Array.from(new Set(points.map(recoveryPointRepresentation))).sort();
    const statusOptions = ['<option value="all">All states</option>'].concat(statuses.map((value) => '<option value="' + esc(value) + '"' + (state.recoveryPointStatus === value ? " selected" : "") + ">" + esc(human(value)) + "</option>"));
    const representationOptions = ['<option value="all">All formats</option>'].concat(representations.map((value) => '<option value="' + esc(value) + '"' + (state.recoveryPointRepresentation === value ? " selected" : "") + ">" + esc(human(value)) + "</option>"));
    const visibleCount = points.filter(recoveryPointMatches).length;
    const cards = points.map((point) => {
      const id = recordId(point);
      const pointState = recoveryPointState(point);
      const representation = recoveryPointRepresentation(point);
      const media = recoveryPointMediaLabels(point);
      const search = [id, pick(point, "dataset_name", "source", "dataset"), pick(point, "level"), representation, pointState, pick(point, "manifest_digest"), ...media].filter(Boolean).join(" ").toLowerCase();
      const hidden = recoveryPointMatches(point) ? "" : " hidden";
      return '<button class="vault-point' + (selected && id === recordId(selected) ? " active" : "") + '" type="button" data-action="select-recovery-point" data-id="' + esc(id) + '" data-status="' + esc(pointState) + '" data-representation="' + esc(representation) + '" data-search="' + esc(search) + '"' + hidden + '><span class="vault-point-head"><strong>' + esc(label(pick(point, "dataset_name", "source", "dataset"))) + '</strong>' + status(pointState) + '</span><span class="vault-point-meta">' + esc(human(pick(point, "level") || "unknown")) + " · " + esc(human(representation)) + " · " + esc(date(pick(point, "recovery_time", "created_at"))) + '</span><span class="vault-point-media">' + (media.length ? esc(media.join(" → ")) : "No media recorded") + '</span><code>' + esc(id) + "</code></button>";
    }).join("");
    return '<aside class="panel vault-catalog" id="recovery-vault-catalog"><div class="panel-head"><div><h2>Recovery catalog</h2><p id="vault-filter-count">' + visibleCount + " of " + points.length + " recovery point" + (points.length === 1 ? "" : "s") + '</p></div></div><div class="vault-catalog-tools"><label class="search-field"><span class="sr-only">Search recovery points</span><input id="vault-search" type="search" value="' + esc(state.recoveryPointQuery) + '" placeholder="Dataset, tape, manifest…" autocomplete="off"></label><div class="vault-filter-row"><select class="select compact" id="vault-status-filter" aria-label="Filter by recovery state">' + statusOptions.join("") + '</select><select class="select compact" id="vault-representation-filter" aria-label="Filter by representation">' + representationOptions.join("") + '</select></div></div><div class="vault-point-list">' + cards + '<div class="vault-filter-empty" id="vault-filter-empty"' + (visibleCount ? " hidden" : "") + '><strong>No matching recovery points</strong><span>Clear the search or choose another filter.</span></div></div></aside>';
  }

  function evidenceOutcome(verifications, kind) {
    const match = verifications.find((item) => String(pick(item, "kind", "verification_kind") || "").toLowerCase() === kind);
    return match ? String(pick(match, "outcome", "status", "state") || "unknown") : "not recorded";
  }

  function renderVaultProof(name, value, detail) {
    const normalized = String(value || "unknown").toLowerCase();
    const symbol = ["passed", "verified", "ready", "finalized"].includes(normalized) ? "✓" : ["failed", "blocked", "missing"].includes(normalized) ? "!" : "·";
    return '<div class="vault-proof ' + esc(normalized.replace(/[^a-z]+/g, "-")) + '"><span class="vault-proof-icon" aria-hidden="true">' + symbol + '</span><div><small>' + esc(name) + '</small><strong>' + esc(human(value || "unknown")) + '</strong><span>' + esc(detail) + "</span></div></div>";
  }

  function renderRecoveryVolumes(volumes) {
    if (!volumes.length) return '<div class="vault-section-empty">No volume map is recorded for this recovery point.</div>';
    return '<ol class="vault-volume-list">' + volumes.map((volume, index) => {
      const cartridge = volumeCartridge(volume);
      const order = Number(pick(volume, "volume_order", "order") || index + 1);
      const finalized = pick(volume, "finalized");
      const recordStart = pick(volume, "first_record", "record_start");
      const recordEnd = pick(volume, "last_record", "record_end");
      const location = pick(volume, "location_name", "confirmed_location", "location") || pick(cartridge, "location_name", "confirmed_location", "location");
      const available = pick(volume, "available");
      const compatibleDrives = pick(volume, "compatible_qualified_drives", "compatible_drives");
      const driveText = Array.isArray(compatibleDrives) && compatibleDrives.length
        ? compatibleDrives.map((drive) => typeof drive === "object" ? pick(drive, "stable_id", "name", "id") : drive).filter(Boolean).join(", ")
        : "No qualified drive reported";
      return '<li><span class="vault-volume-order">' + order + '</span><div class="vault-volume-main"><div class="vault-volume-title"><strong>' + esc(volumeLabel(volume)) + '</strong><span class="vault-volume-statuses">' + status(available === true ? "available" : available === false ? "unavailable" : "availability unknown") + status(finalized === true ? "finalized" : finalized === false ? "not finalized" : "recorded") + '</span></div><div class="vault-volume-facts"><span><small>Record span</small><strong>' + esc(recordStart || recordEnd ? label(recordStart) + " → " + label(recordEnd) : "Not reported") + '</strong></span><span><small>Encoded</small><strong>' + esc(bytes(pick(volume, "encoded_bytes", "bytes"))) + '</strong></span><span><small>Location</small><strong>' + esc(label(location)) + '</strong></span><span><small>Qualified reader</small><strong class="mono">' + esc(driveText) + '</strong></span><span><small>Engine record</small><strong class="mono">' + esc(label(pick(volume, "engine_record_id", "engine_volume_id"))) + "</strong></span></div></div></li>";
    }).join("") + "</ol>";
  }

  function conciseEvidence(value) {
    if (value == null || value === "") return "No detail reported";
    if (typeof value !== "object") return String(value);
    const preferred = pick(value, "detail", "message", "manifest_digest", "sha256", "bytes_read", "files_compared", "result");
    if (preferred !== null) return String(preferred);
    const text = JSON.stringify(value);
    return text.length > 260 ? text.slice(0, 257) + "…" : text;
  }

  function renderRecoveryVerifications(verifications) {
    if (!verifications.length) return '<div class="vault-section-empty">No verification evidence is recorded.</div>';
    return '<div class="vault-verification-grid">' + verifications.map((item) => {
      const media = pick(item, "media_labels", "tape_labels");
      const mediaText = Array.isArray(media) ? media.join(" → ") : label(pick(item, "media_label"));
      return '<article class="vault-verification"><div class="vault-verification-head"><div><small>' + esc(date(pick(item, "finished_at", "started_at"))) + '</small><h4>' + esc(human(pick(item, "kind", "verification_kind") || "verification")) + '</h4></div>' + status(pick(item, "outcome", "status", "state")) + '</div><dl><dt>Drive</dt><dd class="mono">' + esc(label(pick(item, "drive_stable_id", "drive_id"))) + '</dd><dt>Media</dt><dd>' + esc(mediaText) + '</dd><dt>Evidence</dt><dd>' + esc(conciseEvidence(pick(item, "evidence", "detail"))) + "</dd></dl></article>";
    }).join("") + "</div>";
  }

  function renderDependencyList(items, emptyMessage) {
    if (!items.length) return '<div class="vault-section-empty compact">' + esc(emptyMessage) + "</div>";
    return '<ul class="vault-contract-list">' + items.map((item) => {
      const title = pick(item, "dataset_name", "depends_on_id", "recovery_point_id", "kind", "id") || "Recorded item";
      const detail = pick(item, "dependency_kind", "reason", "status", "expires_at") || conciseEvidence(item);
      return '<li><strong>' + esc(title) + '</strong><span>' + esc(detail) + "</span></li>";
    }).join("") + "</ul>";
  }

  function renderRecoveryArtifacts(artifacts) {
    if (!artifacts.length) return '<div class="vault-section-empty compact">No retained artifact metadata was returned.</div>';
    return '<ul class="vault-artifact-list">' + artifacts.map((artifact) => '<li><div><strong>' + esc(human(pick(artifact, "artifact_kind", "kind") || "artifact")) + '</strong><span>' + esc(pick(artifact, "size_bytes") == null ? "Retained evidence" : bytes(pick(artifact, "size_bytes"))) + '</span></div><code>' + esc(label(pick(artifact, "sha256", "digest"))) + "</code></li>").join("") + "</ul>";
  }

  function renderOrigin(origin, point) {
    const facts = [
      ["Product run", pick(origin, "job_run_id", "run_id") || pick(point, "run_id")],
      ["Physical exercise", pick(origin, "single_tape_data_run_id", "recovery_exercise_id", "exercise_id")],
      ["Correlation", pick(origin, "correlation_id")],
      ["Engine job", pick(origin, "engine_job_id", "bacula_job_id")],
      ["Engine", pick(origin, "engine_name")],
      ["Adapter", pick(origin, "adapter_version")]
    ].filter((item) => item[1] !== null && item[1] !== undefined && item[1] !== "");
    if (!facts.length) return '<div class="vault-section-empty compact">No originating-run metadata was returned.</div>';
    return '<dl class="vault-facts compact">' + facts.map(([name, value]) => "<dt>" + esc(name) + '</dt><dd class="mono">' + esc(value) + "</dd>").join("") + "</dl>";
  }

  function renderRecoveryEventsForVault(events) {
    if (!events.length) return "";
    return '<details class="vault-events"><summary>Append-only run activity <span>' + events.length + ' events</span></summary><ol>' + events.map((event) => {
      const evidence = event && event.evidence && typeof event.evidence === "object" ? event.evidence : {};
      return '<li><time>' + esc(date(pick(event, "occurred_at", "created_at"))) + '</time><div><strong>' + esc(human(pick(event, "event_type", "to_state", "phase") || "event")) + '</strong><span>' + esc(conciseEvidence(pick(evidence, "detail", "message") || evidence)) + "</span></div></li>";
    }).join("") + "</ol></details>";
  }

  function recoveryFileBreadcrumbs(pointId, path) {
    const segments = String(path || "").split("/").filter(Boolean);
    const crumbs = ['<button class="source-browser-crumb" type="button" data-action="recovery-files-navigate" data-id="' + esc(pointId) + '" data-path=""><span aria-hidden="true">⌂</span><span>backup root</span></button>'];
    let accumulated = "";
    segments.forEach((segment) => {
      accumulated = sourceBrowserJoin(accumulated, segment);
      crumbs.push('<span class="source-browser-separator" aria-hidden="true">/</span><button class="source-browser-crumb" type="button" data-action="recovery-files-navigate" data-id="' + esc(pointId) + '" data-path="' + esc(accumulated) + '">' + esc(segment) + "</button>");
    });
    return crumbs.join("");
  }

  function recoveryFileEntry(entry, currentPath, pointId) {
    const kind = sourceBrowserEntryKind(entry);
    const name = String(pick(entry, "name", "display_name") || "Unnamed entry");
    const path = String(pick(entry, "path", "relative_path") || sourceBrowserJoin(currentPath, name));
    const modified = pick(entry, "modified_at", "mtime", "last_modified");
    const size = pick(entry, "size_bytes", "size");
    const linkTarget = pick(entry, "link_target", "target");
    const icon = kind === "directory" ? "▰" : kind === "symlink" ? "↗" : kind === "file" ? "▤" : "?";
    const facts = [kind === "directory" ? "Folder" : kind === "symlink" ? "Symbolic link" : kind === "file" ? "File" : "Unsupported object"];
    if (kind === "file" && size !== null && size !== undefined) facts.push(bytes(size));
    if (modified) facts.push(label(modified));
    if (kind === "symlink" && linkTarget) facts.push("→ " + linkTarget);
    const nameControl = kind === "directory"
      ? '<button class="source-browser-name" type="button" data-action="recovery-files-navigate" data-id="' + esc(pointId) + '" data-path="' + esc(path) + '" aria-label="Open retained folder ' + esc(name) + '"><strong>' + esc(name) + "</strong><small>" + esc(facts.join(" · ")) + "</small></button>"
      : '<div class="source-browser-name"><strong>' + esc(name) + "</strong><small>" + esc(facts.join(" · ")) + "</small></div>";
    const action = kind === "directory"
      ? '<button class="button button-quiet button-small" type="button" data-action="recovery-files-navigate" data-id="' + esc(pointId) + '" data-path="' + esc(path) + '">Open</button>'
      : '<span class="source-browser-entry-note">Browse only</span>';
    return '<div class="source-browser-entry ' + esc(kind) + '" role="listitem"><span class="source-browser-icon" aria-hidden="true">' + icon + '</span>' + nameControl + '<div class="source-browser-entry-action">' + action + "</div></div>";
  }

  async function loadRecoveryPointFiles(pointId, path, focusEntries) {
    if (!pointId) return;
    const requestedPath = String(path || "");
    const requestNumber = (state.recoveryFileBrowserRequests[pointId] || 0) + 1;
    state.recoveryFileBrowserRequests[pointId] = requestNumber;
    state.recoveryFileBrowserOpen.add(pointId);
    state.recoveryFileBrowserLoading.add(pointId);
    delete state.recoveryFileBrowserErrors[pointId];
    render();
    try {
      const query = requestedPath ? "?path=" + encodeURIComponent(requestedPath) : "";
      const payload = await request(endpoints.recoveryPoints + "/" + encodeURIComponent(pointId) + "/files" + query);
      if (state.recoveryFileBrowserRequests[pointId] !== requestNumber) return;
      const hasCurrent = payload && Object.prototype.hasOwnProperty.call(payload, "current_path");
      const currentPath = String(hasCurrent ? payload.current_path : pick(payload, "path", "relative_path") || requestedPath);
      const entries = payload && Array.isArray(payload.entries) ? payload.entries : payload && Array.isArray(payload.items) ? payload.items : [];
      state.recoveryFileBrowsers[pointId] = { payload: payload || {}, currentPath, entries };
      delete state.recoveryFileBrowserErrors[pointId];
    } catch (error) {
      if (state.recoveryFileBrowserRequests[pointId] !== requestNumber) return;
      error.failedPath = requestedPath;
      state.recoveryFileBrowserErrors[pointId] = error;
    } finally {
      if (state.recoveryFileBrowserRequests[pointId] === requestNumber) state.recoveryFileBrowserLoading.delete(pointId);
    }
    render();
    if (focusEntries && recordId(selectedRecoveryPoint()) === pointId) {
      window.requestAnimationFrame(() => {
        const first = document.querySelector('[data-recovery-files="' + CSS.escape(pointId) + '"] button.source-browser-name, [data-recovery-files="' + CSS.escape(pointId) + '"] .source-browser-entry button, [data-recovery-files="' + CSS.escape(pointId) + '"] .source-browser-up:not([disabled]), [data-recovery-files="' + CSS.escape(pointId) + '"] .source-browser-crumb');
        if (first) first.focus();
      });
    }
  }

  function openRecoveryPointFiles(pointId) {
    if (!pointId) return;
    if (state.recoveryFileBrowsers[pointId]) {
      state.recoveryFileBrowserOpen.add(pointId);
      render();
      return;
    }
    loadRecoveryPointFiles(pointId, "", true);
  }

  function closeRecoveryPointFiles(pointId) {
    if (!pointId) return;
    state.recoveryFileBrowserOpen.delete(pointId);
    render();
    window.requestAnimationFrame(() => {
      const trigger = document.querySelector('[data-action="open-recovery-files"][data-id="' + CSS.escape(pointId) + '"]');
      if (trigger) trigger.focus();
    });
  }

  function renderRecoveryPointFiles(pointId) {
    const open = state.recoveryFileBrowserOpen.has(pointId);
    if (!open) {
      return '<section class="vault-dossier-section recovery-file-browser-shell"><div class="vault-section-head"><div><h3>Files & directories</h3><p>Explore the retained backup manifest without loading or reading the physical tape.</p></div><button class="button button-quiet button-small" type="button" data-action="open-recovery-files" data-id="' + esc(pointId) + '"><span aria-hidden="true">▰</span> Browse contents</button></div></section>';
    }
    const browser = state.recoveryFileBrowsers[pointId];
    const loading = state.recoveryFileBrowserLoading.has(pointId);
    const error = state.recoveryFileBrowserErrors[pointId];
    if (loading) {
      const loadingPath = browser ? browser.currentPath : "";
      return '<section class="vault-dossier-section recovery-file-browser-shell" data-recovery-files="' + esc(pointId) + '"><div class="source-browser-loading" role="status"><span class="spinner" aria-hidden="true"></span><strong>Opening retained folder…</strong><small>/' + esc(loadingPath) + "</small></div></section>";
    }
    if (error) {
      const retryPath = error.failedPath !== undefined ? error.failedPath : browser ? browser.currentPath : "";
      return '<section class="vault-dossier-section recovery-file-browser-shell" data-recovery-files="' + esc(pointId) + '"><div class="vault-section-head"><div><h3>Files & directories</h3><p>Retained manifest browser</p></div><button class="icon-button" type="button" data-action="close-recovery-files" data-id="' + esc(pointId) + '" aria-label="Close retained contents browser">&times;</button></div><div class="source-browser-error" role="alert"><span aria-hidden="true">!</span><div><strong>Could not load retained contents</strong><p>' + esc(errorMessage(error)) + '</p><button class="button button-quiet button-small" type="button" data-action="recovery-files-navigate" data-id="' + esc(pointId) + '" data-path="' + esc(retryPath) + '">Try again</button></div></div></section>';
    }
    if (!browser) return "";
    const currentPath = browser.currentPath;
    const payload = browser.payload;
    const parentPath = payload && Object.prototype.hasOwnProperty.call(payload, "parent_path") ? payload.parent_path : (currentPath ? sourceBrowserParent(currentPath) : null);
    const entries = browser.entries.slice().sort((left, right) => {
      const order = { directory: 0, file: 1, symlink: 2, other: 3 };
      const kindDifference = order[sourceBrowserEntryKind(left)] - order[sourceBrowserEntryKind(right)];
      if (kindDifference) return kindDifference;
      return String(pick(left, "name", "display_name") || "").localeCompare(String(pick(right, "name", "display_name") || ""), undefined, { numeric: true, sensitivity: "base" });
    });
    const truncated = payload && payload.truncated === true;
    const entryLimit = Number(pick(payload, "entry_limit")) || entries.length;
    const empty = '<div class="source-browser-empty"><span aria-hidden="true">□</span><strong>No retained entries in this directory</strong><small>The manifest reports an empty folder.</small></div>';
    const partialWarning = truncated
      ? '<div class="notice warning source-browser-partial"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Showing the first ' + esc(entryLimit) + ' retained entries in this folder</strong><span>Only this browse-only directory view is bounded. The retained manifest and tape archive are complete, and the backup itself was not limited to these rows.</span></div></div>'
      : "";
    return '<section class="vault-dossier-section recovery-file-browser-shell" data-recovery-files="' + esc(pointId) + '"><div class="vault-section-head"><div><h3>Files & directories</h3><p>Catalog view from the content-addressed manifest.</p></div><button class="icon-button" type="button" data-action="close-recovery-files" data-id="' + esc(pointId) + '" aria-label="Close retained contents browser">&times;</button></div><div class="notice info recovery-files-boundary"><span class="notice-icon" aria-hidden="true">i</span><div class="notice-copy"><strong>Browse-only recovery inventory</strong><span>This view does not read tape and does not change restore scope. A current Alpha Restore run, when explicitly offered by the server, restores the complete Full recovery point across its exact ordered archive. Selecting individual files or folders is not supported yet.</span></div></div>' + partialWarning + '<div class="backup-source-browser retained-browser"><div class="source-browser-toolbar"><button class="icon-button source-browser-up" type="button" data-action="recovery-files-navigate" data-id="' + esc(pointId) + '" data-path="' + esc(parentPath || "") + '" aria-label="Open parent retained directory"' + (currentPath ? "" : " disabled") + '>↑</button><nav class="source-browser-breadcrumbs" aria-label="Retained backup path">' + recoveryFileBreadcrumbs(pointId, currentPath) + '</nav></div><div class="source-browser-context"><div><span>Retained path</span><code>/' + esc(currentPath) + '</code></div><span class="source-browser-scope readonly"><span aria-hidden="true">◇</span> Manifest evidence</span></div><div class="source-browser-column-head" aria-hidden="true"><span>Name</span><span>Action</span></div><div class="source-browser-entries" role="list" aria-label="Retained files and directories in ' + esc(currentPath || "backup root") + '">' + (entries.length ? entries.map((entry) => recoveryFileEntry(entry, currentPath, pointId)).join("") : empty) + '</div><p class="source-browser-live" role="status" aria-live="polite">' + esc(entries.length + (truncated ? "+ retained entries available; partial view loaded" : " retained entries loaded")) + "</p></div></section>";
  }

  const RESTORE_REHEARSAL_PHASES = [
    { name: "Preflight", detail: "Revalidate plan, drive, and tape", aliases: ["queued", "started", "preparing", "preflight"] },
    { name: "Restore files", detail: "Read the complete backup across its ordered tapes", aliases: ["reading", "restoring", "engine_dispatched", "engine_restore", "waiting_for_media", "media_required"] },
    { name: "Verify files", detail: "Hash and compare restored content", aliases: ["comparing", "restored_manifest", "comparison"] },
    { name: "Complete", detail: "Retain restore evidence", aliases: ["passed", "verified", "completed", "complete"] }
  ];

  function restoreRehearsalDetailPayload(rehearsal) {
    const detail = rehearsal && state.data.restoreRehearsalDetails[recordId(rehearsal)];
    return detail && typeof detail === "object" ? detail : {};
  }

  function restoreRehearsalDetailRecord(rehearsal) {
    const detail = restoreRehearsalDetailPayload(rehearsal);
    return detail && (detail.rehearsal || detail.run) || rehearsal;
  }

  function restoreRehearsalEvents(rehearsal) {
    return list(restoreRehearsalDetailPayload(rehearsal), "events");
  }

  function restoreRehearsalArtifacts(rehearsal) {
    return list(restoreRehearsalDetailPayload(rehearsal), "artifacts");
  }

  function restoreRehearsalVolumes(rehearsal) {
    const detail = restoreRehearsalDetailPayload(rehearsal);
    const record = restoreRehearsalDetailRecord(rehearsal) || {};
    const result = record.result && typeof record.result === "object" ? record.result : {};
    let volumes = arrayValue(detail, "volumes", "required_volumes");
    if (!volumes.length) volumes = arrayValue(record, "volumes", "required_volumes");
    if (!volumes.length) volumes = arrayValue(result, "volumes", "required_volumes");
    if (!volumes.length && pick(record, "expected_volume_label", "cartridge_label")) {
      volumes = [{
        volume_order: 1,
        cartridge_id: pick(record, "cartridge_id"),
        cartridge_label: pick(record, "expected_volume_label", "cartridge_label"),
        state: restoreRehearsalState(record)
      }];
    }
    return volumes.map((volume, index) => ({
      raw: volume,
      sequence: Number(pick(volume, "volume_order", "volume_sequence", "sequence", "order") || index + 1),
      cartridgeId: String(pick(volume, "cartridge_id", "media_id") || ""),
      label: String(volumeLabel(volume)),
      state: String(pick(volume, "state", "status", "restore_state") || "pending").toLowerCase()
    })).sort((left, right) => left.sequence - right.sequence);
  }

  function restoreRehearsalMediaRequest(rehearsal) {
    const detail = restoreRehearsalDetailPayload(rehearsal);
    const record = restoreRehearsalDetailRecord(rehearsal) || {};
    const result = record.result && typeof record.result === "object" ? record.result : {};
    const sources = [pick(detail, "media_request", "current_media_request"), pick(record, "media_request", "current_media_request"), pick(result, "media_request", "current_media_request")];
    return sources.find((value) => value && typeof value === "object" && !Array.isArray(value)) || {};
  }

  function restoreRehearsalMediaHandoff(rehearsal) {
    const detail = restoreRehearsalDetailPayload(rehearsal);
    const record = restoreRehearsalDetailRecord(rehearsal) || {};
    const result = record.result && typeof record.result === "object" ? record.result : {};
    const sources = [pick(detail, "media_handoff"), pick(record, "media_handoff"), pick(result, "media_handoff")];
    return sources.find((value) => value && typeof value === "object" && !Array.isArray(value)) || {};
  }

  function restoreHandoffOrder(rehearsal, key) {
    const handoff = restoreRehearsalMediaHandoff(rehearsal);
    const request = restoreRehearsalMediaRequest(rehearsal);
    const value = key === "next"
      ? pick(handoff, "next_volume_order", "volume_order", "volume_sequence") ?? pick(request, "volume_order", "volume_sequence")
      : pick(handoff, "current_volume_order", "loaded_volume_order") ?? pick(request, "current_volume_order");
    const order = Number(value);
    return Number.isInteger(order) && order > 0 ? order : null;
  }

  function restoreHandoffWaiting(rehearsal) {
    const record = restoreRehearsalDetailRecord(rehearsal) || {};
    const handoff = restoreRehearsalMediaHandoff(rehearsal);
    const stateValue = String(pick(handoff, "state") || "").toLowerCase();
    return pick(handoff, "required") === true && (pick(handoff, "waiting") === true || stateValue === "waiting_for_media" || restoreRehearsalPhase(record) === "waiting_for_media");
  }

  function restoreHandoffAccepted(rehearsal) {
    const handoff = restoreRehearsalMediaHandoff(rehearsal);
    const request = restoreRehearsalMediaRequest(rehearsal);
    return [pick(handoff, "state"), pick(request, "state")].some((value) => ["authorized", "loaded", "accepted", "resuming"].includes(String(value || "").toLowerCase()));
  }

  function restoreHandoffLabel(rehearsal) {
    const handoff = restoreRehearsalMediaHandoff(rehearsal);
    const request = restoreRehearsalMediaRequest(rehearsal);
    return String(pick(handoff, "next_cartridge_label", "next_volume_label", "expected_cartridge_label", "cartridge_label") || pick(request, "expected_cartridge_label", "cartridge_label", "expected_volume_label") || "");
  }

  function restoreHandoffTotal(rehearsal) {
    const total = Number(pick(restoreRehearsalMediaHandoff(rehearsal), "total_volumes", "volume_count") || restoreRehearsalVolumes(rehearsal).length);
    return Number.isInteger(total) && total > 0 ? total : restoreRehearsalVolumes(rehearsal).length;
  }

  function renderRestoreVolumeSequence(rehearsal) {
    const volumes = restoreRehearsalVolumes(rehearsal);
    if (!volumes.length) return "";
    const record = restoreRehearsalDetailRecord(rehearsal);
    const runState = restoreRehearsalState(record);
    const passed = ["passed", "verified", "completed", "complete"].includes(runState);
    const waiting = restoreHandoffWaiting(rehearsal);
    const accepted = restoreHandoffAccepted(rehearsal);
    const nextOrder = restoreHandoffOrder(rehearsal, "next");
    const loadedVolume = volumes.find((volume) => volume.state === "loaded");
    const currentOrder = restoreHandoffOrder(rehearsal, "current") || (loadedVolume && loadedVolume.sequence) || (accepted && nextOrder) || (!waiting && restoreRehearsalActive(record) ? 1 : null);
    const rows = volumes.map((volume) => {
      const explicitDone = ["read", "restored", "completed", "complete", "verified"].includes(volume.state);
      const done = passed || explicitDone || Boolean(nextOrder && volume.sequence < nextOrder);
      const next = waiting && volume.sequence === nextOrder;
      const current = !next && !done && volume.sequence === currentOrder;
      const stateText = done ? "Read" : next ? "Required next" : current ? accepted ? "Accepted; rechecking" : "Loaded now" : "Pending";
      return '<li class="' + (done ? "done" : next ? "next" : current ? "current" : "pending") + '"' + (next || current ? ' aria-current="step"' : "") + '><span class="restore-volume-marker">' + esc(done ? "✓" : volume.sequence) + '</span><span><strong>Tape ' + esc(volume.sequence) + '</strong><code>' + esc(volume.label) + '</code><small>' + esc(stateText) + "</small></span></li>";
    }).join("");
    return '<div class="backup-volume-plan restore-volume-plan"><span>Exact ordered restore media</span><ol>' + rows + "</ol></div>";
  }

  function restoreRehearsalPhase(rehearsal) {
    const record = restoreRehearsalDetailRecord(rehearsal);
    const events = restoreRehearsalEvents(rehearsal).slice().sort((left, right) => new Date(pick(right, "occurred_at", "created_at") || 0) - new Date(pick(left, "occurred_at", "created_at") || 0));
    const latest = events[0];
    const evidence = latest && latest.evidence && typeof latest.evidence === "object" ? latest.evidence : {};
    const result = record && record.result && typeof record.result === "object" ? record.result : {};
    return String(pick(record, "phase", "current_phase") || pick(result, "phase", "current_phase") || pick(evidence, "phase", "current_phase") || pick(latest, "event_type", "to_state", "phase") || restoreRehearsalState(record)).toLowerCase();
  }

  function restoreRehearsalPhaseIndex(rehearsal) {
    const record = restoreRehearsalDetailRecord(rehearsal);
    if (["passed", "verified", "completed", "complete"].includes(restoreRehearsalState(record))) return RESTORE_REHEARSAL_PHASES.length - 1;
    const phase = restoreRehearsalPhase(rehearsal);
    const index = RESTORE_REHEARSAL_PHASES.findIndex((item) => item.aliases.includes(phase));
    return index >= 0 ? index : 0;
  }

  function renderRestoreRehearsalTrack(rehearsal) {
    const current = restoreRehearsalPhaseIndex(rehearsal);
    const currentState = restoreRehearsalState(restoreRehearsalDetailRecord(rehearsal));
    const passed = ["passed", "verified", "completed", "complete"].includes(currentState);
    const failed = ["failed", "blocked", "interrupted", "cancelled", "canceled"].includes(currentState);
    return '<div class="phase-track restore-rehearsal-track" aria-label="Restore run phases">' + RESTORE_REHEARSAL_PHASES.map((phase, index) => {
      const phaseClass = passed || index < current ? " done" : index === current ? failed ? " failed" : " current" : "";
      return '<div class="phase' + phaseClass + '"' + (index === current && !passed ? ' aria-current="step"' : "") + '><span class="phase-marker">' + esc(passed || index < current ? "✓" : index + 1) + '</span><strong>' + esc(phase.name) + '</strong><small>' + esc(phase.detail) + "</small></div>";
    }).join("") + "</div>";
  }

  function renderRestoreRehearsalTimeline(rehearsal) {
    const events = restoreRehearsalEvents(rehearsal);
    if (!events.length) return '<div class="vault-section-empty compact">The first durable worker event has not arrived yet.</div>';
    return '<ol class="timeline restore-rehearsal-timeline">' + events.map((event) => {
      const evidence = event && event.evidence && typeof event.evidence === "object" ? event.evidence : {};
      const detail = pick(evidence, "detail", "message", "path", "reason") || pick(event, "detail", "reason") || "Evidence recorded";
      return '<li class="timeline-item"><time class="timeline-time">' + esc(date(pick(event, "occurred_at", "created_at"))) + '</time><span class="timeline-marker" aria-hidden="true"></span><div class="timeline-copy"><strong>' + esc(human(pick(event, "event_type", "to_state", "phase") || "event")) + '</strong><span>' + esc(detail) + "</span></div></li>";
    }).join("") + "</ol>";
  }

  function renderRestoreRehearsalArtifacts(rehearsal) {
    const artifacts = restoreRehearsalArtifacts(rehearsal);
    if (!artifacts.length) return '<div class="vault-section-empty compact">Evidence artifacts will appear as physical phases finish.</div>';
    return '<ul class="vault-artifact-list restore-rehearsal-artifacts">' + artifacts.map((artifact) => '<li><div><strong>' + esc(human(pick(artifact, "artifact_kind", "kind") || "artifact")) + '</strong><span>' + esc(pick(artifact, "size_bytes") == null ? "Retained evidence" : bytes(pick(artifact, "size_bytes"))) + '</span></div><code>' + esc(label(pick(artifact, "sha256", "digest"))) + "</code></li>").join("") + "</ul>";
  }

  function renderRestoreRehearsal(rehearsal) {
    if (!rehearsal) return "";
    const id = recordId(rehearsal);
    const record = restoreRehearsalDetailRecord(rehearsal);
    const runState = restoreRehearsalState(record);
    const active = restoreRehearsalActive(record);
    const error = state.errors["restoreRehearsal:" + id];
    const result = record && record.result && typeof record.result === "object" ? record.result : {};
    const resultDetail = pick(record, "failure_detail", "detail") || pick(result, "failure_detail", "detail", "message");
    const volumes = restoreRehearsalVolumes(rehearsal);
    const multiVolume = volumes.length > 1;
    const waiting = restoreHandoffWaiting(rehearsal);
    const mediaAccepted = restoreHandoffAccepted(rehearsal);
    const nextOrder = restoreHandoffOrder(rehearsal, "next");
    const loadedVolume = volumes.find((volume) => volume.state === "loaded");
    const currentOrder = restoreHandoffOrder(rehearsal, "current") || (loadedVolume && loadedVolume.sequence) || (mediaAccepted && nextOrder) || (!waiting && active ? 1 : null);
    const currentVolume = volumes.find((volume) => volume.sequence === currentOrder);
    const nextLabel = restoreHandoffLabel(rehearsal) || (volumes.find((volume) => volume.sequence === nextOrder) || {}).label || "";
    const totalVolumes = restoreHandoffTotal(rehearsal) || volumes.length;
    const facts = [
      ["Run", id],
      ["Plan", pick(record, "plan_id", "restore_plan_id")],
      ["Drive", pick(record, "expected_stable_id", "drive_stable_id", "drive_id")],
      [volumes.length === 1 ? "Cartridge" : "Archive tapes", volumes.length ? volumes.map((volume) => volume.sequence + ". " + volume.label).join(" → ") : pick(record, "expected_volume_label", "cartridge_label", "expected_cartridge_label")],
      ["Current tape", currentVolume ? currentVolume.sequence + ". " + currentVolume.label : null],
      ["Next tape", waiting ? (nextOrder ? nextOrder + ". " : "") + (nextLabel || "Exact recorded tape") : null],
      ["Destination", pick(record, "destination_subpath", "destination", "restore_destination")],
      ["Started", date(pick(record, "started_at", "created_at"))],
      ["Finished", pick(record, "finished_at") ? date(pick(record, "finished_at")) : null]
    ].filter((item) => item[1] !== null && item[1] !== undefined && item[1] !== "");
    let outcome = '<div class="notice info"><span class="notice-icon" aria-hidden="true">▶</span><div class="notice-copy"><strong>' + esc(human(restoreRehearsalPhase(rehearsal))) + '</strong><span>Keep ' + esc(currentVolume ? currentVolume.label : "the requested tape") + ' loaded and the isolated destination online. Cancellation is honored only at a safe boundary.</span></div></div>';
    if (mediaAccepted) outcome = '<div class="notice info restore-media-handoff"><span class="notice-icon" aria-hidden="true">✓</span><div class="notice-copy"><strong>Tape accepted; Restore run is resuming</strong><span>Keep the selected drive and exact loaded tape available while Bacula validates its on-tape label and continues the same restore job.</span></div></div>';
    if (waiting) outcome = '<div class="notice warning restore-media-handoff"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Load tape ' + esc(nextOrder || "?") + ' of ' + esc(totalVolumes || "?") + ': ' + esc(nextLabel || "exact recorded cartridge") + '</strong><span>The Restore run is durably paused at a read-only media boundary. Unload the previous tape, insert this exact next cartridge, and confirm it below. TapeScale will not erase, relabel, or append to it.</span></div><button class="button button-primary" type="button" data-action="open-restore-media-handoff" data-id="' + esc(id) + '">Load exact next tape</button></div>';
    if (["passed", "verified", "completed", "complete"].includes(runState)) outcome = '<div class="notice success"><span class="notice-icon" aria-hidden="true">✓</span><div class="notice-copy"><strong>Restore run passed</strong><span>Bacula restored the complete retained recovery point from physical tape and TapeScale independently matched the restored manifest.</span></div></div>';
    if (["failed", "blocked", "interrupted", "cancelled", "canceled"].includes(runState)) outcome = '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Restore run ' + esc(runState) + '</strong><span>' + esc(resultDetail || "Inspect the durable activity below for the terminal evidence.") + "</span></div></div>";
    const cancel = active ? '<button class="button button-danger button-small" type="button" data-action="open-cancel-restore-rehearsal" data-id="' + esc(id) + '">Request safe cancel</button>' : "";
    return '<article class="restore-rehearsal-run"><header><div><span class="vault-kicker">Attended ' + (multiVolume ? "multi-tape" : "single-tape") + ' restore</span><h4>Restore run</h4><code>' + esc(id) + '</code></div><div class="restore-rehearsal-actions">' + status(runState) + '<button class="button button-quiet button-small" type="button" data-action="download-restore-rehearsal-evidence" data-id="' + esc(id) + '">Download restore evidence</button>' + cancel + '</div></header>' + renderRestoreRehearsalTrack(rehearsal) + '<div class="restore-rehearsal-body">' + (error ? '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not refresh run evidence</strong><span>' + esc(errorMessage(error)) + '</span></div></div>' : "") + outcome + renderRestoreVolumeSequence(rehearsal) + '<dl class="restore-result-facts">' + facts.map(([name, value]) => '<dt>' + esc(name) + '</dt><dd class="mono">' + esc(value) + "</dd>").join("") + '</dl><div class="restore-rehearsal-evidence-grid"><section><h5>Restore activity</h5>' + renderRestoreRehearsalTimeline(rehearsal) + '</section><section><h5>Restore evidence</h5>' + renderRestoreRehearsalArtifacts(rehearsal) + "</section></div></div></article>";
  }

  function renderRestoreRehearsals(pointId) {
    const runs = restoreRehearsalsForPoint(pointId);
    if (!runs.length) return '<section class="vault-dossier-section restore-rehearsals"><div class="vault-section-head"><div><h3>Restore runs</h3><p>A supported verified Full recovery point can run an attended one- or multi-tape restore into isolation when the server offers that exact execution contract.</p></div><span>0 runs</span></div><div class="vault-section-empty">No Restore run has been started for this recovery point.</div></section>';
    const selected = selectedRestoreRehearsal();
    const choices = runs.length > 1 ? '<div class="restore-rehearsal-picker" role="list" aria-label="Restore run history">' + runs.map((run) => {
      const volumes = restoreRehearsalVolumes(run);
      const nextOrder = restoreHandoffOrder(run, "next");
      const nextLabel = restoreHandoffLabel(run);
      const mediaSummary = restoreHandoffWaiting(run)
        ? "Needs tape " + (nextOrder || "?") + (nextLabel ? " · " + nextLabel : "")
        : volumes.length ? volumes.length + " tape" + (volumes.length === 1 ? "" : "s") + " · " + volumes.map((volume) => volume.label).join(" → ") : recordId(run);
      return '<button class="' + (recordId(run) === recordId(selected) ? "active" : "") + '" type="button" data-action="select-restore-rehearsal" data-id="' + esc(recordId(run)) + '"><span>' + status(restoreRehearsalState(run)) + '</span><strong>' + esc(date(pick(run, "created_at", "started_at"))) + '</strong><small>' + esc(mediaSummary) + "</small></button>";
    }).join("") + "</div>" : "";
    if (selected && !state.data.restoreRehearsalDetails[recordId(selected)] && !state.restoreRehearsalDetailLoading.has(recordId(selected)) && !state.errors["restoreRehearsal:" + recordId(selected)]) window.setTimeout(() => loadRestoreRehearsalDetail(recordId(selected)), 0);
    return '<section class="vault-dossier-section restore-rehearsals"><div class="vault-section-head"><div><h3>Restore runs</h3><p>Every attempt retains its ordered media, current handoff, physical read, file verification, and cancellation evidence.</p></div><span>' + runs.length + " run" + (runs.length === 1 ? "" : "s") + "</span></div>" + choices + renderRestoreRehearsal(selected) + "</section>";
  }

  function renderRecoveryPointDossier(summary) {
    const id = recordId(summary);
    const detail = recoveryPointDetail(id);
    const point = detail && detail.recovery_point && typeof detail.recovery_point === "object" ? detail.recovery_point : summary;
    const volumes = recoveryPointVolumes(detail, point);
    const verifications = detail ? list(detail, "verifications") : [];
    const dependencies = detail ? list(detail, "dependencies") : [];
    const holds = detail ? list(detail, "holds") : [];
    const artifacts = detail ? list(detail, "artifacts") : [];
    const events = detail ? list(detail, "events") : [];
    const origin = detail && detail.origin && typeof detail.origin === "object" ? detail.origin : {};
    const detailError = state.errors["recoveryPoint:" + id];
    const loadingDetail = state.recoveryPointDetailLoading.has(id) || (!detail && !detailError);
    const dataset = pick(point, "dataset_name", "source", "dataset", "logical_root");
    const representation = recoveryPointRepresentation(point);
    const fullReadback = evidenceOutcome(verifications, "full_readback");
    const restoreTest = evidenceOutcome(verifications, "restore_test");
    const coverage = point && point.coverage && typeof point.coverage === "object" ? point.coverage : {};
    const facts = [
      ["Recovery time", date(pick(point, "recovery_time", "created_at"))],
      ["Representation", human(representation)],
      ["Backup level", human(pick(point, "level") || "unknown")],
      ["Format version", pick(point, "format_version")],
      ["Source scope", pick(point, "source_scope_hash")],
      ["Manifest SHA-256", pick(point, "manifest_digest")],
      ["Logical size", (() => { const value = recoveryPointLogicalBytes(point); return value == null ? null : bytes(value); })()],
      ["Expires", pick(point, "indefinite") === true ? "Indefinite retention" : date(pick(point, "expires_at"))],
      ["Key lineage", pick(point, "key_lineage_id") || "No key lineage recorded"]
    ].filter((item) => item[1] !== null && item[1] !== undefined && item[1] !== "");
    let feedback = "";
    if (loadingDetail) feedback = '<div class="notice info vault-detail-notice"><span class="notice-icon" aria-hidden="true">↻</span><div class="notice-copy"><strong>Loading durable recovery evidence</strong><span>The summary is visible while TapeScale assembles the ordered media and verification record.</span></div></div>';
    if (detailError) feedback = '<div class="notice danger vault-detail-notice"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not load the recovery dossier</strong><span>' + esc(errorMessage(detailError)) + '</span></div><button class="button button-small" type="button" data-action="retry-recovery-point" data-id="' + esc(id) + '">Retry</button></div>';
    return '<article class="panel vault-dossier"><header class="vault-dossier-hero"><div><span class="vault-kicker">Recovery point · ' + esc(human(representation)) + '</span><div class="vault-dossier-title"><h2>' + esc(label(dataset)) + '</h2>' + status(recoveryPointState(point)) + '</div><p class="mono">' + esc(id) + '</p><span>' + esc(date(pick(point, "recovery_time", "created_at"))) + " · " + esc(human(pick(point, "level") || "unknown")) + ' backup</span></div><div class="vault-dossier-actions"><button class="button button-quiet" type="button" data-action="download-recovery-evidence" data-id="' + esc(id) + '">Download evidence</button><button class="button button-primary" type="button" data-action="open-restore-plan" data-id="' + esc(id) + '">Plan a restore</button></div></header><div class="vault-plan-boundary"><strong>Plan first; run only when explicitly offered</strong><span>Current Alpha execution is limited to supported verified Full recovery points from operational backups or the recovery exercise. A Restore run recovers the complete point across its exact ordered tape set into a new isolated destination after attended drive and media confirmation. File or directory selection, encrypted recovery points, and clean-catalog recovery are not available yet.</span></div>' + feedback + renderRecoveryPointFiles(id) + renderRestoreRehearsals(id) + '<section class="vault-proof-grid">' + renderVaultProof("Full media readback", fullReadback, verifications.length ? "Recorded independently of write completion" : "Loading verification evidence") + renderVaultProof("Isolated restore test", restoreTest, verifications.length ? "Application recovery evidence" : "Loading verification evidence") + renderVaultProof("Required media", volumes.length ? "recorded" : "missing", volumes.length + " cartridge" + (volumes.length === 1 ? "" : "s") + " in read order") + renderVaultProof("Dependencies", dependencies.length ? "required" : "ready", dependencies.length ? dependencies.length + " prerequisite recovery point(s)" : "No earlier recovery point required") + '</section><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Required media in read order</h3><p>Load cartridges in this exact volume sequence. Engine record boundaries are retained for reconstruction.</p></div><span>' + volumes.length + " volume" + (volumes.length === 1 ? "" : "s") + '</span></div>' + renderRecoveryVolumes(volumes) + '</section><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Verification evidence</h3><p>Write completion, full readback, and restore testing remain separate claims.</p></div><span>' + verifications.length + " record" + (verifications.length === 1 ? "" : "s") + '</span></div>' + renderRecoveryVerifications(verifications) + '</section><div class="vault-two-column"><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Recovery contract</h3><p>The durable identity and retention boundary.</p></div></div><dl class="vault-facts">' + facts.map(([name, value]) => "<dt>" + esc(name) + '</dt><dd class="' + (/hash|manifest|lineage|format/i.test(name) ? "mono" : "") + '">' + esc(value) + "</dd>").join("") + '</dl>' + (Object.keys(coverage).length ? '<details class="vault-json"><summary>Recorded coverage</summary><pre>' + esc(JSON.stringify(coverage, null, 2)) + "</pre></details>" : "") + '</section><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Dependencies & holds</h3><p>Every prerequisite remains visible before planning.</p></div></div><h4>Dependencies</h4>' + renderDependencyList(dependencies, "This is a self-contained full recovery point.") + '<h4>Active holds</h4>' + renderDependencyList(holds, "No active hold was returned.") + '</section></div><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Origin & retained evidence</h3><p>Correlation data for the job, physical exercise, catalog, and content-addressed artifacts.</p></div></div><div class="vault-origin-grid"><div><h4>Origin</h4>' + renderOrigin(origin, point) + '</div><div><h4>Artifacts</h4>' + renderRecoveryArtifacts(artifacts) + "</div></div>" + renderRecoveryEventsForVault(events) + "</section></article>";
  }

  function renderRestore() {
    const points = orderedRecoveryPoints();
    let html = pageHead("Recovery vault", "Inspect durable recovery points, plan safely, and run attended one- or multi-tape restores where the retained Full backup format is supported.", refreshButton()) + noticeFor("recoveryPoints") + noticeFor("restoreRehearsals");
    if (!points.length) return html + empty("No recovery points", state.errors.recoveryPoints ? "Recovery points could not be loaded." : "A verified Full backup or physical recovery exercise will create the first durable recovery point.", '<a class="button button-primary" href="#jobs">Open Backup Center</a> <a class="button button-quiet" href="#recovery-lab">Open recovery exercise</a>');
    const selected = selectedRecoveryPoint();
    const verified = points.filter((point) => recoveryPointState(point) === "verified").length;
    const tapes = new Set(points.flatMap(recoveryPointMediaLabels));
    const logicalBytes = points.reduce((total, point) => {
      const value = Number(recoveryPointLogicalBytes(point));
      return total + (Number.isFinite(value) && value >= 0 ? value : 0);
    }, 0);
    const lastVerified = points.find((point) => recoveryPointState(point) === "verified");
    html += '<section class="panel metric-strip vault-metrics"><div class="metric"><span>Recovery points</span><strong>' + points.length + '</strong><small>All retained outcomes</small></div><div class="metric"><span>Verified</span><strong>' + verified + '</strong><small>Ready for prerequisite planning</small></div><div class="metric"><span>Cataloged media</span><strong>' + tapes.size + '</strong><small>Unique cartridge labels</small></div><div class="metric"><span>Protected data</span><strong>' + (logicalBytes ? esc(bytes(logicalBytes)) : "—") + '</strong><small>' + (lastVerified ? "Latest verified " + esc(date(pick(lastVerified, "recovery_time", "created_at"))) : "No verified point yet") + "</small></div></section>";
    html += '<div class="vault-layout section">' + renderRecoveryPointCatalog(points, selected) + renderRecoveryPointDossier(selected) + "</div>";
    const selectedId = recordId(selected);
    if (selectedId && !recoveryPointDetail(selectedId) && !state.recoveryPointDetailLoading.has(selectedId) && !state.errors["recoveryPoint:" + selectedId]) {
      window.setTimeout(() => loadRecoveryPointDetail(selectedId), 0);
    }
    return html;
  }

  function orderedRecoveryExercises() {
    return state.data.recoveryExercises.slice().sort((left, right) => new Date(pick(right, "created_at", "started_at") || 0) - new Date(pick(left, "created_at", "started_at") || 0));
  }

  function recoveryExerciseState(exercise) {
    return String(pick(exercise, "state", "status") || "queued").toLowerCase();
  }

  function recoveryExercisePhase(exercise) {
    const result = exercise && exercise.result && typeof exercise.result === "object" ? exercise.result : {};
    const evidence = result.evidence && typeof result.evidence === "object" ? result.evidence : {};
    const detail = exercise && state.data.recoveryExerciseDetails[recordId(exercise)];
    const events = detail ? list(detail, "events") : [];
    const orderedEvents = events.slice().sort((left, right) => {
      const timeDifference = new Date(pick(right, "occurred_at", "created_at") || 0) - new Date(pick(left, "occurred_at", "created_at") || 0);
      return timeDifference || Number(pick(right, "id") || 0) - Number(pick(left, "id") || 0);
    });
    const latestEvent = orderedEvents[0];
    const latestEvidence = latestEvent && latestEvent.evidence && typeof latestEvent.evidence === "object" ? latestEvent.evidence : {};
    const phase = String(pick(exercise, "phase", "current_phase") || pick(result, "phase", "current_phase") || pick(evidence, "phase", "current_phase") || pick(latestEvidence, "phase", "current_phase") || pick(latestEvent, "phase", "to_state", "event_type") || recoveryExerciseState(exercise)).toLowerCase();
    const recognized = RECOVERY_PHASES.some((item) => item.aliases.includes(phase));
    if (!recoveryExerciseFailed(exercise) || recognized) return phase;
    for (const event of orderedEvents) {
      const eventEvidence = event && event.evidence && typeof event.evidence === "object" ? event.evidence : {};
      const prior = String(pick(eventEvidence, "phase", "current_phase") || pick(event, "phase", "to_state", "event_type") || "").toLowerCase();
      if (RECOVERY_PHASES.some((item) => item.aliases.includes(prior))) return prior;
    }
    return "preflight";
  }

  function recoveryExerciseActive(exercise) {
    if (!exercise || pick(exercise, "finished_at")) return false;
    const value = recoveryExerciseState(exercise);
    if (RECOVERY_TERMINAL_STATES.has(value)) return false;
    return true;
  }

  function recoveryExerciseFailed(exercise) {
    return ["failed", "blocked", "interrupted", "cancelled", "canceled"].includes(recoveryExerciseState(exercise));
  }

  function recoveryExercisePassed(exercise) {
    return ["passed", "verified", "completed", "complete"].includes(recoveryExerciseState(exercise));
  }

  function recoveryProfileName(profile) {
    return String(pick(profile, "name") || "Unnamed recovery profile");
  }

  function recoveryExerciseProfileName(exercise) {
    const direct = pick(exercise, "profile_name", "expected_profile_name", "name");
    if (direct) return String(direct);
    const profileId = String(pick(exercise, "profile_id") || "");
    const profile = state.data.recoveryProfiles.find((item) => recordId(item) === profileId);
    return profile ? recoveryProfileName(profile) : "Recovery exercise";
  }

  function recoveryExerciseDetail(exercise) {
    const result = exercise && exercise.result && typeof exercise.result === "object" ? exercise.result : {};
    const evidence = result.evidence && typeof result.evidence === "object" ? result.evidence : {};
    const detail = pick(exercise, "failure_detail", "detail") || pick(result, "failure_detail", "detail", "message") || pick(evidence, "failure_detail", "detail", "message");
    if (detail && typeof detail === "object") return JSON.stringify(detail);
    return detail ? String(detail) : null;
  }

  function recoveryExerciseRecord(exercise) {
    if (!exercise) return null;
    const detail = state.data.recoveryExerciseDetails[recordId(exercise)];
    const detailedExercise = detail && (detail.exercise || detail.run || (recordId(detail) === recordId(exercise) ? detail : null));
    return detailedExercise && typeof detailedExercise === "object" ? detailedExercise : exercise;
  }

  function selectedRecoveryExercise() {
    const exercises = orderedRecoveryExercises();
    const selected = state.selectedRecoveryExerciseId && exercises.find((exercise) => recordId(exercise) === state.selectedRecoveryExerciseId);
    return recoveryExerciseRecord(selected || exercises.find(recoveryExerciseActive) || exercises[0] || null);
  }

  function recoveryLabEnabled() {
    return pick(state.data.system, "lab_file_roundtrip_enabled") === true;
  }

  function sameNullable(left, right) {
    return (left === undefined || left === null ? null : left) === (right === undefined || right === null ? null : right);
  }

  function recoveryQualificationTupleMatches(run, drive, item) {
    if (!run || !drive || !item) return false;
    return String(pick(run, "drive_id") || "") === recordId(drive)
      && String(pick(run, "cartridge_id") || "") === recordId(item)
      && String(pick(run, "expected_stable_id") || "") === String(pick(drive, "stable_id") || "")
      && String(pick(run, "expected_path") || "") === String(pick(drive, "observed_path") || "")
      && sameNullable(run.expected_vendor, drive.vendor)
      && sameNullable(run.expected_model, drive.model)
      && sameNullable(run.expected_serial, drive.serial)
      && sameNullable(run.expected_wwn, drive.wwn)
      && sameNullable(run.expected_firmware, drive.firmware)
      && String(pick(run, "expected_cartridge_label") || "") === String(mediaName(item) || "")
      && sameNullable(run.expected_media_generation, item.media_generation)
      && sameNullable(run.expected_media_format, item.media_format)
      && pick(run, "engine_name") === "Bacula Community"
      && pick(run, "engine_version") === "15.0.3"
      && pick(run, "test_kind") === "btape_test";
  }

  function recoveryQualificationMatches(run, drive, item) {
    if (!recoveryQualificationTupleMatches(run, drive, item)) return false;
    const latest = state.data.qualificationRuns
      .filter((candidate) => recoveryQualificationTupleMatches(candidate, drive, item))
      .sort((left, right) => {
        const timeDifference = new Date(pick(right, "created_at") || 0) - new Date(pick(left, "created_at") || 0);
        return timeDifference || recordId(right).localeCompare(recordId(left));
      })[0];
    return latest && recordId(latest) === recordId(run) && recoveryExerciseState(run) === "passed" && Boolean(pick(run, "finished_at"));
  }

  function recoveryDriveSafetyBlocker(drive) {
    const health = drive && drive.health && typeof drive.health === "object" ? drive.health : {};
    const observedAt = new Date(pick(drive, "last_observed_at") || 0).getTime();
    const current = Number.isFinite(observedAt) && Date.now() - observedAt <= 120000;
    const activeExercise = state.data.recoveryExercises.some((exercise) => recoveryExerciseActive(exercise) && (String(pick(exercise, "drive_id")) === recordId(drive) || String(pick(exercise, "expected_stable_id")) === String(pick(drive, "stable_id"))));
    if (activeExercise) return "A recovery exercise already owns this drive.";
    if (!current) return "The drive observation is stale.";
    if (health.present !== true || health.accessible !== true) return "The configured tape device is not accessible.";
    if (health.identity_verified !== true || health.identity_conflict === true) return "The stable SCSI identity is not safe.";
    if (drive.reconciled !== true) return "Trust the exact drive identity first.";
    if (!pick(drive, "observed_path") || pick(drive, "observed_path") !== pick(drive, "expected_path")) return "The current tape path does not match the trusted path.";
    if (health.motion_position_uncertain === true) return "The prior tape position is uncertain; reconcile the drive.";
    if (health.immediate_mode !== false || health.in_flight !== 0) return "Kernel tape I/O state is not safe.";
    if (health.medium_loaded !== true || health.ready !== true) return "Load the designated scratch cartridge and wait for ready.";
    return null;
  }

  function recoveryMediaSafetyBlocker(item) {
    const activeExercise = state.data.recoveryExercises.some((exercise) => recoveryExerciseActive(exercise) && String(pick(exercise, "cartridge_id")) === recordId(item));
    if (activeExercise) return "A recovery exercise already reserves this cartridge.";
    if (item.worm === true) return "WORM media cannot be used for this destructive lab exercise.";
    if (count(pick(item, "active_holds")) > 0) return "The cartridge has an active hold.";
    if (pick(item, "has_content") === true) return "The cartridge already contains a retained recovery point.";
    if (pick(item, "reserved_by")) return "The cartridge is reserved by another workflow.";
    if (["foreign", "mismatch"].includes(String(pick(item, "identity_status") || ""))) return "The cartridge identity is foreign or mismatched.";
    if (String(pick(item, "health_status") || "") === "write_unverified") return "A prior write is unverified; run the destructive btape qualification again.";
    return null;
  }

  function recoveryDriveBlocker(drive) {
    const safetyBlocker = recoveryDriveSafetyBlocker(drive);
    if (safetyBlocker) return safetyBlocker;
    const hasQualifiedPair = state.data.media.some((item) => recoveryMediaSafetyBlocker(item) === null && state.data.qualificationRuns.some((run) => recoveryQualificationMatches(run, drive, item)));
    return hasQualifiedPair ? null : "Pass Bacula btape qualification for this exact drive and an eligible scratch cartridge first.";
  }

  function eligibleRecoveryDrives() {
    return state.data.drives.filter((drive) => recoveryDriveBlocker(drive) === null);
  }

  function recoveryMediaBlocker(item, drive) {
    const safetyBlocker = recoveryMediaSafetyBlocker(item);
    if (safetyBlocker) return safetyBlocker;
    const drives = drive ? [drive] : state.data.drives.filter((candidate) => recoveryDriveSafetyBlocker(candidate) === null);
    const passedQualification = drives.some((candidate) => state.data.qualificationRuns.some((run) => recoveryQualificationMatches(run, candidate, item)));
    return passedQualification ? null : "Pass Bacula btape qualification with this exact drive and cartridge first.";
  }

  function eligibleRecoveryMedia(drive) {
    return state.data.media.filter((item) => recoveryMediaBlocker(item, drive) === null);
  }

  function renderRecoveryOverviewCallout() {
    if (!state.received.has("recoveryExercises") && state.errors.recoveryExercises) return "";
    const latest = recoveryExerciseRecord(orderedRecoveryExercises()[0]);
    if (!latest) {
      return '<section class="recovery-callout section"><div><span class="lab-badge">G0.2 laboratory workflow</span><h2>Create your first recoverable tape</h2><p>Write a complete prepared fixture to one scratch cartridge, decode it from the physical tape, restore it into isolation, and compare the result.</p></div><a class="button button-primary" href="#recovery-lab">Start guided setup</a></section>';
    }
    const stateValue = recoveryExerciseState(latest);
    const title = recoveryExercisePassed(latest) ? "A physical recovery exercise passed" : recoveryExerciseActive(latest) ? "Physical recovery exercise in progress" : "Review the latest recovery exercise";
    const detail = recoveryExerciseDetail(latest) || (recoveryExercisePassed(latest) ? "TapeScale retained the write, readback, isolated restore, and comparison evidence." : "Open the laboratory workflow for the exact phase and retained evidence.");
    return '<section class="recovery-callout section ' + (recoveryExercisePassed(latest) ? "passed" : recoveryExerciseFailed(latest) ? "failed" : "") + '"><div><span class="lab-badge">G0.2 laboratory workflow</span><h2>' + esc(title) + '</h2><p>' + esc(detail) + '</p></div><a class="button button-primary" href="#recovery-lab">Open ' + esc(human(stateValue)) + " run</a></section>";
  }

  function recoveryPhaseIndex(exercise) {
    if (recoveryExercisePassed(exercise)) return RECOVERY_PHASES.length - 1;
    const rawExplicit = recoveryEvidenceValue(exercise, "phase_index");
    const explicit = rawExplicit == null ? NaN : Number(rawExplicit);
    if (Number.isInteger(explicit) && explicit >= 0 && explicit < RECOVERY_PHASES.length) return explicit;
    const phase = recoveryExercisePhase(exercise);
    const index = RECOVERY_PHASES.findIndex((item) => item.aliases.includes(phase));
    return index >= 0 ? index : 0;
  }

  function renderRecoveryPhaseTrack(exercise) {
    const current = recoveryPhaseIndex(exercise);
    const passed = recoveryExercisePassed(exercise);
    const failed = recoveryExerciseFailed(exercise);
    return '<div class="phase-track recovery-phase-track" aria-label="Recovery exercise phases">' + RECOVERY_PHASES.map((phase, index) => {
      const phaseClass = passed || index < current ? " done" : index === current ? failed ? " failed" : " current" : "";
      const marker = passed || index < current ? "✓" : index + 1;
      return '<div class="phase' + phaseClass + '"' + (index === current && !passed ? ' aria-current="step"' : "") + '><span class="phase-marker">' + esc(marker) + '</span><strong>' + esc(phase.name) + '</strong><small>' + esc(phase.detail) + "</small></div>";
    }).join("") + "</div>";
  }

  function recoveryEvidenceValue(exercise, ...keys) {
    const result = exercise && exercise.result && typeof exercise.result === "object" ? exercise.result : {};
    const evidence = result.evidence && typeof result.evidence === "object" ? result.evidence : {};
    const direct = pick(exercise, ...keys);
    if (direct !== null) return direct;
    const resultValue = pick(result, ...keys);
    if (resultValue !== null) return resultValue;
    const resultEvidence = pick(evidence, ...keys);
    if (resultEvidence !== null) return resultEvidence;
    const detail = state.data.recoveryExerciseDetails[recordId(exercise)];
    const events = detail ? list(detail, "events").slice().reverse() : [];
    for (const event of events) {
      const eventEvidence = event && event.evidence && typeof event.evidence === "object" ? event.evidence : {};
      const value = pick(eventEvidence, ...keys);
      if (value !== null) return value;
    }
    const artifacts = detail ? list(detail, "artifacts").slice().reverse() : [];
    for (const artifact of artifacts) {
      const metadata = artifact && artifact.metadata && typeof artifact.metadata === "object" ? artifact.metadata : {};
      const value = pick(metadata, ...keys);
      if (value !== null) return value;
    }
    return null;
  }

  function recoveryArtifactValue(exercise, kind, ...keys) {
    const detail = state.data.recoveryExerciseDetails[recordId(exercise)];
    const artifact = detail && list(detail, "artifacts").find((item) => pick(item, "artifact_kind", "kind") === kind);
    if (!artifact) return null;
    const metadata = artifact.metadata && typeof artifact.metadata === "object" ? artifact.metadata : {};
    return pick(metadata, ...keys) || pick(artifact, "sha256", "digest_sha256", "digest");
  }

  function renderRecoveryEvidence(exercise) {
    const sourceManifest = recoveryEvidenceValue(exercise, "source_manifest_sha256", "source_manifest_digest", "manifest_sha256") || recoveryArtifactValue(exercise, "source_manifest", "manifest_digest");
    const restoredManifest = recoveryEvidenceValue(exercise, "restored_manifest_sha256", "restore_manifest_digest") || recoveryArtifactValue(exercise, "restored_manifest", "manifest_digest");
    const evidence = [
      ["Source entries", recoveryEvidenceValue(exercise, "source_entry_count", "entry_count", "file_count")],
      ["Logical bytes", (() => { const value = recoveryEvidenceValue(exercise, "logical_bytes", "source_bytes"); return value == null ? null : bytes(value); })()],
      ["Source manifest", sourceManifest],
      ["Restored manifest", restoredManifest],
      ["Restore destination", recoveryEvidenceValue(exercise, "restore_destination", "restore_path", "restored_path")],
      ["Recovery point", recoveryEvidenceValue(exercise, "recovery_point_id")],
      ["Engine job", recoveryEvidenceValue(exercise, "engine_job_id", "bacula_job_id", "engine_operation_id", "backup_job_id")],
      ["Cartridge", pick(exercise, "expected_cartridge_label", "cartridge_label")]
    ].filter((item) => item[1] !== null && item[1] !== undefined && item[1] !== "");
    if (!evidence.length) return '<p class="recovery-evidence-pending">Durable evidence will appear as each physical phase completes.</p>';
    return '<div class="recovery-evidence-grid">' + evidence.map(([name, value]) => '<div><span>' + esc(name) + '</span><strong' + (/manifest/i.test(name) ? ' class="digest-value"' : "") + '>' + esc(value) + "</strong></div>").join("") + "</div>";
  }

  function renderRecoveryEvents(exercise) {
    const detail = state.data.recoveryExerciseDetails[recordId(exercise)];
    const events = detail ? list(detail, "events") : [];
    const artifacts = detail ? list(detail, "artifacts") : [];
    let html = "";
    if (events.length) html += '<section class="panel section"><div class="panel-head"><div><h2>Durable activity</h2><p>Append-only phase and evidence history.</p></div></div><div class="panel-body"><ol class="timeline">' + events.map((event) => {
      const eventEvidence = event.evidence && typeof event.evidence === "object" ? event.evidence : {};
      const detailText = pick(eventEvidence, "detail", "message", "path", "cartridge_label") || pick(event, "detail") || "Evidence recorded";
      return '<li class="timeline-item"><time class="timeline-time">' + esc(date(pick(event, "occurred_at", "created_at"))) + '</time><span class="timeline-marker" aria-hidden="true"></span><div class="timeline-copy"><strong>' + esc(human(pick(event, "event_type", "to_state", "phase") || "event")) + '</strong><span>' + esc(detailText) + "</span></div></li>";
    }).join("") + "</ol></div></section>";
    if (artifacts.length) html += '<section class="panel section"><div class="panel-head"><div><h2>Retained artifacts</h2><p>Content-addressed evidence produced by this run.</p></div></div><div class="panel-body"><ul class="detail-list recovery-artifact-list">' + artifacts.map((artifact) => {
      const name = human(pick(artifact, "artifact_kind", "kind") || "artifact");
      const digest = pick(artifact, "sha256", "digest_sha256", "digest");
      const location = pick(artifact, "storage_ref", "path", "location");
      const size = pick(artifact, "size_bytes");
      const description = [location, size == null ? null : bytes(size)].filter(Boolean).join(" · ");
      return '<li><span><strong>' + esc(name) + '</strong>' + (description ? '<small>' + esc(description) + "</small>" : "") + '</span><strong class="digest-value">' + esc(label(digest)) + "</strong></li>";
    }).join("") + "</ul></div></section>";
    return html;
  }

  function renderRecoveryExercise(exercise) {
    if (!exercise) return "";
    const stateValue = recoveryExerciseState(exercise);
    const detail = recoveryExerciseDetail(exercise);
    const profileName = recoveryExerciseProfileName(exercise);
    const terminal = recoveryExercisePassed(exercise) || recoveryExerciseFailed(exercise);
    const recoveryPointId = recoveryEvidenceValue(exercise, "recovery_point_id");
    const vaultAction = recoveryPointId ? '<button class="button button-small button-quiet" type="button" data-action="select-recovery-point" data-id="' + esc(recoveryPointId) + '">Open recovery point</button>' : "";
    const detailError = state.errors["recoveryExercise:" + recordId(exercise)];
    const progressRaw = recoveryEvidenceValue(exercise, "progress_percent", "percent_complete");
    const progressValue = progressRaw == null ? NaN : Number(progressRaw);
    let outcome = "";
    if (recoveryExercisePassed(exercise)) {
      outcome = '<div class="notice success"><span class="notice-icon" aria-hidden="true">✓</span><div class="notice-copy"><strong>Physical recovery exercise passed</strong><span>The full single-tape write was decoded from media, restored into isolation, and validated. This is retained G0.2 evidence, not production qualification.</span></div>' + vaultAction + "</div>";
    } else if (recoveryExerciseFailed(exercise)) {
      outcome = '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Recovery exercise ' + esc(stateValue) + '</strong><span>' + esc(detail || "Open the durable activity record for the exact terminal evidence.") + "</span></div></div>";
    } else {
      outcome = '<div class="notice info"><span class="notice-icon" aria-hidden="true">▶</span><div class="notice-copy"><strong>' + esc(human(recoveryExercisePhase(exercise))) + '</strong><span>' + esc(detail || "Keep the worker, source mount, tape drive, and isolated restore destination online.") + "</span></div></div>";
    }
    const progress = Number.isFinite(progressValue) && progressValue >= 0 && progressValue <= 100 ? '<div class="progress recovery-progress"><div class="progress-track"><div class="progress-value" style="width:' + progressValue + '%"></div></div><div class="progress-meta"><span>Reported progress</span><span>' + progressValue + "%</span></div></div>" : "";
    const detailFailure = detailError ? '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not load retained run evidence</strong><span>' + esc(errorMessage(detailError)) + '</span></div><button class="button button-small" type="button" data-action="refresh">Retry</button></div>' : "";
    return '<section class="panel recovery-exercise-panel section"><div class="recovery-exercise-head"><div><span class="lab-badge">G0.2 · full · single tape · no encryption</span><h2>' + esc(profileName) + '</h2><p>Run ' + esc(recordId(exercise)) + " · " + esc(date(pick(exercise, "started_at", "created_at"))) + '</p></div>' + status(stateValue) + "</div>" + renderRecoveryPhaseTrack(exercise) + '<div class="panel-body recovery-exercise-body">' + detailFailure + outcome + progress + '<section class="recovery-evidence"><h3>' + (terminal ? "Retained result" : "Evidence so far") + "</h3>" + renderRecoveryEvidence(exercise) + "</section></div></section>" + renderRecoveryEvents(exercise);
  }

  function renderRecoveryLab() {
    const profiles = state.data.recoveryProfiles;
    const exercises = orderedRecoveryExercises();
    const active = exercises.find(recoveryExerciseActive);
    const drives = eligibleRecoveryDrives();
    const media = eligibleRecoveryMedia(drives[0]);
    const selected = selectedRecoveryExercise();
    const enabled = recoveryLabEnabled();
    const disabledReason = "The G0.2 recovery exercise is disabled in this deployment.";
    const createProfile = '<button class="button button-primary" type="button" data-action="open-recovery-profile"' + (!enabled ? ' disabled title="' + esc(disabledReason) + '"' : "") + '>Add source profile</button>';
    const blocker = !enabled ? disabledReason : active ? "A recovery exercise is already active." : !profiles.length ? "Create a source and restore profile first." : !drives.length ? "No exact btape-qualified drive and scratch-cartridge pair is ready." : !media.length ? "No cartridge qualified with the selected ready drive is available." : null;
    const start = '<button class="button button-primary" type="button" data-action="open-recovery-run"' + (blocker ? ' disabled title="' + esc(blocker) + '"' : "") + '>Create recoverable tape</button>';
    let html = pageHead("Recovery exercise", "One guided physical write, readback, isolated restore, and manifest comparison.", refreshButton() + createProfile + start);
    html += noticeFor("recoveryProfiles") + noticeFor("recoveryExercises");
    if (!enabled) html += '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Recovery exercise disabled</strong><span>Enable TAPESCALE_LAB_FILE_ROUNDTRIP_ENABLED on both the API and worker before creating profiles or destructive recovery exercises.</span></div></div>';
    html += '<div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>G0.2 laboratory path—not a production backup policy</strong><span>This workflow is full-only, one cartridge, prepared immutable fixture, and intentionally unencrypted. It destroys the selected scratch tape, then restores all captured content into the configured isolated destination. Incrementals, spanning, encryption, scheduling, and production qualification remain open.</span></div></div>';
    html += '<section class="recovery-hero section"><div class="recovery-hero-copy"><span class="lab-badge">Recoverability before convenience</span><h2>' + (exercises.length ? "Run another complete recovery proof" : "Create your first recoverable tape") + '</h2><p>TapeScale will not call a write successful until it can decode the physical tape, restore the fixture, and compare retained manifests.</p><div class="recovery-hero-actions">' + (profiles.length ? start : createProfile) + '</div></div><div class="recovery-constraints"><div><strong>1</strong><span>Prepared immutable source</span></div><div><strong>2</strong><span>Full write to one tape</span></div><div><strong>3</strong><span>Full isolated restore</span></div><div><strong>4</strong><span>Manifest match</span></div></div></section>';
    const profileSummary = profiles.length ? '<strong>' + esc(recoveryProfileName(profiles[0])) + '</strong><span>' + esc(label(pick(profiles[0], "source_subpath"))) + ' → ' + esc(label(pick(profiles[0], "restore_subpath"))) + "</span>" : '<strong>No profile</strong><span>Declare the prepared source and isolated restore subpaths.</span>';
    const driveSummary = drives.length ? '<strong>' + esc(label(pick(drives[0], "model", "stable_id"))) + '</strong><span>' + esc(label(pick(drives[0], "stable_id"))) + "</span>" : '<strong>Drive not ready</strong><span>' + esc(state.data.drives.length ? recoveryDriveBlocker(state.data.drives[0]) : "No configured SAS drive was reported.") + "</span>";
    const mediaSummary = media.length ? '<strong>' + esc(mediaName(media[0])) + '</strong><span>Available unprotected scratch cartridge</span>' : '<strong>No eligible tape</strong><span>Register or release an unprotected non-WORM cartridge.</span>';
    html += '<div class="recovery-readiness-grid section"><section class="panel recovery-ready-card ' + (profiles.length ? "ready" : "blocked") + '"><span class="recovery-ready-number">1</span><div><small>Source and destination</small>' + profileSummary + '</div></section><section class="panel recovery-ready-card ' + (drives.length ? "ready" : "blocked") + '"><span class="recovery-ready-number">2</span><div><small>Qualified hardware</small>' + driveSummary + '</div></section><section class="panel recovery-ready-card ' + (media.length ? "ready" : "blocked") + '"><span class="recovery-ready-number">3</span><div><small>Destructive media</small>' + mediaSummary + "</div></section></div>";
    if (selected) html += renderRecoveryExercise(selected);
    if (exercises.length) {
      const rows = exercises.map((exercise) => {
        const profile = recoveryExerciseProfileName(exercise);
        const detailText = recoveryExerciseDetail(exercise) || (recoveryExerciseActive(exercise) ? human(recoveryExercisePhase(exercise)) : "Retained evidence available");
        return '<tr class="row-action" tabindex="0" data-action="select-recovery-exercise" data-id="' + esc(recordId(exercise)) + '"><td class="primary-cell"><strong>' + esc(profile) + '</strong><small>' + esc(recordId(exercise)) + "</small></td><td>" + status(recoveryExerciseState(exercise)) + "</td>" + cell(pick(exercise, "expected_cartridge_label", "cartridge_label")) + cell(date(pick(exercise, "finished_at", "started_at", "created_at"))) + cell(detailText) + "</tr>";
      });
      html += '<section class="section"><div class="section-head"><div><h2>Recovery exercise history</h2><p>Every attempt remains visible, including blocked and failed runs.</p></div></div>' + table(["Profile", "State", "Cartridge", "Activity", "Result"], rows) + "</section>";
    }
    return html;
  }

  function movementState(movement) {
    return String(pick(movement, "state", "status") || "planned").toLowerCase();
  }

  function movementTerminal(movement) {
    return ["confirmed", "cancelled", "canceled"].includes(movementState(movement));
  }

  function movementOverdue(movement) {
    const due = new Date(pick(movement, "due_at") || 0).getTime();
    return movementState(movement) === "overdue" || (!movementTerminal(movement) && Number.isFinite(due) && due > 0 && due < Date.now());
  }

  function movementRouteId() {
    const parts = location.hash.slice(1).split("/");
    if (parts[0] !== "movements" || !parts[1]) return null;
    try {
      return decodeURIComponent(parts.slice(1).join("/"));
    } catch (_) {
      return null;
    }
  }

  function orderedMovements() {
    return state.data.movements.slice().sort((left, right) => {
      const leftTerminal = movementTerminal(left) ? 1 : 0;
      const rightTerminal = movementTerminal(right) ? 1 : 0;
      if (leftTerminal !== rightTerminal) return leftTerminal - rightTerminal;
      const dueDifference = new Date(pick(left, "due_at") || 0) - new Date(pick(right, "due_at") || 0);
      return dueDifference || new Date(pick(right, "updated_at", "created_at") || 0) - new Date(pick(left, "updated_at", "created_at") || 0);
    });
  }

  function selectedMovement() {
    const movements = orderedMovements();
    const requested = movementRouteId() || state.selectedMovementId;
    const selected = requested && movements.find((movement) => recordId(movement) === requested || String(pick(movement, "external_id")) === requested);
    const movement = selected || movements[0] || null;
    if (movement) state.selectedMovementId = recordId(movement);
    return movement;
  }

  function movementDetail(id) {
    return id ? state.data.movementDetails[id] || null : null;
  }

  async function loadMovementDetail(id, force) {
    if (!id || state.movementDetailLoading.has(id) || (!force && movementDetail(id))) return;
    const errorKey = "movement:" + id;
    state.movementDetailLoading.add(id);
    delete state.errors[errorKey];
    try {
      const payload = await request(endpoints.movements + "/" + encodeURIComponent(id));
      const detail = payload && payload.movement && typeof payload.movement === "object" ? payload.movement : payload;
      state.data.movementDetails[id] = detail && typeof detail === "object" ? detail : {};
      delete state.errors[errorKey];
    } catch (error) {
      state.errors[errorKey] = error;
    } finally {
      state.movementDetailLoading.delete(id);
    }
    if (state.route === "movements" && recordId(selectedMovement()) === id) render();
  }

  async function selectMovement(id) {
    if (!id) return;
    state.selectedMovementId = id;
    const target = "#movements/" + encodeURIComponent(id);
    if (location.hash !== target) location.hash = target;
    else render();
    await loadMovementDetail(id);
  }

  function locationKind(locationRecord) {
    return String(pick(locationRecord, "kind") || "unknown").toLowerCase();
  }

  function renderCustodyLocations() {
    const locations = state.data.locations;
    const cards = locations.map((item) => {
      const details = item.details && typeof item.details === "object" ? item.details : {};
      const description = pick(details, "address", "description", "contact", "notes") || (locationKind(item) === "offsite" ? "Confirmed off-site custody destination" : "Physical custody location");
      return '<article class="custody-location"><span class="custody-location-icon" aria-hidden="true">' + (locationKind(item) === "offsite" ? "◇" : locationKind(item) === "in_transit" ? "→" : locationKind(item) === "unknown" ? "?" : "□") + '</span><div><small>' + esc(human(locationKind(item))) + '</small><strong>' + esc(label(pick(item, "name"))) + '</strong><span>' + esc(description) + "</span></div></article>";
    }).join("");
    return '<section class="panel custody-locations section"><div class="panel-head"><div><h2>Custody locations</h2><p>Only physical confirmations change a cartridge location.</p></div><button class="button button-small" type="button" data-action="add-location">Add location</button></div><div class="custody-location-grid">' + (cards || '<div class="vault-section-empty compact">Create an on-site or off-site location before planning routine movements.</div>') + "</div></section>";
  }

  function renderMovementTaskList(movements, selected) {
    return '<aside class="panel movement-task-panel"><div class="panel-head"><div><h2>Movement tasks</h2><p>' + movements.length + " retained task" + (movements.length === 1 ? "" : "s") + '</p></div></div><div class="task-list custody-task-list">' + movements.map((movement) => {
      const id = recordId(movement);
      const source = pick(movement, "source_name") || "Unknown / unlocated";
      const destination = pick(movement, "destination_name") || "Destination not reported";
      const mediaTotal = count(pick(movement, "media_total"));
      const confirmed = count(pick(movement, "media_confirmed"));
      return '<button class="movement-task' + (selected && id === recordId(selected) ? " active" : "") + (movementOverdue(movement) ? " overdue" : "") + '" type="button" data-action="select-movement" data-id="' + esc(id) + '"><span class="movement-task-head"><strong>' + esc(label(pick(movement, "external_id", "id"))) + '</strong>' + status(movementOverdue(movement) ? "overdue" : movementState(movement)) + '</span><small>' + esc(human(pick(movement, "kind") || "movement")) + " · due " + esc(date(pick(movement, "due_at"))) + '</small><span class="task-route">' + esc(source) + " → " + esc(destination) + '</span><span class="custody-task-progress"><i style="width:' + (mediaTotal ? Math.round(confirmed / mediaTotal * 100) : 0) + '%"></i></span><span class="custody-task-count">' + confirmed + " of " + mediaTotal + " at destination</span></button>";
    }).join("") + "</div></aside>";
  }

  function movementStageIndex(summary) {
    const stateValue = movementState(summary);
    if (stateValue === "confirmed") return 3;
    if (["departed", "partially_confirmed"].includes(stateValue)) return 2;
    if (["ready", "overdue"].includes(stateValue)) return 1;
    return 0;
  }

  function renderMovementStages(summary) {
    const current = movementStageIndex(summary);
    const steps = ["Planned", "Ready", "In transit", "Confirmed"];
    return '<div class="custody-stage" aria-label="Custody movement progress">' + steps.map((name, index) => '<div class="custody-step' + (index < current ? " done" : index === current ? " current" : "") + '"><span class="custody-dot" aria-hidden="true"></span><span>' + name + "</span></div>").join("") + "</div>";
  }

  function renderMovementReadiness(readiness) {
    if (!readiness || typeof readiness !== "object" || !Object.keys(readiness).length) return '<div class="vault-section-empty compact">No additional readiness declaration was returned.</div>';
    return '<div class="custody-readiness-grid">' + Object.entries(readiness).map(([key, value]) => {
      const passing = value === true || ["ready", "passed", "available", "complete"].includes(String(value).toLowerCase());
      const display = typeof value === "object" ? conciseEvidence(value) : value;
      return '<div class="readiness-item' + (passing ? "" : " blocked") + '"><span class="gate-icon" aria-hidden="true">' + (passing ? "✓" : "!") + '</span><div><strong>' + esc(human(key)) + '</strong><span>' + esc(display) + "</span></div></div>";
    }).join("") + "</div>";
  }

  function renderMovementMedia(detail, summary) {
    const media = detail ? list(detail, "media") : [];
    if (!media.length) return '<div class="vault-section-empty">No cartridge manifest was returned for this movement.</div>';
    const sourceUnknown = !pick(summary, "source_location_id");
    const hasTransit = Boolean(pick(summary, "transit_location_id"));
    return '<div class="custody-media"><div class="custody-media-head"><label><input type="checkbox" id="custody-select-all"> Select all actionable tapes</label><span>Check the exact physical labels in hand.</span></div><div class="custody-media-list">' + media.map((item) => {
      const stage = String(pick(item, "stage") || "planned").toLowerCase();
      const actionable = ["planned", "departed"].includes(stage);
      const location = pick(item, "confirmed_location_name") || "Unknown / unlocated";
      return '<label class="custody-media-row ' + esc(stage) + '"><input type="checkbox" name="movement_cartridge" value="' + esc(pick(item, "cartridge_id")) + '" data-label="' + esc(pick(item, "internal_label")) + '" data-stage="' + esc(stage) + '"' + (actionable ? "" : " disabled") + '><span class="custody-tape-glyph" aria-hidden="true">▦</span><span class="custody-media-copy"><strong>' + esc(label(pick(item, "internal_label"))) + '</strong><small>' + esc(pick(item, "barcode") ? "Barcode " + pick(item, "barcode") : "No barcode") + " · confirmed " + esc(location) + '</small></span>' + status(stage) + "</label>";
    }).join("") + '</div><div class="custody-confirm-actions" data-no-print><button class="button" type="button" data-action="confirm-custody" data-confirm-action="departure" data-id="' + esc(recordId(summary)) + '" disabled' + (sourceUnknown || !hasTransit ? " hidden" : "") + '>Confirm departure</button><button class="button" type="button" data-action="confirm-custody" data-confirm-action="arrival" data-id="' + esc(recordId(summary)) + '" disabled' + (!hasTransit ? " hidden" : "") + '>Confirm arrival</button><button class="button button-primary" type="button" data-action="confirm-custody" data-confirm-action="direct" data-id="' + esc(recordId(summary)) + '" disabled>Confirm direct handoff</button></div></div>';
  }

  function renderCustodyTimeline(events) {
    if (!events.length) return '<div class="vault-section-empty compact">No physical custody event has been appended yet.</div>';
    return '<ol class="custody-timeline">' + events.map((event) => {
      const corrected = pick(event, "corrects_event_id");
      const eventType = String(pick(event, "event_type") || "").toLowerCase();
      const route = [pick(event, "source_name"), pick(event, "destination_name")].filter(Boolean).join(" → ");
      const canCorrect = pick(event, "cartridge_id") && ["departure_confirmed", "arrival_confirmed", "direct_move_confirmed", "custody_corrected"].includes(eventType);
      return '<li><time>' + esc(date(pick(event, "occurred_at"))) + '</time><span class="custody-event-dot" aria-hidden="true"></span><div><div class="custody-event-head"><strong>' + esc(human(pick(event, "event_type") || "custody event")) + '</strong>' + (canCorrect ? '<button class="button button-small button-quiet" type="button" data-action="correct-custody" data-event-id="' + esc(recordId(event)) + '" data-cartridge-id="' + esc(pick(event, "cartridge_id")) + '">Append correction</button>' : "") + '</div><span>' + esc(label(pick(event, "internal_label"))) + (route ? " · " + esc(route) : "") + '</span><p>' + esc(pick(event, "note") || "No operator note") + "</p><small>Actor " + esc(label(pick(event, "actor_label"))) + (corrected ? " · corrects " + esc(corrected) : "") + "</small></div></li>";
    }).join("") + "</ol>";
  }

  function renderMovementDetail(summary) {
    const id = recordId(summary);
    const detail = movementDetail(id);
    const resolvedSummary = detail && detail.summary && typeof detail.summary === "object" ? detail.summary : summary;
    const events = detail ? list(detail, "custody_events") : [];
    const readiness = detail && detail.readiness && typeof detail.readiness === "object" ? detail.readiness : {};
    const error = state.errors["movement:" + id];
    const source = pick(resolvedSummary, "source_name") || "Unknown / unlocated";
    const destination = pick(resolvedSummary, "destination_name") || "Destination not reported";
    const transit = pick(resolvedSummary, "transit_name");
    const confirmed = count(pick(resolvedSummary, "media_confirmed"));
    const total = count(pick(resolvedSummary, "media_total"));
    const terminal = movementTerminal(resolvedSummary);
    let feedback = "";
    if (!detail && !error) feedback = '<div class="notice info custody-detail-feedback"><span class="notice-icon" aria-hidden="true">↻</span><div class="notice-copy"><strong>Loading custody manifest</strong><span>Fetching the exact cartridge stages and append-only physical history.</span></div></div>';
    if (error) feedback = '<div class="notice danger custody-detail-feedback"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not load custody detail</strong><span>' + esc(errorMessage(error)) + '</span></div><button class="button button-small" data-action="retry-movement" data-id="' + esc(id) + '">Retry</button></div>';
    return '<article class="panel movement-detail custody-print-manifest"><header class="movement-hero"><div class="movement-hero-head"><div><span class="custody-kicker">' + esc(human(pick(resolvedSummary, "kind") || "movement")) + '</span><h2>' + esc(label(pick(resolvedSummary, "external_id", "id"))) + '</h2><code>' + esc(id) + '</code></div>' + status(movementOverdue(resolvedSummary) ? "overdue" : movementState(resolvedSummary)) + '</div><div class="movement-route"><div class="location-node"><span>Planned source</span><strong>' + esc(source) + '</strong></div><span class="movement-arrow" aria-hidden="true">→</span>' + (transit ? '<div class="location-node transit"><span>Transit</span><strong>' + esc(transit) + '</strong></div><span class="movement-arrow" aria-hidden="true">→</span>' : "") + '<div class="location-node"><span>Planned destination</span><strong>' + esc(destination) + '</strong></div></div><div class="movement-hero-meta"><span><small>Due</small><strong>' + esc(date(pick(resolvedSummary, "due_at"))) + '</strong></span><span><small>Confirmation version</small><strong>' + esc(label(pick(resolvedSummary, "confirmation_version"))) + '</strong></span><span><small>At destination</small><strong>' + confirmed + " / " + total + '</strong></span></div><div class="movement-detail-actions" data-no-print><button class="button button-small" type="button" data-action="print-movement-manifest">Print manifest</button><button class="button button-small button-danger" type="button" data-action="cancel-movement" data-id="' + esc(id) + '"' + (terminal ? " disabled" : "") + '>Cancel task</button></div></header><div class="custody-intent ' + (movementState(resolvedSummary) === "confirmed" ? "confirmed" : "planned") + '"><strong>' + (movementState(resolvedSummary) === "confirmed" ? "Physical custody confirmed" : "Movement intent only") + '</strong><span>' + (movementState(resolvedSummary) === "confirmed" ? "The selected cartridges have append-only arrival or direct-handoff evidence at the destination." : "This task reserves cartridges but does not change their confirmed location until an operator physically confirms them.") + "</span></div>" + renderMovementStages(resolvedSummary) + feedback + '<section class="custody-detail-section"><div class="vault-section-head"><div><h3>Exact cartridge checklist</h3><p>Partial confirmation is supported; unchecked tapes remain at their prior durable stage.</p></div><span>' + total + " tape" + (total === 1 ? "" : "s") + "</span></div>" + renderMovementMedia(detail, resolvedSummary) + '</section><section class="custody-detail-section"><div class="vault-section-head"><div><h3>Readiness declaration</h3><p>Planned prerequisites do not prove physical movement.</p></div></div>' + renderMovementReadiness(readiness) + '</section><section class="custody-detail-section"><div class="vault-section-head"><div><h3>Durable custody timeline</h3><p>Physical confirmations and corrections are append-only.</p></div><span>' + events.length + " event" + (events.length === 1 ? "" : "s") + "</span></div>" + renderCustodyTimeline(events) + "</section></article>";
  }

  function renderMovements() {
    const movements = orderedMovements();
    const selected = selectedMovement();
    const active = movements.filter((movement) => !movementTerminal(movement)).length;
    const overdue = movements.filter(movementOverdue).length;
    const inTransit = movements.reduce((sum, movement) => sum + count(pick(movement, "media_departed")), 0);
    const confirmed = movements.reduce((sum, movement) => sum + count(pick(movement, "media_confirmed")), 0);
    const canPlan = state.data.locations.some((item) => ["onsite", "offsite", "device"].includes(locationKind(item))) && state.data.media.length > 0;
    const actions = refreshButton() + '<button class="button" type="button" data-action="add-location">Add location</button><button class="button button-primary" type="button" data-action="plan-movement"' + (canPlan ? "" : ' disabled title="Create a destination location and register at least one cartridge first."') + '>Plan movement</button>';
    let html = pageHead("Off-site custody", "Plan cartridge movements, print exact manifests, and append physical handoff evidence without confusing intent for confirmed location.", actions) + noticeFor("locations") + noticeFor("movements") + noticeFor("media");
    html += '<section class="panel metric-strip custody-metrics"><div class="metric"><span>Active tasks</span><strong>' + active + '</strong><small>Planned or in progress</small></div><div class="metric"><span>Overdue</span><strong>' + overdue + '</strong><small>Still requires operator action</small></div><div class="metric"><span>In transit</span><strong>' + inTransit + '</strong><small>Departure confirmed only</small></div><div class="metric"><span>At destination</span><strong>' + confirmed + '</strong><small>Arrival or direct handoff confirmed</small></div></section>';
    html += renderCustodyLocations();
    if (!movements.length) return html + empty("No movement tasks", state.errors.movements ? "Custody tasks could not be loaded." : "Plan an off-site delivery, return, swap, recall, or direct onboarding for currently unlocated media.", canPlan ? '<button class="button button-primary" type="button" data-action="plan-movement">Plan first movement</button>' : '<button class="button" type="button" data-action="add-location">Create destination</button>');
    html += '<div class="movement-layout section">' + renderMovementTaskList(movements, selected) + renderMovementDetail(selected) + "</div>";
    const selectedId = recordId(selected);
    if (selectedId && !movementDetail(selectedId) && !state.movementDetailLoading.has(selectedId) && !state.errors["movement:" + selectedId]) window.setTimeout(() => loadMovementDetail(selectedId), 0);
    return html;
  }

  function renderDrives() {
    const motionEnabled = pick(state.data.system, "lab_tape_motion_enabled") === true;
    const qualificationEnabled = pick(state.data.system, "lab_tape_qualification_enabled") === true;
    const initialize = '<button class="button button-danger" type="button" data-action="initialize-tape"' + (qualificationEnabled ? "" : ' disabled title="Enable attended tape setup first."') + '>Prepare tape</button>';
    let html = pageHead("Drives & libraries", "Hardware inventory returned by TapeScale.", refreshButton() + initialize) + noticeFor("drives");
    if (qualificationEnabled) {
      html += '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Destructive tape setup enabled</strong><span>Preparing a tape erases it, exercises the exact drive-and-tape pair, and verifies readback before backup use. Only empty, unprotected, non-WORM scratch media are eligible.</span></div></div>';
    }
    if (motionEnabled) {
      html += '<div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Manual scratch-media motion enabled</strong><span>Rewind and unload physically move tape. Linux can finalize a prior write with a filemark before moving it. Use designated scratch media only; this does not qualify the drive for backup.</span></div></div>';
    } else {
      html += '<div class="notice info"><span class="notice-icon" aria-hidden="true">i</span><div class="notice-copy"><strong>Manual scratch-media motion is disabled</strong><span>Rewind and Unload remain visible for discovered drives, but cannot run until the API and worker use TAPESCALE_LAB_TAPE_MOTION_ENABLED=true.</span></div></div>';
    }
    if (!state.data.drives.length) {
      const noDriveDetail = state.errors.drives ? "Drive and slot inventory could not be loaded." : "No SAS tape drive has been reported. Rewind and Unload appear after the worker discovers the exact mapped /dev/nstN device.";
      if (!state.data.slots.length) return html + empty("No hardware inventory", noDriveDetail);
      html += '<div class="notice info"><span class="notice-icon" aria-hidden="true">i</span><div class="notice-copy"><strong>No tape drive discovered</strong><span>' + esc(noDriveDetail) + "</span></div></div>";
    }
    if (state.data.drives.length) {
      const rows = state.data.drives.map((drive) => {
        const health = drive.health && typeof drive.health === "object" ? drive.health : {};
        const observedAt = new Date(pick(drive, "last_observed_at") || 0).getTime();
        const current = Number.isFinite(observedAt) && Date.now() - observedAt <= 120000;
        const operations = state.data.driveOperations
          .filter((operation) => String(pick(operation, "drive_id")) === recordId(drive))
          .sort((left, right) => new Date(pick(right, "created_at") || 0) - new Date(pick(left, "created_at") || 0));
        const latestOperation = operations[0] || null;
        const activeOperation = operations.find((operation) => ["queued", "running"].includes(String(pick(operation, "state"))));
        const qualificationRuns = state.data.qualificationRuns
          .filter((run) => String(pick(run, "drive_id")) === recordId(drive))
          .sort((left, right) => new Date(pick(right, "created_at") || 0) - new Date(pick(left, "created_at") || 0));
        const latestQualification = qualificationRuns[0] || null;
        const activeQualification = qualificationRuns.find((run) => ["queued", "running"].includes(String(pick(run, "state"))));
        const eligibleMedia = state.data.media.filter((item) => item.worm !== true && count(pick(item, "active_holds")) === 0 && pick(item, "has_content") !== true && !pick(item, "reserved_by") && !["foreign", "mismatch"].includes(String(pick(item, "identity_status") || "")));
        const canReconcile = current && !drive.reconciled && health.present === true && health.accessible === true && health.motion_position_uncertain !== true && health.immediate_mode === false && health.in_flight === 0 && health.identity_verified === true && health.identity_conflict !== true;
        const action = canReconcile ? '<button class="button button-small" type="button" data-action="reconcile-drive" data-id="' + esc(recordId(drive)) + '" data-stable-id="' + esc(pick(drive, "stable_id")) + '">Trust identity</button>' : status(drive.reconciled ? "reconciled" : "unreconciled");
        const access = current ? pick(health, "status") || pick(drive, "qualification_status") : "stale";
        const motionBlocker = activeQualification ? "A destructive qualification run is already " + String(pick(activeQualification, "state") || "active") + "."
          : activeOperation ? "A tape motion request is already " + String(pick(activeOperation, "state") || "active") + "."
          : !motionEnabled ? "Manual scratch-media motion is disabled in this deployment."
            : !current ? "The drive observation is stale; refresh and wait for the worker."
              : health.present !== true ? "The configured tape device is not present."
                : health.accessible !== true ? "The worker cannot access the configured tape device."
                  : health.identity_verified !== true ? "The drive does not have a verified stable SCSI identity."
                    : health.identity_conflict === true ? "The stable identity is present at more than one configured path."
                      : health.motion_position_uncertain === true ? "A prior motion result requires fresh reconciliation."
                        : health.immediate_mode !== false ? "Kernel immediate tape motion is enabled or unknown."
                          : health.in_flight !== 0 ? "The kernel reports outstanding or unknown tape I/O."
                            : drive.reconciled !== true ? "Trust this exact drive identity before requesting tape motion."
                              : health.medium_loaded !== true ? "No cartridge is reported as loaded."
                                : health.ready !== true ? "The loaded tape drive is not ready."
                                  : null;
        const canMove = motionBlocker === null;
        let motion = motionEnabled ? status(activeOperation ? pick(activeOperation, "state") : health.medium_loaded === true ? "ready" : "no loaded tape") : status("disabled");
        if (latestOperation && !activeOperation) motion = status(pick(latestOperation, "state"));
        const disabled = canMove ? "" : ' disabled aria-disabled="true" title="' + esc(motionBlocker) + '"';
        const motionAttributes = ' data-action="drive-motion" data-id="' + esc(recordId(drive)) + '" data-stable-id="' + esc(pick(drive, "stable_id")) + '" data-drive-label="' + esc(pick(drive, "model", "observed_path", "stable_id")) + '"';
        motion += '<div class="drive-actions"><button class="button button-small button-quiet" type="button" data-motion="rewind"' + motionAttributes + disabled + '>Rewind</button><button class="button button-small" type="button" data-motion="unload"' + motionAttributes + disabled + '>Unload</button></div>';
        if (motionBlocker) motion += '<small class="drive-motion-reason">' + esc(motionBlocker) + "</small>";
        const qualificationBlocker = activeQualification ? "A qualification run is already " + String(pick(activeQualification, "state") || "active") + "."
          : activeOperation ? "A tape motion request is already active."
            : !qualificationEnabled ? "Destructive tape qualification is disabled in this deployment."
              : !current ? "The drive observation is stale; refresh and wait for the worker."
                : health.present !== true || health.accessible !== true ? "The configured tape device is not accessible."
                  : health.identity_verified !== true || health.identity_conflict === true ? "The stable drive identity is not safe."
                    : health.motion_position_uncertain === true ? "A prior result requires fresh drive reconciliation."
                      : health.immediate_mode !== false || health.in_flight !== 0 ? "Kernel tape I/O state is not safe."
                        : drive.reconciled !== true ? "Trust this exact drive identity first."
                          : health.medium_loaded !== true || health.ready !== true ? "Load the designated scratch cartridge and wait for ready."
                            : !eligibleMedia.length ? "Register an unprotected, non-WORM scratch cartridge first."
                              : null;
        let qualification = status(latestQualification ? "btape " + String(pick(latestQualification, "state")) : "not run");
        const qualificationDisabled = qualificationBlocker ? ' disabled aria-disabled="true" title="' + esc(qualificationBlocker) + '"' : "";
        qualification += '<div class="drive-actions"><button class="button button-small button-danger" type="button" data-action="open-qualification" data-id="' + esc(recordId(drive)) + '" data-stable-id="' + esc(pick(drive, "stable_id")) + '" data-drive-label="' + esc(pick(drive, "model", "observed_path", "stable_id")) + '"' + qualificationDisabled + '>Run btape test</button></div>';
        qualification += '<small class="drive-motion-reason">Product gate: ' + esc(label(pick(drive, "qualification_status"))) + "." + (qualificationBlocker ? " " + esc(qualificationBlocker) : " This will destroy the selected tape.") + "</small>";
        return '<tr class="row-action" tabindex="0" data-action="open-record" data-kind="drives" data-id="' + esc(recordId(drive)) + '"><td class="primary-cell"><strong>' + esc(label(pick(drive, "stable_id", "name", "id", "device"))) + '</strong><small>' + esc(label(pick(drive, "model", "serial", "observed_path"))) + "</small></td>" + cell(pick(drive, "generation", "model")) + "<td>" + status(access) + "</td><td>" + qualification + "</td><td>" + action + "</td><td>" + motion + "</td></tr>";
      });
      html += table(["Drive", "Generation / model", "Access", "G0 evidence", "Identity", "Manual motion"], rows);
    }
    if (state.data.slots.length) {
      const rows = state.data.slots.map((slot) => "<tr>" + cell(pick(slot, "element_address", "name", "id", "slot")) + cell(pick(slot, "type", "kind")) + cell(pick(slot, "state", "status")) + cell(pick(slot, "observed_barcode", "media_label", "barcode", "cartridge")) + "</tr>");
      html += '<section class="section"><div class="section-head"><div><h2>Library slots</h2></div></div>' + table(["Slot", "Type", "Status", "Media"], rows) + "</section>";
    }
    return html;
  }

  function renderVerification() {
    const items = state.data.verification;
    let html = pageHead("Verification", "Verification evidence returned by TapeScale.", refreshButton()) + noticeFor("verification");
    if (!items.length) return html + empty("No verification records", state.errors.verification ? "Verification records could not be loaded." : "No verification work or evidence has been reported.");
    const rows = items.map((item) => {
      const pointId = pick(item, "recovery_point_id");
      const action = pointId ? 'data-action="select-recovery-point" data-id="' + esc(pointId) + '"' : 'data-action="open-record" data-kind="verification" data-id="' + esc(recordId(item)) + '"';
      return '<tr class="row-action" tabindex="0" ' + action + '><td class="primary-cell"><strong>' + esc(label(pick(item, "kind", "name", "id"))) + '</strong><small>' + esc(label(pick(item, "dataset_name", "media_label", "job_name", "source"))) + (pointId ? " · open recovery point" : "") + "</small></td><td>" + status(pick(item, "outcome", "state", "status", "result")) + "</td>" + cell(date(pick(item, "finished_at", "started_at", "completed_at", "updated_at", "time"))) + cell(pick(item, "detail", "message", "drive_stable_id")) + "</tr>";
    });
    return html + table(["Verification", "Status", "Activity", "Evidence"], rows);
  }

  function metadata(object, excluded) {
    if (!object || typeof object !== "object") return "";
    const skip = new Set(excluded || []);
    const entries = Object.entries(object).filter(([key, value]) => !skip.has(key) && !Array.isArray(value));
    if (!entries.length) return "";
    return entries.map(([key, value]) => {
      if (value && typeof value === "object") {
        const nested = Object.entries(value).map(([nestedKey, nestedValue]) => "<li><span>" + esc(human(nestedKey)) + "</span><strong>" + esc(label(nestedValue)) + "</strong></li>").join("");
        return '<section class="panel section"><div class="panel-head"><h2>' + esc(human(key)) + '</h2></div><div class="panel-body"><ul class="detail-list">' + nested + "</ul></div></section>";
      }
      return '<section class="panel section"><div class="panel-body"><strong>' + esc(human(key)) + ':</strong> ' + esc(label(value)) + "</div></section>";
    }).join("");
  }

  function renderPreflight() {
    const value = state.data.preflight;
    let html = noticeFor("preflight");
    if (!value) return html + empty("Preflight not available", state.errors.preflight ? "Preflight could not be loaded." : "No preflight result has been reported.");
    const checks = list(value, "checks");
    html += '<section class="panel"><div class="panel-head"><div><h2>Preflight</h2><p>' + esc(label(pick(value, "status"))) + (pick(value, "checked_at", "run_at", "runAt") ? " · " + esc(date(pick(value, "checked_at", "run_at", "runAt"))) : "") + "</p></div></div><div class=\"panel-body\">";
    if (!checks.length) return html + "<p>No preflight checks were returned.</p></div></section>";
    return html + '<div class="preflight-grid">' + checks.map((check) => '<div class="preflight-item"><span>' + status(pick(check, "state", "status")) + '</span><div><strong>' + esc(label(pick(check, "label", "name", "id"))) + '</strong><span>' + esc(label(pick(check, "detail", "message"))) + "</span></div></div>").join("") + "</div></div></section>";
  }

  function renderQualification() {
    const value = state.data.qualification;
    let html = noticeFor("qualification") + noticeFor("qualificationRuns");
    const runs = state.data.qualificationRuns;
    if (runs.length) html += qualificationRunNotice(runs[0]);
    if (!value || !Object.keys(value).length) {
      html += empty("Qualification metadata not available", state.errors.qualification ? "The qualification definition could not be loaded; durable run history is still shown below." : "No qualification definition has been reported; durable run history is still shown below.");
    } else {
      html += '<div class="notice warning"><span class="notice-icon" aria-hidden="true">i</span><div class="notice-copy"><strong>G0 is still open</strong><span>A passing btape run proves Bacula record, filemark, positioning, append, and re-read behavior for one exact drive and scratch cartridge. It is not yet backup, full readback, restore, ACL, spanning, encryption, or clean-catalog recovery evidence.</span></div></div>';
      html += metadata(value, ["cases", "generations"]);
    }
    if (runs.length) {
      const rows = runs.map((run) => {
        const result = run.result && typeof run.result === "object" ? run.result : {};
        const evidence = result.evidence && typeof result.evidence === "object" ? result.evidence : {};
        const detail = pick(result, "detail") || pick(evidence, "detail") || (pick(run, "state") === "passed" ? "btape passed and cleanup rewind established BOT" : null);
        return '<tr class="row-action" tabindex="0" data-action="open-record" data-kind="qualificationRuns" data-id="' + esc(recordId(run)) + '"><td class="primary-cell"><strong>' + esc(label(pick(run, "expected_cartridge_label"))) + '</strong><small>' + esc(label(pick(run, "engine_name"))) + " " + esc(label(pick(run, "engine_version"))) + "</small></td>" + cell(pick(run, "expected_stable_id")) + "<td>" + status(pick(run, "state")) + "</td>" + cell(date(pick(run, "finished_at", "started_at", "created_at"))) + cell(detail) + "</tr>";
      });
      html += '<section class="section"><div class="section-head"><div><h2>Physical qualification runs</h2><p>Durable, append-only evidence from the attended scratch-media harness.</p></div></div>' + table(["Scratch cartridge", "Drive", "Status", "Activity", "Evidence"], rows) + "</section>";
    } else {
      html += empty("No physical qualification runs", "Use Run btape test on a trusted, ready drive after registering the loaded scratch cartridge.");
    }
    const cases = list(value, "cases");
    if (cases.length) {
      const rows = cases.map((item) => "<tr>" + cell(pick(item, "id", "name")) + "<td>" + status(pick(item, "status", "state")) + "</td>" + cell(pick(item, "reason", "detail")) + "</tr>");
      html += table(["Case", "Status", "Reason"], rows);
    }
    const generations = list(value, "generations");
    if (generations.length) {
      const rows = generations.map((item) => "<tr>" + cell(pick(item, "generation")) + cell(pick(item, "reads")) + cell(pick(item, "writes")) + "<td>" + status(pick(item, "physical")) + "</td></tr>");
      html += table(["Generation", "Reads", "Writes", "Physical qualification"], rows);
    }
    return html;
  }

  function renderInterface() {
    let html = noticeFor("system");
    if (!state.data.system) return html + empty("System metadata not available", state.errors.system ? "System metadata could not be loaded." : "The API has not reported system metadata.");
    return html + metadata(state.data.system);
  }

  function renderSettings() {
    const tabs = [
      ["preflight", "Preflight"],
      ["qualification", "Qualification"],
      ["interface", "System & API"]
    ];
    const nav = tabs.map(([id, name]) => '<button class="' + (state.settingsTab === id ? "active" : "") + '" type="button" data-action="settings-tab" data-tab="' + id + '">' + name + "</button>").join("");
    const body = state.settingsTab === "qualification" ? renderQualification() : state.settingsTab === "interface" ? renderInterface() : renderPreflight();
    return pageHead("System & qualification", "Configuration and evidence reported by TapeScale.", refreshButton()) + '<div class="settings-layout"><nav class="panel settings-nav" aria-label="Settings sections">' + nav + '</nav><div>' + body + "</div></div>";
  }

  function render() {
    state.route = routes.has(location.hash.slice(1).split("/")[0]) ? location.hash.slice(1).split("/")[0] : "overview";
    document.querySelectorAll("[data-route]").forEach((link) => {
      const active = link.dataset.route === state.route;
      link.classList.toggle("active", active);
      if (active) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
    if (state.loading) {
      main.innerHTML = loading();
      return;
    }
    const renderers = {
      overview: renderOverview,
      "recovery-lab": renderRecoveryLab,
      jobs: renderJobs,
      restore: renderRestore,
      movements: renderMovements,
      tapes: renderMedia,
      drives: renderDrives,
      verification: renderVerification,
      settings: renderSettings
    };
    main.innerHTML = renderers[state.route]();
    const routeTitle = state.route === "jobs" ? "Backup Center" : state.route === "recovery-lab" ? "Recovery exercise" : human(state.route);
    document.title = routeTitle + " · TapeScale Alpha";
  }

  function findRecord(kind, id) {
    const records = state.data[kind] || [];
    return Array.isArray(records) ? records.find((record) => recordId(record) === id) : null;
  }

  function renderRecordDrawer(kind, id, record, events) {
    if (!record) return;
    const title = pick(record, "name", "internal_label", "external_id", "stable_id", "label", "id", "job_id") || human(kind);
    const rows = Object.entries(record)
      .map(([key, value]) => "<li><span>" + esc(human(key)) + "</span><strong>" + esc(value && typeof value === "object" ? JSON.stringify(value) : label(value)) + "</strong></li>")
      .join("");
    const history = Array.isArray(events) && events.length ? '<section class="drawer-section"><h3>Append-only events</h3><ol class="event-list">' + events.map((event) => {
      const evidence = event.evidence && typeof event.evidence === "object" ? event.evidence : {};
      return "<li><span>" + status(pick(event, "event_type")) + "</span><div><strong>" + esc(date(pick(event, "occurred_at"))) + "</strong><small>" + esc(label(pick(evidence, "detail", "path", "cartridge_label"))) + "</small></div></li>";
    }).join("") + "</ol></section>" : "";
    drawerLayer.innerHTML = '<aside class="drawer" role="dialog" aria-modal="true" aria-labelledby="drawer-title"><div class="drawer-head"><h2 id="drawer-title">' + esc(title) + '</h2><button class="icon-button" type="button" data-action="close-drawer" aria-label="Close">&times;</button></div><div class="drawer-body"><ul class="detail-list">' + rows + "</ul>" + history + "</div></aside>";
    drawerLayer.dataset.recordKey = kind + ":" + id;
    drawerLayer.hidden = false;
  }

  async function openRecord(kind, id) {
    const record = findRecord(kind, id);
    if (!record) return;
    renderRecordDrawer(kind, id, record, null);
    if (kind !== "qualificationRuns") return;
    try {
      const detail = await request(endpoints.qualificationRuns + "/" + encodeURIComponent(id));
      if (drawerLayer.hidden || drawerLayer.dataset.recordKey !== kind + ":" + id) return;
      renderRecordDrawer(kind, id, pick(detail, "run") || record, list(detail, "events"));
    } catch (error) {
      if (!drawerLayer.hidden && drawerLayer.dataset.recordKey === kind + ":" + id) toast("Could not load qualification events", errorMessage(error));
    }
  }

  function closeDrawer() {
    drawerLayer.hidden = true;
    drawerLayer.innerHTML = "";
    delete drawerLayer.dataset.recordKey;
  }

  function openModal(title, body, footer) {
    if (modalLayer.hidden) {
      modalReturnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    }
    modalLayer.innerHTML = '<section class="modal" role="dialog" aria-modal="true" aria-labelledby="modal-title"><div class="modal-head"><h2 id="modal-title">' + esc(title) + '</h2><button class="icon-button" type="button" data-action="close-modal" aria-label="Close">&times;</button></div><div class="modal-body">' + body + '</div>' + (footer ? '<div class="modal-foot">' + footer + "</div>" : "") + "</section>";
    modalLayer.hidden = false;
    shell.setAttribute("inert", "");
    drawerLayer.setAttribute("inert", "");
    const initialFocus = modalLayer.querySelector("[autofocus], input:not([type='hidden']):not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled]), a[href], [tabindex]:not([tabindex='-1'])");
    if (initialFocus) initialFocus.focus();
  }

  function closeModal(preserveBackupSetup) {
    if (modalLayer.hidden) return;
    state.backupReadinessModalToken += 1;
    state.restoreMediaModalToken += 1;
    if (preserveBackupSetup !== true && modalLayer.querySelector("#tape-initialize-form")) state.pendingBackupSetup = null;
    modalLayer.hidden = true;
    modalLayer.innerHTML = "";
    shell.removeAttribute("inert");
    drawerLayer.removeAttribute("inert");
    const returnFocus = modalReturnFocus;
    modalReturnFocus = null;
    if (returnFocus && returnFocus.isConnected && typeof returnFocus.focus === "function") returnFocus.focus();
  }

  function recoveryPointPlanDraft(point) {
    const id = recordId(point);
    if (state.restorePlanDrafts[id]) return state.restorePlanDrafts[id];
    const representation = recoveryPointRepresentation(point);
    const dataset = String(pick(point, "dataset_name", "source", "dataset") || "recovery");
    const slug = dataset.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 48) || "recovery";
    const recoveryTime = new Date(pick(point, "recovery_time", "created_at") || Date.now());
    const stamp = Number.isNaN(recoveryTime.getTime()) ? "restore" : recoveryTime.toISOString().slice(0, 10);
    const logicalBytes = Number(recoveryPointLogicalBytes(point));
    return {
      recovery_point_id: id,
      mode: representation === "native_zfs" ? "native_dataset" : "file_archive",
      destination: "restores/" + stamp + "-" + slug,
      conflict_policy: "skip",
      key_available: !pick(point, "key_lineage_id"),
      key_recovery_package_tested: !pick(point, "key_lineage_id"),
      destination_approved: true,
      is_new_destination: true,
      conflicts_found: 0,
      acl_semantics_supported: true,
      identity_mapping_complete: true,
      estimated_required_bytes: Number.isFinite(logicalBytes) && logicalBytes >= 0 ? logicalBytes : null,
      available_bytes: null,
      required_free_reserve_bytes: 0,
      administrator_reauthenticated: false,
      destructive_preview_accepted: false,
      automatic_publication_requested: false
    };
  }

  function checked(value) {
    return value ? " checked" : "";
  }

  function selectedOption(actual, expected) {
    return actual === expected ? " selected" : "";
  }

  function openRestorePlan(id) {
    const summary = state.data.recoveryPoints.find((point) => recordId(point) === id);
    if (!summary) return;
    const detail = recoveryPointDetail(id);
    const point = detail && detail.recovery_point && typeof detail.recovery_point === "object" ? detail.recovery_point : summary;
    const draft = recoveryPointPlanDraft(point);
    const media = recoveryPointVolumes(detail, point).map(volumeLabel);
    const keyLineage = pick(point, "key_lineage_id");
    const body = '<form id="restore-plan-form" data-recovery-point-id="' + esc(id) + '"><div class="restore-plan-banner"><span aria-hidden="true">◇</span><div><strong>Generate a readiness plan</strong><p>This evaluates durable catalog evidence and your destination assessment. It cannot read tape or restore data. When execution is explicitly offered, this Alpha restores the complete supported Full point across the server-returned ordered tape set.</p></div></div><div class="restore-plan-point"><div><small>Recovery point</small><strong>' + esc(label(pick(point, "dataset_name", "source", "dataset"))) + '</strong><code>' + esc(id) + '</code></div>' + status(recoveryPointState(point)) + '</div><div class="restore-plan-media"><span>Required media</span><strong>' + esc(media.length ? media.join(" → ") : "No ordered media returned") + '</strong></div><div class="form-grid restore-plan-fields"><div class="form-field"><label for="restore-plan-mode">Recovery mode</label><select class="select" id="restore-plan-mode" name="mode"><option value="file_archive"' + selectedOption(draft.mode, "file_archive") + '>File archive</option><option value="native_dataset"' + selectedOption(draft.mode, "native_dataset") + '>Native ZFS dataset</option><option value="files_from_native"' + selectedOption(draft.mode, "files_from_native") + '>Files from native stream</option></select><small>Native extraction may require temporary staging capacity.</small></div><div class="form-field"><label for="restore-conflict-policy">Existing-file behavior</label><select class="select" id="restore-conflict-policy" name="conflict_policy"><option value="skip"' + selectedOption(draft.conflict_policy, "skip") + '>Skip conflicts (safe default)</option><option value="rename"' + selectedOption(draft.conflict_policy, "rename") + '>Rename recovered files</option><option value="replace"' + selectedOption(draft.conflict_policy, "replace") + '>Replace existing data</option></select><small>Replacement stays blocked without administrator reauthentication and a destructive preview.</small></div><div class="form-field full"><label for="restore-destination">Destination inside the configured recovery root</label><input class="input input-mono" id="restore-destination" name="destination" value="' + esc(draft.destination) + '" maxlength="512" required placeholder="restores/2026-11-12-dataset"><small>Use a contained relative path. Absolute paths and parent traversal are rejected.</small></div><div class="form-field"><label for="restore-available-bytes">Available destination bytes</label><input class="input input-mono" id="restore-available-bytes" name="available_bytes" type="number" min="0" step="1" value="' + esc(draft.available_bytes == null ? "" : draft.available_bytes) + '" placeholder="Optional"><small>Leave blank to retain an explicit capacity-estimate warning.</small></div><div class="form-field"><label for="restore-reserve-bytes">Required free-space reserve</label><input class="input input-mono" id="restore-reserve-bytes" name="required_free_reserve_bytes" type="number" min="0" step="1" value="' + esc(draft.required_free_reserve_bytes) + '"><small>Logical recovery estimate: ' + esc(draft.estimated_required_bytes == null ? "not reported" : bytes(draft.estimated_required_bytes)) + '.</small></div><div class="form-field full"><fieldset class="restore-assessment"><legend>Destination assessment</legend><label class="check-option"><input type="checkbox" name="is_new_destination"' + checked(draft.is_new_destination) + '> This is a new, isolated destination.</label><label class="check-option"><input type="checkbox" name="destination_approved"' + checked(draft.destination_approved) + '> The path is inside the approved recovery area.</label><label class="check-option"><input type="checkbox" name="acl_semantics_supported"' + checked(draft.acl_semantics_supported) + '> The destination preserves the required ACL and metadata semantics.</label><label class="check-option"><input type="checkbox" name="identity_mapping_complete"' + checked(draft.identity_mapping_complete) + '> User and group identity mapping has been reviewed.</label>' + (keyLineage ? '<label class="check-option"><input type="checkbox" name="key_available"' + checked(draft.key_available) + '> Required key lineage <code>' + esc(keyLineage) + '</code> is available.</label><label class="check-option"><input type="checkbox" name="key_recovery_package_tested"' + checked(draft.key_recovery_package_tested) + '> The independent key recovery package has been tested.</label>' : '<div class="restore-no-key"><span aria-hidden="true">✓</span>No encryption key lineage is recorded for this point.</div>') + '</fieldset></div></div><div class="notice warning restore-replace-warning" id="restore-replace-warning"' + (draft.conflict_policy === "replace" ? "" : " hidden") + '><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Replacement cannot be authorized here</strong><span>The resulting plan will retain reauthentication and destructive-preview blockers. This form never executes the replacement.</span></div></div><p class="notice danger" id="restore-plan-error" hidden></p><div class="modal-foot restore-plan-submit"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Generate readiness plan</button></div></form>';
    openModal("Plan a restore", body);
    const modal = modalLayer.querySelector(".modal");
    if (modal) modal.classList.add("restore-plan-modal");
    const destination = document.querySelector("#restore-destination");
    if (destination) destination.focus();
  }

  function optionalFormNumber(values, name) {
    const raw = String(values.get(name) || "").trim();
    if (!raw) return null;
    const value = Number(raw);
    return Number.isFinite(value) && value >= 0 ? value : null;
  }

  function planIssueText(issue) {
    if (issue == null) return "Unspecified prerequisite";
    if (typeof issue === "string") return human(issue.replace(/([a-z])([A-Z])/g, "$1_$2"));
    if (typeof issue !== "object") return String(issue);
    const code = pick(issue, "code", "kind", "type", "name", "blocker", "warning");
    const detail = pick(issue, "detail", "message", "reason", "tape_id", "key_id", "dependency_id");
    if (code && detail) return human(String(code).replace(/([a-z])([A-Z])/g, "$1_$2")) + ": " + detail;
    if (code) return human(String(code).replace(/([a-z])([A-Z])/g, "$1_$2"));
    const entries = Object.entries(issue).filter(([, value]) => value !== null && value !== undefined);
    if (entries.length === 1) {
      const [key, value] = entries[0];
      return human(key.replace(/([a-z])([A-Z])/g, "$1_$2")) + (value === true ? "" : ": " + (typeof value === "object" ? JSON.stringify(value) : value));
    }
    return conciseEvidence(issue);
  }

  function planArray(plan, ...keys) {
    const value = pick(plan, ...keys);
    return Array.isArray(value) ? value : [];
  }

  function renderPlanRequirements(title, values, emptyText) {
    return '<section class="restore-result-section"><h3>' + esc(title) + '</h3>' + (values.length ? '<ul class="restore-result-list">' + values.map((value) => '<li>' + esc(typeof value === "object" ? planIssueText(value) : value) + "</li>").join("") + "</ul>" : '<p class="restore-result-empty">' + esc(emptyText) + "</p>") + "</section>";
  }

  function planObjectFacts(value) {
    if (!value || typeof value !== "object") return [];
    return Object.entries(value)
      .filter(([, item]) => item !== null && item !== undefined && item !== "" && !Array.isArray(item))
      .map(([key, item]) => human(key) + ": " + (typeof item === "object" ? conciseEvidence(item) : item));
  }

  function restorePlanExecution(plan) {
    const value = pick(plan, "execution", "rehearsal_execution");
    return value && typeof value === "object" && !Array.isArray(value) ? value : {};
  }

  function restorePlanTapes(plan) {
    const direct = planArray(plan, "required_tape_ids", "required_tapes", "tapes");
    return direct.length ? direct : planArray(restorePlanExecution(plan), "required_tape_ids", "required_tapes", "tapes");
  }

  function restorePlanMultiVolumeSupported(plan) {
    const execution = restorePlanExecution(plan);
    const tapes = restorePlanTapes(plan);
    const declaredCount = Number(pick(execution, "volume_count"));
    return tapes.length > 1 && pick(execution, "multi_volume") === true && Number.isInteger(declaredCount) && declaredCount === tapes.length;
  }

  function restorePlanVolumeContractReady(plan) {
    const tapes = restorePlanTapes(plan);
    return tapes.length === 1 || restorePlanMultiVolumeSupported(plan);
  }

  function renderRestorePlanTapeSequence(tapes, currentOrder) {
    if (!tapes.length) return "";
    return '<div class="backup-volume-plan restore-volume-plan"><span>Exact ordered restore media</span><ol>' + tapes.map((tape, index) => {
      const sequence = index + 1;
      return '<li class="' + (sequence === currentOrder ? "current" : "pending") + '"' + (sequence === currentOrder ? ' aria-current="step"' : "") + '><span class="restore-volume-marker">' + esc(sequence) + '</span><span><strong>Tape ' + esc(sequence) + '</strong><code>' + esc(tape) + '</code><small>' + (sequence === currentOrder ? "Load to start" : "Requested during restore") + "</small></span></li>";
    }).join("") + "</ol></div>";
  }

  function restorePlanDrives(plan) {
    const execution = restorePlanExecution(plan);
    const drives = arrayValue(execution, "compatible_drives", "eligible_drives", "drives");
    return drives.length ? drives : planArray(plan, "execution_drives", "compatible_drives", "eligible_drives");
  }

  function restorePlanConfirmation(plan) {
    const planId = pick(plan, "id", "plan_id");
    return String(pick(restorePlanExecution(plan), "confirmation", "confirmation_phrase") || pick(plan, "confirmation", "confirmation_phrase") || (planId ? "RESTORE PLAN " + planId + " TO ISOLATED DESTINATION" : ""));
  }

  function renderRestorePlanResult(plan, point) {
    const ready = plan && plan.ready === true;
    const blockers = planArray(plan, "blockers");
    const warnings = planArray(plan, "warnings");
    const tapes = restorePlanTapes(plan);
    const multiVolume = tapes.length > 1;
    const volumeContractReady = restorePlanVolumeContractReady(plan);
    const dependencies = planArray(plan, "dependency_chain", "required_dependencies", "dependencies");
    const keys = planArray(plan, "required_key_ids", "required_keys", "keys");
    const verification = planArray(plan, "verification_prerequisites");
    const destination = planObjectFacts(pick(plan, "destination"));
    const estimates = planObjectFacts(pick(plan, "estimates"));
    const planId = pick(plan, "id", "plan_id");
    const previewHash = pick(plan, "preview_hash", "sha256", "digest");
    const expiresAt = pick(plan, "expires_at");
    const executionOffered = pick(plan, "execution_supported") === true;
    const executionSupported = ready && executionOffered && volumeContractReady;
    const executionDrives = restorePlanDrives(plan);
    const resultNotice = ready
      ? '<div class="restore-result-hero ready"><span aria-hidden="true">✓</span><div><small>Readiness result</small><h3>Prerequisites satisfied</h3><p>' + (executionSupported ? "This supported verified Full recovery point can advance to an attended " + (multiVolume ? tapes.length + "-tape" : "single-tape") + " Restore run." : "The plan is ready for review, but the server did not offer a matching attended execution contract for this ordered media set.") + '</p></div></div>'
      : '<div class="restore-result-hero blocked"><span aria-hidden="true">!</span><div><small>Readiness result</small><h3>Restore remains blocked</h3><p>' + blockers.length + " prerequisite" + (blockers.length === 1 ? "" : "s") + " must be resolved before any execution workflow.</p></div></div>";
    const facts = [
      ["Plan ID", planId],
      ["Preview SHA-256", previewHash],
      ["Generated", date(pick(plan, "created_at", "generated_at"))],
      ["Expires", date(expiresAt)],
      ["Plan status", pick(plan, "status")],
      ["Recovery point", recordId(point)],
      ["Restore media", tapes.length ? tapes.length + " ordered tape" + (tapes.length === 1 ? "" : "s") : null]
    ].filter((item) => item[1] !== null && item[1] !== undefined && item[1] !== "Not reported");
    const executionDetail = executionOffered && !volumeContractReady
      ? "The server did not bind this multi-tape plan to both multi_volume=true and an exact matching volume_count. Generate a fresh plan before loading media."
      : pick(plan, "execution_detail") || (ready ? "This recovery format is not supported by the attended Restore run worker." : "Resolve every planning blocker and generate a fresh plan.");
    const mediaInstructions = multiVolume
      ? "Load tape 1 to start. TapeScale will pause durably at each media boundary and name the exact next cartridge before the same Bacula restore job continues."
      : "Load the one retained cartridge to start and keep it in the selected drive until the read completes.";
    const executionAction = executionSupported
      ? '<section class="restore-execution-offer"><div><span class="lab-badge">Alpha · attended ' + (multiVolume ? "multi-tape" : "single-tape") + ' Restore run</span><h3>Restore this complete recovery point</h3><p>' + esc(mediaInstructions) + ' TapeScale writes only beneath the isolated destination and verifies the restored manifest. This flow remains Full-point-only and does not support file selection, encrypted recovery points, or clean-catalog recovery.</p>' + renderRestorePlanTapeSequence(tapes, 1) + '</div><button class="button button-primary" type="button" data-action="open-restore-rehearsal" data-plan-id="' + esc(planId) + '">Start Restore run</button></section>'
      : '<div class="notice info"><span class="notice-icon" aria-hidden="true">i</span><div class="notice-copy"><strong>' + (ready ? "This ready plan remains planning-only" : "Execution remains blocked") + '</strong><span>' + esc(executionDetail) + "</span></div></div>";
    return resultNotice + '<div class="restore-result-boundary"><strong>Plan retained; nothing executed yet</strong><span>A Restore run starts only after exact-drive, first-tape, isolated-destination, and attended confirmation. Every later tape requires its own exact attended handoff.</span></div><dl class="restore-result-facts">' + facts.map(([name, value]) => "<dt>" + esc(name) + '</dt><dd class="mono">' + esc(value) + "</dd>").join("") + '</dl><div class="restore-result-grid">' + renderPlanRequirements("Required media in order", tapes.map((tape, index) => (index + 1) + ". " + tape), "No media requirement was returned.") + renderPlanRequirements("Dependency chain", dependencies, "No earlier recovery point is required.") + renderPlanRequirements("Required keys", keys, "No recovery key requirement was returned.") + renderPlanRequirements("Verification gates", verification, "No verification prerequisite was returned.") + renderPlanRequirements("Destination", destination, "No destination assessment was returned.") + renderPlanRequirements("Read & staging estimate", estimates, "No work estimate was returned.") + renderPlanRequirements("Blockers", blockers.map(planIssueText), "No blocking prerequisite remains.") + renderPlanRequirements("Warnings", warnings.map(planIssueText), "No planning warning was returned.") + "</div>" + executionAction;
  }

  async function submitRestorePlan(form) {
    const id = form.dataset.recoveryPointId;
    const point = state.data.recoveryPoints.find((item) => recordId(item) === id);
    if (!point) return;
    const values = new FormData(form);
    const destination = String(values.get("destination") || "").trim();
    const errorBox = form.querySelector("#restore-plan-error");
    const submit = form.querySelector('[type="submit"]');
    if (!destination || destination.startsWith("/") || destination.split(/[\\/]+/).includes("..") || /[\u0000-\u001f]/.test(destination)) {
      errorBox.textContent = "Destination must be a contained relative path without parent traversal.";
      errorBox.hidden = false;
      return;
    }
    const current = recoveryPointPlanDraft(point);
    const payload = {
      recovery_point_id: id,
      mode: String(values.get("mode") || "file_archive"),
      destination,
      conflict_policy: String(values.get("conflict_policy") || "skip"),
      key_available: values.get("key_available") === "on" || !pick(point, "key_lineage_id"),
      key_recovery_package_tested: values.get("key_recovery_package_tested") === "on" || !pick(point, "key_lineage_id"),
      destination_approved: values.get("destination_approved") === "on",
      is_new_destination: values.get("is_new_destination") === "on",
      conflicts_found: current.conflicts_found || 0,
      acl_semantics_supported: values.get("acl_semantics_supported") === "on",
      identity_mapping_complete: values.get("identity_mapping_complete") === "on",
      estimated_required_bytes: current.estimated_required_bytes,
      available_bytes: optionalFormNumber(values, "available_bytes"),
      required_free_reserve_bytes: optionalFormNumber(values, "required_free_reserve_bytes") || 0,
      administrator_reauthenticated: false,
      destructive_preview_accepted: false,
      automatic_publication_requested: false
    };
    state.restorePlanDrafts[id] = payload;
    submit.disabled = true;
    submit.textContent = "Evaluating prerequisites…";
    errorBox.hidden = true;
    try {
      const response = await request("/restores/plan", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify(payload)
      });
      const plan = response && response.plan && typeof response.plan === "object" ? response.plan : response;
      if (plan && pick(plan, "id", "plan_id")) state.restorePlans[String(pick(plan, "id", "plan_id"))] = plan;
      openModal("Restore readiness plan", renderRestorePlanResult(plan || {}, point), '<button class="button button-quiet" type="button" data-action="revise-restore-plan" data-id="' + esc(id) + '">Revise assessment</button><button class="button button-primary" type="button" data-action="close-modal">Done</button>');
      const modal = modalLayer.querySelector(".modal");
      if (modal) modal.classList.add("restore-plan-modal", "restore-plan-result-modal");
      live.textContent = plan && plan.ready ? "Restore readiness plan has no blockers. No restore was executed." : "Restore readiness plan retained with blocking prerequisites. No restore was executed.";
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Generate readiness plan";
    }
  }

  function openRestoreRehearsal(planId) {
    const plan = state.restorePlans[String(planId)];
    if (!plan || plan.ready !== true || pick(plan, "execution_supported") !== true) {
      toast("Restore run is not available", "Generate a fresh ready plan for a supported verified Full recovery point with an attended execution contract.");
      return;
    }
    const drives = restorePlanDrives(plan);
    const tapes = restorePlanTapes(plan);
    if (!restorePlanVolumeContractReady(plan)) {
      toast("Restore run is not available", tapes.length > 1 ? "The server did not explicitly offer multi-volume execution with a matching ordered volume count." : "The server did not return one exact retained cartridge for this execution.");
      return;
    }
    const multiVolume = tapes.length > 1;
    const firstTape = tapes[0];
    const destination = objectValue(plan, "destination");
    const destinationPath = pick(destination, "path", "destination") || pick(restorePlanExecution(plan), "destination_subpath", "destination") || pick(plan, "destination_subpath") || "the recorded isolated destination";
    const phrase = restorePlanConfirmation(plan);
    const driveOptions = drives.map((drive) => {
      const driveId = typeof drive === "string" ? drive : recordId(drive);
      const name = typeof drive === "string" ? drive : [pick(drive, "vendor"), pick(drive, "model"), pick(drive, "stable_id")].filter(Boolean).join(" ");
      const path = typeof drive === "string" ? null : pick(drive, "observed_path", "path");
      return '<option value="' + esc(driveId) + '">' + esc(name || driveId) + (path ? " · " + esc(path) : "") + "</option>";
    });
    const boundary = multiVolume
      ? "This Alpha restores the complete recovery point across " + tapes.length + " exact ordered cartridges. Load only tape 1 now; the run pauses durably and names each next tape before it can continue."
      : "This Alpha restores the complete recovery point from one retained cartridge.";
    const body = '<form id="restore-rehearsal-form" data-plan-id="' + esc(planId) + '"><input type="hidden" name="preview_hash" value="' + esc(pick(plan, "preview_hash")) + '"><div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Attended ' + (multiVolume ? "multi-tape" : "single-tape") + ' Restore run</strong><span>' + esc(boundary) + ' It writes only beneath the new isolated destination and never writes to tape or source data. File or directory selection, encrypted recovery points, and clean-catalog recovery are not supported. This local console has no application sign-in, so use it only on a trusted management network and keep the operator present.</span></div></div><div class="restore-rehearsal-summary"><div><small>Ordered restore tapes</small><strong>' + esc(tapes.join(" → ")) + '</strong></div><div><small>Load first</small><strong>' + esc(firstTape) + '</strong></div><div><small>Isolated destination</small><strong class="mono">' + esc(destinationPath) + '</strong></div><div><small>Plan preview</small><code>' + esc(pick(plan, "preview_hash")) + '</code></div></div>' + renderRestorePlanTapeSequence(tapes, 1) + '<div class="form-grid"><div class="form-field full"><label for="restore-rehearsal-drive">Drive containing tape 1</label><select class="select" id="restore-rehearsal-drive" name="drive_id" required>' + driveOptions.join("") + '</select><small>The worker rechecks stable identity, path, firmware, cartridge, qualification, and plan hash before reading. Later tapes use separate exact handoffs.</small></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="tape_loaded_confirmed" required> I physically verified that tape 1, <strong>' + esc(firstTape) + '</strong>, is loaded in the selected drive.</label></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="isolated_destination_confirmed" required> I verified that <strong class="mono">' + esc(destinationPath) + '</strong> is a new isolated recovery destination, not live data.</label></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="attended_confirmed" required> I will remain available for every exact tape handoff and safe-cancellation prompt until this Restore run finishes.</label></div><div class="form-field full"><label for="restore-rehearsal-confirmation">Type the exact attended confirmation</label><input class="input input-mono" id="restore-rehearsal-confirmation" name="confirmation" autocomplete="off" spellcheck="false" required><small>Required: <strong class="mono">' + esc(phrase) + '</strong></small></div><div class="form-field full"><label for="restore-rehearsal-actor">Operator</label><input class="input" id="restore-rehearsal-actor" name="actor" maxlength="128" value="Web operator" required></div></div><p class="notice danger" id="restore-rehearsal-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit"' + (drives.length ? "" : " disabled") + '>Start Restore run</button></div></form>';
    openModal("Start Restore run", body);
    const input = document.querySelector("#restore-rehearsal-confirmation");
    if (input) input.focus();
  }

  async function submitRestoreRehearsal(form) {
    const values = new FormData(form);
    const planId = form.dataset.planId;
    const plan = state.restorePlans[String(planId)];
    const multiVolume = restorePlanMultiVolumeSupported(plan || {});
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#restore-rehearsal-error");
    const phrase = restorePlanConfirmation(plan || { id: planId });
    if (String(values.get("confirmation") || "") !== phrase) {
      errorBox.textContent = "Confirmation must exactly match the displayed phrase.";
      errorBox.hidden = false;
      return;
    }
    submit.disabled = true;
    submit.textContent = "Starting Restore run…";
    errorBox.hidden = true;
    try {
      const response = await request(endpoints.restoreRehearsals, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          plan_id: planId,
          preview_hash: String(values.get("preview_hash") || ""),
          drive_id: String(values.get("drive_id") || ""),
          confirmation: String(values.get("confirmation") || ""),
          tape_loaded_confirmed: values.get("tape_loaded_confirmed") === "on",
          isolated_destination_confirmed: values.get("isolated_destination_confirmed") === "on",
          attended_confirmed: values.get("attended_confirmed") === "on",
          actor: String(values.get("actor") || "").trim()
        })
      });
      const rehearsal = response && (response.rehearsal || response.run || response);
      if (rehearsal && typeof rehearsal === "object") {
        state.data.restoreRehearsals = [rehearsal].concat(state.data.restoreRehearsals.filter((item) => recordId(item) !== recordId(rehearsal)));
        state.selectedRestoreRehearsalId = recordId(rehearsal);
      }
      closeModal();
      await loadOne("restoreRehearsals");
      const selected = selectedRestoreRehearsal();
      if (selected) await loadRestoreRehearsalDetail(recordId(selected), true);
      updateChrome();
      render();
      scheduleRestoreRehearsalPoll();
      live.textContent = "Restore run queued. The Recovery Vault is showing its durable phase timeline.";
      toast("Restore run queued", multiVolume ? "Keep tape 1 loaded. TapeScale will pause and identify each exact next cartridge in the Recovery Vault." : "Keep the exact tape loaded and the isolated destination online.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Start Restore run";
    }
  }

  function restoreMediaReadinessValue(payload) {
    if (!payload || typeof payload !== "object") return {};
    const nested = payload.readiness && typeof payload.readiness === "object" ? payload.readiness : {};
    return Object.assign({}, payload, nested);
  }

  async function loadRestoreMediaReadiness(id) {
    const payload = await request(endpoints.restoreRehearsals + "/" + encodeURIComponent(id) + "/media-readiness");
    const readiness = restoreMediaReadinessValue(payload);
    state.data.restoreMediaReadiness[id] = readiness;
    return readiness;
  }

  function restoreReadinessMediaRequest(readiness) {
    return objectValue(readiness, "media_request", "current_media_request");
  }

  function restoreReadinessHandoff(readiness) {
    return objectValue(readiness, "media_handoff", "handoff");
  }

  function restoreReadinessCandidates(readiness) {
    const handoff = restoreReadinessHandoff(readiness);
    let candidates = arrayValue(readiness, "candidates", "eligible_pairs", "compatible_pairs");
    if (!candidates.length) candidates = arrayValue(handoff, "candidates", "eligible_pairs", "compatible_pairs");
    const recommended = pick(readiness, "recommended_candidate", "candidate") || pick(handoff, "recommended_candidate", "candidate");
    if (!candidates.length && recommended && typeof recommended === "object") candidates = [recommended];
    const directDrive = objectValue(readiness, "drive");
    if (Object.keys(directDrive).length) candidates = candidates.map((candidate) => Object.assign({ drive: directDrive }, candidate));
    return candidates;
  }

  function restoreMediaCandidateDrive(candidate) {
    const nested = objectValue(candidate, "drive");
    const driveId = String(pick(candidate, "drive_id") || recordId(nested) || "");
    return state.data.drives.find((drive) => recordId(drive) === driveId) || Object.assign({
      id: driveId,
      stable_id: pick(candidate, "stable_id", "expected_stable_id"),
      observed_path: pick(candidate, "device_path", "observed_path", "path")
    }, nested);
  }

  function restoreMediaBlockerMessage(readiness) {
    const blockers = arrayValue(readiness, "blockers");
    const blocker = blockers[0];
    return blocker ? String(pick(blocker, "message", "detail", "reason") || planIssueText(blocker)) : String(pick(readiness, "reason") || "No currently safe drive can accept this exact restore tape.");
  }

  function restoreMediaModalCurrent(form, token) {
    return token === state.restoreMediaModalToken && !modalLayer.hidden && Boolean(form && form.isConnected);
  }

  async function openRestoreMediaHandoff(id) {
    if (!id) return;
    const token = ++state.restoreMediaModalToken;
    openModal("Load next restore tape", '<div class="source-browser-loading" role="status"><span class="spinner" aria-hidden="true"></span><strong>Rechecking the exact tape request…</strong><small>The Restore run remains durably paused while this page refreshes.</small></div>');
    let readiness;
    try {
      readiness = await loadRestoreMediaReadiness(id);
    } catch (error) {
      if (token !== state.restoreMediaModalToken || modalLayer.hidden) return;
      openModal("Load next restore tape", '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not load the restore media request</strong><span>' + esc(errorMessage(error)) + '</span></div></div><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Finish later</button><button class="button button-primary" type="button" data-action="open-restore-media-handoff" data-id="' + esc(id) + '">Try again</button></div>');
      return;
    }
    if (token !== state.restoreMediaModalToken || modalLayer.hidden) return;
    const requestRecord = restoreReadinessMediaRequest(readiness);
    const handoff = restoreReadinessHandoff(readiness);
    const required = pick(handoff, "required") === true;
    const waiting = pick(handoff, "waiting") === true || String(pick(handoff, "state") || "").toLowerCase() === "waiting_for_media";
    if (!required || !waiting) {
      closeModal();
      await Promise.all([loadOne("restoreRehearsals"), loadRestoreRehearsalDetail(id, true)]);
      render();
      toast("Tape request changed", "This Restore run is no longer waiting for that handoff. Current durable status is now displayed.");
      return;
    }
    const requestId = String(pick(requestRecord, "id", "request_id") || "");
    const volumeOrder = Number(pick(handoff, "next_volume_order", "volume_order") || pick(requestRecord, "volume_order", "volume_sequence"));
    const totalVolumes = Number(pick(handoff, "total_volumes", "volume_count") || arrayValue(readiness, "volumes", "required_volumes").length);
    const cartridgeId = String(pick(handoff, "next_cartridge_id", "expected_cartridge_id", "cartridge_id") || pick(requestRecord, "expected_cartridge_id", "cartridge_id") || "");
    const cartridgeLabel = String(pick(handoff, "next_cartridge_label", "next_volume_label", "expected_cartridge_label", "cartridge_label") || pick(requestRecord, "expected_cartridge_label", "cartridge_label", "expected_volume_label") || "");
    const confirmation = String(pick(handoff, "confirmation_phrase", "required_confirmation") || pick(requestRecord, "confirmation_phrase", "required_confirmation") || "");
    if (!requestId || !Number.isInteger(volumeOrder) || volumeOrder < 1 || !cartridgeId || !cartridgeLabel || !confirmation) {
      openModal("Load next restore tape", '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Incomplete durable tape request</strong><span>The server did not bind this handoff to one request ID, ordered volume, cartridge, label, and confirmation phrase. No tape was accepted.</span></div></div><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Finish later</button><button class="button button-primary" type="button" data-action="open-restore-media-handoff" data-id="' + esc(id) + '">Check again</button></div>');
      return;
    }
    const candidates = restoreReadinessCandidates(readiness);
    const blockers = arrayValue(readiness, "blockers");
    const drives = candidates.map(restoreMediaCandidateDrive).filter((drive, index, values) => recordId(drive) && values.findIndex((item) => recordId(item) === recordId(drive)) === index);
    const blocked = pick(readiness, "ready") !== true || blockers.length > 0 || drives.length === 0;
    const driveOptions = drives.map((drive) => '<option value="' + esc(recordId(drive)) + '" data-stable-id="' + esc(pick(drive, "stable_id")) + '"' + (drives.length === 1 ? " selected" : "") + '>' + esc([pick(drive, "vendor"), pick(drive, "model", "stable_id"), pick(drive, "observed_path", "path")].filter(Boolean).join(" · ")) + "</option>").join("");
    const pseudoRun = {
      state: "running",
      phase: "waiting_for_media",
      volumes: arrayValue(readiness, "volumes", "required_volumes"),
      media_request: requestRecord,
      media_handoff: handoff
    };
    const blockedNotice = blocked ? '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Exact tape handoff is not ready</strong><span>' + esc(restoreMediaBlockerMessage(readiness)) + '</span></div><button class="button button-small" type="button" data-action="open-restore-media-handoff" data-id="' + esc(id) + '">Check again</button></div>' : "";
    const body = '<form id="restore-media-handoff-form" data-run-id="' + esc(id) + '" data-request-id="' + esc(requestId) + '" data-volume-order="' + esc(volumeOrder) + '" data-cartridge-id="' + esc(cartridgeId) + '" data-cartridge-label="' + esc(cartridgeLabel) + '" data-confirmation="' + esc(confirmation) + '"><div class="backup-boundary warning restore-media-boundary"><span aria-hidden="true">!</span><div><strong>Load tape ' + esc(volumeOrder) + ' of ' + esc(totalVolumes || "?") + ': ' + esc(cartridgeLabel) + '</strong><p>Unload and remove the previous tape, then insert this exact recorded cartridge. This handoff is read-only: TapeScale will not erase, relabel, or append to it.</p></div></div>' + renderRestoreVolumeSequence(pseudoRun) + '<div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Attended Alpha boundary</strong><span>This continues one complete Full-point restore into its existing isolated destination. Selective, encrypted, and clean-catalog recovery remain unsupported. This local console has no application sign-in.</span></div></div>' + blockedNotice + '<div class="form-grid"' + (blocked ? " hidden" : "") + '><div class="form-field full"><label for="restore-media-drive">Drive containing the exact next tape</label><select class="select" id="restore-media-drive" name="drive_id" required><option value="">Choose drive</option>' + driveOptions + '</select><small>The server will recheck drive identity, qualification, reservation, and the expected cartridge before Bacula resumes.</small></div><div class="form-field full"><label>Exact recorded tape</label><div class="restore-exact-media"><strong>' + esc(cartridgeLabel) + '</strong><span>Tape ' + esc(volumeOrder) + ' of ' + esc(totalVolumes || "?") + '</span></div></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="loaded_media_confirmed" required> I physically verified that <strong>' + esc(cartridgeLabel) + '</strong> is loaded in the selected drive.</label></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="attended_confirmed" required> I am attending this handoff and will keep the isolated destination and tape drive available while the Restore run resumes.</label></div><div class="form-field full"><label for="restore-media-confirmation">Type the exact tape handoff confirmation</label><input class="input input-mono" id="restore-media-confirmation" name="confirmation" autocomplete="off" spellcheck="false" required><small>Required: <strong class="mono">' + esc(confirmation) + '</strong></small></div></div><p class="notice danger" id="restore-media-error" role="alert" aria-live="assertive" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Finish later</button><button class="button button-primary" type="submit"' + (blocked ? " disabled" : "") + '>Confirm exact tape and continue</button></div></form>';
    openModal("Load restore tape " + volumeOrder, body);
    const modal = modalLayer.querySelector(".modal");
    if (modal) modal.classList.add("restore-media-modal");
  }

  async function submitRestoreMediaHandoff(form) {
    const runId = form.dataset.runId;
    const token = state.restoreMediaModalToken;
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#restore-media-error");
    const loaded = values.get("loaded_media_confirmed") === "on";
    const attended = values.get("attended_confirmed") === "on";
    const typedConfirmation = String(values.get("confirmation") || "");
    if (!loaded || !attended || typedConfirmation !== form.dataset.confirmation) {
      errorBox.textContent = "Confirm the exact loaded tape, attended handoff, and server-provided phrase before continuing.";
      errorBox.hidden = false;
      return;
    }
    submit.disabled = true;
    submit.textContent = "Rechecking exact tape request…";
    errorBox.hidden = true;
    try {
      const driveId = String(values.get("drive_id") || "");
      const readiness = await loadRestoreMediaReadiness(runId);
      if (!restoreMediaModalCurrent(form, token)) return;
      const requestRecord = restoreReadinessMediaRequest(readiness);
      const handoff = restoreReadinessHandoff(readiness);
      const currentRequestId = String(pick(requestRecord, "id", "request_id") || "");
      const currentOrder = Number(pick(handoff, "next_volume_order", "volume_order") || pick(requestRecord, "volume_order", "volume_sequence"));
      const currentCartridgeId = String(pick(handoff, "next_cartridge_id", "expected_cartridge_id", "cartridge_id") || pick(requestRecord, "expected_cartridge_id", "cartridge_id") || "");
      const currentLabel = String(pick(handoff, "next_cartridge_label", "next_volume_label", "expected_cartridge_label", "cartridge_label") || pick(requestRecord, "expected_cartridge_label", "cartridge_label", "expected_volume_label") || "");
      const currentConfirmation = String(pick(handoff, "confirmation_phrase", "required_confirmation") || pick(requestRecord, "confirmation_phrase", "required_confirmation") || "");
      if (currentRequestId !== form.dataset.requestId || currentOrder !== Number(form.dataset.volumeOrder) || currentCartridgeId !== form.dataset.cartridgeId || currentLabel !== form.dataset.cartridgeLabel || currentConfirmation !== form.dataset.confirmation || typedConfirmation !== currentConfirmation) {
        throw new Error("The exact restore tape request changed while this form was open. Review the current durable handoff before continuing.");
      }
      const candidate = restoreReadinessCandidates(readiness).find((item) => recordId(restoreMediaCandidateDrive(item)) === driveId);
      if (!candidate) throw new Error(restoreMediaBlockerMessage(readiness));
      if (String(pick(candidate, "cartridge_id") || "") !== currentCartridgeId || Number(pick(candidate, "volume_order")) !== currentOrder) throw new Error("The currently observed tape does not match the exact requested restore volume.");
      const drive = restoreMediaCandidateDrive(candidate);
      const stableId = String(pick(candidate, "stable_id", "expected_stable_id") || pick(drive, "stable_id") || "");
      if (!stableId) throw new Error("The selected drive has no stable identity in the current readiness response.");
      submit.textContent = "Continuing Restore run…";
      await request(endpoints.restoreRehearsals + "/" + encodeURIComponent(runId) + "/media", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          request_id: currentRequestId,
          drive_id: driveId,
          expected_stable_id: stableId,
          cartridge_id: currentCartridgeId,
          typed_cartridge_label: currentLabel,
          volume_order: currentOrder,
          loaded_media_confirmed: loaded,
          attended_confirmed: attended,
          confirmation: currentConfirmation
        })
      });
      await Promise.all([loadOne("restoreRehearsals"), loadOne("drives"), loadOne("media")]);
      await loadRestoreRehearsalDetail(runId, true);
      if (restoreMediaModalCurrent(form, token)) closeModal();
      updateChrome();
      render();
      scheduleRestoreRehearsalPoll();
      toast("Restore tape accepted", "TapeScale retained the attended handoff. Bacula will verify the on-tape label before continuing the same restore job.");
    } catch (error) {
      if (!restoreMediaModalCurrent(form, token)) return;
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Confirm exact tape and continue";
    }
  }

  function openCancelRestoreRehearsal(id) {
    const phrase = "CANCEL RESTORE REHEARSAL " + id;
    const body = '<form id="cancel-restore-rehearsal-form" data-rehearsal-id="' + esc(id) + '"><div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Cancellation is a request, not an immediate tape interruption</strong><span>The worker will stop at a safe engine boundary and retain every event and artifact already produced.</span></div></div><div class="form-field"><label for="cancel-restore-rehearsal-confirmation">Type the exact confirmation</label><input class="input input-mono" id="cancel-restore-rehearsal-confirmation" name="confirmation" autocomplete="off" spellcheck="false" required><small>Required: <strong class="mono">' + esc(phrase) + '</strong></small></div><div class="form-field section"><label for="cancel-restore-rehearsal-actor">Operator</label><input class="input" id="cancel-restore-rehearsal-actor" name="actor" maxlength="128" value="Web operator" required></div><p class="notice danger" id="cancel-restore-rehearsal-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Keep running</button><button class="button button-danger" type="submit">Request safe cancel</button></div></form>';
    openModal("Cancel Restore run", body);
    document.querySelector("#cancel-restore-rehearsal-confirmation").focus();
  }

  async function submitCancelRestoreRehearsal(form) {
    const id = form.dataset.rehearsalId;
    const values = new FormData(form);
    const errorBox = form.querySelector("#cancel-restore-rehearsal-error");
    const submit = form.querySelector('[type="submit"]');
    const phrase = "CANCEL RESTORE REHEARSAL " + id;
    if (String(values.get("confirmation") || "") !== phrase) {
      errorBox.textContent = "Confirmation must exactly match the displayed phrase.";
      errorBox.hidden = false;
      return;
    }
    submit.disabled = true;
    submit.textContent = "Recording request…";
    errorBox.hidden = true;
    try {
      await request(endpoints.restoreRehearsals + "/" + encodeURIComponent(id) + "/cancel", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({ confirmation: phrase, actor: String(values.get("actor") || "").trim() })
      });
      closeModal();
      await Promise.all([loadOne("restoreRehearsals"), loadRestoreRehearsalDetail(id, true)]);
      updateChrome();
      render();
      scheduleRestoreRehearsalPoll();
      toast("Safe cancellation requested", "The worker will retain evidence and stop at an allowed boundary.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Request safe cancel";
    }
  }

  async function downloadRestoreRehearsalEvidence(button) {
    const id = button.dataset.id;
    if (!id) return;
    const original = button.textContent;
    button.disabled = true;
    button.textContent = "Preparing evidence…";
    try {
      const response = await fetch(API + endpoints.restoreRehearsals + "/" + encodeURIComponent(id) + "/evidence", { headers: { Accept: "application/json" }, cache: "no-store" });
      const body = await response.text();
      if (!response.ok) {
        let payload = null;
        try { payload = JSON.parse(body); } catch (_) { payload = null; }
        const error = new Error("Request failed with HTTP " + response.status);
        error.payload = payload || { message: body };
        throw error;
      }
      let content = body;
      try { content = JSON.stringify(JSON.parse(body), null, 2) + "\n"; } catch (_) { /* Preserve server bytes if JSON formatting is unavailable. */ }
      const disposition = response.headers.get("Content-Disposition") || "";
      const match = disposition.match(/filename\*?=(?:UTF-8''|\")?([^\";]+)/i);
      const suggested = match ? decodeURIComponent(match[1].replace(/^\"|\"$/g, "")) : "tapescale-restore-run-" + id + "-evidence.json";
      const filename = suggested.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+/, "") || "tapescale-restore-run-evidence.json";
      const url = URL.createObjectURL(new Blob([content], { type: "application/json;charset=utf-8" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = filename;
      document.body.append(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      toast("Restore evidence downloaded", "Saved the bounded canonical report as " + filename + ".");
    } catch (error) {
      toast("Evidence download failed", errorMessage(error));
    } finally {
      button.disabled = false;
      button.textContent = original;
    }
  }

  async function downloadRecoveryEvidence(button) {
    const id = button.dataset.id;
    if (!id) return;
    const original = button.textContent;
    button.disabled = true;
    button.textContent = "Preparing evidence…";
    try {
      const response = await fetch(API + endpoints.recoveryPoints + "/" + encodeURIComponent(id) + "/evidence", {
        headers: { Accept: "application/json" },
        cache: "no-store"
      });
      const textBody = await response.text();
      if (!response.ok) {
        let payload = null;
        try { payload = JSON.parse(textBody); } catch (_) { payload = null; }
        const error = new Error("Request failed with HTTP " + response.status);
        error.payload = payload || { message: textBody };
        throw error;
      }
      let content = textBody;
      try { content = JSON.stringify(JSON.parse(textBody), null, 2) + "\n"; } catch (_) { /* Preserve a valid server-provided JSON stream as received. */ }
      const disposition = response.headers.get("Content-Disposition") || "";
      const match = disposition.match(/filename\*?=(?:UTF-8''|\")?([^\";]+)/i);
      const suggested = match ? decodeURIComponent(match[1].replace(/^\"|\"$/g, "")) : "tapescale-recovery-evidence-" + id + ".json";
      const filename = suggested.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+/, "") || "tapescale-recovery-evidence.json";
      const url = URL.createObjectURL(new Blob([content], { type: "application/json;charset=utf-8" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = filename;
      document.body.append(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      toast("Evidence downloaded", "The canonical recovery report was saved as " + filename + ".");
    } catch (error) {
      toast("Evidence download failed", errorMessage(error));
    } finally {
      button.disabled = false;
      button.textContent = original;
    }
  }

  function operationKey(prefix) {
    const suffix = window.crypto && typeof window.crypto.randomUUID === "function" ? window.crypto.randomUUID() : Date.now() + "-" + Math.random().toString(16).slice(2);
    return prefix + "-" + suffix;
  }

  function openLocationForm() {
    const body = '<form id="location-form"><div class="notice info"><span class="notice-icon" aria-hidden="true">◇</span><div class="notice-copy"><strong>Define a physical custody location</strong><span>Creating a location does not move any cartridge. Custody changes only after a physical confirmation.</span></div></div><div class="form-grid"><div class="form-field full"><label for="location-name">Location name</label><input class="input" id="location-name" name="name" maxlength="256" required placeholder="Iron Mountain · Vault B"></div><div class="form-field"><label for="location-kind">Location type</label><select class="select" id="location-kind" name="kind"><option value="onsite">On-site</option><option value="offsite">Off-site</option><option value="in_transit">In transit</option><option value="device">Tape device</option><option value="unknown">Unknown</option></select></div><div class="form-field"><label for="location-contact">Contact or carrier</label><input class="input" id="location-contact" name="contact" maxlength="256" placeholder="Optional"></div><div class="form-field full"><label for="location-address">Address / handling detail</label><textarea class="textarea" id="location-address" name="address" maxlength="2000" placeholder="Optional physical address, vault reference, or handling instructions"></textarea></div><div class="form-field full"><label for="location-actor">Operator label</label><input class="input" id="location-actor" name="actor" maxlength="128" value="Web operator" required></div></div><p class="notice danger" id="location-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Create location</button></div></form>';
    openModal("Add custody location", body);
    document.querySelector("#location-name").focus();
  }

  async function submitLocation(form) {
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#location-error");
    submit.disabled = true;
    submit.textContent = "Creating…";
    errorBox.hidden = true;
    try {
      await request(endpoints.locations, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          name: String(values.get("name") || "").trim(),
          kind: String(values.get("kind") || "onsite"),
          details: {
            contact: String(values.get("contact") || "").trim() || null,
            address: String(values.get("address") || "").trim() || null
          },
          actor: String(values.get("actor") || "").trim() || null
        })
      });
      closeModal();
      await loadOne("locations");
      render();
      toast("Location created", "The location is available for movement planning; no cartridge custody changed.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Create location";
    }
  }

  function localDateTimeValue(value) {
    const input = value instanceof Date ? value : new Date(value);
    const adjusted = new Date(input.getTime() - input.getTimezoneOffset() * 60000);
    return adjusted.toISOString().slice(0, 16);
  }

  function mediaMatchesMovementSource(item, sourceValue) {
    const nestedLocation = objectValue(item, "confirmed_location", "location");
    const rawLocationId = pick(item, "confirmed_location_id", "location_id") || pick(nestedLocation, "id", "uuid");
    const rawLocationName = pick(item, "confirmed_location_name", "location_name") || pick(nestedLocation, "name");
    if (sourceValue === "unknown") return !rawLocationId && !rawLocationName;
    const locationRecord = state.data.locations.find((locationItem) => recordId(locationItem) === sourceValue);
    if (!locationRecord) return false;
    const resolvedLocationId = mediaLocationId(item);
    return resolvedLocationId !== "unknown"
      ? resolvedLocationId === sourceValue
      : String(rawLocationName || "") === String(pick(locationRecord, "name") || "");
  }

  function movementMediaChoices(sourceValue) {
    const candidates = state.data.media.filter((item) => mediaMatchesMovementSource(item, sourceValue));
    if (!candidates.length) return '<div class="vault-section-empty compact">' + (sourceValue === "unknown" ? "No unlocated cartridge is available for onboarding." : "No cartridge is confirmed at this source location.") + "</div>";
    return candidates.map((item) => {
      const reserved = Boolean(pick(item, "reserved_by"));
      const blockers = reserved ? "Reserved by " + pick(item, "reserved_by") : count(pick(item, "active_holds")) ? pick(item, "active_holds") + " active hold(s); movement planning may be blocked" : "Available for exact-set planning";
      return '<label class="movement-media-choice' + (reserved ? " blocked" : "") + '"><input type="checkbox" name="cartridge_ids" value="' + esc(recordId(item)) + '"' + (reserved ? " disabled" : "") + '><span class="custody-tape-glyph" aria-hidden="true">▦</span><span><strong>' + esc(mediaName(item)) + '</strong><small>' + esc(pick(item, "barcode") ? "Barcode " + pick(item, "barcode") + " · " + blockers : blockers) + "</small></span></label>";
    }).join("");
  }

  function updateMovementMediaChoices(form) {
    const source = String(new FormData(form).get("source_location_id") || "unknown");
    const container = form.querySelector("#movement-media-choices");
    if (container) container.innerHTML = movementMediaChoices(source);
    const onboarding = form.querySelector("#unknown-onboarding-notice");
    if (onboarding) onboarding.hidden = source !== "unknown";
  }

  function openMovementForm() {
    if (!state.data.locations.length || !state.data.media.length) return;
    const endpointLocations = state.data.locations.filter((item) => ["onsite", "offsite", "device"].includes(locationKind(item)));
    if (!endpointLocations.length) return;
    const unlocated = state.data.media.some((item) => mediaMatchesMovementSource(item, "unknown") && !pick(item, "reserved_by"));
    const initialSource = unlocated ? "unknown" : recordId(endpointLocations[0]);
    const locationOptions = endpointLocations.map((item) => '<option value="' + esc(recordId(item)) + '"' + (recordId(item) === initialSource ? " selected" : "") + '>' + esc(label(pick(item, "name"))) + " · " + esc(human(locationKind(item))) + "</option>");
    const destinationOptions = endpointLocations.map((item) => '<option value="' + esc(recordId(item)) + '">' + esc(label(pick(item, "name"))) + " · " + esc(human(locationKind(item))) + "</option>");
    const transitOptions = ['<option value="">No declared transit location</option>'].concat(state.data.locations.filter((item) => locationKind(item) === "in_transit").map((item) => '<option value="' + esc(recordId(item)) + '">' + esc(label(pick(item, "name"))) + "</option>"));
    const due = localDateTimeValue(new Date(Date.now() + 24 * 60 * 60 * 1000));
    const body = '<form id="movement-form" data-idempotency-key="' + esc(operationKey("movement")) + '"><div class="custody-plan-banner"><strong>Plan intent—not custody proof</strong><span>The selected cartridges will be reserved. Their confirmed locations remain unchanged until a physical handoff is appended.</span></div><div class="form-grid"><div class="form-field"><label for="movement-kind">Movement type</label><select class="select" id="movement-kind" name="kind"><option value="take_offsite">Take off-site</option><option value="return">Return on-site</option><option value="swap">Swap media</option><option value="recall">Emergency recall</option></select></div><div class="form-field"><label for="movement-external-id">Operator reference</label><input class="input" id="movement-external-id" name="external_id" maxlength="128" placeholder="Optional · NOV-2026-VAULT"></div><div class="form-field"><label for="movement-source">Confirmed source</label><select class="select" id="movement-source" name="source_location_id"><option value="unknown"' + (initialSource === "unknown" ? " selected" : "") + '>Unknown / unlocated onboarding</option>' + locationOptions.join("") + '</select></div><div class="form-field"><label for="movement-destination">Planned destination</label><select class="select" id="movement-destination" name="destination_location_id" required><option value="">Select destination</option>' + destinationOptions.join("") + '</select></div><div class="form-field"><label for="movement-transit">Transit location</label><select class="select" id="movement-transit" name="transit_location_id">' + transitOptions.join("") + '</select></div><div class="form-field"><label for="movement-due">Due</label><input class="input" id="movement-due" name="due_at" type="datetime-local" value="' + esc(due) + '" required></div><div class="form-field full"><label for="movement-reason">Reason and handling objective</label><textarea class="textarea" id="movement-reason" name="reason" maxlength="2000" required placeholder="Deliver the newest complete verified recovery point to off-site custody."></textarea></div><div class="form-field full"><div class="notice warning" id="unknown-onboarding-notice"' + (initialSource === "unknown" ? "" : " hidden") + '><span class="notice-icon" aria-hidden="true">?</span><div class="notice-copy"><strong>Unknown-location onboarding</strong><span>Only direct physical confirmation can advance unlocated media. Departure cannot be claimed without a confirmed source.</span></div></div><fieldset class="movement-media-fieldset"><legend>Exact cartridge set</legend><div id="movement-media-choices">' + movementMediaChoices(initialSource) + '</div></fieldset></div><div class="form-field full"><label for="movement-actor">Operator label</label><input class="input" id="movement-actor" name="actor" maxlength="128" value="Web operator" required></div></div><p class="notice danger" id="movement-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Reserve tapes and create plan</button></div></form>';
    openModal("Plan custody movement", body);
    const modal = modalLayer.querySelector(".modal");
    if (modal) modal.classList.add("custody-modal");
    document.querySelector("#movement-kind").focus();
  }

  async function submitMovement(form) {
    const values = new FormData(form);
    const cartridgeIds = values.getAll("cartridge_ids").map(String);
    const errorBox = form.querySelector("#movement-error");
    const submit = form.querySelector('[type="submit"]');
    if (!cartridgeIds.length) {
      errorBox.textContent = "Select at least one exact cartridge.";
      errorBox.hidden = false;
      return;
    }
    const source = String(values.get("source_location_id") || "unknown");
    const due = new Date(String(values.get("due_at") || ""));
    if (Number.isNaN(due.getTime())) {
      errorBox.textContent = "Enter a valid due date and time.";
      errorBox.hidden = false;
      return;
    }
    const payload = {
      idempotency_key: form.dataset.idempotencyKey,
      kind: String(values.get("kind")),
      source_location_id: source === "unknown" ? null : source,
      destination_location_id: String(values.get("destination_location_id")),
      transit_location_id: String(values.get("transit_location_id") || "") || null,
      cartridge_ids: cartridgeIds,
      reason: String(values.get("reason") || "").trim(),
      due_at: due.toISOString(),
      actor: String(values.get("actor") || "").trim() || null
    };
    const externalId = String(values.get("external_id") || "").trim();
    if (externalId) payload.external_id = externalId;
    submit.disabled = true;
    submit.textContent = "Reserving exact tapes…";
    errorBox.hidden = true;
    try {
      const response = await request(endpoints.movements, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify(payload)
      });
      closeModal();
      await Promise.all([loadOne("movements"), loadOne("media")]);
      const createdId = String(
        pick(response && response.receipt, "movement_task_id", "id")
        || pick(response && response.movement && response.movement.summary, "id", "movement_task_id")
        || pick(response, "movement_task_id", "id")
        || ""
      );
      const created = state.data.movements.find((item) => recordId(item) === createdId || (externalId && String(pick(item, "external_id")) === externalId)) || state.data.movements[0];
      if (created) await selectMovement(recordId(created));
      else render();
      toast("Movement planned", "The exact cartridges are reserved. Confirmed physical locations have not changed.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Reserve tapes and create plan";
    }
  }

  function selectedCustodyCartridges(action) {
    const allowed = action === "arrival" ? "departed" : "planned";
    return Array.from(document.querySelectorAll('[name="movement_cartridge"]:checked')).filter((input) => input.dataset.stage === allowed);
  }

  function updateCustodyActionButtons() {
    const checkedInputs = Array.from(document.querySelectorAll('[name="movement_cartridge"]:checked'));
    document.querySelectorAll('[data-action="confirm-custody"]').forEach((button) => {
      const allowed = button.dataset.confirmAction === "arrival" ? "departed" : "planned";
      button.disabled = !checkedInputs.length || !checkedInputs.every((input) => input.dataset.stage === allowed);
    });
    const selectAll = document.querySelector("#custody-select-all");
    const actionable = Array.from(document.querySelectorAll('[name="movement_cartridge"]:not(:disabled)'));
    if (selectAll) {
      selectAll.checked = actionable.length > 0 && actionable.every((input) => input.checked);
      selectAll.indeterminate = actionable.some((input) => input.checked) && !selectAll.checked;
    }
  }

  function openCustodyConfirmation(button) {
    const action = button.dataset.confirmAction;
    const movementId = button.dataset.id;
    const selected = selectedCustodyCartridges(action);
    const detail = movementDetail(movementId);
    const summary = detail && detail.summary ? detail.summary : state.data.movements.find((item) => recordId(item) === movementId);
    if (!selected.length || !summary) return;
    const hiddenInputs = selected.map((input) => '<input type="hidden" name="cartridge_ids" value="' + esc(input.value) + '">').join("");
    const labels = selected.map((input) => input.dataset.label);
    const actionText = action === "departure" ? "departure into transit" : action === "arrival" ? "arrival at the destination" : "direct handoff to the destination";
    const itemCountText = "This append-only event applies to " + selected.length + " exact cartridge" + (selected.length === 1 ? "" : "s") + ". Unselected cartridges remain unchanged.";
    const body = '<form id="custody-confirm-form" data-movement-id="' + esc(movementId) + '" data-action-name="' + esc(action) + '" data-idempotency-key="' + esc(operationKey("custody-" + action)) + '">' + hiddenInputs + '<input type="hidden" name="expected_confirmation_version" value="' + esc(pick(summary, "confirmation_version")) + '"><div class="custody-confirm-hero"><span aria-hidden="true">✓</span><div><strong>Physically confirm ' + esc(actionText) + '</strong><p>' + esc(itemCountText) + '</p></div></div><div class="custody-confirm-tapes">' + labels.map((item) => "<code>" + esc(item) + "</code>").join("") + '</div><div class="form-grid"><div class="form-field full"><label class="check-option custody-physical-check"><input type="checkbox" name="physically_confirmed" required> I am physically handling or directly witnessing these exact labeled cartridges at this custody boundary.</label></div><div class="form-field full"><label for="custody-confirm-note">Handoff note</label><textarea class="textarea" id="custody-confirm-note" name="note" maxlength="2000" placeholder="Courier reference, receiving person, seal condition, or exception detail"></textarea></div><div class="form-field full"><label for="custody-confirm-actor">Operator label</label><input class="input" id="custody-confirm-actor" name="actor" maxlength="128" value="Web operator" required></div></div><p class="notice danger" id="custody-confirm-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Append physical confirmation</button></div></form>';
    openModal("Confirm physical custody", body);
    const modal = modalLayer.querySelector(".modal");
    if (modal) modal.classList.add("custody-modal");
  }

  async function submitCustodyConfirmation(form) {
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#custody-confirm-error");
    submit.disabled = true;
    submit.textContent = "Appending custody evidence…";
    errorBox.hidden = true;
    try {
      await request(endpoints.movements + "/" + encodeURIComponent(form.dataset.movementId) + "/confirm", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          action: form.dataset.actionName,
          cartridge_ids: values.getAll("cartridge_ids").map(String),
          expected_confirmation_version: Number(values.get("expected_confirmation_version")),
          idempotency_key: form.dataset.idempotencyKey,
          physically_confirmed: values.get("physically_confirmed") === "on",
          note: String(values.get("note") || "").trim(),
          actor: String(values.get("actor") || "").trim() || null
        })
      });
      closeModal();
      await Promise.all([loadOne("movements"), loadOne("media"), loadMovementDetail(form.dataset.movementId, true)]);
      render();
      toast("Physical custody appended", "Confirmed locations advanced only for the selected cartridges.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Append physical confirmation";
    }
  }

  function openCancelMovement(id) {
    const detail = movementDetail(id);
    const summary = detail && detail.summary ? detail.summary : state.data.movements.find((item) => recordId(item) === id);
    if (!summary) return;
    const body = '<form id="cancel-movement-form" data-movement-id="' + esc(id) + '" data-idempotency-key="' + esc(operationKey("cancel-movement")) + '"><input type="hidden" name="expected_confirmation_version" value="' + esc(pick(summary, "confirmation_version")) + '"><div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Cancel the remaining movement intent</strong><span>Previously confirmed custody events remain durable. Cancellation releases only tapes that have not completed this task.</span></div></div><div class="form-field"><label for="cancel-movement-reason">Cancellation reason</label><textarea class="textarea" id="cancel-movement-reason" name="reason" maxlength="2000" required></textarea></div><div class="form-field section"><label for="cancel-movement-actor">Operator label</label><input class="input" id="cancel-movement-actor" name="actor" value="Web operator" maxlength="128" required></div><p class="notice danger" id="cancel-movement-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Keep task</button><button class="button button-danger" type="submit">Cancel remaining task</button></div></form>';
    openModal("Cancel movement", body);
  }

  async function submitCancelMovement(form) {
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#cancel-movement-error");
    submit.disabled = true;
    submit.textContent = "Cancelling…";
    errorBox.hidden = true;
    try {
      await request(endpoints.movements + "/" + encodeURIComponent(form.dataset.movementId) + "/cancel", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          expected_confirmation_version: Number(values.get("expected_confirmation_version")),
          idempotency_key: form.dataset.idempotencyKey,
          reason: String(values.get("reason") || "").trim(),
          actor: String(values.get("actor") || "").trim() || null
        })
      });
      closeModal();
      await Promise.all([loadOne("movements"), loadOne("media"), loadMovementDetail(form.dataset.movementId, true)]);
      render();
      toast("Movement cancelled", "Unfinished reservations were released; prior custody evidence was preserved.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Cancel remaining task";
    }
  }

  function openCustodyCorrection(button) {
    const movement = selectedMovement();
    const id = recordId(movement);
    const detail = movementDetail(id);
    if (!detail) return;
    const media = list(detail, "media").find((item) => String(pick(item, "cartridge_id")) === button.dataset.cartridgeId);
    if (!media) return;
    const options = state.data.locations.filter((item) => locationKind(item) !== "unknown").map((item) => '<option value="' + esc(recordId(item)) + '">' + esc(label(pick(item, "name"))) + " · " + esc(human(locationKind(item))) + "</option>");
    const body = '<form id="custody-correction-form" data-movement-id="' + esc(id) + '" data-idempotency-key="' + esc(operationKey("custody-correction")) + '"><input type="hidden" name="corrects_event_id" value="' + esc(button.dataset.eventId) + '"><input type="hidden" name="cartridge_id" value="' + esc(button.dataset.cartridgeId) + '"><input type="hidden" name="expected_current_location_id" value="' + esc(pick(media, "confirmed_location_id") || "") + '"><input type="hidden" name="expected_confirmation_version" value="' + esc(pick(detail.summary, "confirmation_version")) + '"><div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Append a correction; never rewrite history</strong><span>The original custody event remains visible. Use this only after physically verifying the current cartridge location.</span></div></div><div class="form-grid"><div class="form-field full"><label>Cartridge</label><div class="static-field"><strong>' + esc(label(pick(media, "internal_label"))) + '</strong><small>Currently confirmed: ' + esc(pick(media, "confirmed_location_name") || "Unknown") + '</small></div></div><div class="form-field full"><label for="correction-location">Correct physical location</label><select class="select" id="correction-location" name="confirmed_location_id" required><option value="">Select verified location</option>' + options.join("") + '</select></div><div class="form-field full"><label class="check-option custody-physical-check"><input type="checkbox" name="physically_confirmed" required> I physically verified this exact cartridge at the corrected location.</label></div><div class="form-field full"><label for="correction-note">Correction reason and evidence</label><textarea class="textarea" id="correction-note" name="note" maxlength="2000" required></textarea></div><div class="form-field full"><label for="correction-actor">Operator label</label><input class="input" id="correction-actor" name="actor" maxlength="128" value="Web operator" required></div></div><p class="notice danger" id="custody-correction-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-danger" type="submit">Append correction</button></div></form>';
    openModal("Correct custody evidence", body);
    const modal = modalLayer.querySelector(".modal");
    if (modal) modal.classList.add("custody-modal");
  }

  async function submitCustodyCorrection(form) {
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#custody-correction-error");
    submit.disabled = true;
    submit.textContent = "Appending correction…";
    errorBox.hidden = true;
    try {
      await request(endpoints.movements + "/" + encodeURIComponent(form.dataset.movementId) + "/corrections", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          corrects_event_id: String(values.get("corrects_event_id")),
          cartridge_id: String(values.get("cartridge_id")),
          expected_current_location_id: String(values.get("expected_current_location_id") || "") || null,
          confirmed_location_id: String(values.get("confirmed_location_id")),
          expected_confirmation_version: Number(values.get("expected_confirmation_version")),
          idempotency_key: form.dataset.idempotencyKey,
          physically_confirmed: values.get("physically_confirmed") === "on",
          note: String(values.get("note") || "").trim(),
          actor: String(values.get("actor") || "").trim() || null
        })
      });
      closeModal();
      await Promise.all([loadOne("movements"), loadOne("media"), loadMovementDetail(form.dataset.movementId, true)]);
      render();
      toast("Custody correction appended", "The original event remains visible beside the corrective evidence.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Append correction";
    }
  }

  function printMovementManifest() {
    document.body.classList.add("printing-custody-manifest");
    window.print();
    window.setTimeout(() => document.body.classList.remove("printing-custody-manifest"), 500);
  }

  function rememberRecoveryProfile(profile) {
    if (!profile || typeof profile !== "object" || !recordId(profile)) return;
    state.data.recoveryProfiles = [profile].concat(state.data.recoveryProfiles.filter((item) => recordId(item) !== recordId(profile)));
    state.received.add("recoveryProfiles");
    delete state.errors.recoveryProfiles;
  }

  function rememberRecoveryExercise(exercise) {
    if (!exercise || typeof exercise !== "object" || !recordId(exercise)) return;
    state.data.recoveryExercises = [exercise].concat(state.data.recoveryExercises.filter((item) => recordId(item) !== recordId(exercise)));
    state.received.add("recoveryExercises");
    delete state.errors.recoveryExercises;
  }

  function openRecoveryProfile() {
    if (!recoveryLabEnabled()) {
      toast("Recovery exercise is disabled", "Enable the file roundtrip on both the API and worker first.");
      return;
    }
    const body = [
      '<form id="recovery-profile-form">',
      '<div class="notice info"><span class="notice-icon" aria-hidden="true">1</span><div class="notice-copy"><strong>Declare the prepared recovery fixture</strong><span>Both paths are contained subpaths of the source and restore roots configured by the TrueNAS Compose file. TapeScale will not accept host-absolute paths here.</span></div></div>',
      '<div class="form-grid">',
      '<div class="form-field full"><label for="recovery-profile-name">Profile name</label><input class="input" id="recovery-profile-name" name="name" maxlength="128" placeholder="Quarterly recovery fixture" required><small>This name appears in the destructive confirmation and retained evidence.</small></div>',
      '<div class="form-field full"><label for="recovery-source-subpath">Immutable source directory</label><div class="source-browser-input"><input class="input input-mono" id="recovery-source-subpath" name="source_subpath" maxlength="512" placeholder="tank/test/.zfs/snapshot/fixture-2026-11" required autocomplete="off"><button class="button button-quiet" type="button" data-action="open-source-browser" data-browser-target="recovery" aria-controls="recovery-source-browser" aria-expanded="false"><span aria-hidden="true">▰</span> Browse</button></div><small>Browse the same contained read-only source mount used by Backup Center, or enter a relative directory manually.</small><div class="source-browser-selected" id="recovery-source-selection" hidden></div></div>',
      '<section class="backup-source-browser" id="recovery-source-browser" aria-label="Recovery fixture source browser" hidden></section>',
      '<div class="form-field full"><label for="recovery-restore-subpath">Isolated restore subpath</label><input class="input input-mono" id="recovery-restore-subpath" name="restore_subpath" maxlength="512" placeholder="exercises" required><small>Relative to the configured restore root; each run receives its own directory.</small></div>',
      '<div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="immutable_snapshot_confirmed" required> I confirm the source directory is a prepared immutable snapshot or expendable fixture and will not change during this exercise.</label></div>',
      '</div><p class="notice danger" id="recovery-profile-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Save and choose hardware</button></div></form>'
    ].join("");
    sourceBrowserRequest += 1;
    sourceBrowserPath = "";
    sourceBrowserReadOnly = null;
    sourceBrowserSelectable = false;
    sourceBrowserTarget = { inputId: "recovery-source-subpath", panelId: "recovery-source-browser", summaryId: "recovery-source-selection", nameId: "recovery-profile-name" };
    openModal("Set up a recovery exercise", body);
    document.querySelector("#recovery-profile-name").focus();
  }

  async function submitRecoveryProfile(form) {
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#recovery-profile-error");
    const values = new FormData(form);
    submit.disabled = true;
    submit.textContent = "Validating paths…";
    errorBox.hidden = true;
    try {
      const response = await request(endpoints.recoveryProfiles, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          name: String(values.get("name") || "").trim(),
          source_subpath: String(values.get("source_subpath") || "").trim(),
          restore_subpath: String(values.get("restore_subpath") || "").trim(),
          immutable_snapshot_confirmed: values.get("immutable_snapshot_confirmed") === "on"
        })
      });
      const profile = response && (response.profile || response);
      rememberRecoveryProfile(profile);
      closeModal();
      if (location.hash !== "#recovery-lab") location.hash = "#recovery-lab";
      await loadOne("recoveryProfiles");
      updateChrome();
      render();
      toast("Recovery profile ready", "Choose the exact loaded scratch tape to begin the destructive exercise.");
      const readyDrive = eligibleRecoveryDrives()[0];
      if (profile && readyDrive && eligibleRecoveryMedia(readyDrive).length) window.setTimeout(() => openRecoveryRun(recordId(profile)), 0);
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Save and choose hardware";
    }
  }

  function openRecoveryRun(profileId) {
    if (!recoveryLabEnabled()) {
      toast("Recovery exercise is disabled", "Enable the file roundtrip on both the API and worker first.");
      return;
    }
    const profiles = state.data.recoveryProfiles;
    const drives = eligibleRecoveryDrives();
    const media = eligibleRecoveryMedia(drives[0]);
    if (!profiles.length || !drives.length || !media.length || state.data.recoveryExercises.some(recoveryExerciseActive)) {
      toast("Recovery exercise is not ready", "Review the source, drive, and cartridge readiness cards for the exact blocker.");
      return;
    }
    const profileOptions = profiles.map((profile) => '<option value="' + esc(recordId(profile)) + '" data-name="' + esc(recoveryProfileName(profile)) + '"' + (profileId === recordId(profile) ? " selected" : "") + '>' + esc(recoveryProfileName(profile)) + " · " + esc(label(pick(profile, "source_subpath"))) + "</option>");
    const driveOptions = drives.map((drive) => '<option value="' + esc(recordId(drive)) + '" data-stable-id="' + esc(pick(drive, "stable_id")) + '">' + esc(label(pick(drive, "vendor"))) + " " + esc(label(pick(drive, "model"))) + " · " + esc(label(pick(drive, "stable_id"))) + "</option>");
    const mediaOptions = media.map((item) => '<option value="' + esc(recordId(item)) + '" data-label="' + esc(mediaName(item)) + '">' + esc(mediaName(item)) + (pick(item, "barcode") ? " · barcode " + esc(pick(item, "barcode")) : "") + " · " + esc(label(pick(item, "media_format", "media_generation"))) + "</option>");
    const body = '<form id="recovery-exercise-form"><div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>This erases the selected cartridge</strong><span>The lab path performs one full unencrypted write, reads it back from physical tape, restores all captured content into isolation, and compares manifests. It is not a production backup policy.</span></div></div><div class="form-grid"><div class="form-field full"><label for="recovery-run-profile">Prepared source profile</label><select class="select" id="recovery-run-profile" name="profile_id" required>' + profileOptions.join("") + '</select></div><div class="form-field full"><label for="recovery-run-drive">Exact loaded drive</label><select class="select" id="recovery-run-drive" name="drive_id" required>' + driveOptions.join("") + '</select><small>The worker will re-probe this stable identity immediately before writing.</small></div><div class="form-field full"><label for="recovery-run-cartridge">Loaded scratch cartridge</label><select class="select" id="recovery-run-cartridge" name="cartridge_id" required>' + mediaOptions.join("") + '</select><small>You are asserting that this registered cartridge is physically loaded in the selected standalone drive.</small></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="scratch_media_confirmed" required> I physically verified that the loaded cartridge is designated scratch media.</label></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="destructive_overwrite_confirmed" required> I authorize complete destruction of every existing byte on this cartridge.</label></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="encryption_deviation_confirmed" required> I understand this G0.2 exercise is intentionally unencrypted and must contain only expendable test data.</label></div><div class="form-field full"><label for="recovery-run-confirmation">Type the exact destructive confirmation</label><input class="input input-mono" id="recovery-run-confirmation" name="confirmation" autocomplete="off" spellcheck="false" required><small id="recovery-run-confirmation-help"></small></div></div><p class="notice danger" id="recovery-run-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-danger" type="submit">Erase tape and prove recovery</button></div></form>';
    openModal("Create a recoverable tape", body);
    updateRecoveryRunConfirmation(document.querySelector("#recovery-exercise-form"));
    document.querySelector("#recovery-run-confirmation").focus();
  }

  function updateRecoveryRunMediaOptions(form) {
    if (!form) return;
    const driveSelect = form.querySelector('[name="drive_id"]');
    const cartridgeSelect = form.querySelector('[name="cartridge_id"]');
    const submit = form.querySelector('[type="submit"]');
    if (!driveSelect || !cartridgeSelect) return;
    const drive = state.data.drives.find((item) => recordId(item) === driveSelect.value);
    const priorCartridgeId = cartridgeSelect.value;
    const media = eligibleRecoveryMedia(drive);
    cartridgeSelect.innerHTML = media.map((item) => '<option value="' + esc(recordId(item)) + '" data-label="' + esc(mediaName(item)) + '"' + (recordId(item) === priorCartridgeId ? " selected" : "") + '>' + esc(mediaName(item)) + (pick(item, "barcode") ? " · barcode " + esc(pick(item, "barcode")) : "") + " · " + esc(label(pick(item, "media_format", "media_generation"))) + "</option>").join("");
    cartridgeSelect.disabled = media.length === 0;
    if (submit) submit.disabled = media.length === 0;
    updateRecoveryRunConfirmation(form);
  }

  function updateRecoveryRunConfirmation(form) {
    if (!form) return;
    const profileSelect = form.querySelector('[name="profile_id"]');
    const cartridgeSelect = form.querySelector('[name="cartridge_id"]');
    const profileOption = profileSelect && profileSelect.options[profileSelect.selectedIndex];
    const cartridgeOption = cartridgeSelect && cartridgeSelect.options[cartridgeSelect.selectedIndex];
    const profileName = profileOption && profileOption.dataset.name;
    const cartridgeLabel = cartridgeOption && cartridgeOption.dataset.label;
    const phrase = profileName && cartridgeLabel ? "WRITE " + profileName + " TO " + cartridgeLabel + " AND RESTORE VERIFY" : "Select a profile and cartridge.";
    const help = form.querySelector("#recovery-run-confirmation-help");
    if (help) help.textContent = "Required: " + phrase;
  }

  async function submitRecoveryExercise(form) {
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#recovery-run-error");
    const values = new FormData(form);
    const profileSelect = form.querySelector('[name="profile_id"]');
    const driveSelect = form.querySelector('[name="drive_id"]');
    const cartridgeSelect = form.querySelector('[name="cartridge_id"]');
    const profileOption = profileSelect.options[profileSelect.selectedIndex];
    const driveOption = driveSelect.options[driveSelect.selectedIndex];
    const cartridgeOption = cartridgeSelect.options[cartridgeSelect.selectedIndex];
    const profileName = profileOption && profileOption.dataset.name;
    const stableId = driveOption && driveOption.dataset.stableId;
    const cartridgeLabel = cartridgeOption && cartridgeOption.dataset.label;
    if (!profileName || !stableId || !cartridgeLabel) return;
    submit.disabled = true;
    submit.textContent = "Queueing recovery proof…";
    errorBox.hidden = true;
    try {
      const response = await request(endpoints.recoveryExercises, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          profile_id: String(values.get("profile_id")),
          drive_id: String(values.get("drive_id")),
          stable_id: stableId,
          cartridge_id: String(values.get("cartridge_id")),
          cartridge_label: cartridgeLabel,
          scratch_media_confirmed: values.get("scratch_media_confirmed") === "on",
          destructive_overwrite_confirmed: values.get("destructive_overwrite_confirmed") === "on",
          encryption_deviation_confirmed: values.get("encryption_deviation_confirmed") === "on",
          confirmation: String(values.get("confirmation") || "")
        })
      });
      const exercise = response && (response.exercise || response.run || response);
      rememberRecoveryExercise(exercise);
      state.selectedRecoveryExerciseId = recordId(exercise);
      closeModal();
      if (location.hash !== "#recovery-lab") location.hash = "#recovery-lab";
      updateChrome();
      render();
      scheduleRecoveryExercisePoll();
      live.textContent = "Physical recovery exercise queued for " + profileName + " on cartridge " + cartridgeLabel + ".";
      toast("Recovery exercise queued", "Opening the durable phase timeline. Keep the source, drive, and restore destination online.");
      await Promise.all([loadOne("recoveryExercises"), loadOne("drives"), loadOne("media")]);
      await loadRecoveryExerciseDetail(state.selectedRecoveryExerciseId);
      updateChrome();
      render();
      scheduleRecoveryExercisePoll();
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Erase tape and prove recovery";
    }
  }

  async function loadRecoveryExerciseDetail(id) {
    if (!id) return;
    const errorKey = "recoveryExercise:" + id;
    try {
      const detail = await request(endpoints.recoveryExercises + "/" + encodeURIComponent(id));
      state.data.recoveryExerciseDetails[id] = detail && typeof detail === "object" ? detail : {};
      const detailedExercise = detail && (detail.exercise || detail.run || (recordId(detail) === id ? detail : null));
      if (detailedExercise) rememberRecoveryExercise(detailedExercise);
      delete state.errors[errorKey];
    } catch (error) {
      state.errors[errorKey] = error;
    }
  }

  async function selectRecoveryExercise(id) {
    if (!id) return;
    state.selectedRecoveryExerciseId = id;
    render();
    await loadRecoveryExerciseDetail(id);
    updateChrome();
    render();
    main.focus();
  }

  function scheduleRecoveryExercisePoll() {
    if (state.recoveryExercisePollTimer) window.clearTimeout(state.recoveryExercisePollTimer);
    const active = state.data.recoveryExercises.filter(recoveryExerciseActive);
    if (!active.length) {
      state.recoveryExercisePollTimer = null;
      return;
    }
    state.recoveryExercisePollTimer = window.setTimeout(async () => {
      const previouslyActive = new Set(active.map(recordId));
      await Promise.all([loadOne("recoveryExercises"), loadOne("drives"), loadOne("media"), loadOne("recoveryPoints"), loadOne("verification")]);
      const selected = state.selectedRecoveryExerciseId || (state.data.recoveryExercises.find(recoveryExerciseActive) && recordId(state.data.recoveryExercises.find(recoveryExerciseActive)));
      if (selected) await loadRecoveryExerciseDetail(selected);
      const completed = state.data.recoveryExercises.find((exercise) => previouslyActive.has(recordId(exercise)) && !recoveryExerciseActive(exercise));
      if (completed) toast("Recovery exercise " + recoveryExerciseState(completed), recoveryExerciseDetail(completed) || "Durable result and evidence are ready.");
      updateChrome();
      render();
      scheduleRecoveryExercisePoll();
    }, 4000);
  }

  function scheduleRestoreRehearsalPoll() {
    if (state.restoreRehearsalPollTimer) window.clearTimeout(state.restoreRehearsalPollTimer);
    const active = state.data.restoreRehearsals.filter(restoreRehearsalActive);
    if (!active.length) {
      state.restoreRehearsalPollTimer = null;
      return;
    }
    state.restoreRehearsalPollTimer = window.setTimeout(async () => {
      const previouslyActive = new Set(active.map(recordId));
      await Promise.all([loadOne("restoreRehearsals"), loadOne("recoveryPoints"), loadOne("verification"), loadOne("drives"), loadOne("media")]);
      const selected = selectedRestoreRehearsal();
      if (selected) await loadRestoreRehearsalDetail(recordId(selected), true);
      const completed = state.data.restoreRehearsals.find((run) => previouslyActive.has(recordId(run)) && !restoreRehearsalActive(run));
      if (completed) toast("Restore run " + restoreRehearsalState(completed), "The durable restore result and evidence are ready in the Recovery Vault.");
      updateChrome();
      render();
      scheduleRestoreRehearsalPoll();
    }, 3500);
  }

  function backupReadinessKey(scope, id) {
    return scope + ":" + id;
  }

  async function loadBackupMediaReadiness(scope, id) {
    const endpoint = scope === "run" ? endpoints.backupRuns : endpoints.backupProfiles;
    const payload = await request(endpoint + "/" + encodeURIComponent(id) + "/media-readiness");
    const readiness = payload && payload.readiness && typeof payload.readiness === "object" ? payload.readiness : null;
    if (!readiness || !Array.isArray(readiness.candidates) || !Array.isArray(readiness.blockers)) throw new Error("TapeScale returned an invalid tape-readiness response.");
    state.data.backupMediaReadiness[backupReadinessKey(scope, id)] = readiness;
    return readiness;
  }

  function backupReadinessCandidates(key) {
    const readiness = state.data.backupMediaReadiness[key];
    return readiness && Array.isArray(readiness.candidates) ? readiness.candidates : [];
  }

  function backupCandidateDrive(candidate) {
    return state.data.drives.find((item) => recordId(item) === String(pick(candidate, "drive_id"))) || {
      id: pick(candidate, "drive_id"),
      stable_id: pick(candidate, "stable_id"),
      observed_path: pick(candidate, "device_path")
    };
  }

  function backupCandidateMedia(candidate) {
    return state.data.media.find((item) => recordId(item) === String(pick(candidate, "cartridge_id"))) || {
      id: pick(candidate, "cartridge_id"),
      internal_label: pick(candidate, "cartridge_label"),
      media_generation: pick(candidate, "media_generation"),
      media_format: pick(candidate, "media_format")
    };
  }

  function backupPrimaryReadinessBlocker(readiness) {
    const blockers = readiness && Array.isArray(readiness.blockers) ? readiness.blockers : [];
    const priority = [
      "request_not_waiting_for_media", "profile_wrong_workflow", "profile_disabled",
      "profile_safety_contract_invalid", "snapshot_unavailable", "setup_test_required",
      "resource_busy", "no_tape_drive",
      "drive_not_present", "drive_inaccessible", "drive_identity_unconfirmed",
      "drive_identity_ambiguous", "drive_path_changed", "drive_observation_stale",
      "drive_position_uncertain", "tape_not_loaded", "no_media_in_pool", "no_registered_media",
      "media_write_unverified", "media_wrong_pool", "media_protected", "media_worm",
      "media_identity_conflict", "media_reserved", "media_ineligible", "drive_io_busy"
    ];
    return priority.map((code) => blockers.find((item) => pick(item, "code") === code)).find(Boolean) || blockers[0] || null;
  }

  function backupServerReadinessMessage(readiness) {
    const blocker = backupPrimaryReadinessBlocker(readiness);
    return blocker ? String(pick(blocker, "message") || "No safe drive-and-tape pair is ready.") : "No safe drive-and-tape pair is ready.";
  }

  function backupReadinessRemediation(readiness) {
    const blocker = backupPrimaryReadinessBlocker(readiness);
    const code = String(pick(blocker, "code") || "");
    const action = String(pick(blocker, "action") || "");
    const resourceId = String(pick(blocker, "resource_id") || "");
    const relatedResourceId = String(pick(blocker, "related_resource_id") || "");
    if (action === "choose_snapshot" || ["snapshot_unavailable", "profile_safety_contract_invalid"].includes(code)) {
      return { kind: "choose_snapshot", heading: "Choose an available snapshot", label: "Choose another snapshot" };
    }
    if (action === "open_backup" || ["request_not_waiting_for_media", "profile_wrong_workflow", "profile_disabled"].includes(code)) {
      return { kind: "close", heading: "This backup cannot accept a tape", label: "Return to backup" };
    }
    if (action === "prepare_tape" || ["no_registered_media", "setup_test_required", "media_write_unverified"].includes(code)) {
      return {
        kind: "prepare",
        heading: "Prepare a scratch tape",
        label: "Prepare loaded tape",
        driveId: code === "setup_test_required" ? resourceId : "",
        mediaId: code === "setup_test_required" ? relatedResourceId : code === "media_write_unverified" ? resourceId : ""
      };
    }
    if (["open_media", "choose_other_tape", "reconcile_media"].includes(action) || ["no_media_in_pool", "media_wrong_pool", "media_protected", "media_worm", "media_identity_conflict", "media_ineligible"].includes(code)) {
      return { kind: "media", heading: "Choose an eligible scratch tape", label: "Open tape inventory", href: "#tapes" };
    }
    if (["reconcile_drive", "connect_drive", "check_device_access", "load_tape"].includes(action) || code.startsWith("drive_") || code === "no_tape_drive" || code === "tape_not_loaded") {
      return { kind: "drive", heading: "The tape drive needs attention", label: "Open drive status", href: "#drives" };
    }
    if (action === "wait" || action === "refresh" || ["resource_busy", "media_reserved"].includes(code)) {
      return { kind: "refresh", heading: "Tape resources are busy", label: "Check again" };
    }
    return { kind: "refresh", heading: "No safe drive and tape pair is ready", label: "Check again" };
  }

  function backupReadinessAction(remediation, scope, id) {
    if (remediation.kind === "prepare") return '<button class="button button-primary button-small" type="button" data-action="prepare-backup-tape" data-' + scope + '-id="' + esc(id) + '"' + (remediation.driveId ? ' data-drive-id="' + esc(remediation.driveId) + '"' : "") + (remediation.mediaId ? ' data-media-id="' + esc(remediation.mediaId) + '"' : "") + '>' + esc(remediation.label) + "</button>";
    if (remediation.kind === "choose_snapshot") return '<button class="button button-primary button-small" type="button" data-action="choose-backup-snapshot">' + esc(remediation.label) + "</button>";
    if (remediation.kind === "media" || remediation.kind === "drive") return '<a class="button button-primary button-small" href="' + esc(remediation.href) + '">' + esc(remediation.label) + "</a>";
    if (remediation.kind === "close") return '<button class="button button-primary button-small" type="button" data-action="close-modal">' + esc(remediation.label) + "</button>";
    return "";
  }

  function sourceBrowserEntryKind(entry) {
    if (entry && (entry.symlink === true || entry.is_symlink === true)) return "symlink";
    const kind = String(pick(entry, "kind", "entry_type", "type") || "").toLowerCase();
    if (entry && entry.is_directory === true || ["directory", "dir", "folder"].includes(kind)) return "directory";
    if (["symlink", "link", "symbolic_link"].includes(kind)) return "symlink";
    if (["file", "regular", "regular_file"].includes(kind)) return "file";
    return "other";
  }

  function sourceBrowserJoin(parent, name) {
    return [String(parent || "").replace(/\/+$/, ""), String(name || "").replace(/^\/+/, "")]
      .filter(Boolean)
      .join("/");
  }

  function sourceBrowserParent(path) {
    const parts = String(path || "").split("/").filter(Boolean);
    parts.pop();
    return parts.join("/");
  }

  function sourceBrowserSnapshotPath(path) {
    const segments = String(path || "").split("/").filter(Boolean);
    return segments.some((segment, index) => segment === ".zfs" && segments[index + 1] === "snapshot" && Boolean(segments[index + 2]));
  }

  function sourceBrowserBreadcrumbs(path) {
    const segments = String(path || "").split("/").filter(Boolean);
    const crumbs = ['<button class="source-browser-crumb" type="button" data-action="source-browser-navigate" data-path="" aria-label="Open source root"><span aria-hidden="true">⌂</span><span>source</span></button>'];
    let accumulated = "";
    segments.forEach((segment) => {
      accumulated = sourceBrowserJoin(accumulated, segment);
      crumbs.push('<span class="source-browser-separator" aria-hidden="true">/</span><button class="source-browser-crumb" type="button" data-action="source-browser-navigate" data-path="' + esc(accumulated) + '">' + esc(segment) + "</button>");
    });
    return crumbs.join("");
  }

  function sourceBrowserEntry(entry, currentPath) {
    const kind = sourceBrowserEntryKind(entry);
    const name = String(pick(entry, "name", "display_name") || "Unnamed entry");
    const path = String(pick(entry, "path", "relative_path") || sourceBrowserJoin(currentPath, name));
    const modified = pick(entry, "modified_at", "mtime", "last_modified");
    const size = pick(entry, "size_bytes", "size");
    const readOnly = entry && (entry.read_only === true || entry.readonly === true);
    const icon = kind === "directory" ? "▰" : kind === "symlink" ? "↗" : kind === "file" ? "▤" : "?";
    const typeLabel = kind === "directory" ? "Folder" : kind === "symlink" ? "Symbolic link" : kind === "file" ? "File" : "Unsupported object";
    const facts = [typeLabel];
    if (kind === "file" && size !== null && size !== undefined) facts.push(bytes(size));
    if (modified) facts.push(label(modified));
    if (readOnly) facts.push("Read only");
    const nameControl = kind === "directory"
      ? '<button class="source-browser-name" type="button" data-action="source-browser-navigate" data-path="' + esc(path) + '" aria-label="Open folder ' + esc(name) + '"><strong>' + esc(name) + "</strong><small>" + esc(facts.join(" · ")) + "</small></button>"
      : '<div class="source-browser-name"><strong>' + esc(name) + "</strong><small>" + esc(facts.join(" · ")) + "</small></div>";
    const action = kind === "directory"
      ? '<span class="source-browser-open" aria-hidden="true">›</span>'
      : kind === "symlink" ? '<span class="source-browser-entry-note">Not followed</span>' : kind === "file" ? '<span class="source-browser-entry-note">Included with folder</span>' : '<span class="source-browser-entry-note">Backup will reject</span>';
    return '<div class="source-browser-entry ' + esc(kind) + '" role="listitem"><span class="source-browser-icon" aria-hidden="true">' + icon + '</span>' + nameControl + '<div class="source-browser-entry-action">' + action + "</div></div>";
  }

  function renderSourceBrowser(payload, requestedPath) {
    const panel = document.getElementById(sourceBrowserTarget.panelId);
    if (!panel) return;
    const hasCurrent = payload && Object.prototype.hasOwnProperty.call(payload, "current_path");
    const currentPath = String(hasCurrent ? payload.current_path : pick(payload, "path", "relative_path") || requestedPath || "");
    const rawEntries = payload && Array.isArray(payload.entries) ? payload.entries : payload && Array.isArray(payload.items) ? payload.items : [];
    const entries = rawEntries.slice().sort((left, right) => {
      const order = { directory: 0, file: 1, symlink: 2, other: 3 };
      const kindDifference = order[sourceBrowserEntryKind(left)] - order[sourceBrowserEntryKind(right)];
      if (kindDifference) return kindDifference;
      return String(pick(left, "name", "display_name") || "").localeCompare(String(pick(right, "name", "display_name") || ""), undefined, { numeric: true, sensitivity: "base" });
    });
    const payloadReadOnly = payload && (payload.read_only ?? payload.current_read_only ?? payload.readonly);
    const truncated = payload && payload.truncated === true;
    const entryLimit = Number(pick(payload, "entry_limit")) || entries.length;
    sourceBrowserPath = currentPath;
    sourceBrowserReadOnly = typeof payloadReadOnly === "boolean" ? payloadReadOnly : null;
    const snapshot = payload && (payload.is_snapshot === true || payload.snapshot === true) || sourceBrowserSnapshotPath(currentPath);
    const backupPlanSelection = sourceBrowserTarget.inputId === "backup-source-subpath";
    // Browsing is selection assistance, not the authority for plan creation.
    // Operational plans require a named ZFS snapshot, while the create-plan
    // endpoint remains authoritative for containment and read-only mount state.
    sourceBrowserSelectable = Boolean(currentPath) && (!backupPlanSelection || snapshot);
    const parentPath = payload && Object.prototype.hasOwnProperty.call(payload, "parent_path") ? payload.parent_path : (currentPath ? sourceBrowserParent(currentPath) : null);
    const scopeBadge = snapshot
      ? '<span class="source-browser-scope snapshot"><span aria-hidden="true">◆</span> ZFS snapshot</span>'
      : backupPlanSelection
        ? '<span class="source-browser-scope caution"><span aria-hidden="true">i</span> Open a ZFS snapshot</span>'
        : sourceBrowserReadOnly === true
        ? '<span class="source-browser-scope readonly"><span aria-hidden="true">✓</span> Read-only source</span>'
        : '<span class="source-browser-scope caution"><span aria-hidden="true">i</span> Checked when saved</span>';
    const empty = '<div class="source-browser-empty"><span aria-hidden="true">□</span><strong>This directory is empty</strong><small>Choose a different folder or use this empty scope intentionally.</small></div>';
    const partialWarning = truncated
      ? '<div class="notice warning source-browser-partial"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Showing the first ' + esc(entryLimit) + ' entries in this folder</strong><span>This is a partial browser view. The folder is still selectable, you can type a known snapshot path in the field, and the backup itself has no TapeScale total file-count limit.</span></div></div>'
      : "";
    const selectionBlocker = !currentPath
      ? "Open a folder below the source root first."
      : backupPlanSelection && !snapshot
        ? "Open a folder inside .zfs/snapshot/<snapshot-name> before selecting it."
        : "";
    panel.innerHTML = '<div class="source-browser-toolbar"><button class="icon-button source-browser-up" type="button" data-action="source-browser-navigate" data-path="' + esc(parentPath || "") + '" aria-label="Open parent directory"' + (currentPath ? "" : " disabled") + '>↑</button><nav class="source-browser-breadcrumbs" aria-label="Source path">' + sourceBrowserBreadcrumbs(currentPath) + '</nav><button class="icon-button" type="button" data-action="close-source-browser" aria-label="Close source browser">&times;</button></div><div class="source-browser-context"><div><span>Browsing</span><code>/source' + (currentPath ? "/" + esc(currentPath) : "") + '</code></div>' + scopeBadge + '</div>' + partialWarning + '<div class="source-browser-column-head" aria-hidden="true"><span>Name</span><span>Open</span></div><div class="source-browser-entries" role="list" aria-label="Files and directories in ' + esc(currentPath || "source root") + '">' + (entries.length ? entries.map((entry) => sourceBrowserEntry(entry, currentPath)).join("") : empty) + '</div><div class="source-browser-footer"><div class="source-browser-selection"><span>Selected folder</span><strong>' + esc(currentPath || "Open a folder to select it") + '</strong><small>' + esc(sourceBrowserSelectable ? "Everything inside this folder will be included in the Full backup." : selectionBlocker) + '</small></div><button class="button button-primary" type="button" data-action="select-source-directory"' + (sourceBrowserSelectable ? "" : ' disabled title="' + esc(selectionBlocker) + '"') + '>Select this folder</button></div><p class="source-browser-live" id="source-browser-live" role="status" aria-live="polite">' + esc(entries.length + (truncated ? "+ entries available; partial view loaded" : " entries loaded")) + "</p>";
  }

  async function loadSourceBrowser(path, focusEntries) {
    const panel = document.getElementById(sourceBrowserTarget.panelId);
    if (!panel) return;
    const requestNumber = ++sourceBrowserRequest;
    const requestedPath = String(path || "");
    panel.innerHTML = '<div class="source-browser-loading" role="status"><span class="spinner" aria-hidden="true"></span><strong>Opening folder…</strong><small>/source' + (requestedPath ? "/" + esc(requestedPath) : "") + "</small></div>";
    try {
      const query = requestedPath ? "?path=" + encodeURIComponent(requestedPath) : "";
      const payload = await request(sourceBrowserEndpoint + query);
      if (requestNumber !== sourceBrowserRequest) return;
      renderSourceBrowser(payload || {}, requestedPath);
      if (focusEntries) {
        const firstEntry = panel.querySelector("button.source-browser-name, .source-browser-entry button, .source-browser-up:not([disabled]), .source-browser-crumb");
        if (firstEntry) firstEntry.focus();
      }
    } catch (error) {
      if (requestNumber !== sourceBrowserRequest || !document.getElementById(sourceBrowserTarget.panelId)) return;
      panel.innerHTML = '<div class="source-browser-error" role="alert"><span aria-hidden="true">!</span><div><strong>Could not open this directory</strong><p>' + esc(errorMessage(error)) + '</p><button class="button button-quiet button-small" type="button" data-action="source-browser-navigate" data-path="' + esc(requestedPath) + '">Try again</button></div></div>';
    }
  }

  function openSourceBrowser(targetKind) {
    sourceBrowserTarget = targetKind === "recovery"
      ? { inputId: "recovery-source-subpath", panelId: "recovery-source-browser", summaryId: "recovery-source-selection", nameId: "recovery-profile-name" }
      : { inputId: "backup-source-subpath", panelId: "backup-source-browser", summaryId: "backup-source-selection", nameId: "backup-profile-name" };
    const panel = document.getElementById(sourceBrowserTarget.panelId);
    const input = document.getElementById(sourceBrowserTarget.inputId);
    const toggle = document.querySelector('[data-action="open-source-browser"][data-browser-target="' + (targetKind === "recovery" ? "recovery" : "backup") + '"]');
    if (!panel || !input) return;
    panel.hidden = false;
    if (toggle) toggle.setAttribute("aria-expanded", "true");
    const modal = modalLayer.querySelector(".modal");
    if (modal) modal.classList.add("source-browser-modal");
    loadSourceBrowser(input.value.trim(), true);
  }

  function closeSourceBrowser() {
    const panel = document.getElementById(sourceBrowserTarget.panelId);
    const toggle = document.querySelector('[data-action="open-source-browser"][aria-controls="' + sourceBrowserTarget.panelId + '"]');
    if (panel) panel.hidden = true;
    if (toggle) {
      toggle.setAttribute("aria-expanded", "false");
      toggle.focus();
    }
  }

  function selectSourceDirectory() {
    const input = document.getElementById(sourceBrowserTarget.inputId);
    const summary = document.getElementById(sourceBrowserTarget.summaryId);
    if (!input || !sourceBrowserPath || !sourceBrowserSelectable) return;
    input.value = sourceBrowserPath;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    if (summary) {
      summary.hidden = false;
      summary.innerHTML = '<span aria-hidden="true">✓</span><div><strong>Folder selected</strong><code>/source/' + esc(sourceBrowserPath) + '</code><small>' + (sourceBrowserSnapshotPath(sourceBrowserPath) ? "ZFS snapshot selected. TapeScale will verify read-only access when you save." : "TapeScale will verify containment and read-only access when you save.") + "</small></div>";
    }
    closeSourceBrowser();
    const name = document.getElementById(sourceBrowserTarget.nameId);
    if (name && !name.value.trim()) name.focus();
  }

  function openBackupProfile() {
    const timezone = (() => {
      try {
        return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
      } catch (_) {
        return "UTC";
      }
    })();
    const poolOptions = ['<option value="">No fixed pool</option>'].concat(state.data.mediaPools.filter((pool) => pick(pool, "purpose") !== "cleaning").map((pool) => '<option value="' + esc(recordId(pool)) + '">' + esc(label(pick(pool, "name"))) + " · " + esc(human(pick(pool, "purpose") || "routine")) + "</option>"));
    const body = [
      '<form id="backup-profile-form" data-timezone="' + esc(timezone) + '">',
      '<div class="backup-boundary"><span aria-hidden="true">1</span><div><strong>Choose an existing snapshot</strong><p>Everything inside the selected folder is written together. Choosing it does not touch a tape.</p></div></div>',
      '<div class="form-grid">',
      '<div class="form-field full"><label for="backup-profile-name">Backup name</label><input class="input" id="backup-profile-name" name="name" maxlength="128" required placeholder="Sports archive snapshot"></div>',
      '<div class="form-field full backup-source-field"><label for="backup-source-subpath">Snapshot folder to back up</label><div class="source-browser-input"><input class="input input-mono" id="backup-source-subpath" name="source_subpath" maxlength="512" required autocomplete="off" placeholder="Choose a ZFS snapshot folder"><button class="button button-primary" type="button" data-action="open-source-browser" data-browser-target="backup" aria-controls="backup-source-browser" aria-expanded="false"><span aria-hidden="true">▰</span> Choose folder</button></div><small>Choose a folder inside <code>.zfs/snapshot/&lt;snapshot-name&gt;</code>. TapeScale verifies containment and read-only mount state when you save.</small><div class="source-browser-selected" id="backup-source-selection" hidden></div></div>',
      '<section class="backup-source-browser" id="backup-source-browser" aria-label="Source file browser" hidden></section>',
      '<div class="form-field"><label for="backup-retention-days">Keep completed backups for</label><div class="input-suffix"><input class="input" id="backup-retention-days" name="retention_days" type="number" min="1" max="36500" value="28" required><span>days</span></div></div>',
      '<div class="form-field"><label for="backup-media-pool">Tape pool <small>(optional)</small></label><select class="select" id="backup-media-pool" name="media_pool_id">' + poolOptions.join("") + '</select></div>',
      '<div class="form-field"><label for="backup-compression">Compression</label><select class="select" id="backup-compression" name="compression_mode"><option value="auto" selected>Automatic · fast GZIP</option><option value="gzip">GZIP · balanced</option><option value="lzo">LZO · fastest</option><option value="none">None</option></select><small>Automatic explicitly uses fast GZIP for a predictable on-tape format. Already-compressed files may not shrink.</small></div>',
      '<div class="form-field"><label>Archive size</label><label class="check-option backup-span-option"><input type="checkbox" id="backup-media-spanning" name="spanning_enabled" checked> Continue onto additional prepared tapes when this archive fills a cartridge.</label></div>',
      '<div class="form-field full"><div class="backup-plan-summary" id="backup-plan-summary"><span aria-hidden="true">✓</span><div><strong>Full backup · fast GZIP · spans tapes as needed</strong><small>Next, TapeScale shows the tapes prepared now, then requests each additional tape only if the running archive needs it. Automatic rolling snapshots are not enabled yet.</small></div></div></div>',
      '</div>',
      '<p class="notice danger" id="backup-profile-error" role="alert" aria-live="assertive" hidden></p>',
      '<div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Continue to tape</button></div>',
      '</form>'
    ].join("");
    sourceBrowserRequest += 1;
    sourceBrowserPath = "";
    sourceBrowserReadOnly = null;
    sourceBrowserSelectable = false;
    sourceBrowserTarget = { inputId: "backup-source-subpath", panelId: "backup-source-browser", summaryId: "backup-source-selection", nameId: "backup-profile-name" };
    openModal("Choose snapshot to back up", body);
    updateBackupProfileOptions(document.querySelector("#backup-profile-form"));
    document.querySelector("#backup-profile-name").focus();
  }

  function updateBackupProfileOptions(form) {
    if (!form) return;
    const compression = backupCompressionLabel({ compression_mode: String(form.elements.compression_mode && form.elements.compression_mode.value || "auto") });
    const spanning = Boolean(form.elements.spanning_enabled && form.elements.spanning_enabled.checked);
    const summary = form.querySelector("#backup-plan-summary strong");
    if (summary) summary.textContent = "Full backup · " + compression + " · " + (spanning ? "spans tapes as needed" : "one tape maximum");
  }

  async function submitBackupProfile(form) {
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#backup-profile-error");
    const source = String(values.get("source_subpath") || "").trim();
    if (!sourceBrowserSnapshotPath(source)) {
      errorBox.textContent = "Choose a folder inside .zfs/snapshot/<snapshot-name>.";
      errorBox.hidden = false;
      const chooser = form.querySelector('[data-action="open-source-browser"]');
      if (chooser) chooser.focus();
      return;
    }
    submit.disabled = true;
    submit.textContent = "Checking snapshot…";
    errorBox.hidden = true;
    try {
      const response = await request(endpoints.backupProfiles, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          name: String(values.get("name") || "").trim(),
          source_subpath: source,
          media_pool_id: String(values.get("media_pool_id") || "") || null,
          compression_mode: String(values.get("compression_mode") || "auto"),
          spanning_enabled: values.get("spanning_enabled") === "on",
          schedule_timezone: form.dataset.timezone || "UTC",
          schedule_enabled: false,
          retention_days: Number(values.get("retention_days")),
          requested_by_label: "Web operator"
        })
      });
      const profile = response && (response.profile || response);
      rememberBackupProfile(profile);
      await loadOne("backupProfiles");
      updateChrome();
      render();
      openStartBackup(recordId(profile));
      toast("Snapshot saved", "Now choose the loaded tape. No tape has been written yet.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Continue to tape";
    }
  }

  function backupCapacityPlan(readiness) {
    const plan = objectValue(readiness, "capacity_plan", "capacity", "archive_estimate");
    const logicalRaw = pick(plan, "logical_bytes", "source_bytes", "estimated_logical_bytes") ?? pick(readiness, "logical_bytes", "source_bytes");
    const estimatedMediaRaw = pick(plan, "estimated_media_bytes", "encoded_bytes", "estimated_encoded_bytes");
    const tapeCapacityRaw = pick(plan, "tape_capacity_bytes", "usable_bytes_per_tape", "cartridge_capacity_bytes");
    const estimatedTapesRaw = pick(plan, "estimated_tapes", "minimum_tapes", "tapes_required");
    const logicalBytes = logicalRaw === null ? Number.NaN : Number(logicalRaw);
    const estimatedMediaBytes = estimatedMediaRaw === null ? Number.NaN : Number(estimatedMediaRaw);
    const tapeCapacityBytes = tapeCapacityRaw === null ? Number.NaN : Number(tapeCapacityRaw);
    const estimatedTapes = estimatedTapesRaw === null ? Number.NaN : Number(estimatedTapesRaw);
    return {
      logicalBytes: Number.isFinite(logicalBytes) && logicalBytes >= 0 ? logicalBytes : null,
      estimatedMediaBytes: Number.isFinite(estimatedMediaBytes) && estimatedMediaBytes >= 0 ? estimatedMediaBytes : null,
      tapeCapacityBytes: Number.isFinite(tapeCapacityBytes) && tapeCapacityBytes > 0 ? tapeCapacityBytes : null,
      estimatedTapes: Number.isInteger(estimatedTapes) && estimatedTapes > 0 ? estimatedTapes : null
    };
  }

  function renderBackupCapacityPlan(readiness, readyTapeCount, spanning) {
    const plan = backupCapacityPlan(readiness);
    const estimate = plan.estimatedTapes ? plan.estimatedTapes + " tape" + (plan.estimatedTapes === 1 ? "" : "s") : "Determined while writing";
    const facts = [
      ["Estimated tapes", estimate],
      ["Prepared now", readyTapeCount + " tape" + (readyTapeCount === 1 ? "" : "s")],
      ["Snapshot data", plan.logicalBytes === null ? "Measured during scan" : bytes(plan.logicalBytes)],
      ["Estimated on tape", plan.estimatedMediaBytes === null ? "Depends on compression" : bytes(plan.estimatedMediaBytes)]
    ];
    const blocked = !spanning && plan.estimatedTapes && plan.estimatedTapes > 1;
    return '<section class="backup-capacity-plan' + (blocked || !spanning ? " warning" : "") + '"><div class="backup-capacity-head"><span aria-hidden="true">' + (blocked || !spanning ? "!" : "▦") + '</span><div><strong>' + (blocked ? "This snapshot is unlikely to fit one tape" : spanning ? "Archive capacity plan" : "Single-tape archive plan") + '</strong><p>Estimates are planning guidance, not a guaranteed fit. Actual tape use depends on file mix, hardware compression, media condition, and engine overhead.</p></div></div><div class="backup-capacity-facts">' + facts.map(([name, value]) => '<div><span>' + esc(name) + '</span><strong>' + esc(value) + '</strong></div>').join("") + '</div>' + (spanning ? '<p class="backup-capacity-action">If another tape is needed, the run pauses safely and asks you to load and authorize the next prepared tape.</p>' : '<p class="backup-capacity-action">This saved plan cannot continue onto another tape. To remove that ceiling, create a new backup plan for this snapshot with media spanning enabled.</p>') + '</section>';
  }

  async function openStartBackup(profileId) {
    const profile = state.data.backupProfiles.find((item) => recordId(item) === profileId);
    if (!profile) return;
    const token = ++state.backupReadinessModalToken;
    openModal("Start backup", '<div class="source-browser-loading" role="status"><span class="spinner" aria-hidden="true"></span><strong>Checking the loaded drive and tape…</strong><small>No backup request or tape write has started.</small></div>');
    let readiness;
    try {
      readiness = await loadBackupMediaReadiness("profile", profileId);
    } catch (error) {
      if (token !== state.backupReadinessModalToken || modalLayer.hidden) return;
      const body = '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not check tape readiness</strong><span>' + esc(errorMessage(error)) + '</span></div></div><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Close</button><button class="button button-primary" type="button" data-action="refresh-backup-readiness" data-profile-id="' + esc(profileId) + '">Try again</button></div>';
      openModal("Start backup", body);
      return;
    }
    if (token !== state.backupReadinessModalToken || modalLayer.hidden) return;
    const source = pick(profile, "source_subpath", "source_path", "logical_root");
    const requiredPoolId = String(pick(profile, "media_pool_id") || "");
    const readinessKey = backupReadinessKey("profile", profileId);
    const idempotencyKey = operationKey("manual-backup");
    const pairs = readiness.candidates;
    const driveCandidates = pairs.filter((candidate, index) => pairs.findIndex((item) => String(pick(item, "drive_id")) === String(pick(candidate, "drive_id"))) === index);
    const drives = driveCandidates.map(backupCandidateDrive);
    const initialPair = readiness.recommended_candidate || (pairs.length === 1 ? pairs[0] : null);
    const readyTapeCount = new Set(pairs.map((candidate) => String(pick(candidate, "cartridge_id") || "")).filter(Boolean)).size;
    const compression = backupCompressionLabel(profile);
    const spanning = backupSpanningEnabled(profile);
    const capacityPlan = backupCapacityPlan(readiness);
    const estimatedTapes = capacityPlan.estimatedTapes ? capacityPlan.estimatedTapes + " estimated" : "As needed";
    const initialDrive = initialPair ? backupCandidateDrive(initialPair) : drives.length === 1 ? drives[0] : null;
    const initialMedia = initialPair ? backupCandidateMedia(initialPair) : null;
    const driveOptions = drives.map((drive) => '<option value="' + esc(recordId(drive)) + '"' + (initialDrive && recordId(initialDrive) === recordId(drive) ? " selected" : "") + '>' + esc(label(pick(drive, "model", "stable_id", "observed_path"))) + " · " + esc(label(pick(drive, "observed_path"))) + "</option>").join("");
    const mediaOptions = initialDrive ? pairs.filter((candidate) => String(pick(candidate, "drive_id")) === recordId(initialDrive)).map(backupCandidateMedia).map((item) => '<option value="' + esc(recordId(item)) + '" data-label="' + esc(mediaName(item)) + '"' + (initialMedia && recordId(initialMedia) === recordId(item) ? " selected" : "") + '>' + esc(mediaName(item)) + " · " + esc(label(pick(item, "media_format", "media_generation"))) + "</option>").join("") : "";
    const blocked = pairs.length === 0;
    const remediation = backupReadinessRemediation(readiness);
    const primaryAction = backupReadinessAction(remediation, "profile", profileId);
    const readinessPanel = blocked
      ? '<div class="backup-readiness blocked"><span aria-hidden="true">!</span><div><strong>' + esc(remediation.heading) + '</strong><p>' + esc(backupServerReadinessMessage(readiness)) + '</p><div class="backup-readiness-actions">' + primaryAction + '<button class="button button-quiet button-small" type="button" data-action="refresh-backup-readiness" data-profile-id="' + esc(profileId) + '">Check again</button></div></div></div>'
      : '<div class="backup-readiness ready"><span aria-hidden="true">✓</span><div><strong>' + readyTapeCount + " backup-ready tape" + (readyTapeCount === 1 ? "" : "s") + ' found</strong><p>Choose the cartridge physically loaded in the selected standalone drive. TapeScale cannot read its inventory label from the drive.</p></div></div>';
    const body = [
      '<form id="start-backup-form" data-profile-id="' + esc(profileId) + '" data-readiness-key="' + esc(readinessKey) + '" data-required-pool-id="' + esc(requiredPoolId) + '" data-idempotency-key="' + esc(idempotencyKey) + '">',
      '<ol class="backup-start-steps" aria-label="Start backup steps"><li class="done"><span>✓</span><strong>Snapshot</strong></li><li class="active"><span>2</span><strong>Tape</strong></li><li><span>3</span><strong>Confirm & start</strong></li></ol>',
      '<div class="backup-boundary"><span aria-hidden="true">◆</span><div><strong>' + esc(backupProfileName(profile)) + '</strong><p><code>' + esc(label(source)) + '</code></p></div></div>',
      '<div class="backup-start-facts"><div><span>Backup</span><strong>Full</strong></div><div><span>Compression</span><strong>' + esc(compression) + '</strong></div><div><span>Estimated tapes</span><strong>' + esc(spanning ? estimatedTapes : "One maximum") + '</strong></div><div><span>Verification</span><strong>Complete readback</strong></div><div><span>Retention</span><strong>' + esc(count(pick(profile, "retention_days")) || 28) + ' days</strong></div></div>',
      renderBackupCapacityPlan(readiness, readyTapeCount, spanning),
      readinessPanel,
      '<div class="form-grid backup-tape-choice"' + (blocked ? " hidden" : "") + '><div class="form-field full"><label for="start-backup-drive">Loaded tape drive</label><select class="select" id="start-backup-drive" name="drive_id" required><option value="">Choose drive</option>' + driveOptions + '</select></div><div class="form-field full"><label for="start-backup-cartridge">Loaded scratch tape</label><select class="select" id="start-backup-cartridge" name="cartridge_id" required><option value="">Choose tape</option>' + mediaOptions + '</select></div></div>',
      '<p class="notice danger" id="start-backup-error" role="alert" aria-live="assertive" hidden></p>',
      '<div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Finish later</button><button class="button button-primary" type="submit"' + (blocked ? " disabled" : "") + '>Continue to confirmation</button></div>',
      '</form>'
    ].join("");
    openModal("Start backup", body);
    updateStartBackupTape(document.querySelector("#start-backup-form"));
  }

  function updateStartBackupTape(form) {
    if (!form) return;
    const driveSelect = form.elements.drive_id;
    const cartridgeSelect = form.elements.cartridge_id;
    const submit = form.querySelector('[type="submit"]');
    if (!driveSelect || !cartridgeSelect || !submit) return;
    const candidates = backupReadinessCandidates(form.dataset.readinessKey);
    const drive = candidates.map(backupCandidateDrive).find((item) => recordId(item) === String(driveSelect.value));
    const previous = String(cartridgeSelect.value || "");
    const eligible = candidates.filter((candidate) => String(pick(candidate, "drive_id")) === String(driveSelect.value)).map(backupCandidateMedia);
    const selected = eligible.some((item) => recordId(item) === previous) ? previous : eligible.length === 1 ? recordId(eligible[0]) : "";
    cartridgeSelect.innerHTML = '<option value="">Choose the tape physically loaded in this drive</option>' + eligible.map((item) => '<option value="' + esc(recordId(item)) + '" data-label="' + esc(mediaName(item)) + '"' + (recordId(item) === selected ? " selected" : "") + '>' + esc(mediaName(item)) + " · " + esc(label(pick(item, "media_format", "media_generation"))) + "</option>").join("");
    cartridgeSelect.disabled = !drive || !eligible.length;
    submit.disabled = !drive || !selected;
  }

  function backupModalSubmissionCurrent(form, token) {
    return token === state.backupReadinessModalToken && !modalLayer.hidden && Boolean(form && form.isConnected);
  }

  async function submitStartBackup(form) {
    const profileId = form.dataset.profileId;
    const token = state.backupReadinessModalToken;
    const driveId = String(form.elements.drive_id && form.elements.drive_id.value || "");
    const cartridgeId = String(form.elements.cartridge_id && form.elements.cartridge_id.value || "");
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#start-backup-error");
    let requestAttempted = false;
    let requestCreated = false;
    submit.disabled = true;
    submit.textContent = "Checking tape…";
    errorBox.hidden = true;
    try {
      const readiness = await loadBackupMediaReadiness("profile", profileId);
      if (!backupModalSubmissionCurrent(form, token)) return;
      const candidate = readiness.candidates.find((item) => String(pick(item, "drive_id")) === driveId && String(pick(item, "cartridge_id")) === cartridgeId);
      if (!candidate) throw new Error(backupServerReadinessMessage(readiness));
      submit.textContent = "Saving request…";
      requestAttempted = true;
      const response = await request(endpoints.backupProfiles + "/" + encodeURIComponent(profileId) + "/runs", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          idempotency_key: form.dataset.idempotencyKey || operationKey("manual-backup"),
          compression_mode: backupCompressionMode(state.data.backupProfiles.find((profile) => recordId(profile) === profileId)),
          spanning_enabled: backupSpanningEnabled(state.data.backupProfiles.find((profile) => recordId(profile) === profileId)),
          requested_by_label: "Web operator"
        })
      });
      requestCreated = true;
      const run = response && (response.run || response.backup_run || response.request || response);
      rememberBackupRun(run);
      state.selectedBackupRunId = recordId(run);
      await Promise.all([loadOne("backupRuns"), loadOne("jobs"), loadOne("overview")]);
      await loadBackupRunDetail(state.selectedBackupRunId, true);
      const continueGuided = backupModalSubmissionCurrent(form, token);
      updateChrome();
      render();
      scheduleBackupRunPoll();
      if (!continueGuided) {
        toast("Backup request saved", "Open the Backup Center when you are ready to confirm the tape and start writing.");
        return;
      }
      const retained = orderedBackupRuns().find((item) => recordId(item) === state.selectedBackupRunId) || run;
      if (backupRunState(retained) === "waiting_for_media" || backupNeedsNextMedia(retained)) {
        await openAssignBackupMedia(state.selectedBackupRunId, { driveId, cartridgeId, guided: true });
      } else {
        closeModal();
        const target = "#jobs/" + encodeURIComponent(state.selectedBackupRunId);
        if (location.hash !== target) location.hash = target;
        toast("Backup already started", "Opening the retained request; no duplicate run was created.");
      }
    } catch (error) {
      if (!backupModalSubmissionCurrent(form, token)) {
        if (requestAttempted && !requestCreated) toast("Backup request needs review", errorMessage(error));
        return;
      }
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Continue to confirmation";
    }
  }

  function backupMediaConfirmation(run, cartridgeLabel) {
    const explicit = pick(run, "required_confirmation", "media_confirmation");
    if (explicit) return String(explicit);
    if (backupMediaIsReadback(run)) return "Select the exact archive tape to retrieve the server confirmation.";
    return cartridgeLabel ? "OVERWRITE " + cartridgeLabel + " FOR BACKUP " + recordId(run) : "Select a cartridge.";
  }

  async function openAssignBackupMedia(runId, options) {
    const run = orderedBackupRuns().find((item) => recordId(item) === runId);
    if (!run) return;
    let readbackMedia = backupMediaIsReadback(run);
    let nextMedia = backupNeedsNextMedia(run);
    let volumeSequence = backupNextVolumeSequence(run);
    let modalTitle = readbackMedia ? "Load tape " + volumeSequence + " for readback" : nextMedia ? "Load tape " + volumeSequence : "Confirm first tape and start";
    const token = ++state.backupReadinessModalToken;
    openModal(modalTitle, '<div class="source-browser-loading" role="status"><span class="spinner" aria-hidden="true"></span><strong>Rechecking the loaded drive and tape…</strong><small>' + (readbackMedia ? "The completed archive is paused safely until its exact next tape is loaded for readback." : nextMedia ? "The archive is paused safely at the media boundary." : "The retained backup request is still waiting; nothing has been written.") + '</small></div>');
    let readiness;
    try {
      readiness = await loadBackupMediaReadiness("run", runId);
    } catch (error) {
      if (token !== state.backupReadinessModalToken || modalLayer.hidden) return;
      const body = '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Could not check tape readiness</strong><span>' + esc(errorMessage(error)) + '</span></div></div><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Finish later</button><button class="button button-primary" type="button" data-action="refresh-backup-readiness" data-run-id="' + esc(runId) + '">Try again</button></div>';
      openModal(modalTitle, body);
      return;
    }
    if (token !== state.backupReadinessModalToken || modalLayer.hidden) return;
    const mediaRequest = objectValue(readiness, "media_request");
    const requestedSequence = Number(pick(mediaRequest, "volume_order", "volume_sequence", "media_sequence"));
    if (Number.isInteger(requestedSequence) && requestedSequence > 0) volumeSequence = requestedSequence;
    const mediaPhase = String(pick(mediaRequest, "phase") || "backup_write").toLowerCase();
    readbackMedia = mediaPhase === "full_readback";
    nextMedia = !readbackMedia && (nextMedia || mediaPhase === "next_media" || volumeSequence > 1);
    modalTitle = readbackMedia ? "Load tape " + volumeSequence + " for readback" : nextMedia ? "Load tape " + volumeSequence : "Confirm first tape and start";
    if (["assigned", "media_authorized", "media_loaded"].includes(readiness.state)) {
      closeModal();
      const target = "#jobs/" + encodeURIComponent(runId);
      if (location.hash !== target) location.hash = target;
      return;
    }
    const readinessKey = backupReadinessKey("run", runId);
    const initial = options || {};
    const pairs = readiness.candidates;
    const expectedLabel = String(pick(mediaRequest, "expected_label", "expected_cartridge_label", "cartridge_label") || "");
    const driveCandidates = pairs.filter((candidate, index) => pairs.findIndex((item) => String(pick(item, "drive_id")) === String(pick(candidate, "drive_id"))) === index);
    const readyDrives = driveCandidates.map(backupCandidateDrive);
    const selectedDrive = readyDrives.find((drive) => recordId(drive) === String(initial.driveId || "")) || (readyDrives.length === 1 ? readyDrives[0] : null);
    const eligibleCandidates = selectedDrive ? pairs.filter((candidate) => String(pick(candidate, "drive_id")) === recordId(selectedDrive)) : [];
    const eligibleMedia = eligibleCandidates.map(backupCandidateMedia);
    const selectedMedia = eligibleMedia.find((item) => recordId(item) === String(initial.cartridgeId || "")) || eligibleMedia.find((item) => String(mediaName(item)) === expectedLabel) || (eligibleMedia.length === 1 ? eligibleMedia[0] : null);
    const driveOptions = readyDrives.map((drive) => {
      const stableId = String(pick(drive, "stable_id") || "");
      return '<option value="' + esc(recordId(drive)) + '" data-stable-id="' + esc(stableId) + '"' + (selectedDrive && recordId(selectedDrive) === recordId(drive) ? " selected" : "") + '>' + esc(label(pick(drive, "model", "stable_id"))) + " · " + esc(label(pick(drive, "observed_path"))) + "</option>";
    }).join("");
    const mediaOptions = eligibleMedia.map((item) => '<option value="' + esc(recordId(item)) + '" data-label="' + esc(mediaName(item)) + '"' + (selectedMedia && recordId(selectedMedia) === recordId(item) ? " selected" : "") + '>' + esc(mediaName(item)) + " · " + esc(label(pick(item, "media_format", "media_generation"))) + "</option>").join("");
    const blocked = pairs.length === 0;
    const remediation = backupReadinessRemediation(readiness);
    const primaryAction = backupReadinessAction(remediation, "run", runId);
    const blockedNotice = blocked ? '<div class="backup-readiness blocked"><span aria-hidden="true">!</span><div><strong>' + esc(remediation.heading) + '</strong><p>' + esc(backupServerReadinessMessage(readiness)) + '</p><div class="backup-readiness-actions">' + primaryAction + '<button class="button button-quiet button-small" type="button" data-action="refresh-backup-readiness" data-run-id="' + esc(runId) + '">Check again</button></div></div></div>' : "";
    const archiveVolumes = backupRunVolumes(run);
    const volumeAlreadyListed = archiveVolumes.some((volume) => volume.sequence === volumeSequence);
    const volumePlan = archiveVolumes.length ? '<div class="backup-volume-plan"><span>' + (readbackMedia ? "Readback order" : "Archive so far") + '</span><ol>' + archiveVolumes.map((volume) => '<li' + (readbackMedia && volume.sequence === volumeSequence ? ' class="current" aria-current="step"' : "") + '><strong>Tape ' + esc(volume.sequence) + '</strong><code>' + esc(volume.label) + '</code></li>').join("") + (!readbackMedia && !volumeAlreadyListed ? '<li class="next"><strong>Tape ' + esc(volumeSequence) + '</strong><span>' + esc(expectedLabel || "Choose below") + '</span></li>' : "") + '</ol></div>' : "";
    const steps = readbackMedia
      ? '<ol class="backup-start-steps" aria-label="Readback tape steps"><li class="done"><span>✓</span><strong>Archive written</strong></li><li class="' + (blocked ? "active" : "done") + '"><span>' + (blocked ? "2" : "✓") + '</span><strong>Exact tape ' + esc(volumeSequence) + '</strong></li><li' + (blocked ? "" : ' class="active"') + '><span>3</span><strong>Confirm & read</strong></li></ol>'
      : '<ol class="backup-start-steps" aria-label="Start backup steps"><li class="done"><span>✓</span><strong>Snapshot</strong></li><li class="' + (blocked ? "active" : "done") + '"><span>' + (blocked ? "2" : "✓") + '</span><strong>' + (nextMedia ? "Next tape" : "First tape") + '</strong></li><li' + (blocked ? "" : ' class="active"') + '><span>3</span><strong>' + (nextMedia ? "Confirm & continue" : "Confirm & start") + '</strong></li></ol>';
    const boundary = readbackMedia
      ? '<div class="backup-boundary info"><span aria-hidden="true">✓</span><div><strong>Load recorded archive tape ' + esc(volumeSequence) + '</strong><p>Unload and remove the current tape before inserting this one. ' + (expectedLabel ? "The required label is " + esc(expectedLabel) + ". " : "") + 'This is a read-only media handoff: TapeScale will not erase, relabel, or append to this tape.</p></div></div>'
      : '<div class="backup-boundary danger"><span aria-hidden="true">!</span><div><strong>Tape ' + esc(volumeSequence) + ' will be erased</strong><p>' + (nextMedia ? "Unload and remove the previous archive tape before inserting this one. " : "") + (expectedLabel ? "The engine requested label " + esc(expectedLabel) + ". " : "") + (nextMedia ? "This tape becomes the next ordered volume in the existing archive. " : "This tape becomes the first volume in the archive. ") + 'TapeScale checks identity, label, protection, holds, reservation, and setup again before writing.</p></div></div>';
    const mediaFields = readbackMedia
      ? '<div class="form-grid"' + (blocked ? " hidden" : "") + '><div class="form-field full"><label for="backup-media-drive">Tape drive containing the archive tape</label><select class="select" id="backup-media-drive" name="drive_id" required><option value="">Choose drive</option>' + driveOptions + '</select></div><div class="form-field full"><label for="backup-media-cartridge">Archive tape ' + esc(volumeSequence) + ' to read</label><select class="select" id="backup-media-cartridge" name="cartridge_id" required><option value="">Choose tape</option>' + mediaOptions + '</select><small>Standalone SAS drives cannot report the physical cartridge label. Match the recorded label to the tape you load; Bacula validates its on-tape volume label after rewind before reading backup data.</small></div><div class="form-field full"><label class="check-option backup-check"><input type="checkbox" name="backup_authority_confirmed" required> I physically verified that archive tape ' + esc(volumeSequence) + ' is loaded in this drive and authorize read-only verification.</label></div><div class="form-field full"><label for="backup-media-confirmation">Type the exact load confirmation</label><input class="input input-mono" id="backup-media-confirmation" name="confirmation" autocomplete="off" spellcheck="false" aria-describedby="backup-media-confirmation-help" required><small id="backup-media-confirmation-help" aria-live="polite">Choose the required tape to reveal the server confirmation.</small></div></div>'
      : '<div class="form-grid"' + (blocked ? " hidden" : "") + '><div class="form-field full"><label for="backup-media-drive">Loaded tape drive</label><select class="select" id="backup-media-drive" name="drive_id" required><option value="">Choose drive</option>' + driveOptions + '</select></div><div class="form-field full"><label for="backup-media-cartridge">Tape ' + esc(volumeSequence) + ' to erase and use</label><select class="select" id="backup-media-cartridge" name="cartridge_id" required><option value="">Choose tape</option>' + mediaOptions + '</select><small>Standalone drives cannot report the physical cartridge label. Match the selected label to the tape you loaded. Previously written archive tapes cannot be selected again.</small></div><div class="form-field full"><label class="check-option backup-check"><input type="checkbox" name="backup_authority_confirmed" required> I physically verified this labeled scratch tape is loaded in this drive, authorize erasing it as archive volume ' + esc(volumeSequence) + ', and understand this backup is not encrypted.</label></div><div class="form-field full"><label for="backup-media-confirmation">Type the exact erase confirmation</label><input class="input input-mono" id="backup-media-confirmation" name="confirmation" autocomplete="off" spellcheck="false" aria-describedby="backup-media-confirmation-help" required><small id="backup-media-confirmation-help" aria-live="polite">Choose a tape to reveal the exact phrase.</small></div></div>';
    const body = [
      '<form id="backup-media-form" data-run-id="' + esc(runId) + '" data-readiness-key="' + esc(readinessKey) + '" data-volume-sequence="' + esc(volumeSequence) + '" data-media-phase="' + esc(mediaPhase) + '">',
      steps,
      boundary,
      volumePlan,
      '<div class="notice warning backup-encryption-warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>This archive is not encrypted</strong><span>' + (readbackMedia ? "Readback does not change the tape, but its contents remain unencrypted." : "Do not use this workflow for sensitive production data until encryption and independent key recovery are enabled.") + '</span></div></div>',
      blockedNotice,
      mediaFields,
      '<p class="notice danger" id="backup-media-error" role="alert" aria-live="assertive" hidden></p>',
      '<div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Finish later</button><button class="button ' + (readbackMedia ? "button-primary" : "button-danger") + '" type="submit"' + (blocked ? " disabled" : "") + '>' + (readbackMedia ? "Confirm tape and continue readback" : nextMedia ? "Erase tape and continue archive" : "Erase tape and start backup") + '</button></div>',
      '</form>'
    ].join("");
    openModal(modalTitle, body);
    updateBackupMediaForDrive(document.querySelector("#backup-media-form"));
  }

  function updateBackupMediaForDrive(form) {
    if (!form) return;
    const driveSelect = form.querySelector('[name="drive_id"]');
    const cartridgeSelect = form.querySelector('[name="cartridge_id"]');
    const submit = form.querySelector('[type="submit"]');
    if (!driveSelect || !cartridgeSelect) return;
    const candidates = backupReadinessCandidates(form.dataset.readinessKey);
    const drive = candidates.map(backupCandidateDrive).find((item) => recordId(item) === driveSelect.value);
    const previous = String(cartridgeSelect.value || "");
    const media = candidates.filter((candidate) => String(pick(candidate, "drive_id")) === String(driveSelect.value)).map(backupCandidateMedia);
    const selected = media.some((item) => recordId(item) === previous) ? previous : media.length === 1 ? recordId(media[0]) : "";
    cartridgeSelect.innerHTML = '<option value="">Choose the tape physically loaded in this drive</option>' + media.map((item) => '<option value="' + esc(recordId(item)) + '" data-label="' + esc(mediaName(item)) + '"' + (recordId(item) === selected ? " selected" : "") + '>' + esc(label(mediaName(item))) + " · " + esc(label(pick(item, "media_format", "media_generation"))) + "</option>").join("");
    cartridgeSelect.disabled = !drive || !media.length;
    if (submit) submit.disabled = !selected;
    updateBackupMediaConfirmation(form);
  }

  function updateBackupMediaConfirmation(form) {
    if (!form) return;
    const run = orderedBackupRuns().find((item) => recordId(item) === form.dataset.runId);
    const cartridge = form.querySelector('[name="cartridge_id"]');
    const option = cartridge && cartridge.options[cartridge.selectedIndex];
    const cartridgeLabel = option && option.dataset.label;
    const candidate = backupReadinessCandidates(form.dataset.readinessKey).find((item) => String(pick(item, "drive_id")) === String(form.elements.drive_id && form.elements.drive_id.value || "") && String(pick(item, "cartridge_id")) === String(cartridge && cartridge.value || ""));
    const phrase = String(pick(candidate, "required_confirmation") || backupMediaConfirmation(run, cartridgeLabel));
    const pairKey = String(form.elements.drive_id && form.elements.drive_id.value || "") + ":" + String(cartridge && cartridge.value || "");
    if (form.dataset.pairKey && form.dataset.pairKey !== pairKey) {
      if (form.elements.backup_authority_confirmed) form.elements.backup_authority_confirmed.checked = false;
      if (form.elements.confirmation) form.elements.confirmation.value = "";
    }
    form.dataset.pairKey = pairKey;
    const help = form.querySelector("#backup-media-confirmation-help");
    if (help) help.textContent = cartridgeLabel ? "Required: " + phrase : phrase;
    form.dataset.confirmation = cartridgeLabel ? phrase : "";
    const submit = form.querySelector('[type="submit"]');
    if (submit) submit.disabled = !cartridgeLabel;
  }

  async function submitBackupMedia(form) {
    const runId = form.dataset.runId;
    const volumeSequence = Number(form.dataset.volumeSequence || 1);
    const mediaPhase = String(form.dataset.mediaPhase || "backup_write");
    const readbackMedia = mediaPhase === "full_readback";
    const continuingArchive = !readbackMedia && volumeSequence > 1;
    const token = state.backupReadinessModalToken;
    const values = new FormData(form);
    const drive = form.querySelector('[name="drive_id"]');
    const driveOption = drive.options[drive.selectedIndex];
    const cartridge = form.querySelector('[name="cartridge_id"]');
    const cartridgeOption = cartridge.options[cartridge.selectedIndex];
    const cartridgeLabel = String(cartridgeOption && cartridgeOption.dataset.label || "");
    const authorized = values.get("backup_authority_confirmed") === "on";
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#backup-media-error");
    let assignmentAttempted = false;
    if (!authorized || !cartridgeLabel) {
      errorBox.textContent = readbackMedia ? "Confirm the exact loaded archive tape and type the server-provided load phrase shown above." : "Confirm the loaded scratch tape and type the exact erase phrase shown above.";
      errorBox.hidden = false;
      return;
    }
    submit.disabled = true;
    submit.textContent = "Rechecking tape…";
    errorBox.hidden = true;
    try {
      const driveId = String(values.get("drive_id") || "");
      const cartridgeId = String(values.get("cartridge_id") || "");
      const readiness = await loadBackupMediaReadiness("run", runId);
      if (!backupModalSubmissionCurrent(form, token)) return;
      const currentMediaRequest = objectValue(readiness, "media_request");
      const currentPhase = String(pick(currentMediaRequest, "phase") || "backup_write").toLowerCase();
      const currentSequence = Number(pick(currentMediaRequest, "volume_order", "volume_sequence", "media_sequence") || 1);
      if (currentPhase !== mediaPhase || currentSequence !== volumeSequence) throw new Error("The requested tape changed while this form was open. Review the current media request before continuing.");
      const candidate = readiness.candidates.find((item) => String(pick(item, "drive_id")) === driveId && String(pick(item, "cartridge_id")) === cartridgeId);
      if (!candidate) throw new Error(backupServerReadinessMessage(readiness));
      const exactLabel = String(pick(candidate, "cartridge_label") || "");
      const expectedConfirmation = String(pick(candidate, "required_confirmation") || "");
      if (!expectedConfirmation || exactLabel !== cartridgeLabel || String(values.get("confirmation") || "") !== expectedConfirmation) throw new Error(readbackMedia ? "The readback request changed. Review the exact tape and type the current server-provided load phrase." : "The tape selection changed. Review it and type the current exact erase phrase.");
      submit.textContent = readbackMedia ? "Continuing readback…" : continuingArchive ? "Continuing archive…" : "Starting backup…";
      assignmentAttempted = true;
      await request(endpoints.backupRuns + "/" + encodeURIComponent(runId) + "/media", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          drive_id: driveId,
          expected_stable_id: String(pick(candidate, "stable_id") || (driveOption && driveOption.dataset.stableId) || ""),
          cartridge_id: cartridgeId,
          volume_sequence: volumeSequence,
          typed_cartridge_label: exactLabel,
          loaded_media_confirmed: authorized,
          destructive_overwrite_confirmed: !readbackMedia && authorized,
          encryption_deviation_confirmed: !readbackMedia && authorized,
          confirmation: String(values.get("confirmation") || "")
        })
      });
      await Promise.all([loadOne("backupRuns"), loadOne("jobs"), loadOne("drives"), loadOne("media")]);
      await loadBackupRunDetail(runId, true);
      const continueGuided = backupModalSubmissionCurrent(form, token);
      state.pendingBackupSetup = null;
      updateChrome();
      if (continueGuided) {
        closeModal();
        const target = "#jobs/" + encodeURIComponent(runId);
        if (location.hash !== target) location.hash = target;
        else render();
      } else {
        render();
      }
      scheduleBackupRunPoll();
      toast(continueGuided ? (readbackMedia ? "Readback tape accepted" : continuingArchive ? "Archive continued" : "Backup started") : (readbackMedia ? "Readback tape accepted in background" : continuingArchive ? "Archive continued in background" : "Backup started in background"), readbackMedia ? "TapeScale accepted your physical-label attestation. Bacula will validate the on-tape volume label after rewind before reading backup data." : continuingArchive ? "TapeScale is writing the next ordered archive volume and will request another tape only if needed." : "TapeScale is scanning the snapshot and will verify every tape in the archive after writing it.");
    } catch (error) {
      if (!backupModalSubmissionCurrent(form, token)) {
        if (assignmentAttempted) toast("Backup start needs review", errorMessage(error));
        return;
      }
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = readbackMedia ? "Confirm tape and continue readback" : continuingArchive ? "Erase tape and continue archive" : "Erase tape and start backup";
    }
  }

  function prepareBackupTape(button) {
    const pending = {
      profileId: String(button.dataset.profileId || ""),
      runId: String(button.dataset.runId || "")
    };
    state.pendingBackupSetup = null;
    if (openTapeInitialization({ dataset: { driveId: String(button.dataset.driveId || ""), mediaId: String(button.dataset.mediaId || "") } }) === true) state.pendingBackupSetup = pending;
  }

  async function refreshBackupReadiness(button) {
    const token = state.backupReadinessModalToken;
    button.disabled = true;
    button.textContent = "Checking…";
    await Promise.all([loadOne("drives"), loadOne("media"), loadOne("qualificationRuns")]);
    if (modalLayer.hidden || token !== state.backupReadinessModalToken) return;
    const runId = String(button.dataset.runId || "");
    const profileId = String(button.dataset.profileId || "");
    if (runId) openAssignBackupMedia(runId);
    else if (profileId) openStartBackup(profileId);
  }

  async function resumePendingBackupSetupIfReady() {
    const pending = state.pendingBackupSetup;
    if (!pending) return false;
    let readiness;
    try {
      readiness = await loadBackupMediaReadiness(pending.runId ? "run" : "profile", pending.runId || pending.profileId);
    } catch (_) {
      return false;
    }
    if (!readiness.candidates.length) return false;
    state.pendingBackupSetup = null;
    const target = pending.runId ? "#jobs/" + encodeURIComponent(pending.runId) : "#jobs";
    const reopen = () => {
      if (pending.runId) openAssignBackupMedia(pending.runId);
      else if (pending.profileId) openStartBackup(pending.profileId);
      toast("Tape is ready", "Continue the backup with the prepared drive and tape.");
    };
    if (location.hash !== target) {
      window.addEventListener("hashchange", reopen, { once: true });
      location.hash = target;
    } else {
      reopen();
    }
    return true;
  }

  function openCancelBackupRun(runId) {
    const run = orderedBackupRuns().find((item) => recordId(item) === runId);
    if (!run || !backupRunActive(run)) return;
    const body = '<form id="cancel-backup-form" data-run-id="' + esc(runId) + '"><div class="backup-boundary warning"><span aria-hidden="true">×</span><div><strong>Request safe cancellation</strong><p>TapeScale will preserve previous valid backups, retain honest partial-write evidence, and perform required tape cleanup. Cancellation may wait for a safe engine boundary.</p></div></div><div class="form-grid"><div class="form-field full"><label for="backup-cancel-reason">Reason</label><textarea class="textarea" id="backup-cancel-reason" name="reason" maxlength="1000" required placeholder="Why this backup should stop"></textarea></div><div class="form-field full"><label for="backup-cancel-operator">Operator</label><input class="input" id="backup-cancel-operator" name="requested_by_label" maxlength="128" value="Web operator" required></div></div><p class="notice danger" id="backup-cancel-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Keep running</button><button class="button button-danger" type="submit">Request cancellation</button></div></form>';
    openModal("Cancel backup run", body);
    document.querySelector("#backup-cancel-reason").focus();
  }

  async function submitCancelBackup(form) {
    const runId = form.dataset.runId;
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#backup-cancel-error");
    submit.disabled = true;
    submit.textContent = "Requesting…";
    errorBox.hidden = true;
    try {
      await request(endpoints.backupRuns + "/" + encodeURIComponent(runId) + "/cancel", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          reason: String(values.get("reason") || "").trim(),
          requested_by_label: String(values.get("requested_by_label") || "").trim(),
          actor_label: String(values.get("requested_by_label") || "").trim()
        })
      });
      closeModal();
      await loadOne("backupRuns");
      await loadBackupRunDetail(runId, true);
      updateChrome();
      render();
      scheduleBackupRunPoll();
      toast("Cancellation requested", "The durable run record will show the safe terminal result.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Request cancellation";
    }
  }

  function scheduleBackupRunPoll() {
    if (state.backupRunPollTimer) window.clearTimeout(state.backupRunPollTimer);
    const active = state.data.backupRuns.filter(backupRunActive);
    if (!active.length) {
      state.backupRunPollTimer = null;
      return;
    }
    state.backupRunPollTimer = window.setTimeout(async () => {
      const previouslyActive = new Set(active.map(recordId));
      await Promise.all([loadOne("backupRuns"), loadOne("jobs"), loadOne("overview"), loadOne("drives"), loadOne("media"), loadOne("recoveryPoints")]);
      const selected = selectedBackupRun();
      if (selected) await loadBackupRunDetail(recordId(selected), true);
      const completed = state.data.backupRuns.find((run) => previouslyActive.has(recordId(run)) && !backupRunActive(run));
      if (completed) toast("Backup " + human(backupRunState(completed)), backupFailureDetail(completed) || "The durable result and retained evidence are ready.");
      updateChrome();
      render();
      scheduleBackupRunPoll();
    }, 3500);
  }

  function openAddMedia() {
    const options = ['<option value="">Select a generation</option>'];
    for (let generation = 1; generation <= 10; generation += 1) options.push('<option value="' + generation + '">LTO-' + generation + "</option>");
    const body = '<form id="add-media-form"><div class="form-grid"><div class="form-field full"><label for="media-label">Internal label</label><input class="input" id="media-label" name="internal_label" required pattern="[-A-Za-z0-9._:]+"><small>Letters, numbers, hyphen, underscore, period, and colon.</small></div><div class="form-field"><label for="media-barcode">Barcode</label><input class="input" id="media-barcode" name="barcode"></div><div class="form-field"><label for="media-generation">Media generation</label><select class="select" id="media-generation" name="media_generation" required>' + options.join("") + '</select></div><div class="form-field full"><label for="media-format">Media format</label><input class="input" id="media-format" name="media_format"><small>Optional. TapeScale defaults this from the generation.</small></div><div class="form-field full"><label class="check-option"><input type="checkbox" name="worm"> WORM cartridge</label><small>Mark this cartridge as write-once media.</small></div></div><p class="notice danger" id="media-form-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Add cartridge</button></div></form>';
    openModal("Add cartridge", body);
    document.querySelector("#media-label").focus();
  }

  async function submitMedia(form) {
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#media-form-error");
    submit.disabled = true;
    submit.textContent = "Adding…";
    errorBox.hidden = true;
    const values = new FormData(form);
    const payload = {
      internal_label: String(values.get("internal_label") || "").trim(),
      barcode: String(values.get("barcode") || "").trim() || null,
      media_generation: Number(values.get("media_generation")),
      media_format: String(values.get("media_format") || "").trim() || null,
      worm: values.get("worm") === "on"
    };
    try {
      const response = await request(endpoints.media, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify(payload)
      });
      const created = response && (response.media || response);
      if (created && typeof created === "object") state.data.media = [created].concat(state.data.media.filter((item) => recordId(item) !== recordId(created)));
      closeModal();
      await loadOne("media");
      updateChrome();
      render();
      toast("Cartridge added", "Inventory was refreshed from TapeScale.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Add cartridge";
    }
  }

  function mediaSummaryById(id) {
    return state.data.media.find((item) => recordId(item) === id || String(mediaName(item)) === id) || null;
  }

  function currentMediaStateVersion(id) {
    const summary = mediaSummaryById(id) || {};
    const detail = mediaDetail(id) || {};
    const cartridge = Object.assign({}, summary, objectValue(detail, "cartridge"));
    return Number(pick(cartridge, "state_version") || 0);
  }

  async function refreshMediaRecord(id) {
    invalidateMediaReuseEvaluations(id);
    await Promise.all([loadOne("media"), loadOne("mediaPools")]);
    await loadMediaDetail(id, true);
    updateChrome();
    render();
  }

  function openMediaPoolForm() {
    const body = '<form id="media-pool-form"><div class="media-modal-boundary"><span aria-hidden="true">P</span><div><strong>Create an administrative retention pool</strong><p>Pool creation groups cartridges by purpose. It does not write to or otherwise modify physical media.</p></div></div><div class="form-grid"><div class="form-field full"><label for="media-pool-name">Pool name</label><input class="input" id="media-pool-name" name="name" maxlength="128" required placeholder="Monthly archive"></div><div class="form-field"><label for="media-pool-purpose">Purpose</label><select class="select" id="media-pool-purpose" name="purpose" required><option value="routine">Routine protection</option><option value="archive">Archive</option><option value="scratch">Scratch / reuse candidates</option><option value="cleaning">Cleaning cartridges</option></select></div><div class="form-field"><label class="check-option media-check"><input type="checkbox" name="indefinite_retention"> Indefinite retention</label><small>Use when cartridges remain protected without a scheduled expiry.</small></div><div class="form-field full"><label for="media-pool-description">Description</label><textarea class="textarea" id="media-pool-description" name="description" maxlength="1000" placeholder="Optional handling and retention context"></textarea></div><div class="form-field full"><label for="media-pool-actor">Operator</label><input class="input" id="media-pool-actor" name="actor" maxlength="128" value="Web operator" required></div></div><p class="notice danger" id="media-pool-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Create pool</button></div></form>';
    openModal("Create retention pool", body);
    document.querySelector("#media-pool-name").focus();
  }

  async function submitMediaPool(form) {
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#media-pool-error");
    submit.disabled = true;
    submit.textContent = "Creating…";
    errorBox.hidden = true;
    try {
      await request(endpoints.mediaPools, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          name: String(values.get("name") || "").trim(),
          purpose: String(values.get("purpose") || "routine"),
          indefinite_retention: values.get("indefinite_retention") === "on",
          details: { description: String(values.get("description") || "").trim() || null },
          actor: String(values.get("actor") || "").trim() || null
        })
      });
      closeModal();
      await loadOne("mediaPools");
      render();
      toast("Retention pool created", "The pool is ready for explicit cartridge assignment.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Create pool";
    }
  }

  function openMediaPoolAssignment(id) {
    const summary = mediaSummaryById(id);
    if (!summary) return;
    const currentPoolId = mediaPoolId(summary);
    const version = currentMediaStateVersion(id);
    const options = ['<option value=""' + (currentPoolId === "unassigned" ? " selected" : "") + '>Unassigned</option>'].concat(state.data.mediaPools.map((pool) => '<option value="' + esc(recordId(pool)) + '"' + (recordId(pool) === currentPoolId ? " selected" : "") + '>' + esc(label(pick(pool, "name"))) + " · " + esc(human(pick(pool, "purpose") || "routine")) + "</option>"));
    const body = '<form id="media-pool-assignment-form" data-media-id="' + esc(id) + '"><div class="media-modal-boundary"><span aria-hidden="true">↦</span><div><strong>Administrative assignment only</strong><p>Changing the pool updates retention metadata for ' + esc(label(mediaName(summary))) + '. Physical tape content is unchanged.</p></div></div><div class="form-grid"><div class="form-field full"><label for="media-assignment-pool">Destination pool</label><select class="select" id="media-assignment-pool" name="pool_id">' + options.join("") + '</select></div><div class="form-field"><label for="media-assignment-actor">Operator</label><input class="input" id="media-assignment-actor" name="actor" maxlength="128" value="Web operator" required></div><div class="form-field"><label>Expected record state</label><input class="input input-mono" value="v' + version + '" disabled><input type="hidden" name="expected_state_version" value="' + version + '"></div><div class="form-field full"><small>TapeScale rejects this request if another operator changed the cartridge after this dossier was loaded.</small></div></div><input type="hidden" name="idempotency_key" value="' + esc(operationKey("pool-assignment")) + '"><p class="notice danger" id="media-assignment-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Save assignment</button></div></form>';
    openModal("Assign " + mediaName(summary) + " to a pool", body);
    document.querySelector("#media-assignment-pool").focus();
  }

  async function submitMediaPoolAssignment(form) {
    const id = form.dataset.mediaId;
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#media-assignment-error");
    submit.disabled = true;
    submit.textContent = "Saving…";
    errorBox.hidden = true;
    try {
      await request(endpoints.media + "/" + encodeURIComponent(id) + "/pool-assignment", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          pool_id: String(values.get("pool_id") || "") || null,
          expected_state_version: Number(values.get("expected_state_version")),
          idempotency_key: String(values.get("idempotency_key")),
          actor: String(values.get("actor") || "").trim() || null
        })
      });
      closeModal();
      await refreshMediaRecord(id);
      toast("Pool assignment saved", "The cartridge dossier now reflects the current retention pool.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Save assignment";
    }
  }

  function updateMediaHoldExpiry(form) {
    if (!form) return;
    const kind = form.querySelector('[name="kind"]');
    const expiry = form.querySelector('[name="expires_at"]');
    if (!kind || !expiry) return;
    const manual = kind.value === "manual";
    if (!manual) expiry.value = "";
    expiry.disabled = !manual;
    const help = expiry.nextElementSibling;
    if (help) {
      help.id = "media-hold-expiry-help";
      help.textContent = manual
        ? "Only manual holds can auto-expire. Leave blank to require an explicit release."
        : human(kind.value) + " holds cannot auto-expire and require an explicit release.";
      expiry.setAttribute("aria-describedby", help.id);
    }
  }

  function openMediaHoldForm(id) {
    const summary = mediaSummaryById(id);
    if (!summary) return;
    const version = currentMediaStateVersion(id);
    const body = '<form id="media-hold-form" data-media-id="' + esc(id) + '"><div class="media-modal-boundary warning"><span aria-hidden="true">!</span><div><strong>Hold ' + esc(label(mediaName(summary))) + ' against reuse</strong><p>Manual, legal, and archive holds remain visible in the cartridge dossier and are evaluated before any future write workflow.</p></div></div><div class="form-grid"><div class="form-field"><label for="media-hold-kind">Hold type</label><select class="select" id="media-hold-kind" name="kind" required><option value="manual">Manual</option><option value="legal">Legal</option><option value="archive">Archive</option></select></div><div class="form-field"><label for="media-hold-expiry">Expires at</label><input class="input" id="media-hold-expiry" name="expires_at" type="datetime-local"><small>Only manual holds can auto-expire. Leave blank to require an explicit release.</small></div><div class="form-field full"><label for="media-hold-reason">Reason</label><textarea class="textarea" id="media-hold-reason" name="reason" maxlength="1000" required placeholder="Why this cartridge must remain protected"></textarea></div><div class="form-field"><label for="media-hold-actor">Operator</label><input class="input" id="media-hold-actor" name="actor" maxlength="128" value="Web operator" required></div><div class="form-field"><label>Expected record state</label><input class="input input-mono" value="v' + version + '" disabled><input type="hidden" name="expected_state_version" value="' + version + '"></div></div><input type="hidden" name="idempotency_key" value="' + esc(operationKey("media-hold")) + '"><p class="notice danger" id="media-hold-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Add hold</button></div></form>';
    openModal("Add cartridge hold", body);
    updateMediaHoldExpiry(document.querySelector("#media-hold-form"));
    document.querySelector("#media-hold-kind").focus();
  }

  async function submitMediaHold(form) {
    const id = form.dataset.mediaId;
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#media-hold-error");
    submit.disabled = true;
    submit.textContent = "Recording…";
    errorBox.hidden = true;
    const expires = String(values.get("expires_at") || "");
    const kind = String(values.get("kind"));
    try {
      await request(endpoints.media + "/" + encodeURIComponent(id) + "/holds", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          kind,
          reason: String(values.get("reason") || "").trim(),
          expires_at: kind === "manual" && expires ? new Date(expires).toISOString() : null,
          expected_state_version: Number(values.get("expected_state_version")),
          idempotency_key: String(values.get("idempotency_key")),
          actor: String(values.get("actor") || "").trim() || null
        })
      });
      closeModal();
      await refreshMediaRecord(id);
      toast("Cartridge hold added", "The hold now participates in every reuse evaluation.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Add hold";
    }
  }

  function openMediaHoldRelease(mediaId, holdId) {
    const summary = mediaSummaryById(mediaId);
    if (!summary || !holdId) return;
    const version = currentMediaStateVersion(mediaId);
    const body = '<form id="media-hold-release-form" data-media-id="' + esc(mediaId) + '" data-hold-id="' + esc(holdId) + '"><div class="media-modal-boundary warning"><span aria-hidden="true">!</span><div><strong>Release a recorded hold</strong><p>This removes one retention blocker from ' + esc(label(mediaName(summary))) + '. Other holds and recovery dependencies remain in force.</p></div></div><div class="form-grid"><div class="form-field full"><label for="media-release-reason">Release reason</label><textarea class="textarea" id="media-release-reason" name="reason" maxlength="1000" required placeholder="Document the authority and reason for this release"></textarea></div><div class="form-field"><label for="media-release-actor">Operator</label><input class="input" id="media-release-actor" name="actor" maxlength="128" value="Web operator" required></div><div class="form-field"><label>Expected record state</label><input class="input input-mono" value="v' + version + '" disabled><input type="hidden" name="expected_state_version" value="' + version + '"></div></div><input type="hidden" name="idempotency_key" value="' + esc(operationKey("hold-release")) + '"><p class="notice danger" id="media-release-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Release hold</button></div></form>';
    openModal("Release cartridge hold", body);
    document.querySelector("#media-release-reason").focus();
  }

  async function submitMediaHoldRelease(form) {
    const mediaId = form.dataset.mediaId;
    const holdId = form.dataset.holdId;
    const values = new FormData(form);
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#media-release-error");
    submit.disabled = true;
    submit.textContent = "Releasing…";
    errorBox.hidden = true;
    try {
      await request("/holds/" + encodeURIComponent(holdId) + "/release", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          reason: String(values.get("reason") || "").trim(),
          expected_state_version: Number(values.get("expected_state_version")),
          idempotency_key: String(values.get("idempotency_key")),
          actor: String(values.get("actor") || "").trim() || null
        })
      });
      closeModal();
      await refreshMediaRecord(mediaId);
      toast("Cartridge hold released", "The release is retained in the cartridge history.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Release hold";
    }
  }

  function openMediaReuseEvaluation(id) {
    const summary = mediaSummaryById(id);
    if (!summary) return;
    const drives = state.data.drives;
    const options = drives.map((drive) => {
      const driveId = recordId(drive);
      const selected = state.mediaReuseDriveByMedia[id] === driveId;
      return '<option value="' + esc(driveId) + '"' + (selected ? " selected" : "") + '>' + esc(label(pick(drive, "vendor"))) + " " + esc(label(pick(drive, "model"))) + " · " + esc(label(pick(drive, "stable_id", "name", "device"))) + "</option>";
    });
    const body = '<form id="media-reuse-form" data-media-id="' + esc(id) + '"><div class="media-modal-boundary"><span aria-hidden="true">✓</span><div><strong>Read-only decision support</strong><p>TapeScale will evaluate ' + esc(label(mediaName(summary))) + ' against one exact drive, current holds, retained recovery points, content, and physical compatibility. No media mutation is available here.</p></div></div><div class="form-grid"><div class="form-field full"><label for="media-reuse-drive">Exact physical drive</label><select class="select" id="media-reuse-drive" name="drive_id" required><option value="">Select a drive</option>' + options.join("") + '</select><small>The drive UUID is sent with the evaluation and the stable hardware identity is shown for operator confirmation.</small></div></div>' + (drives.length ? "" : '<div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>No drives are available</strong><span>Connect and reconcile a SAS tape drive before requesting a compatibility decision.</span></div></div>') + '<p class="notice danger" id="media-reuse-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit"' + (drives.length ? "" : " disabled") + '>Run evaluation</button></div></form>';
    openModal("Evaluate cartridge reuse", body);
    document.querySelector("#media-reuse-drive").focus();
  }

  async function submitMediaReuseEvaluation(form) {
    const id = form.dataset.mediaId;
    const values = new FormData(form);
    const driveId = String(values.get("drive_id") || "");
    if (!id || !driveId) return;
    const key = id + ":" + driveId;
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#media-reuse-error");
    submit.disabled = true;
    submit.textContent = "Evaluating…";
    errorBox.hidden = true;
    state.mediaReuseDriveByMedia[id] = driveId;
    state.mediaReuseLoading.add(key);
    delete state.data.mediaReuseEvaluations[key];
    delete state.mediaReuseEvaluatedAt[key];
    try {
      const payload = await request(endpoints.media + "/" + encodeURIComponent(id) + "/reuse-evaluation?drive_id=" + encodeURIComponent(driveId));
      state.data.mediaReuseEvaluations[key] = payload && typeof payload === "object" ? payload : {};
      const evaluatedAt = Date.now();
      state.mediaReuseEvaluatedAt[key] = evaluatedAt;
      window.setTimeout(() => {
        if (state.mediaReuseEvaluatedAt[key] === evaluatedAt && state.route === "tapes" && recordId(selectedMedia()) === id) render();
      }, 90050);
      state.mediaReuseLoading.delete(key);
      closeModal();
      render();
      const evaluation = payload && (payload.evaluation || payload);
      toast(pick(evaluation, "allowed", "eligible") === true ? "Reuse evaluation passed" : "Reuse evaluation found blockers", "The exact-drive decision and affected recovery points are shown in the dossier.");
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Run evaluation";
    } finally {
      state.mediaReuseLoading.delete(key);
    }
  }

  function qualificationMedia() {
    return state.data.media.filter((item) => item.worm !== true && count(pick(item, "active_holds")) === 0 && pick(item, "has_content") !== true && !pick(item, "reserved_by") && !["foreign", "mismatch"].includes(String(pick(item, "identity_status") || "")));
  }

  function newQualificationIdempotencyKey() {
    if (!window.crypto || typeof window.crypto.getRandomValues !== "function") throw new Error("This browser cannot create a safe qualification request identity.");
    if (typeof window.crypto.randomUUID === "function") return window.crypto.randomUUID();
    const bytes = new Uint8Array(16);
    window.crypto.getRandomValues(bytes);
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
    return hex.slice(0, 8) + "-" + hex.slice(8, 12) + "-" + hex.slice(12, 16) + "-" + hex.slice(16, 20) + "-" + hex.slice(20);
  }

  function tapeInitializePendingKey(driveId, cartridgeId) {
    return "tapescale.qualification-request.v1." + driveId + "." + cartridgeId;
  }

  function tapeInitializeIdempotencyKey(form, driveId, stableId, cartridgeId, cartridgeLabel) {
    const storageKey = tapeInitializePendingKey(driveId, cartridgeId);
    const requestScope = driveId + "\n" + stableId + "\n" + cartridgeId + "\n" + cartridgeLabel;
    if (form && form.dataset.idempotencyScope === requestScope && form.dataset.idempotencyKey) return form.dataset.idempotencyKey;
    try {
      const retained = JSON.parse(window.localStorage.getItem(storageKey) || "null");
      if (retained
        && typeof retained.idempotencyKey === "string"
        && retained.stableId === stableId
        && retained.cartridgeLabel === cartridgeLabel) {
        if (form) {
          form.dataset.idempotencyScope = requestScope;
          form.dataset.idempotencyKey = retained.idempotencyKey;
        }
        return retained.idempotencyKey;
      }
    } catch (_) {
      // A corrupt or unavailable browser store must not weaken server checks.
    }
    const idempotencyKey = newQualificationIdempotencyKey();
    try {
      window.localStorage.setItem(storageKey, JSON.stringify({ idempotencyKey, stableId, cartridgeLabel }));
    } catch (_) {
      // The current form still retains the key for same-page retries.
    }
    if (form) {
      form.dataset.idempotencyScope = requestScope;
      form.dataset.idempotencyKey = idempotencyKey;
    }
    return idempotencyKey;
  }

  function clearTapeInitializeIdempotency(form, driveId, cartridgeId) {
    try {
      window.localStorage.removeItem(tapeInitializePendingKey(driveId, cartridgeId));
    } catch (_) {
      // Storage may be disabled; the server remains the authority.
    }
    if (form) {
      delete form.dataset.idempotencyScope;
      delete form.dataset.idempotencyKey;
    }
  }

  function activeQualificationForDrive(driveId) {
    return state.data.qualificationRuns.find((run) => String(pick(run, "drive_id")) === String(driveId) && ["queued", "running"].includes(String(pick(run, "state") || "").toLowerCase())) || null;
  }

  function tapeInitializeMedia(driveId) {
    const eligible = qualificationMedia();
    const active = activeQualificationForDrive(driveId);
    const activeCartridge = active && state.data.media.find((item) => recordId(item) === String(pick(active, "cartridge_id")));
    if (!activeCartridge || eligible.some((item) => recordId(item) === recordId(activeCartridge))) return eligible;
    return [activeCartridge].concat(eligible);
  }

  function tapeInitializeCartridgeAllowed(item, driveId) {
    if (!item) return false;
    if (qualificationMedia().some((candidate) => recordId(candidate) === recordId(item))) return true;
    const active = activeQualificationForDrive(driveId);
    return Boolean(active)
      && String(pick(active, "cartridge_id")) === recordId(item)
      && String(pick(active, "expected_cartridge_label", "cartridge_label")) === String(mediaName(item));
  }

  function activeOperationForDrive(driveId) {
    return state.data.driveOperations.find((operation) => String(pick(operation, "drive_id")) === String(driveId) && ["queued", "running"].includes(String(pick(operation, "state") || "").toLowerCase())) || null;
  }

  function tapeInitializeDriveBlocker(drive) {
    if (!drive) return "Select a drive.";
    const health = objectValue(drive, "health");
    const observedAt = new Date(pick(drive, "last_observed_at") || 0).getTime();
    if (pick(state.data.system, "lab_tape_qualification_enabled") !== true) return "Destructive tape qualification is disabled in this deployment.";
    if (!Number.isFinite(observedAt) || Date.now() - observedAt > 120000) return "The drive observation is stale; refresh and wait for the worker.";
    if (health.present !== true || health.accessible !== true) return "The configured tape device is not present and accessible.";
    if (health.identity_verified !== true || health.identity_conflict === true) return "The drive lacks one unambiguous stable SCSI identity.";
    if (health.motion_position_uncertain === true) return "A prior result left tape position uncertain; reconcile the drive first.";
    if (health.immediate_mode !== false || health.in_flight !== 0) return "Kernel tape I/O state is not safe for qualification.";
    if (health.medium_loaded !== true || health.ready !== true) return "Load the designated scratch cartridge and wait for the drive to report ready.";
    if (activeOperationForDrive(recordId(drive))) return "A tape motion request is already active on this drive.";
    return null;
  }

  function exactQualificationRun(driveId, cartridgeId, cartridgeLabel) {
    const drive = state.data.drives.find((item) => recordId(item) === String(driveId));
    const cartridge = state.data.media.find((item) => recordId(item) === String(cartridgeId));
    return state.data.qualificationRuns
      .filter((run) => String(pick(run, "drive_id")) === String(driveId) && String(pick(run, "cartridge_id")) === String(cartridgeId) && String(pick(run, "expected_cartridge_label", "cartridge_label")) === String(cartridgeLabel))
      .sort((left, right) => new Date(pick(right, "created_at", "started_at") || 0) - new Date(pick(left, "created_at", "started_at") || 0))
      .find((run) => {
        const runState = String(pick(run, "state") || "").toLowerCase();
        if (["queued", "running"].includes(runState)) return true;
        return runState === "passed" && Boolean(drive) && Boolean(cartridge) && recoveryQualificationMatches(run, drive, cartridge);
      }) || null;
  }

  function tapeInitializeDriveOptions(initialDriveId) {
    return ['<option value="">Select the loaded tape drive</option>'].concat(state.data.drives.map((drive) => {
      const blocker = tapeInitializeDriveBlocker(drive);
      const active = activeQualificationForDrive(recordId(drive));
      const selected = String(recordId(drive)) === String(initialDriveId) ? " selected" : "";
      const unavailable = blocker && !active ? " disabled" : "";
      const suffix = active ? " · qualification " + human(pick(active, "state")) : blocker ? " · " + blocker : drive.reconciled === true ? " · ready" : " · identity trust required";
      return '<option value="' + esc(recordId(drive)) + '"' + selected + unavailable + '>' + esc(label(pick(drive, "model", "stable_id", "observed_path"))) + " · " + esc(label(pick(drive, "observed_path"))) + esc(suffix) + "</option>";
    }));
  }

  function tapeInitializeMediaOptions(initialMediaId, driveId) {
    const active = activeQualificationForDrive(driveId);
    return ['<option value="">Select the physically loaded scratch cartridge</option>'].concat(tapeInitializeMedia(driveId).map((item) => {
      const resume = active && String(pick(active, "cartridge_id")) === recordId(item);
      return '<option value="' + esc(recordId(item)) + '" data-label="' + esc(mediaName(item)) + '"' + (String(recordId(item)) === String(initialMediaId) ? " selected" : "") + '>' + esc(mediaName(item)) + (pick(item, "barcode") ? " · " + esc(pick(item, "barcode")) : "") + " · " + esc(label(pick(item, "media_format", "media_generation"))) + (resume ? " · resume " + esc(human(pick(active, "state"))) : "") + "</option>";
    }));
  }

  function openTapeInitialization(button) {
    if (pick(state.data.system, "lab_tape_qualification_enabled") !== true) {
      toast("Tape setup is disabled", "Enable the attended tape setup gate on both the API and worker first.");
      return false;
    }
    const requestedDriveId = button && button.dataset.driveId;
    const safeDrives = state.data.drives.filter((drive) => !tapeInitializeDriveBlocker(drive) || activeQualificationForDrive(recordId(drive)));
    const initialDriveId = requestedDriveId || (safeDrives.length === 1 ? recordId(safeDrives[0]) : "");
    const requestedMediaId = button && button.dataset.mediaId || (state.route === "tapes" && selectedMedia() ? recordId(selectedMedia()) : "");
    const existingMedia = tapeInitializeMedia(initialDriveId);
    const active = activeQualificationForDrive(initialDriveId);
    const activeMediaId = active && String(pick(active, "cartridge_id") || "");
    const initialMediaId = existingMedia.some((item) => recordId(item) === activeMediaId) ? activeMediaId : existingMedia.some((item) => String(recordId(item)) === String(requestedMediaId)) ? requestedMediaId : existingMedia.length === 1 ? recordId(existingMedia[0]) : "";
    const initialMode = existingMedia.length ? "existing" : "new";
    const generations = [];
    generations.push('<option value="">Select media generation</option>');
    for (let generation = 1; generation <= 10; generation += 1) generations.push('<option value="' + generation + '">LTO-' + generation + "</option>");
    const body = [
      '<form id="tape-initialize-form">',
      '<div class="tape-init-hero"><span aria-hidden="true">!</span><div><small>Required for this exact drive and tape pairing</small><strong>Prepare a scratch tape for backups</strong><p>TapeScale confirms the drive, records the tape label, erases it, and runs a complete write/read setup test. Setup repeats after relevant drive identity, path, or firmware changes. A standalone drive cannot identify the cartridge for you.</p></div></div>',
      '<ol class="tape-init-steps" aria-label="Tape preparation steps"><li><span>1</span><strong>Drive</strong><small>Confirm hardware</small></li><li><span>2</span><strong>Tape</strong><small>Record label</small></li><li><span>3</span><strong>Erase & test</strong><small>Verify readback</small></li></ol>',
      '<section class="tape-init-section"><div class="tape-init-section-head"><span>1</span><div><strong>Loaded SAS tape drive</strong><small>TapeScale revalidates this exact stable identity before any command.</small></div></div><div class="form-field"><label for="tape-init-drive">Drive reporting loaded and ready</label><select class="select" id="tape-init-drive" name="drive_id" required>' + tapeInitializeDriveOptions(initialDriveId).join("") + '</select></div><div class="tape-init-drive-state" id="tape-init-drive-state"></div><label class="check-option danger-check" id="tape-init-trust-wrap" hidden><input type="checkbox" name="trust_drive_identity"> Trust this exact stable drive identity and observed non-rewinding device path.</label></section>',
      '<section class="tape-init-section"><div class="tape-init-section-head"><span>2</span><div><strong>Scratch cartridge inventory</strong><small>The selected label is an operator assertion because the standalone drive has no barcode reader.</small></div></div><div class="form-field"><label for="tape-init-media-mode">Cartridge record</label><select class="select" id="tape-init-media-mode" name="media_mode"><option value="existing"' + (initialMode === "existing" ? " selected" : "") + '>Use a registered eligible cartridge</option><option value="new"' + (initialMode === "new" ? " selected" : "") + '>Register a new non-WORM cartridge</option></select></div><div id="tape-init-existing"><div class="form-field"><label for="tape-init-cartridge">Registered scratch cartridge</label><select class="select" id="tape-init-cartridge" name="cartridge_id">' + tapeInitializeMediaOptions(initialMediaId, initialDriveId).join("") + '</select></div></div><div class="tape-init-new" id="tape-init-new" hidden><div class="form-grid"><div class="form-field full"><label for="tape-init-label">New internal label</label><input class="input input-mono" id="tape-init-label" name="internal_label" maxlength="128" pattern="[-A-Za-z0-9._:]+" placeholder="SCRATCH001"><small>Letters, numbers, hyphen, underscore, period, and colon.</small></div><div class="form-field"><label for="tape-init-barcode">Barcode</label><input class="input input-mono" id="tape-init-barcode" name="barcode" maxlength="128" placeholder="Optional"></div><div class="form-field"><label for="tape-init-generation">Media generation</label><select class="select" id="tape-init-generation" name="media_generation">' + generations.join("") + '</select></div></div><div class="tape-init-nonworm"><span aria-hidden="true">✓</span><div><strong>Non-WORM only</strong><small>This workflow always registers the new cartridge with WORM disabled.</small></div></div></div></section>',
      '<section class="tape-init-section destructive"><div class="tape-init-section-head"><span>3</span><div><strong>Confirm the physical tape and erase it</strong><small>Every existing byte on the loaded tape will be lost.</small></div></div><div class="form-field"><label class="check-option danger-check"><input type="checkbox" name="scratch_media_confirmed" required> I physically verified that the selected label matches the tape loaded in this standalone drive and that it is designated scratch.</label></div><div class="form-field"><label class="check-option danger-check"><input type="checkbox" name="destructive_overwrite_confirmed" required> I authorize TapeScale to erase, move, write, and read back this tape during setup.</label></div><div class="form-field"><label for="tape-init-confirmation">Type the exact erase confirmation</label><input class="input input-mono" id="tape-init-confirmation" name="confirmation" autocomplete="off" spellcheck="false" required><small id="tape-init-confirmation-help">Select or enter a cartridge label first.</small></div></section>',
      '<div class="tape-init-progress" id="tape-init-progress" role="status" aria-live="polite" hidden></div>',
      '<p class="notice danger" id="tape-init-error" hidden></p>',
      '<div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-danger" type="submit"' + (safeDrives.length ? "" : " disabled") + '>Erase and prepare tape</button></div>',
      '</form>'
    ].join("");
    openModal("Prepare tape", body);
    const modal = modalLayer.querySelector(".modal");
    if (modal) modal.classList.add("tape-init-modal");
    updateTapeInitializeForm(document.querySelector("#tape-initialize-form"));
    return true;
  }

  function tapeInitializeLabel(form) {
    if (!form) return "";
    const mode = form.elements.media_mode.value;
    if (mode === "new") return String(form.elements.internal_label.value || "").trim();
    const select = form.elements.cartridge_id;
    const option = select && select.options[select.selectedIndex];
    return String(option && option.dataset.label || "");
  }

  function resetTapeInitializeAssertions(form, resetDriveTrust) {
    if (!form) return;
    if (resetDriveTrust && form.elements.trust_drive_identity) form.elements.trust_drive_identity.checked = false;
    if (form.elements.scratch_media_confirmed) form.elements.scratch_media_confirmed.checked = false;
    if (form.elements.destructive_overwrite_confirmed) form.elements.destructive_overwrite_confirmed.checked = false;
    if (form.elements.confirmation) form.elements.confirmation.value = "";
  }

  function updateTapeInitializeForm(form) {
    if (!form) return;
    let mode = form.elements.media_mode.value;
    const existing = form.querySelector("#tape-init-existing");
    const fresh = form.querySelector("#tape-init-new");
    const existingSelect = form.elements.cartridge_id;
    const driveId = String(form.elements.drive_id.value || "");
    if (form.dataset.driveId !== undefined && form.dataset.driveId !== driveId) {
      const active = activeQualificationForDrive(driveId);
      const choices = tapeInitializeMedia(driveId);
      const preferred = active ? String(pick(active, "cartridge_id") || "") : choices.length === 1 ? recordId(choices[0]) : "";
      existingSelect.innerHTML = tapeInitializeMediaOptions(preferred, driveId).join("");
      if (active) {
        form.elements.media_mode.value = "existing";
        mode = "existing";
      }
    }
    form.dataset.driveId = driveId;
    const newControls = [form.elements.internal_label, form.elements.barcode, form.elements.media_generation].filter(Boolean);
    existing.hidden = mode !== "existing";
    fresh.hidden = mode !== "new";
    existingSelect.disabled = mode !== "existing";
    existingSelect.required = mode === "existing";
    newControls.forEach((control) => { control.disabled = mode !== "new"; });
    form.elements.internal_label.required = mode === "new";
    form.elements.media_generation.required = mode === "new";
    const drive = state.data.drives.find((item) => recordId(item) === driveId);
    const driveGeneration = Number(pick(drive, "generation", "media_generation"));
    if (mode === "new" && !form.elements.media_generation.value && Number.isInteger(driveGeneration) && driveGeneration >= 1 && driveGeneration <= 10) {
      form.elements.media_generation.value = String(driveGeneration);
    }
    const driveState = form.querySelector("#tape-init-drive-state");
    const trustWrap = form.querySelector("#tape-init-trust-wrap");
    const trust = form.elements.trust_drive_identity;
    const blocker = tapeInitializeDriveBlocker(drive);
    const active = drive && activeQualificationForDrive(recordId(drive));
    const stableId = drive && pick(drive, "stable_id");
    if (driveState) {
      driveState.className = "tape-init-drive-state " + (blocker && !active ? "blocked" : active ? "active" : "ready");
      driveState.innerHTML = drive ? '<span aria-hidden="true">' + (blocker && !active ? "!" : active ? "▶" : "✓") + '</span><div><strong>' + esc(blocker && !active ? blocker : active ? "An exact qualification may already be active" : "Drive is loaded and motion-safe") + '</strong><small>' + esc(label(stableId)) + " · " + esc(label(pick(drive, "observed_path"))) + "</small></div>" : '<span aria-hidden="true">·</span><div><strong>Select a loaded drive</strong><small>No hardware is bound yet.</small></div>';
    }
    const needsTrust = Boolean(drive && drive.reconciled !== true && !blocker);
    trustWrap.hidden = !needsTrust;
    trust.disabled = !needsTrust;
    trust.required = needsTrust;
    const labelText = tapeInitializeLabel(form);
    const help = form.querySelector("#tape-init-confirmation-help");
    if (help) help.textContent = labelText ? "Required: DESTROY " + labelText + " WITH BACULA BTAPE TEST" : "Select or enter a cartridge label first.";
  }

  function setTapeInitializeProgress(form, step, detail) {
    const progress = form.querySelector("#tape-init-progress");
    progress.hidden = false;
    progress.innerHTML = '<span class="spinner" aria-hidden="true"></span><div><strong>' + esc(step) + '</strong><small>' + esc(detail) + "</small></div>";
  }

  function tapeInitializeEligibleRecord(labelText, generation, barcode) {
    const record = state.data.media.find((item) => String(mediaName(item)) === String(labelText));
    if (!record) return null;
    const expectedBarcode = String(barcode || "").trim();
    const recordedBarcode = String(pick(record, "barcode") || "").trim();
    if (record.worm === true || expectedBarcode !== recordedBarcode || (generation && Number(pick(record, "media_generation", "generation")) !== Number(generation)) || !qualificationMedia().some((item) => recordId(item) === recordId(record))) return null;
    return record;
  }

  async function submitTapeInitialization(form) {
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#tape-init-error");
    const driveId = String(form.elements.drive_id.value || "");
    let drive = state.data.drives.find((item) => recordId(item) === driveId);
    const expectedStableId = String(pick(drive, "stable_id") || "");
    const labelText = tapeInitializeLabel(form);
    const expectedConfirmation = "DESTROY " + labelText + " WITH BACULA BTAPE TEST";
    const initialBlocker = tapeInitializeDriveBlocker(drive);
    const activeAtSubmit = activeQualificationForDrive(driveId);
    const selectedExistingCartridgeId = form.elements.media_mode.value === "existing" ? String(form.elements.cartridge_id.value || "") : "";
    const resumeRunId = activeAtSubmit
      && String(pick(activeAtSubmit, "cartridge_id")) === selectedExistingCartridgeId
      && String(pick(activeAtSubmit, "expected_cartridge_label", "cartridge_label")) === labelText
      ? recordId(activeAtSubmit)
      : null;
    if (!drive || (initialBlocker && !activeQualificationForDrive(driveId))) {
      errorBox.textContent = initialBlocker || "Select a valid loaded drive.";
      errorBox.hidden = false;
      return;
    }
    if (!labelText || String(form.elements.confirmation.value || "") !== expectedConfirmation) {
      errorBox.textContent = "Type the exact destructive confirmation shown above.";
      errorBox.hidden = false;
      return;
    }
    if (activeAtSubmit && !resumeRunId) {
      errorBox.textContent = "This drive already has a different destructive qualification in progress. Resume its exact cartridge or wait for it to finish.";
      errorBox.hidden = false;
      return;
    }
    submit.disabled = true;
    submit.textContent = "Preparing tape…";
    errorBox.hidden = true;
    try {
      if (drive.reconciled !== true) {
        if (!form.elements.trust_drive_identity.checked) throw new Error("Confirm trust in the exact drive identity before continuing.");
        setTapeInitializeProgress(form, "Trusting exact drive identity", "Any later path, firmware, or stable-identity change invalidates this trust.");
        try {
          await request("/drives/" + encodeURIComponent(driveId) + "/reconcile", {
            method: "POST",
            headers: { Accept: "application/json", "Content-Type": "application/json" },
            body: JSON.stringify({ stable_id: expectedStableId })
          });
        } catch (error) {
          await loadOne("drives");
          drive = state.data.drives.find((item) => recordId(item) === driveId);
          if (!drive || drive.reconciled !== true) throw error;
        }
        await loadOne("drives");
        drive = state.data.drives.find((item) => recordId(item) === driveId);
        if (!drive || drive.reconciled !== true || String(pick(drive, "stable_id") || "") !== expectedStableId) {
          throw new Error("The exact drive identity did not remain reconciled.");
        }
      }

      let cartridge;
      if (form.elements.media_mode.value === "new") {
        const generation = Number(form.elements.media_generation.value);
        const barcode = String(form.elements.barcode.value || "").trim();
        setTapeInitializeProgress(form, "Registering non-WORM scratch tape", "The inventory label is saved before the destructive setup test begins.");
        await loadOne("media");
        cartridge = tapeInitializeEligibleRecord(labelText, generation, barcode);
        if (!cartridge) {
          try {
            const response = await request(endpoints.media, {
              method: "POST",
              headers: { Accept: "application/json", "Content-Type": "application/json" },
              body: JSON.stringify({
                internal_label: labelText,
                barcode: barcode || null,
                media_generation: generation,
                media_format: "LTO-" + generation,
                worm: false
              })
            });
            cartridge = response && (response.media || response);
          } catch (error) {
            await loadOne("media");
            cartridge = tapeInitializeEligibleRecord(labelText, generation, barcode);
            if (!cartridge) throw error;
          }
          await loadOne("media");
          cartridge = tapeInitializeEligibleRecord(labelText, generation, barcode);
        }
      } else {
        cartridge = tapeInitializeMedia(driveId).find((item) => recordId(item) === String(form.elements.cartridge_id.value));
      }
      if (!tapeInitializeCartridgeAllowed(cartridge, driveId)) throw new Error("The selected cartridge is no longer eligible scratch media and has no exact active qualification to resume.");

      setTapeInitializeProgress(form, "Checking for an existing exact run", "Response-loss recovery prevents a second destructive qualification from being queued.");
      await Promise.all([loadOne("qualificationRuns"), loadOne("drives"), loadOne("media")]);
      let run = resumeRunId ? state.data.qualificationRuns.find((item) => recordId(item) === resumeRunId) : exactQualificationRun(driveId, recordId(cartridge), labelText);
      if (resumeRunId && !run) {
        try {
          const detail = await request(endpoints.qualificationRuns + "/" + encodeURIComponent(resumeRunId));
          run = detail && (detail.run || detail);
        } catch (_) {
          throw new Error("The exact qualification being resumed could not be reloaded. No replacement destructive run was queued.");
        }
      }
      const otherActive = activeQualificationForDrive(driveId);
      if (!run && otherActive) throw new Error("This drive already has a different destructive qualification in progress.");
      if (run) clearTapeInitializeIdempotency(form, driveId, recordId(cartridge));
      if (!run) {
        const stableId = String(pick(drive, "stable_id"));
        const idempotencyKey = tapeInitializeIdempotencyKey(form, driveId, stableId, recordId(cartridge), labelText);
        setTapeInitializeProgress(form, "Starting tape setup test", "The worker will write, move, read back, and rewind the tape before it becomes backup-ready.");
        try {
          const response = await request(endpoints.qualificationRuns, {
            method: "POST",
            headers: { Accept: "application/json", "Content-Type": "application/json" },
            body: JSON.stringify({
              drive_id: driveId,
              stable_id: stableId,
              cartridge_id: recordId(cartridge),
              cartridge_label: labelText,
              confirmation: expectedConfirmation,
              scratch_media_confirmed: form.elements.scratch_media_confirmed.checked,
              destructive_overwrite_confirmed: form.elements.destructive_overwrite_confirmed.checked,
              idempotency_key: idempotencyKey
            })
          });
          run = response && response.run;
          if (!run) throw new Error("The durable qualification response did not include a run.");
          clearTapeInitializeIdempotency(form, driveId, recordId(cartridge));
        } catch (error) {
          await loadOne("qualificationRuns");
          run = exactQualificationRun(driveId, recordId(cartridge), labelText);
          if (run) clearTapeInitializeIdempotency(form, driveId, recordId(cartridge));
          if (!run) throw error;
        }
      }
      if (!run) throw new Error("The durable qualification response did not include a run.");
      rememberQualificationRun(run);
      closeModal(true);
      state.settingsTab = "qualification";
      if (location.hash !== "#settings") location.hash = "#settings";
      updateChrome();
      render();
      scheduleQualificationPoll();
      live.textContent = "Tape setup is showing progress for " + labelText + ".";
      const runState = String(pick(run, "state") || "").toLowerCase();
      const activeRun = ["queued", "running"].includes(runState);
      toast(runState === "passed" ? "Tape is ready" : activeRun ? "Tape setup started" : "Tape setup did not pass", runState === "passed" ? "This exact drive and tape already passed setup; no duplicate destructive test was created." : activeRun ? "Keep the worker and tape drive online while setup finishes." : "Opening the retained result. No replacement destructive test was queued.");
      if (runState === "passed") await resumePendingBackupSetupIfReady();
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      const progress = form.querySelector("#tape-init-progress");
      if (progress) progress.hidden = true;
      submit.disabled = false;
      submit.textContent = "Erase and prepare tape";
    }
  }

  function qualificationRunDetail(run) {
    const result = run && run.result && typeof run.result === "object" ? run.result : {};
    const evidence = result.evidence && typeof result.evidence === "object" ? result.evidence : {};
    return pick(result, "detail") || pick(evidence, "detail");
  }

  function qualificationRunNotice(run) {
    if (!run) return "";
    const runState = String(pick(run, "state") || "queued").toLowerCase();
    const tone = runState === "passed" ? " success" : ["failed", "blocked"].includes(runState) ? " danger" : runState === "interrupted" ? " warning" : " info";
    const titles = {
      queued: "Bacula qualification queued",
      running: "Bacula qualification is running",
      passed: "Bacula qualification passed",
      failed: "Bacula qualification failed",
      blocked: "Bacula qualification was blocked",
      interrupted: "Bacula qualification was interrupted"
    };
    const cartridge = label(pick(run, "expected_cartridge_label"));
    const detail = qualificationRunDetail(run) || (runState === "queued"
      ? "The worker has not claimed this destructive run yet."
      : runState === "running"
        ? "Keep the worker and tape drive online. TapeScale is retaining evidence and will finish with a rewind-to-BOT check."
        : "Open the run record below for retained evidence.");
    return '<div class="notice' + tone + ' qualification-run-notice"><span class="notice-icon" aria-hidden="true">' + (runState === "passed" ? "✓" : runState === "running" ? "▶" : "!") + '</span><div class="notice-copy"><strong>' + esc(titles[runState] || "Bacula qualification " + human(runState)) + '</strong><span>Scratch cartridge ' + esc(cartridge) + " · " + esc(detail) + '</span></div><button class="button button-small button-quiet" type="button" data-action="open-record" data-kind="qualificationRuns" data-id="' + esc(recordId(run)) + '">View evidence</button></div>';
  }

  function rememberQualificationRun(run) {
    if (!run || typeof run !== "object" || !recordId(run)) return;
    state.data.qualificationRuns = [run].concat(state.data.qualificationRuns.filter((item) => recordId(item) !== recordId(run)));
    state.received.add("qualificationRuns");
    delete state.errors.qualificationRuns;
  }

  function openQualification(button) {
    const driveId = button.dataset.id;
    const stableId = button.dataset.stableId;
    const driveLabel = button.dataset.driveLabel || stableId;
    const media = qualificationMedia();
    if (!driveId || !stableId || !media.length) return;
    const options = ['<option value="">Select the loaded scratch cartridge</option>'].concat(media.map((item) => '<option value="' + esc(recordId(item)) + '" data-label="' + esc(mediaName(item)) + '">' + esc(mediaName(item)) + (pick(item, "barcode") ? " · barcode " + esc(pick(item, "barcode")) : "") + " · " + esc(label(pick(item, "media_format", "media_generation"))) + "</option>"));
    const body = '<form id="qualification-form" data-drive-id="' + esc(driveId) + '" data-stable-id="' + esc(stableId) + '"><div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>This destroys the selected tape</strong><span>Bacula btape writes, rewinds, spaces, appends, and re-reads test records. Every existing byte on the loaded cartridge must be treated as lost.</span></div></div><div class="form-grid"><div class="form-field full"><label>Exact trusted drive</label><div class="static-field"><strong>' + esc(driveLabel) + "</strong><small>" + esc(stableId) + '</small></div></div><div class="form-field full"><label for="qualification-cartridge">Loaded scratch cartridge</label><select class="select" id="qualification-cartridge" name="cartridge_id" required>' + options.join("") + '</select><small>The standalone drive cannot verify this inventory label; you are asserting that this exact registered cartridge is physically loaded.</small></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="scratch_media_confirmed" required> I physically verified the loaded cartridge is designated scratch media.</label></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="destructive_overwrite_confirmed" required> I authorize complete destruction of all existing data on that cartridge.</label></div><div class="form-field full"><label for="qualification-confirmation">Type the exact destructive confirmation</label><input class="input input-mono" id="qualification-confirmation" name="confirmation" autocomplete="off" spellcheck="false" required><small id="qualification-confirmation-help">Select a cartridge to display the required phrase.</small></div></div><p class="notice danger" id="qualification-form-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-danger" type="submit">Destroy tape and run btape</button></div></form>';
    openModal("Run Bacula tape qualification", body);
    document.querySelector("#qualification-cartridge").focus();
  }

  function updateQualificationConfirmation(select) {
    const option = select.options[select.selectedIndex];
    const cartridgeLabel = option && option.dataset.label;
    const form = select.form;
    const cartridgeId = String(select.value || "");
    if (form && form.dataset.qualificationCartridgeId !== cartridgeId) {
      delete form.dataset.idempotencyKey;
      delete form.dataset.idempotencyScope;
      if (form.elements.scratch_media_confirmed) form.elements.scratch_media_confirmed.checked = false;
      if (form.elements.destructive_overwrite_confirmed) form.elements.destructive_overwrite_confirmed.checked = false;
      if (form.elements.confirmation) form.elements.confirmation.value = "";
      form.dataset.qualificationCartridgeId = cartridgeId;
    }
    const help = document.querySelector("#qualification-confirmation-help");
    if (help) help.textContent = cartridgeLabel ? "Required: DESTROY " + cartridgeLabel + " WITH BACULA BTAPE TEST" : "Select a cartridge to display the required phrase.";
  }

  async function submitQualification(form) {
    const submit = form.querySelector('[type="submit"]');
    const errorBox = form.querySelector("#qualification-form-error");
    const values = new FormData(form);
    const select = form.querySelector('[name="cartridge_id"]');
    const option = select.options[select.selectedIndex];
    const cartridgeLabel = option && option.dataset.label;
    if (!cartridgeLabel) return;
    submit.disabled = true;
    submit.textContent = "Queueing destructive test…";
    errorBox.hidden = true;
    try {
      const cartridgeId = String(values.get("cartridge_id"));
      const idempotencyKey = tapeInitializeIdempotencyKey(form, form.dataset.driveId, form.dataset.stableId, cartridgeId, cartridgeLabel);
      const response = await request(endpoints.qualificationRuns, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          drive_id: form.dataset.driveId,
          stable_id: form.dataset.stableId,
          cartridge_id: cartridgeId,
          cartridge_label: cartridgeLabel,
          confirmation: String(values.get("confirmation") || ""),
          scratch_media_confirmed: values.get("scratch_media_confirmed") === "on",
          destructive_overwrite_confirmed: values.get("destructive_overwrite_confirmed") === "on",
          idempotency_key: idempotencyKey
        })
      });
      const queuedRun = response && response.run;
      if (!queuedRun) throw new Error("The durable qualification response did not include a run.");
      clearTapeInitializeIdempotency(form, form.dataset.driveId, cartridgeId);
      rememberQualificationRun(queuedRun);
      closeModal();
      state.settingsTab = "qualification";
      if (location.hash !== "#settings") location.hash = "#settings";
      updateChrome();
      render();
      scheduleQualificationPoll();
      const queuedState = String(pick(queuedRun, "state") || "").toLowerCase();
      live.textContent = "Bacula qualification " + human(queuedState || "queued") + " for " + cartridgeLabel + ". Opening durable run status.";
      toast(queuedState === "passed" ? "Bacula qualification already passed" : ["failed", "blocked", "interrupted"].includes(queuedState) ? "Bacula qualification did not pass" : "Bacula qualification queued", response && response.idempotent_replay === true ? "The server returned the original durable request; no duplicate destructive run was created." : "Opening the durable run status. Keep the worker and tape drive online.");
      await Promise.all([loadOne("qualificationRuns"), loadOne("drives"), loadOne("media")]);
      updateChrome();
      render();
      scheduleQualificationPoll();
    } catch (error) {
      errorBox.textContent = errorMessage(error);
      errorBox.hidden = false;
      submit.disabled = false;
      submit.textContent = "Destroy tape and run btape";
    }
  }

  function scheduleQualificationPoll() {
    if (state.qualificationPollTimer) window.clearTimeout(state.qualificationPollTimer);
    const active = state.data.qualificationRuns.some((run) => ["queued", "running"].includes(String(pick(run, "state"))));
    if (!active) {
      state.qualificationPollTimer = null;
      return;
    }
    state.qualificationPollTimer = window.setTimeout(async () => {
      await Promise.all([loadOne("qualificationRuns"), loadOne("drives"), loadOne("media")]);
      if (await resumePendingBackupSetupIfReady()) return;
      updateChrome();
      render();
      scheduleQualificationPoll();
    }, 5000);
  }

  async function reconcileDrive(button) {
    const id = button.dataset.id;
    const stableId = button.dataset.stableId;
    if (!id || !stableId || !window.confirm("Trust this exact SAS drive identity? A path, firmware, or identity change will require reconciliation again.")) return;
    button.disabled = true;
    button.textContent = "Checking…";
    try {
      await request("/drives/" + encodeURIComponent(id) + "/reconcile", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({ stable_id: stableId })
      });
      await Promise.all([loadOne("drives"), loadOne("preflight")]);
      updateChrome();
      render();
      toast("Drive identity trusted", "TapeScale will block the drive again if its path, firmware, or stable identity changes.");
    } catch (error) {
      button.disabled = false;
      button.textContent = "Trust identity";
      toast("Drive reconciliation failed", errorMessage(error));
    }
  }

  async function queueDriveMotion(button) {
    const id = button.dataset.id;
    const stableId = button.dataset.stableId;
    const action = String(button.dataset.motion || "").toLowerCase();
    const driveLabel = button.dataset.driveLabel || stableId;
    if (!id || !stableId || !["rewind", "unload"].includes(action)) return;
    const description = action === "unload"
      ? "UNLOAD will rewind and put the loaded tape offline in " + driveLabel + "; the drive may physically eject it."
      : "REWIND will physically move the loaded tape to beginning-of-tape in " + driveLabel + ".";
    const warning = " Linux may finalize a prior write by writing a filemark before this motion. Continue only with a designated scratch cartridge. TapeScale issues no data-write command, but cannot promise unchanged media.";
    if (!window.confirm(description + warning + " Continue?")) return;
    button.disabled = true;
    button.textContent = "Queueing…";
    try {
      await request("/drives/" + encodeURIComponent(id) + "/actions", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          stable_id: stableId,
          action,
          confirmation: action.toUpperCase() + " SCRATCH MEDIA; PRIOR WRITE MAY BE FINALIZED",
          scratch_media_confirmed: true
        })
      });
      await loadOne("drives");
      updateChrome();
      render();
      toast(human(action) + " queued", "The worker will recheck the exact drive identity and execute this attended scratch-media motion request.");
      window.setTimeout(async () => {
        await loadOne("drives");
        updateChrome();
        render();
      }, 6500);
    } catch (error) {
      button.disabled = false;
      button.textContent = human(action);
      toast(human(action) + " was not queued", errorMessage(error));
    }
  }

  function toast(title, message) {
    const region = document.querySelector("#toast-region");
    const element = document.createElement("div");
    element.className = "toast";
    element.innerHTML = '<span aria-hidden="true">✓</span><div><strong>' + esc(title) + '</strong><span>' + esc(message) + '</span></div><button type="button" data-action="close-toast" aria-label="Dismiss">&times;</button>';
    region.append(element);
    window.setTimeout(() => element.remove(), 5000);
  }

  function showActions() {
    const overview = state.data.overview;
    const actions = overview.actions;
    const boundedWarning = overview.actions_truncated
      ? '<div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Showing the first ' + esc(actions.length) + " of " + esc(overview.actions_total) + ' required actions</strong><span>The total includes additional actions outside this bounded list.</span></div></div>'
      : "";
    const body = actions.length ? '<ul class="action-list">' + actions.map(renderActionItem).join("") + "</ul>" + boundedWarning : '<p>No unresolved actions are reported.</p>';
    openModal("Notifications", body);
  }

  function showPreflight() {
    openModal("Preflight", state.data.preflight ? renderPreflight() : "<p>No preflight result is available.</p>");
  }

  document.addEventListener("click", (event) => {
    const target = event.target.closest("[data-action]");
    if (!target) return;
    const action = target.dataset.action;
    if (action === "open-nav") shell.classList.add("nav-open");
    if (action === "close-nav") shell.classList.remove("nav-open");
    if (action === "refresh") loadAll(false);
    if (action === "create-backup-profile") openBackupProfile();
    if (action === "open-source-browser") openSourceBrowser(target.dataset.browserTarget || "backup");
    if (action === "close-source-browser") closeSourceBrowser();
    if (action === "source-browser-navigate") loadSourceBrowser(target.dataset.path || "", true);
    if (action === "select-source-directory") selectSourceDirectory();
    if (action === "choose-backup-snapshot") {
      closeModal();
      openBackupProfile();
    }
    if (action === "start-backup") openStartBackup(target.dataset.profileId);
    if (action === "select-backup-run") selectBackupRun(target.dataset.id);
    if (action === "retry-backup-run") loadBackupRunDetail(target.dataset.id, true);
    if (action === "assign-backup-media") openAssignBackupMedia(target.dataset.id);
    if (action === "prepare-backup-tape") prepareBackupTape(target);
    if (action === "refresh-backup-readiness") refreshBackupReadiness(target);
    if (action === "cancel-backup-run") openCancelBackupRun(target.dataset.id);
    if (action === "add-media") openAddMedia();
    if (action === "initialize-tape") openTapeInitialization(target);
    if (action === "create-media-pool") openMediaPoolForm();
    if (action === "select-media") selectMedia(target.dataset.id);
    if (action === "retry-media-detail") loadMediaDetail(target.dataset.id, true);
    if (action === "assign-media-pool") openMediaPoolAssignment(target.dataset.id);
    if (action === "add-media-hold") openMediaHoldForm(target.dataset.id);
    if (action === "release-media-hold") openMediaHoldRelease(target.dataset.mediaId, target.dataset.holdId);
    if (action === "open-media-reuse") openMediaReuseEvaluation(target.dataset.id);
    if (action === "open-recovery-profile") openRecoveryProfile();
    if (action === "open-recovery-run") openRecoveryRun(target.dataset.profileId || null);
    if (action === "select-recovery-exercise") selectRecoveryExercise(target.dataset.id);
    if (action === "select-recovery-point") selectRecoveryPoint(target.dataset.id);
    if (action === "browse-backup-files") browseBackupFiles(target.dataset.id);
    if (action === "add-location") openLocationForm();
    if (action === "plan-movement") openMovementForm();
    if (action === "select-movement") selectMovement(target.dataset.id);
    if (action === "retry-movement") loadMovementDetail(target.dataset.id, true);
    if (action === "confirm-custody") openCustodyConfirmation(target);
    if (action === "cancel-movement") openCancelMovement(target.dataset.id);
    if (action === "correct-custody") openCustodyCorrection(target);
    if (action === "print-movement-manifest") printMovementManifest();
    if (action === "retry-recovery-point") loadRecoveryPointDetail(target.dataset.id, true);
    if (action === "open-recovery-files") openRecoveryPointFiles(target.dataset.id);
    if (action === "close-recovery-files") closeRecoveryPointFiles(target.dataset.id);
    if (action === "recovery-files-navigate") loadRecoveryPointFiles(target.dataset.id, target.dataset.path || "", true);
    if (action === "download-recovery-evidence") downloadRecoveryEvidence(target);
    if (action === "open-restore-plan" || action === "revise-restore-plan") openRestorePlan(target.dataset.id);
    if (action === "open-restore-rehearsal") openRestoreRehearsal(target.dataset.planId);
    if (action === "select-restore-rehearsal") selectRestoreRehearsal(target.dataset.id);
    if (action === "open-restore-media-handoff") openRestoreMediaHandoff(target.dataset.id);
    if (action === "open-cancel-restore-rehearsal") openCancelRestoreRehearsal(target.dataset.id);
    if (action === "download-restore-rehearsal-evidence") downloadRestoreRehearsalEvidence(target);
    if (action === "open-qualification") openQualification(target);
    if (action === "reconcile-drive") reconcileDrive(target);
    if (action === "drive-motion") queueDriveMotion(target);
    if (action === "close-modal") closeModal();
    if (action === "close-drawer") closeDrawer();
    if (action === "open-record") openRecord(target.dataset.kind, target.dataset.id);
    if (action === "settings-tab") {
      state.settingsTab = target.dataset.tab;
      render();
    }
    if (action === "notifications") showActions();
    if (action === "show-preflight") showPreflight();
    if (action === "site-menu") openModal("Protection site", "<p>" + esc(document.querySelector("#site-name").textContent) + "</p>");
    if (action === "user-menu") openModal("Local console access", '<div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>No application sign-in</strong><span>TapeScale Alpha does not provide application authentication for this local console. Restrict it to a trusted management network or place it behind an authenticating reverse proxy.</span></div></div>');
    if (action === "close-toast") target.closest(".toast").remove();
  });

  document.addEventListener("submit", (event) => {
    if (event.target.id === "tape-initialize-form") {
      event.preventDefault();
      submitTapeInitialization(event.target);
    }
    if (event.target.id === "backup-profile-form") {
      event.preventDefault();
      submitBackupProfile(event.target);
    }
    if (event.target.id === "start-backup-form") {
      event.preventDefault();
      submitStartBackup(event.target);
    }
    if (event.target.id === "backup-media-form") {
      event.preventDefault();
      submitBackupMedia(event.target);
    }
    if (event.target.id === "cancel-backup-form") {
      event.preventDefault();
      submitCancelBackup(event.target);
    }
    if (event.target.id === "recovery-profile-form") {
      event.preventDefault();
      submitRecoveryProfile(event.target);
    }
    if (event.target.id === "recovery-exercise-form") {
      event.preventDefault();
      submitRecoveryExercise(event.target);
    }
    if (event.target.id === "add-media-form") {
      event.preventDefault();
      submitMedia(event.target);
    }
    if (event.target.id === "media-pool-form") {
      event.preventDefault();
      submitMediaPool(event.target);
    }
    if (event.target.id === "media-pool-assignment-form") {
      event.preventDefault();
      submitMediaPoolAssignment(event.target);
    }
    if (event.target.id === "media-hold-form") {
      event.preventDefault();
      submitMediaHold(event.target);
    }
    if (event.target.id === "media-hold-release-form") {
      event.preventDefault();
      submitMediaHoldRelease(event.target);
    }
    if (event.target.id === "media-reuse-form") {
      event.preventDefault();
      submitMediaReuseEvaluation(event.target);
    }
    if (event.target.id === "qualification-form") {
      event.preventDefault();
      submitQualification(event.target);
    }
    if (event.target.id === "restore-plan-form") {
      event.preventDefault();
      submitRestorePlan(event.target);
    }
    if (event.target.id === "restore-rehearsal-form") {
      event.preventDefault();
      submitRestoreRehearsal(event.target);
    }
    if (event.target.id === "restore-media-handoff-form") {
      event.preventDefault();
      submitRestoreMediaHandoff(event.target);
    }
    if (event.target.id === "cancel-restore-rehearsal-form") {
      event.preventDefault();
      submitCancelRestoreRehearsal(event.target);
    }
    if (event.target.id === "location-form") {
      event.preventDefault();
      submitLocation(event.target);
    }
    if (event.target.id === "movement-form") {
      event.preventDefault();
      submitMovement(event.target);
    }
    if (event.target.id === "custody-confirm-form") {
      event.preventDefault();
      submitCustodyConfirmation(event.target);
    }
    if (event.target.id === "cancel-movement-form") {
      event.preventDefault();
      submitCancelMovement(event.target);
    }
    if (event.target.id === "custody-correction-form") {
      event.preventDefault();
      submitCustodyCorrection(event.target);
    }
  });

  document.addEventListener("input", (event) => {
    if (["tape-init-label", "tape-init-barcode"].includes(event.target.id)) {
      resetTapeInitializeAssertions(event.target.form, false);
      updateTapeInitializeForm(event.target.form);
    }
    if (["backup-source-subpath", "recovery-source-subpath"].includes(event.target.id)) {
      const summaryId = event.target.id === "recovery-source-subpath" ? "recovery-source-selection" : "backup-source-selection";
      const summary = document.getElementById(summaryId);
      if (summary && event.target.value.trim() !== sourceBrowserPath) summary.hidden = true;
    }
    if (event.target.id === "backup-search") {
      state.backupQuery = event.target.value;
      applyBackupFilters();
    }
    if (event.target.id === "vault-search") {
      state.recoveryPointQuery = event.target.value;
      applyRecoveryPointFilters();
    }
    if (event.target.id === "media-search") {
      state.mediaQuery = event.target.value;
      applyMediaFilters();
    }
  });

  document.addEventListener("change", (event) => {
    if (["tape-init-drive", "tape-init-media-mode", "tape-init-cartridge", "tape-init-generation"].includes(event.target.id)) {
      resetTapeInitializeAssertions(event.target.form, event.target.id === "tape-init-drive");
      updateTapeInitializeForm(event.target.form);
    }
    if (event.target.id === "backup-media-drive") updateBackupMediaForDrive(event.target.form);
    if (event.target.id === "backup-media-cartridge") updateBackupMediaConfirmation(event.target.form);
    if (event.target.id === "start-backup-drive") updateStartBackupTape(event.target.form);
    if (event.target.id === "start-backup-cartridge") updateStartBackupTape(event.target.form);
    if (["backup-compression", "backup-media-spanning"].includes(event.target.id)) updateBackupProfileOptions(event.target.form);
    if (event.target.id === "backup-state-filter") {
      state.backupStateFilter = event.target.value;
      applyBackupFilters();
    }
    if (event.target.id === "qualification-cartridge") updateQualificationConfirmation(event.target);
    if (event.target.id === "media-hold-kind") updateMediaHoldExpiry(event.target.form);
    if (event.target.id === "recovery-run-drive") updateRecoveryRunMediaOptions(event.target.form);
    if (["recovery-run-profile", "recovery-run-cartridge"].includes(event.target.id)) updateRecoveryRunConfirmation(event.target.form);
    if (event.target.id === "vault-status-filter") {
      state.recoveryPointStatus = event.target.value;
      applyRecoveryPointFilters();
    }
    if (event.target.id === "vault-representation-filter") {
      state.recoveryPointRepresentation = event.target.value;
      applyRecoveryPointFilters();
    }
    if (event.target.id === "media-pool-filter") {
      state.mediaPoolFilter = event.target.value;
      applyMediaFilters();
    }
    if (event.target.id === "media-status-filter") {
      state.mediaStatusFilter = event.target.value;
      applyMediaFilters();
    }
    if (event.target.id === "media-location-filter") {
      state.mediaLocationFilter = event.target.value;
      applyMediaFilters();
    }
    if (event.target.id === "media-identity-filter") {
      state.mediaIdentityFilter = event.target.value;
      applyMediaFilters();
    }
    if (event.target.id === "media-health-filter") {
      state.mediaHealthFilter = event.target.value;
      applyMediaFilters();
    }
    if (event.target.id === "media-protection-filter") {
      state.mediaProtectionFilter = event.target.value;
      applyMediaFilters();
    }
    if (event.target.id === "restore-conflict-policy") {
      const warning = document.querySelector("#restore-replace-warning");
      if (warning) warning.hidden = event.target.value !== "replace";
    }
    if (event.target.id === "movement-source") updateMovementMediaChoices(event.target.form);
    if (event.target.name === "movement_cartridge") updateCustodyActionButtons();
    if (event.target.id === "custody-select-all") {
      document.querySelectorAll('[name="movement_cartridge"]:not(:disabled)').forEach((input) => { input.checked = event.target.checked; });
      updateCustodyActionButtons();
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Tab" && !modalLayer.hidden) {
      const focusable = Array.from(modalLayer.querySelectorAll("a[href], button:not([disabled]), input:not([type='hidden']):not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])"))
        .filter((element) => !element.hidden && element.getAttribute("aria-hidden") !== "true" && element.getClientRects().length > 0);
      if (!focusable.length) {
        event.preventDefault();
      } else {
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (event.shiftKey && (document.activeElement === first || !modalLayer.contains(document.activeElement))) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && (document.activeElement === last || !modalLayer.contains(document.activeElement))) {
          event.preventDefault();
          first.focus();
        }
      }
    }
    if (event.key === "Escape") {
      closeModal();
      closeDrawer();
      shell.classList.remove("nav-open");
    }
    if ((event.key === "Enter" || event.key === " ") && event.target.matches("tr[data-action='open-record'], tr[data-action='select-backup-run'], tr[data-action='select-recovery-exercise'], tr[data-action='select-recovery-point']")) event.target.click();
  });

  modalLayer.addEventListener("click", (event) => {
    if (event.target === modalLayer) closeModal();
  });
  drawerLayer.addEventListener("click", (event) => {
    if (event.target === drawerLayer) closeDrawer();
  });
  window.addEventListener("hashchange", () => {
    shell.classList.remove("nav-open");
    closeModal();
    closeDrawer();
    render();
    main.focus();
    if (state.route === "recovery-lab") {
      const exercise = selectedRecoveryExercise();
      const id = recordId(exercise);
      if (id && !state.data.recoveryExerciseDetails[id]) {
        loadRecoveryExerciseDetail(id).then(() => {
          if (state.route === "recovery-lab") {
            updateChrome();
            render();
          }
        });
      }
    }
    if (state.route === "restore") {
      const point = selectedRecoveryPoint();
      const id = recordId(point);
      if (id && !state.data.recoveryPointDetails[id]) loadRecoveryPointDetail(id);
      const rehearsal = selectedRestoreRehearsal();
      const rehearsalId = recordId(rehearsal);
      if (rehearsalId && !state.data.restoreRehearsalDetails[rehearsalId]) loadRestoreRehearsalDetail(rehearsalId);
    }
    if (state.route === "movements") {
      const movement = selectedMovement();
      const id = recordId(movement);
      if (id && !state.data.movementDetails[id]) loadMovementDetail(id);
    }
    if (state.route === "tapes") {
      const media = selectedMedia();
      const id = recordId(media);
      if (id && !state.data.mediaDetails[id]) loadMediaDetail(id);
    }
    if (state.route === "jobs") {
      const run = selectedBackupRun();
      const id = recordId(run);
      if (id && !state.data.backupRunDetails[id]) loadBackupRunDetail(id);
    }
  });

  updateChrome();
  loadAll(true).then(() => {
    live.textContent = "TapeScale data loaded.";
  });
})();
