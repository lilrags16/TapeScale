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
    qualificationRuns: "/qualification/runs",
    recoveryProfiles: "/recovery-exercises/profiles",
    recoveryExercises: "/recovery-exercises"
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
      qualificationRuns: [],
      recoveryProfiles: [],
      recoveryExercises: [],
      recoveryExerciseDetails: {},
      recoveryPointDetails: {}
    },
    received: new Set(),
    errors: {},
    qualificationPollTimer: null,
    recoveryExercisePollTimer: null,
    selectedRecoveryExerciseId: null,
    selectedRecoveryPointId: null,
    recoveryPointQuery: "",
    recoveryPointStatus: "all",
    recoveryPointRepresentation: "all",
    recoveryPointDetailLoading: new Set(),
    restorePlanDrafts: {}
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
    if (key === "recoveryProfiles") return list(payload, "profiles");
    if (key === "recoveryExercises") {
      const exercises = list(payload, "exercises");
      return exercises.length ? exercises : list(payload, "runs");
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
    }
    state.loading = false;
    state.refreshing = false;
    updateChrome();
    render();
    scheduleQualificationPoll();
    scheduleRecoveryExercisePoll();
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
    return pick(volume, "media_label", "cartridge_label", "internal_label", "barcode") || pick(cartridge, "internal_label", "label", "barcode", "id") || "Unidentified cartridge";
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
    return '<article class="panel vault-dossier"><header class="vault-dossier-hero"><div><span class="vault-kicker">Recovery point · ' + esc(human(representation)) + '</span><div class="vault-dossier-title"><h2>' + esc(label(dataset)) + '</h2>' + status(recoveryPointState(point)) + '</div><p class="mono">' + esc(id) + '</p><span>' + esc(date(pick(point, "recovery_time", "created_at"))) + " · " + esc(human(pick(point, "level") || "unknown")) + ' backup</span></div><div class="vault-dossier-actions"><button class="button button-quiet" type="button" data-action="download-recovery-evidence" data-id="' + esc(id) + '">Download evidence</button><button class="button button-primary" type="button" data-action="open-restore-plan" data-id="' + esc(id) + '">Plan restore</button></div></header><div class="vault-plan-boundary"><strong>Readiness planning only</strong><span>Planning does not move a tape, start Bacula, create a destination, or restore data.</span></div>' + feedback + '<section class="vault-proof-grid">' + renderVaultProof("Full media readback", fullReadback, verifications.length ? "Recorded independently of write completion" : "Loading verification evidence") + renderVaultProof("Isolated restore test", restoreTest, verifications.length ? "Application recovery evidence" : "Loading verification evidence") + renderVaultProof("Required media", volumes.length ? "recorded" : "missing", volumes.length + " cartridge" + (volumes.length === 1 ? "" : "s") + " in read order") + renderVaultProof("Dependencies", dependencies.length ? "required" : "ready", dependencies.length ? dependencies.length + " prerequisite recovery point(s)" : "No earlier recovery point required") + '</section><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Required media in read order</h3><p>Load cartridges in this exact volume sequence. Engine record boundaries are retained for reconstruction.</p></div><span>' + volumes.length + " volume" + (volumes.length === 1 ? "" : "s") + '</span></div>' + renderRecoveryVolumes(volumes) + '</section><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Verification evidence</h3><p>Write completion, full readback, and restore testing remain separate claims.</p></div><span>' + verifications.length + " record" + (verifications.length === 1 ? "" : "s") + '</span></div>' + renderRecoveryVerifications(verifications) + '</section><div class="vault-two-column"><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Recovery contract</h3><p>The durable identity and retention boundary.</p></div></div><dl class="vault-facts">' + facts.map(([name, value]) => "<dt>" + esc(name) + '</dt><dd class="' + (/hash|manifest|lineage|format/i.test(name) ? "mono" : "") + '">' + esc(value) + "</dd>").join("") + '</dl>' + (Object.keys(coverage).length ? '<details class="vault-json"><summary>Recorded coverage</summary><pre>' + esc(JSON.stringify(coverage, null, 2)) + "</pre></details>" : "") + '</section><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Dependencies & holds</h3><p>Every prerequisite remains visible before planning.</p></div></div><h4>Dependencies</h4>' + renderDependencyList(dependencies, "This is a self-contained full recovery point.") + '<h4>Active holds</h4>' + renderDependencyList(holds, "No active hold was returned.") + '</section></div><section class="vault-dossier-section"><div class="vault-section-head"><div><h3>Origin & retained evidence</h3><p>Correlation data for the job, physical exercise, catalog, and content-addressed artifacts.</p></div></div><div class="vault-origin-grid"><div><h4>Origin</h4>' + renderOrigin(origin, point) + '</div><div><h4>Artifacts</h4>' + renderRecoveryArtifacts(artifacts) + "</div></div>" + renderRecoveryEventsForVault(events) + "</section></article>";
  }

  function renderRestore() {
    const points = orderedRecoveryPoints();
    let html = pageHead("Recovery vault", "Search durable recovery points, inspect exact media and verification evidence, and generate a non-executing restore readiness plan.", refreshButton()) + noticeFor("recoveryPoints");
    if (!points.length) return html + empty("No recovery points", state.errors.recoveryPoints ? "Recovery points could not be loaded." : "A verified physical recovery exercise will create the first durable recovery point.", '<a class="button button-primary" href="#recovery-lab">Open recovery laboratory</a>');
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
    const disabledReason = "The laboratory file roundtrip is disabled in this deployment.";
    const createProfile = '<button class="button button-primary" type="button" data-action="open-recovery-profile"' + (!enabled ? ' disabled title="' + esc(disabledReason) + '"' : "") + '>Add source profile</button>';
    const blocker = !enabled ? disabledReason : active ? "A recovery exercise is already active." : !profiles.length ? "Create a source and restore profile first." : !drives.length ? "No exact btape-qualified drive and scratch-cartridge pair is ready." : !media.length ? "No cartridge qualified with the selected ready drive is available." : null;
    const start = '<button class="button button-primary" type="button" data-action="open-recovery-run"' + (blocker ? ' disabled title="' + esc(blocker) + '"' : "") + '>Create recoverable tape</button>';
    let html = pageHead("Recovery laboratory", "One guided physical write, readback, isolated restore, and manifest comparison.", refreshButton() + createProfile + start);
    html += noticeFor("recoveryProfiles") + noticeFor("recoveryExercises");
    if (!enabled) html += '<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Recovery laboratory disabled</strong><span>Enable TAPESCALE_LAB_FILE_ROUNDTRIP_ENABLED on both the API and worker before creating profiles or destructive recovery exercises.</span></div></div>';
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
    document.querySelectorAll("[data-route]").forEach((link) => link.classList.toggle("active", link.dataset.route === state.route));
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
    const body = '<form id="restore-plan-form" data-recovery-point-id="' + esc(id) + '"><div class="restore-plan-banner"><span aria-hidden="true">◇</span><div><strong>Generate a readiness plan</strong><p>This evaluates durable catalog evidence and your destination assessment. It cannot read tape or restore data.</p></div></div><div class="restore-plan-point"><div><small>Recovery point</small><strong>' + esc(label(pick(point, "dataset_name", "source", "dataset"))) + '</strong><code>' + esc(id) + '</code></div>' + status(recoveryPointState(point)) + '</div><div class="restore-plan-media"><span>Required media</span><strong>' + esc(media.length ? media.join(" → ") : "No ordered media returned") + '</strong></div><div class="form-grid restore-plan-fields"><div class="form-field"><label for="restore-plan-mode">Recovery mode</label><select class="select" id="restore-plan-mode" name="mode"><option value="file_archive"' + selectedOption(draft.mode, "file_archive") + '>File archive</option><option value="native_dataset"' + selectedOption(draft.mode, "native_dataset") + '>Native ZFS dataset</option><option value="files_from_native"' + selectedOption(draft.mode, "files_from_native") + '>Files from native stream</option></select><small>Native extraction may require temporary staging capacity.</small></div><div class="form-field"><label for="restore-conflict-policy">Existing-file behavior</label><select class="select" id="restore-conflict-policy" name="conflict_policy"><option value="skip"' + selectedOption(draft.conflict_policy, "skip") + '>Skip conflicts (safe default)</option><option value="rename"' + selectedOption(draft.conflict_policy, "rename") + '>Rename recovered files</option><option value="replace"' + selectedOption(draft.conflict_policy, "replace") + '>Replace existing data</option></select><small>Replacement stays blocked without administrator reauthentication and a destructive preview.</small></div><div class="form-field full"><label for="restore-destination">Destination inside the configured recovery root</label><input class="input input-mono" id="restore-destination" name="destination" value="' + esc(draft.destination) + '" maxlength="512" required placeholder="restores/2026-11-12-dataset"><small>Use a contained relative path. Absolute paths and parent traversal are rejected.</small></div><div class="form-field"><label for="restore-available-bytes">Available destination bytes</label><input class="input input-mono" id="restore-available-bytes" name="available_bytes" type="number" min="0" step="1" value="' + esc(draft.available_bytes == null ? "" : draft.available_bytes) + '" placeholder="Optional"><small>Leave blank to retain an explicit capacity-estimate warning.</small></div><div class="form-field"><label for="restore-reserve-bytes">Required free-space reserve</label><input class="input input-mono" id="restore-reserve-bytes" name="required_free_reserve_bytes" type="number" min="0" step="1" value="' + esc(draft.required_free_reserve_bytes) + '"><small>Logical recovery estimate: ' + esc(draft.estimated_required_bytes == null ? "not reported" : bytes(draft.estimated_required_bytes)) + '.</small></div><div class="form-field full"><fieldset class="restore-assessment"><legend>Destination assessment</legend><label class="check-option"><input type="checkbox" name="is_new_destination"' + checked(draft.is_new_destination) + '> This is a new, isolated destination.</label><label class="check-option"><input type="checkbox" name="destination_approved"' + checked(draft.destination_approved) + '> The path is inside the approved recovery area.</label><label class="check-option"><input type="checkbox" name="acl_semantics_supported"' + checked(draft.acl_semantics_supported) + '> The destination preserves the required ACL and metadata semantics.</label><label class="check-option"><input type="checkbox" name="identity_mapping_complete"' + checked(draft.identity_mapping_complete) + '> User and group identity mapping has been reviewed.</label>' + (keyLineage ? '<label class="check-option"><input type="checkbox" name="key_available"' + checked(draft.key_available) + '> Required key lineage <code>' + esc(keyLineage) + '</code> is available.</label><label class="check-option"><input type="checkbox" name="key_recovery_package_tested"' + checked(draft.key_recovery_package_tested) + '> The independent key recovery package has been tested.</label>' : '<div class="restore-no-key"><span aria-hidden="true">✓</span>No encryption key lineage is recorded for this point.</div>') + '</fieldset></div></div><div class="notice warning restore-replace-warning" id="restore-replace-warning"' + (draft.conflict_policy === "replace" ? "" : " hidden") + '><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>Replacement cannot be authorized here</strong><span>The resulting plan will retain reauthentication and destructive-preview blockers. This form never executes the replacement.</span></div></div><p class="notice danger" id="restore-plan-error" hidden></p><div class="modal-foot restore-plan-submit"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Generate readiness plan</button></div></form>';
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

  function renderRestorePlanResult(plan, point) {
    const ready = plan && plan.ready === true;
    const blockers = planArray(plan, "blockers");
    const warnings = planArray(plan, "warnings");
    const tapes = planArray(plan, "required_tape_ids", "required_tapes", "tapes");
    const dependencies = planArray(plan, "dependency_chain", "required_dependencies", "dependencies");
    const keys = planArray(plan, "required_key_ids", "required_keys", "keys");
    const verification = planArray(plan, "verification_prerequisites");
    const destination = planObjectFacts(pick(plan, "destination"));
    const estimates = planObjectFacts(pick(plan, "estimates"));
    const planId = pick(plan, "id", "plan_id");
    const previewHash = pick(plan, "preview_hash", "sha256", "digest");
    const expiresAt = pick(plan, "expires_at");
    const resultNotice = ready
      ? '<div class="restore-result-hero ready"><span aria-hidden="true">✓</span><div><small>Readiness result</small><h3>Prerequisites satisfied</h3><p>This point can advance to an attended restore workflow when execution support is enabled.</p></div></div>'
      : '<div class="restore-result-hero blocked"><span aria-hidden="true">!</span><div><small>Readiness result</small><h3>Restore remains blocked</h3><p>' + blockers.length + " prerequisite" + (blockers.length === 1 ? "" : "s") + " must be resolved before any execution workflow.</p></div></div>";
    const facts = [
      ["Plan ID", planId],
      ["Preview SHA-256", previewHash],
      ["Generated", date(pick(plan, "created_at", "generated_at"))],
      ["Expires", date(expiresAt)],
      ["Plan status", pick(plan, "status")],
      ["Recovery point", recordId(point)]
    ].filter((item) => item[1] !== null && item[1] !== undefined && item[1] !== "Not reported");
    return resultNotice + '<div class="restore-result-boundary"><strong>Plan retained; nothing executed</strong><span>TapeScale did not move media, invoke Bacula, create a destination, or write recovered data.</span></div><dl class="restore-result-facts">' + facts.map(([name, value]) => "<dt>" + esc(name) + '</dt><dd class="mono">' + esc(value) + "</dd>").join("") + '</dl><div class="restore-result-grid">' + renderPlanRequirements("Required media", tapes, "No media requirement was returned.") + renderPlanRequirements("Dependency chain", dependencies, "No earlier recovery point is required.") + renderPlanRequirements("Required keys", keys, "No recovery key requirement was returned.") + renderPlanRequirements("Verification gates", verification, "No verification prerequisite was returned.") + renderPlanRequirements("Destination", destination, "No destination assessment was returned.") + renderPlanRequirements("Read & staging estimate", estimates, "No work estimate was returned.") + renderPlanRequirements("Blockers", blockers.map(planIssueText), "No blocking prerequisite remains.") + renderPlanRequirements("Warnings", warnings.map(planIssueText), "No planning warning was returned.") + '</div>' + (pick(plan, "execution_supported") === false ? '<div class="notice info"><span class="notice-icon" aria-hidden="true">i</span><div class="notice-copy"><strong>Execution is not exposed in this build</strong><span>The readiness plan is durable evidence, not a restore command.</span></div></div>' : "");
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
      toast("Recovery laboratory is disabled", "Enable the laboratory file roundtrip on both the API and worker first.");
      return;
    }
    const body = '<form id="recovery-profile-form"><div class="notice info"><span class="notice-icon" aria-hidden="true">1</span><div class="notice-copy"><strong>Declare the prepared recovery fixture</strong><span>Both paths are contained subpaths of the source and restore roots configured by the TrueNAS Compose file. TapeScale will not accept host-absolute paths here.</span></div></div><div class="form-grid"><div class="form-field full"><label for="recovery-profile-name">Profile name</label><input class="input" id="recovery-profile-name" name="name" maxlength="128" placeholder="Quarterly recovery fixture" required><small>This name appears in the destructive confirmation and retained evidence.</small></div><div class="form-field"><label for="recovery-source-subpath">Immutable source subpath</label><input class="input input-mono" id="recovery-source-subpath" name="source_subpath" maxlength="512" placeholder="fixture-2026-11" required><small>Relative to the configured read-only source root.</small></div><div class="form-field"><label for="recovery-restore-subpath">Isolated restore subpath</label><input class="input input-mono" id="recovery-restore-subpath" name="restore_subpath" maxlength="512" placeholder="exercises" required><small>Relative to the configured restore root; each run receives its own directory.</small></div><div class="form-field full"><label class="check-option danger-check"><input type="checkbox" name="immutable_snapshot_confirmed" required> I confirm the source subpath is a prepared immutable snapshot or expendable fixture and will not change during this exercise.</label></div></div><p class="notice danger" id="recovery-profile-error" hidden></p><div class="modal-foot"><button class="button button-quiet" type="button" data-action="close-modal">Cancel</button><button class="button button-primary" type="submit">Save and choose hardware</button></div></form>';
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
      toast("Recovery laboratory is disabled", "Enable the laboratory file roundtrip on both the API and worker first.");
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
    if (action === "open-recovery-profile") openRecoveryProfile();
    if (action === "open-recovery-run") openRecoveryRun(target.dataset.profileId || null);
    if (action === "select-recovery-exercise") selectRecoveryExercise(target.dataset.id);
    if (action === "select-recovery-point") selectRecoveryPoint(target.dataset.id);
    if (action === "retry-recovery-point") loadRecoveryPointDetail(target.dataset.id, true);
    if (action === "download-recovery-evidence") downloadRecoveryEvidence(target);
    if (action === "open-restore-plan" || action === "revise-restore-plan") openRestorePlan(target.dataset.id);
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
    if (event.target.id === "qualification-form") {
      event.preventDefault();
      submitQualification(event.target);
    }
    if (event.target.id === "restore-plan-form") {
      event.preventDefault();
      submitRestorePlan(event.target);
    }
  });

  document.addEventListener("input", (event) => {
    if (event.target.id === "vault-search") {
      state.recoveryPointQuery = event.target.value;
      applyRecoveryPointFilters();
    }
  });

  document.addEventListener("change", (event) => {
    if (event.target.id === "qualification-cartridge") updateQualificationConfirmation(event.target);
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
    if (event.target.id === "restore-conflict-policy") {
      const warning = document.querySelector("#restore-replace-warning");
      if (warning) warning.hidden = event.target.value !== "replace";
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeModal();
      closeDrawer();
      shell.classList.remove("nav-open");
    }
    if ((event.key === "Enter" || event.key === " ") && event.target.matches("tr[data-action='open-record'], tr[data-action='select-recovery-exercise'], tr[data-action='select-recovery-point']")) event.target.click();
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
    }
  });

  updateChrome();
  loadAll(true).then(() => {
    live.textContent = "TapeScale data loaded.";
  });
})();
