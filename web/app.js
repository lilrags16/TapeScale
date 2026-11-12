(() => {
  "use strict";

  const API = "/api/v1";
  const endpoints = {
    system: "/system",
    overview: "/overview",
    jobs: "/jobs",
    media: "/media",
    drives: "/drives",
    movements: "/movements",
    verification: "/verification",
    recoveryPoints: "/recovery-points",
    preflight: "/preflight",
    qualification: "/qualification",
    qualificationRuns: "/qualification/runs"
  };
  const routes = new Set(["overview", "jobs", "restore", "movements", "tapes", "drives", "verification", "settings"]);
  const state = {
    route: "overview",
    loading: true,
    refreshing: false,
    settingsTab: "preflight",
    data: {
      system: null,
      overview: { summary: { protected_sources: 0, active_jobs: 0, required_actions: 0 }, sources: [], actions: [] },
      jobs: [],
      media: [],
      drives: [],
      slots: [],
      driveOperations: [],
      movements: [],
      verification: [],
      recoveryPoints: [],
      preflight: null,
      qualification: null,
      qualificationRuns: []
    },
    received: new Set(),
    errors: {},
    qualificationPollTimer: null
  };

  const main = document.querySelector("#main-content");
  const modalLayer = document.querySelector("#modal-layer");
  const drawerLayer = document.querySelector("#drawer-layer");
  const shell = document.querySelector("#app-shell");
  const live = document.querySelector("#live-region");

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
    return String(value).replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
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
      return Object.assign({}, raw, {
        summary: {
          protected_sources: count(pick(summary, "protected_sources", "protectedSources")),
          active_jobs: count(pick(summary, "active_jobs", "activeJobs")),
          required_actions: count(pick(summary, "required_actions", "requiredActions"))
        },
        sources: list(raw, "sources"),
        actions: list(raw, "actions")
      });
    }
    if (key === "jobs") return list(payload, "jobs");
    if (key === "media") return list(payload, "media");
    if (key === "movements") return list(payload, "movements");
    if (key === "verification") return list(payload, "verification");
    if (key === "recoveryPoints") return list(payload, "recovery_points");
    if (key === "qualificationRuns") return list(payload, "runs");
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
    state.loading = false;
    state.refreshing = false;
    updateChrome();
    render();
    scheduleQualificationPoll();
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
    if (site) site.textContent = String(pick(state.data.overview, "site_name", "siteName") || pick(siteObject, "name") || "Not configured");
    const checks = state.data.preflight;
    const preflight = document.querySelector("#preflight-status");
    if (preflight) preflight.textContent = label(pick(checks, "status") || "Setup pending");
    const preflightDot = document.querySelector(".system-pill .health-dot");
    const preflightState = String(pick(checks, "status") || "").toLowerCase();
    if (preflightDot) preflightDot.className = "health-dot" + (/fail|block|error/.test(preflightState) ? " failed" : /warn|attention|pending/.test(preflightState) ? " warning" : "");
    const summary = state.data.overview.summary || {};
    setBadge("#nav-job-count", state.data.jobs.length);
    setBadge("#nav-move-count", state.data.movements.length);
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

  function renderOverview() {
    const overview = state.data.overview;
    const summary = overview.summary || {};
    const ready = state.received.has("overview");
    const metric = (name, value) => '<div class="metric"><span>' + esc(name) + '</span><strong>' + esc(ready ? value : "—") + "</strong></div>";
    let html = pageHead("Overview", "Current protection state reported by TapeScale.", refreshButton()) + noticeFor("overview");
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
      html += '<ul class="action-list">' + overview.actions.map((action) => '<li class="action-item"><span class="action-symbol" aria-hidden="true">!</span><span class="action-copy"><strong>' + esc(label(pick(action, "title", "label", "name"))) + '</strong><span>' + esc(label(pick(action, "detail", "message", "state"))) + "</span></span></li>").join("") + "</ul>";
    } else {
      html += "<p>No actions are currently reported.</p>";
    }
    return html + "</div></section></div>";
  }

  function renderJobs() {
    const jobs = state.data.jobs;
    let html = pageHead("Jobs", "Protection jobs returned by TapeScale.", refreshButton()) + noticeFor("jobs");
    if (!jobs.length) return html + empty("No jobs", state.errors.jobs ? "Jobs could not be loaded." : "No protection jobs have been configured.");
    const rows = jobs.map((job) => {
      const id = recordId(job);
      return '<tr class="row-action" tabindex="0" data-action="open-record" data-kind="jobs" data-id="' + esc(id) + '"><td class="primary-cell"><strong>' + esc(label(pick(job, "external_id", "name", "label", "job_id"))) + '</strong><small>' + esc(label(pick(job, "kind", "job_id"))) + "</small></td>" + cell(pick(job, "dataset_name", "source", "dataset", "path")) + "<td>" + status(pick(job, "state", "status")) + "</td>" + cell(pick(job, "policy_name", "schedule")) + cell(date(pick(job, "started_at", "finished_at", "job_created_at", "last_run", "updated_at"))) + "</tr>";
    });
    return html + table(["Job", "Source", "Status", "Schedule", "Last activity"], rows);
  }

  function mediaName(item) {
    return pick(item, "internal_label", "internalLabel", "label", "id");
  }

  function renderMedia() {
    const media = state.data.media;
    const add = '<button class="button button-primary" type="button" data-action="add-media">Add cartridge</button>';
    let html = pageHead("Tapes & pools", "Cartridge inventory returned by TapeScale.", refreshButton() + add) + noticeFor("media");
    if (!media.length) return html + empty("No cartridges", state.errors.media ? "Cartridge inventory could not be loaded." : "Add a cartridge to begin building inventory.", add);
    const rows = media.map((item) => '<tr class="row-action" tabindex="0" data-action="open-record" data-kind="media" data-id="' + esc(recordId(item)) + '"><td class="primary-cell"><strong class="tape-id">' + esc(label(mediaName(item))) + '</strong><small>' + esc(label(pick(item, "barcode"))) + "</small></td>" + cell(pick(item, "generation", "media_generation")) + cell(pick(item, "media_format", "format")) + cell(pick(item, "pool_name", "pool")) + cell(pick(item, "location_name", "location")) + "<td>" + status(pick(item, "health_status", "health", "status")) + "</td></tr>");
    return html + table(["Cartridge", "Generation", "Format", "Pool", "Location", "Health"], rows);
  }

  function renderRestore() {
    const points = state.data.recoveryPoints;
    let html = pageHead("Restore", "Recovery points returned by TapeScale.", refreshButton()) + noticeFor("recoveryPoints");
    if (!points.length) return html + empty("No recovery points", state.errors.recoveryPoints ? "Recovery points could not be loaded." : "A restore can be planned after a recovery point is available.");
    const rows = points.map((point) => '<tr class="row-action" tabindex="0" data-action="open-record" data-kind="recoveryPoints" data-id="' + esc(recordId(point)) + '"><td class="primary-cell"><strong>' + esc(label(pick(point, "name", "id"))) + '</strong><small>' + esc(label(pick(point, "level", "kind", "representation"))) + "</small></td>" + cell(pick(point, "dataset_name", "source", "dataset")) + cell(date(pick(point, "recovery_time", "time", "created_at"))) + "<td>" + status(pick(point, "verification", "status", "state")) + "</td></tr>");
    return html + table(["Recovery point", "Source", "Created", "Verification"], rows);
  }

  function renderMovements() {
    const movements = state.data.movements;
    let html = pageHead("Movements", "Custody movements returned by TapeScale.", refreshButton()) + noticeFor("movements");
    if (!movements.length) return html + empty("No movements", state.errors.movements ? "Movements could not be loaded." : "No custody movements are pending or recorded.");
    const rows = movements.map((movement) => '<tr class="row-action" tabindex="0" data-action="open-record" data-kind="movements" data-id="' + esc(recordId(movement)) + '"><td class="primary-cell"><strong>' + esc(label(pick(movement, "external_id", "name", "id"))) + '</strong><small>' + esc(label(pick(movement, "kind", "media_label", "tape", "barcode"))) + "</small></td>" + cell(pick(movement, "source_name", "from", "origin", "source")) + cell(pick(movement, "destination_name", "to", "destination")) + "<td>" + status(pick(movement, "state", "status")) + "</td>" + cell(date(pick(movement, "due_at", "updated_at", "created_at", "time"))) + "</tr>");
    return html + table(["Movement", "From", "To", "Status", "Activity"], rows);
  }

  function renderDrives() {
    const motionEnabled = pick(state.data.system, "lab_tape_motion_enabled") === true;
    const qualificationEnabled = pick(state.data.system, "lab_tape_qualification_enabled") === true;
    let html = pageHead("Drives & libraries", "Hardware inventory returned by TapeScale.", refreshButton()) + noticeFor("drives");
    if (qualificationEnabled) {
      html += '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Destructive Bacula qualification enabled</strong><span>The attended btape test overwrites the selected cartridge. It is limited to registered, unprotected, non-WORM scratch media and does not complete the broader G0 product gate.</span></div></div>';
    }
    if (motionEnabled) {
      html += '<div class="notice warning"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Scratch-media lab motion enabled</strong><span>Rewind and unload physically move tape. Linux can finalize a prior write with a filemark before moving it. Use designated scratch media only; this does not qualify the drive for backup.</span></div></div>';
    } else {
      html += '<div class="notice info"><span class="notice-icon" aria-hidden="true">i</span><div class="notice-copy"><strong>Lab tape motion is disabled</strong><span>Rewind and Unload remain visible for discovered drives, but cannot run until the API and worker use TAPESCALE_LAB_TAPE_MOTION_ENABLED=true.</span></div></div>';
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
          : !motionEnabled ? "Lab tape motion is disabled in this deployment."
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
      html += table(["Drive", "Generation / model", "Access", "G0 evidence", "Identity", "Lab motion"], rows);
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
    const rows = items.map((item) => '<tr class="row-action" tabindex="0" data-action="open-record" data-kind="verification" data-id="' + esc(recordId(item)) + '"><td class="primary-cell"><strong>' + esc(label(pick(item, "kind", "name", "id"))) + '</strong><small>' + esc(label(pick(item, "dataset_name", "media_label", "job_name", "source"))) + "</small></td><td>" + status(pick(item, "outcome", "state", "status", "result")) + "</td>" + cell(date(pick(item, "finished_at", "started_at", "completed_at", "updated_at", "time"))) + cell(pick(item, "detail", "message", "drive_stable_id")) + "</tr>");
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
    document.querySelectorAll("[data-route]").forEach((link) => link.classList.toggle("active", link.dataset.route === state.route));
    if (state.loading) {
      main.innerHTML = loading();
      return;
    }
    const renderers = {
      overview: renderOverview,
      jobs: renderJobs,
      restore: renderRestore,
      movements: renderMovements,
      tapes: renderMedia,
      drives: renderDrives,
      verification: renderVerification,
      settings: renderSettings
    };
    main.innerHTML = renderers[state.route]();
    document.title = human(state.route) + " · TapeScale";
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
    modalLayer.innerHTML = '<section class="modal" role="dialog" aria-modal="true" aria-labelledby="modal-title"><div class="modal-head"><h2 id="modal-title">' + esc(title) + '</h2><button class="icon-button" type="button" data-action="close-modal" aria-label="Close">&times;</button></div><div class="modal-body">' + body + '</div>' + (footer ? '<div class="modal-foot">' + footer + "</div>" : "") + "</section>";
    modalLayer.hidden = false;
  }

  function closeModal() {
    modalLayer.hidden = true;
    modalLayer.innerHTML = "";
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

  function qualificationMedia() {
    return state.data.media.filter((item) => item.worm !== true && count(pick(item, "active_holds")) === 0 && pick(item, "has_content") !== true && !pick(item, "reserved_by") && !["foreign", "mismatch"].includes(String(pick(item, "identity_status") || "")));
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
      const response = await request(endpoints.qualificationRuns, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({
          drive_id: form.dataset.driveId,
          stable_id: form.dataset.stableId,
          cartridge_id: String(values.get("cartridge_id")),
          cartridge_label: cartridgeLabel,
          confirmation: String(values.get("confirmation") || ""),
          scratch_media_confirmed: values.get("scratch_media_confirmed") === "on",
          destructive_overwrite_confirmed: values.get("destructive_overwrite_confirmed") === "on"
        })
      });
      const queuedRun = response && response.run;
      rememberQualificationRun(queuedRun);
      closeModal();
      state.settingsTab = "qualification";
      if (location.hash !== "#settings") location.hash = "#settings";
      updateChrome();
      render();
      scheduleQualificationPoll();
      live.textContent = "Bacula qualification queued for " + cartridgeLabel + ". Opening durable run status.";
      toast("Bacula qualification queued", "Opening the durable run status. Keep the worker and tape drive online.");
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
      toast(human(action) + " queued", "The worker will recheck the exact drive identity and execute this lab-only motion request.");
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
    const actions = state.data.overview.actions;
    const body = actions.length ? '<ul class="action-list">' + actions.map((action) => '<li class="action-item"><span class="action-symbol" aria-hidden="true">!</span><span class="action-copy"><strong>' + esc(label(pick(action, "title", "label", "name"))) + '</strong><span>' + esc(label(pick(action, "detail", "message", "state"))) + "</span></span></li>").join("") + "</ul>" : '<p>No unresolved actions are reported.</p>';
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
    if (action === "add-media") openAddMedia();
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
    if (action === "user-menu") openModal("Account status", "<p>Authentication is not configured in this interface.</p>");
    if (action === "close-toast") target.closest(".toast").remove();
  });

  document.addEventListener("submit", (event) => {
    if (event.target.id === "add-media-form") {
      event.preventDefault();
      submitMedia(event.target);
    }
    if (event.target.id === "qualification-form") {
      event.preventDefault();
      submitQualification(event.target);
    }
  });

  document.addEventListener("change", (event) => {
    if (event.target.id === "qualification-cartridge") updateQualificationConfirmation(event.target);
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeModal();
      closeDrawer();
      shell.classList.remove("nav-open");
    }
    if ((event.key === "Enter" || event.key === " ") && event.target.matches("tr[data-action='open-record']")) event.target.click();
  });

  modalLayer.addEventListener("click", (event) => {
    if (event.target === modalLayer) closeModal();
  });
  drawerLayer.addEventListener("click", (event) => {
    if (event.target === drawerLayer) closeDrawer();
  });
  window.addEventListener("hashchange", () => {
    shell.classList.remove("nav-open");
    render();
    main.focus();
  });

  updateChrome();
  loadAll(true).then(() => {
    live.textContent = "TapeScale data loaded.";
  });
})();
