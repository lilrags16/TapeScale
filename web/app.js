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
    qualification: "/qualification"
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
      movements: [],
      verification: [],
      recoveryPoints: [],
      preflight: null,
      qualification: null
    },
    received: new Set(),
    errors: {}
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
    if (key === "drives") {
      return { drives: list(payload, "drives"), slots: list(payload, "slots") };
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
    let html = pageHead("Drives & libraries", "Hardware inventory returned by TapeScale.", refreshButton()) + noticeFor("drives");
    if (!state.data.drives.length && !state.data.slots.length) return html + empty("No hardware inventory", state.errors.drives ? "Drive and slot inventory could not be loaded." : "No drives or library slots have been reported.");
    if (state.data.drives.length) {
      const rows = state.data.drives.map((drive) => '<tr class="row-action" tabindex="0" data-action="open-record" data-kind="drives" data-id="' + esc(recordId(drive)) + '"><td class="primary-cell"><strong>' + esc(label(pick(drive, "stable_id", "name", "id", "device"))) + '</strong><small>' + esc(label(pick(drive, "model", "serial", "observed_path"))) + "</small></td>" + cell(pick(drive, "generation", "model")) + "<td>" + status(pick(drive, "qualification_status", "state", "status", "health")) + "</td>" + cell(mediaName(pick(drive, "media") || {}) || pick(drive, "loaded_media", "cartridge")) + "</tr>");
      html += table(["Drive", "Generation / model", "Status", "Loaded media"], rows);
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
    let html = noticeFor("qualification");
    if (!value || !Object.keys(value).length) return html + empty("Qualification not available", state.errors.qualification ? "Qualification metadata could not be loaded." : "No qualification metadata has been reported.");
    html += metadata(value, ["cases", "generations"]);
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

  function openRecord(kind, id) {
    const record = findRecord(kind, id);
    if (!record) return;
    const title = pick(record, "name", "internal_label", "external_id", "stable_id", "label", "id", "job_id") || human(kind);
    const rows = Object.entries(record)
      .filter(([, value]) => value == null || ["string", "number", "boolean"].includes(typeof value))
      .map(([key, value]) => "<li><span>" + esc(human(key)) + "</span><strong>" + esc(label(value)) + "</strong></li>")
      .join("");
    drawerLayer.innerHTML = '<aside class="drawer" role="dialog" aria-modal="true" aria-labelledby="drawer-title"><div class="drawer-head"><h2 id="drawer-title">' + esc(title) + '</h2><button class="icon-button" type="button" data-action="close-drawer" aria-label="Close">&times;</button></div><div class="drawer-body"><ul class="detail-list">' + rows + "</ul></div></aside>";
    drawerLayer.hidden = false;
  }

  function closeDrawer() {
    drawerLayer.hidden = true;
    drawerLayer.innerHTML = "";
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
    if (event.target.id !== "add-media-form") return;
    event.preventDefault();
    submitMedia(event.target);
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
