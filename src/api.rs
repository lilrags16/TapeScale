use std::{
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::info;
use uuid::Uuid;

use crate::{
    backup_ops,
    custody_ops::{self, CustodyActor, CustodyOpsError},
    data_path::{
        isolated_restore_destination, resolve_existing_selection,
        resolve_existing_zfs_snapshot_selection, verify_read_only_mount,
    },
    device::SasMotionOperation,
    domain::{
        path::validate_archive_path,
        restore::{
            preflight_restore, ConflictPolicy, DestinationAssessment, KeyPrerequisite,
            RestoreBlocker, RestoreMode, RestoreRequest, RestoreWarning, TapePrerequisite,
            VerificationEvidence,
        },
    },
    engine::{
        bacula::BACULA_VERSION, simulator::SimulatorEngine, EngineCapabilities, EngineIdentity,
        QualificationStatus, TapeEngine,
    },
    error::AppError,
    media_ops, qualification,
    source_browser::{self, RetainedManifestReference, SourceBrowserError},
    store, VERSION,
};

#[derive(Clone)]
pub struct AppState {
    pub pool: Option<PgPool>,
    pub demo_mode: bool,
    pub engine: Arc<dyn TapeEngine>,
    pub lab_tape_motion_enabled: bool,
    pub lab_tape_qualification_enabled: bool,
    pub lab_file_roundtrip_enabled: bool,
    pub source_root: PathBuf,
    pub restore_root: PathBuf,
    pub evidence_root: PathBuf,
    retained_manifest_browser_slots: Arc<Semaphore>,
}

const RETAINED_MANIFEST_BROWSER_CONCURRENCY: usize = 1;
const BACULA_ADAPTER_VERSION: &str = "g0.2-lab-v1";
const BACULA_FORMAT_VERSION: &str = "bacula-community-15.0.3-lab-v1";

impl AppState {
    #[must_use]
    pub fn new(
        pool: Option<PgPool>,
        demo_mode: bool,
        lab_tape_motion_enabled: bool,
        lab_tape_qualification_enabled: bool,
        lab_file_roundtrip_enabled: bool,
        source_root: PathBuf,
        restore_root: PathBuf,
    ) -> Self {
        Self {
            pool,
            demo_mode,
            engine: Arc::new(SimulatorEngine::new()),
            lab_tape_motion_enabled,
            lab_tape_qualification_enabled,
            lab_file_roundtrip_enabled,
            source_root,
            restore_root,
            evidence_root: PathBuf::from("/var/lib/tapescale/recovery"),
            retained_manifest_browser_slots: Arc::new(Semaphore::new(
                RETAINED_MANIFEST_BROWSER_CONCURRENCY,
            )),
        }
    }

    #[must_use]
    pub fn with_evidence_root(mut self, evidence_root: PathBuf) -> Self {
        self.evidence_root = evidence_root;
        self
    }
}

pub fn router(state: AppState, web_dir: PathBuf) -> Router {
    let request_id = HeaderName::from_static("x-request-id");
    let api = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/system", get(system))
        .route("/api/v1/overview", get(overview))
        .route("/api/v1/jobs", get(jobs))
        .route("/api/v1/jobs/{id}", get(job))
        .route("/api/v1/source-browser", get(source_browser_listing))
        .route(
            "/api/v1/backup-profiles",
            get(backup_profiles).post(create_backup_profile),
        )
        .route("/api/v1/backup-profiles/{id}", get(backup_profile))
        .route(
            "/api/v1/backup-profiles/{id}/media-readiness",
            get(backup_profile_media_readiness),
        )
        .route("/api/v1/backup-profiles/{id}/runs", post(create_backup_run))
        .route("/api/v1/backup-runs", get(backup_runs))
        .route("/api/v1/backup-runs/{id}", get(backup_run))
        .route(
            "/api/v1/backup-runs/{id}/media-readiness",
            get(backup_run_media_readiness),
        )
        .route("/api/v1/backup-runs/{id}/media", post(assign_backup_media))
        .route("/api/v1/backup-runs/{id}/cancel", post(cancel_backup_run))
        .route("/api/v1/media", get(media).post(create_media))
        .route(
            "/api/v1/media-pools",
            get(media_pools).post(create_media_pool),
        )
        .route("/api/v1/media/{id}", get(media_detail))
        .route(
            "/api/v1/media/{id}/pool-assignment",
            post(assign_media_pool),
        )
        .route("/api/v1/media/{id}/holds", post(create_media_hold))
        .route(
            "/api/v1/media/{id}/reuse-evaluation",
            get(evaluate_media_reuse),
        )
        .route("/api/v1/holds/{id}/release", post(release_media_hold))
        .route("/api/v1/drives", get(drives))
        .route("/api/v1/drives/{id}/reconcile", post(reconcile_drive))
        .route("/api/v1/drives/{id}/actions", post(queue_drive_action))
        .route("/api/v1/locations", get(locations).post(create_location))
        .route("/api/v1/movements", get(movements).post(create_movement))
        .route("/api/v1/movements/{id}", get(movement))
        .route("/api/v1/movements/{id}/confirm", post(confirm_movement))
        .route("/api/v1/movements/{id}/cancel", post(cancel_movement))
        .route(
            "/api/v1/movements/{id}/corrections",
            post(correct_movement_custody),
        )
        .route("/api/v1/verification", get(verification))
        .route("/api/v1/recovery-points", get(recovery_points))
        .route("/api/v1/recovery-points/{id}", get(recovery_point))
        .route(
            "/api/v1/recovery-points/{id}/files",
            get(recovery_point_files),
        )
        .route(
            "/api/v1/recovery-points/{id}/evidence",
            get(recovery_point_evidence),
        )
        .route(
            "/api/v1/recovery-exercises/profiles",
            get(recovery_exercise_profiles).post(create_recovery_exercise_profile),
        )
        .route(
            "/api/v1/recovery-exercises",
            get(recovery_exercises).post(queue_recovery_exercise),
        )
        .route("/api/v1/recovery-exercises/{id}", get(recovery_exercise))
        .route("/api/v1/restores/plan", post(plan_restore))
        .route(
            "/api/v1/restore-rehearsals",
            get(restore_rehearsals).post(queue_restore_rehearsal),
        )
        .route("/api/v1/restore-rehearsals/{id}", get(restore_rehearsal))
        .route(
            "/api/v1/restore-rehearsals/{id}/media-readiness",
            get(restore_rehearsal_media_readiness),
        )
        .route(
            "/api/v1/restore-rehearsals/{id}/media",
            post(authorize_restore_rehearsal_media),
        )
        .route(
            "/api/v1/restore-rehearsals/{id}/cancel",
            post(cancel_restore_rehearsal),
        )
        .route(
            "/api/v1/restore-rehearsals/{id}/evidence",
            get(restore_rehearsal_evidence),
        )
        .route("/api/v1/preflight", get(preflight))
        .route("/api/v1/qualification", get(qualification_report))
        .route(
            "/api/v1/qualification/runs",
            get(qualification_runs).post(queue_qualification_run),
        )
        .route("/api/v1/qualification/runs/{id}", get(qualification_run));

    let static_files = ServeDir::new(&web_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(web_dir.join("index.html")));

    api.fallback_service(static_files)
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn live() -> impl IntoResponse {
    Json(json!({"status":"alive","version":VERSION}))
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let database_ready = match &state.pool {
        Some(pool) => store::ready(pool).await,
        None => state.demo_mode,
    };
    let status = if database_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let database = match (&state.pool, database_ready) {
        (Some(_), true) => "connected",
        (Some(_), false) => "unavailable",
        (None, _) => "not_configured",
    };
    (
        status,
        Json(json!({
            "status": if database_ready {"ready"} else {"not_ready"},
            "database": database,
            "mode": runtime_mode(&state),
            "physical_qualification": "not_established"
        })),
    )
}

async fn system(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let (identity, capabilities) = system_engine_metadata(&state).await?;
    Ok(Json(json!({
        "product":"TapeScale",
        "version":VERSION,
        "release_channel": crate::RELEASE_CHANNEL,
        "release_stage": "pre_release",
        "support_scope": "attended_non_production",
        "mode":runtime_mode(&state),
        "backend_language":"Rust",
        "engine":identity,
        "capabilities":capabilities,
        "lab_tape_motion_enabled":state.lab_tape_motion_enabled,
        "lab_tape_qualification_enabled":state.lab_tape_qualification_enabled,
        "lab_file_roundtrip_enabled":state.lab_file_roundtrip_enabled,
        "file_roundtrip_scope": if state.lab_file_roundtrip_enabled {
            "attended_multivolume_full_file_backup_and_single_volume_recovery_exercise"
        } else {
            "disabled"
        },
        "operational_backup_data_path": {
            "enabled": state.lab_file_roundtrip_enabled,
            "engine": "Bacula Community",
            "engine_version": BACULA_VERSION,
            "adapter_version": BACULA_ADAPTER_VERSION,
            "format_version": BACULA_FORMAT_VERSION,
            "backup_level": "full",
            "compression_modes": ["auto_gzip1", "gzip6", "lzo", "none"],
            "manual_media_spanning": true,
            "complete_physical_readback": true,
            "production_eligible": false
        },
        "laboratory_data_path": {
            "enabled": state.lab_file_roundtrip_enabled,
            "engine": "Bacula Community",
            "engine_version": BACULA_VERSION,
            "adapter_version": BACULA_ADAPTER_VERSION,
            "format_version": BACULA_FORMAT_VERSION,
            "production_eligible": false,
            "encryption": "disabled_with_explicit_per_run_deviation"
        },
        "operational_data":"attended_bacula_full_backup_alpha"
    })))
}

async fn system_engine_metadata(
    state: &AppState,
) -> Result<(EngineIdentity, EngineCapabilities), AppError> {
    if state.demo_mode {
        let identity = state
            .engine
            .identity()
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let capabilities = state
            .engine
            .capabilities()
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        return Ok((identity, capabilities));
    }

    Ok((
        EngineIdentity {
            adapter: "tapescale-bacula-attended".to_owned(),
            adapter_version: BACULA_ADAPTER_VERSION.to_owned(),
            engine_name: "Bacula Community".to_owned(),
            engine_version: BACULA_VERSION.to_owned(),
            qualification: QualificationStatus::DetectedUnqualified,
            format_selected: false,
        },
        EngineCapabilities {
            file_backup: true,
            native_stream: false,
            multi_volume: true,
            full_readback: true,
            changer_control: false,
            catalog_rebuild: false,
            evidence: [
                (
                    "availability".to_owned(),
                    if state.lab_file_roundtrip_enabled {
                        "enabled for attended alpha workflows".to_owned()
                    } else {
                        "disabled by configuration".to_owned()
                    },
                ),
                (
                    "scope".to_owned(),
                    "compressed or uncompressed Full file backup, attended standalone-drive spanning, and complete physical readback"
                        .to_owned(),
                ),
                (
                    "physical_hardware".to_owned(),
                    "qualification is retained only for each exact recorded drive and cartridge tuple"
                        .to_owned(),
                ),
                (
                    "format".to_owned(),
                    "Bacula format exercised; not selected or qualified for production"
                        .to_owned(),
                ),
                (
                    "encryption".to_owned(),
                    "not implemented in the attended alpha data path".to_owned(),
                ),
            ]
            .into(),
        },
    ))
}

async fn overview(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let Some(pool) = state.pool.as_ref() else {
        return Ok(Json(json!({
            "summary": empty_summary(),
            "sources": [],
            "actions": [],
            "actions_total": 0,
            "actions_returned": 0,
            "actions_truncated": false,
            "storage": "not_configured"
        })));
    };

    let mut summary = store::overview_counts(pool).await?;
    let sources = store::list_sources(pool).await?;
    let actions = store::list_required_actions(pool).await?;
    let actions_returned = i64::try_from(actions.len()).unwrap_or(i64::MAX);
    summary.required_actions = summary.required_actions.max(actions_returned);
    let actions_total = summary.required_actions;
    Ok(Json(json!({
        "summary": summary,
        "sources": sources,
        "actions": actions,
        "actions_total": actions_total,
        "actions_returned": actions_returned,
        "actions_truncated": actions_returned < actions_total,
        "storage": "postgresql"
    })))
}

async fn jobs(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let jobs = match state.pool.as_ref() {
        Some(pool) => store::list_jobs(pool).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({"jobs": jobs})))
}

async fn job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let job = store::get_job(pool, &id).await?.ok_or(AppError::NotFound)?;
    let transitions = match job.run_id {
        Some(run_id) => store::job_transitions(pool, run_id).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({"job":job,"transitions":transitions})))
}

#[derive(Debug, Default, Deserialize)]
struct BrowserQuery {
    #[serde(default)]
    path: String,
}

async fn source_browser_listing(
    State(state): State<AppState>,
    Query(query): Query<BrowserQuery>,
) -> Result<Response, AppError> {
    let source_root = state.source_root.clone();
    let listing = tokio::task::spawn_blocking(move || {
        source_browser::browse_source(&source_root, &query.path)
    })
    .await
    .map_err(|error| AppError::Internal(format!("source browser task failed: {error}")))?
    .map_err(map_source_browser_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(listing)).into_response())
}

async fn backup_profiles(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let profiles = match state.pool.as_ref() {
        Some(pool) => backup_ops::list_backup_profiles(pool)
            .await
            .map_err(map_backup_ops_error)?,
        None => Vec::new(),
    };
    Ok(Json(json!({
        "profiles": profiles,
        "execution_scope": "attended_full_file_ordered_multivolume",
        "production_eligible": false
    })))
}

async fn backup_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "backup profile")?;
    let profile = backup_ops::get_backup_profile(pool, id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    Ok(Json(json!({"profile": profile})))
}

#[derive(Debug, Deserialize)]
struct CreateBackupProfileRequest {
    name: String,
    source_subpath: String,
    #[serde(default = "default_backup_timezone")]
    schedule_timezone: String,
    #[serde(default)]
    schedule_enabled: bool,
    #[serde(default = "default_backup_retention_days")]
    retention_days: i32,
    #[serde(default)]
    media_pool_id: Option<Uuid>,
    #[serde(default = "default_backup_compression", alias = "compression")]
    compression_mode: String,
    #[serde(default = "default_backup_spanning", alias = "allow_media_spanning")]
    spanning_enabled: bool,
    // Kept as optional, ignored compatibility fields for clients released
    // before the server made its read-only mount check authoritative. Plan
    // creation must not require an operator to restate a property the API can
    // and does verify itself.
    #[serde(default, rename = "source_snapshot_attested_read_only")]
    _legacy_source_snapshot_attested_read_only: Option<bool>,
    #[serde(default, rename = "limitations_acknowledged")]
    _legacy_limitations_acknowledged: Option<bool>,
    #[serde(default)]
    requested_by_label: Option<String>,
}

fn default_backup_timezone() -> String {
    "UTC".to_owned()
}

const fn default_backup_retention_days() -> i32 {
    28
}

fn default_backup_compression() -> String {
    "auto".to_owned()
}

const fn default_backup_spanning() -> bool {
    true
}

fn backup_actor_label(value: Option<&str>) -> Result<String, AppError> {
    let value = value.unwrap_or("Web operator").trim();
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "requested_by_label must contain 1 to 256 printable bytes".to_owned(),
        ));
    }
    Ok(value.to_owned())
}

async fn create_backup_profile(
    State(state): State<AppState>,
    Json(request): Json<CreateBackupProfileRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "name must contain 1 to 128 printable characters".to_owned(),
        ));
    }
    request
        .schedule_timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| AppError::Invalid("schedule_timezone is invalid".to_owned()))?;
    if !(1..=36_500).contains(&request.retention_days) {
        return Err(AppError::Invalid(
            "retention_days must be between 1 and 36500".to_owned(),
        ));
    }
    if !matches!(
        request.compression_mode.as_str(),
        "auto" | "none" | "gzip" | "lzo"
    ) {
        return Err(AppError::Invalid(
            "compression_mode must be auto, none, gzip, or lzo".to_owned(),
        ));
    }
    let actor_label = backup_actor_label(request.requested_by_label.as_deref())?;
    if let Some(media_pool_id) = request.media_pool_id {
        let purpose =
            sqlx::query_scalar::<_, String>("SELECT purpose FROM media_pools WHERE id = $1")
                .bind(media_pool_id)
                .fetch_optional(pool)
                .await?
                .ok_or(AppError::NotFound)?;
        if purpose == "cleaning" {
            return Err(AppError::Invalid(
                "a cleaning-media pool cannot be a backup target".to_owned(),
            ));
        }
    }
    let requested_source_subpath =
        normalized_relative_path(&request.source_subpath, "source_subpath")?;
    let source_mount =
        resolve_existing_zfs_snapshot_selection(&state.source_root, &requested_source_subpath)
            .map_err(|error| AppError::Blocked(error.to_string()))?;
    let canonical_source_root = state.source_root.canonicalize().map_err(|error| {
        AppError::Blocked(format!(
            "configured source root {} is unavailable: {error}",
            state.source_root.display()
        ))
    })?;
    let source_subpath = source_mount
        .strip_prefix(&canonical_source_root)
        .map_err(|_| {
            AppError::Blocked(
                "resolved backup source escaped the configured source root".to_owned(),
            )
        })?
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            AppError::Blocked("resolved backup source must be valid UTF-8".to_owned())
        })?;
    verify_read_only_mount(&source_mount).map_err(|error| AppError::Blocked(error.to_string()))?;
    let restore_subpath = "backup-rehearsals".to_owned();
    let restore_probe =
        isolated_restore_destination(&state.restore_root, &restore_subpath, Uuid::nil())
            .map_err(|error| AppError::Blocked(error.to_string()))?;
    let restore_root = restore_probe
        .parent()
        .ok_or_else(|| AppError::Invalid("configured restore root is invalid".to_owned()))?;
    let source_profile = store::NewSingleTapeDataProfile {
        name: name.to_owned(),
        source_subpath,
        restore_subpath,
        source_mount_path: utf8_path(&source_mount, "resolved source path")?,
        restore_root_path: utf8_path(restore_root, "resolved restore path")?,
        immutable_snapshot_confirmed: true,
    };
    let source_profile = match store::create_single_tape_data_profile(pool, &source_profile).await?
    {
        store::CreateSingleTapeDataProfileResult::Created(profile) => profile,
        store::CreateSingleTapeDataProfileResult::NotFound => return Err(AppError::NotFound),
        store::CreateSingleTapeDataProfileResult::Blocked(reason) => {
            return Err(AppError::Blocked(reason))
        }
    };
    let profile = backup_ops::create_backup_profile(
        pool,
        &backup_ops::NewBackupProfile {
            source_profile_id: source_profile.id,
            media_pool_id: request.media_pool_id,
            schedule_timezone: request.schedule_timezone,
            full_schedule: backup_ops::DEFAULT_FULL_SCHEDULE.to_owned(),
            schedule_enabled: request.schedule_enabled,
            retention_days: request.retention_days,
            representation: "file".to_owned(),
            requested_level: "full".to_owned(),
            verification_mode: "full_readback".to_owned(),
            encryption_enabled: false,
            compression_mode: request.compression_mode,
            spanning_enabled: request.spanning_enabled,
            actor_label,
        },
    )
    .await
    .map_err(map_backup_ops_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "profile": profile,
            "source_mount": "read_only_verified",
            "durability": "postgresql",
            "audit": "appended",
            "production_eligible": false
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct CreateBackupRunRequest {
    // Accepted only for compatibility with older clients. The API verifies the
    // configured source again instead of asking an operator to certify it.
    #[serde(default, rename = "source_snapshot_attested_read_only")]
    _legacy_source_snapshot_attested_read_only: Option<bool>,
    #[serde(default)]
    requested_by_label: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

fn verify_backup_source_preflight(
    state: &AppState,
    profile: &backup_ops::BackupProfileRecord,
) -> Result<(), AppError> {
    let source_mount =
        resolve_existing_zfs_snapshot_selection(&state.source_root, &profile.source_subpath)
            .map_err(|error| AppError::Blocked(error.to_string()))?;
    let persisted_source_mount =
        std::fs::canonicalize(&profile.source_mount_path).map_err(|error| {
            AppError::Blocked(format!(
                "configured backup source mount {} is unavailable: {error}",
                profile.source_mount_path
            ))
        })?;
    if source_mount != persisted_source_mount {
        return Err(AppError::Blocked(
            "configured backup source no longer resolves to its retained snapshot mount".to_owned(),
        ));
    }
    verify_read_only_mount(&source_mount).map_err(|error| AppError::Blocked(error.to_string()))
}

async fn create_backup_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CreateBackupRunRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    let profile_id = parse_uuid_path(&id, "backup profile")?;
    let actor = backup_actor_label(request.requested_by_label.as_deref())?;
    let idempotency_key = request
        .idempotency_key
        .unwrap_or_else(|| format!("manual:{}", Uuid::now_v7()));
    if let Some(existing) =
        backup_ops::get_backup_request_by_idempotency_key(pool, profile_id, &idempotency_key)
            .await
            .map_err(map_backup_ops_error)?
    {
        return backup_run_response(
            pool,
            backup_ops::CreateBackupRequestResult::Existing(existing),
        )
        .await;
    }
    let profile = backup_ops::get_backup_profile(pool, profile_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    let source_preflight = verify_backup_source_preflight(&state, &profile);
    if let Err(error) = source_preflight {
        // A concurrent request may have committed after the first lookup but
        // before this source check. Preserve exact idempotent replay semantics
        // even if the configured snapshot disappeared during that interval.
        if let Some(existing) = backup_ops::get_backup_request_by_idempotency_key_serialized(
            pool,
            profile_id,
            &idempotency_key,
        )
        .await
        .map_err(map_backup_ops_error)?
        {
            return backup_run_response(
                pool,
                backup_ops::CreateBackupRequestResult::Existing(existing),
            )
            .await;
        }
        return Err(error);
    }
    let disposition =
        backup_ops::create_manual_backup_request(pool, profile_id, &idempotency_key, &actor)
            .await
            .map_err(map_backup_ops_error)?;
    backup_run_response(pool, disposition).await
}

async fn backup_run_response(
    pool: &PgPool,
    disposition: backup_ops::CreateBackupRequestResult,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let created = matches!(
        &disposition,
        backup_ops::CreateBackupRequestResult::Created(_)
    );
    let response_state = disposition.record().state.clone();
    let run = build_backup_run_view(pool, disposition.record()).await?;
    Ok((
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        Json(json!({
            "run": run,
            "durability": "postgresql",
            "state": response_state,
            "production_eligible": false
        })),
    ))
}

async fn backup_runs(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let Some(pool) = state.pool.as_ref() else {
        return Ok(Json(json!({"runs": []})));
    };
    let records = backup_ops::list_backup_requests(pool, None, 250)
        .await
        .map_err(map_backup_ops_error)?;
    let mut runs = Vec::with_capacity(records.len());
    for record in &records {
        runs.push(build_backup_run_view(pool, record).await?);
    }
    Ok(Json(json!({
        "runs": runs,
        "execution_scope": "attended_full_file_ordered_multivolume",
        "production_eligible": false
    })))
}

async fn backup_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let request_id = parse_uuid_path(&id, "backup run")?;
    let request = backup_ops::get_backup_request(pool, request_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    let request_events = backup_ops::backup_request_events(pool, request_id)
        .await
        .map_err(map_backup_ops_error)?;
    let mut events = serde_json::to_value(request_events)
        .map_err(|error| AppError::Internal(error.to_string()))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut artifacts = Vec::new();
    if let Some(data_run_id) = request.assigned_data_run_id {
        let data_events = store::single_tape_data_events(pool, data_run_id).await?;
        for event in data_events {
            events.push(json!({
                "id": format!("data:{}", event.id),
                "event_type": event.event_type,
                "evidence": bounded_restore_evidence_value(&event.evidence),
                "correlation_id": event.correlation_id,
                "occurred_at": event.occurred_at
            }));
        }
        for artifact in store::single_tape_data_artifacts(pool, data_run_id).await? {
            artifacts.push(json!({
                "id": artifact.id,
                "artifact_kind": artifact.artifact_kind,
                "sha256": artifact.sha256,
                "size_bytes": artifact.size_bytes,
                "metadata": bounded_restore_evidence_value(&artifact.metadata),
                "created_at": artifact.created_at
            }));
        }
    }
    events.sort_by_key(|event| {
        event
            .get("occurred_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    });
    let run = build_backup_run_view(pool, &request).await?;
    Ok(Json(json!({
        "run": run,
        "events": events,
        "artifacts": artifacts,
        "production_eligible": false
    })))
}

fn backup_media_readiness_response(value: Value) -> Response {
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static cache-control is valid"),
    );
    response
}

fn backup_source_unavailable_response(
    profile_id: Uuid,
    backup_request_id: Option<Uuid>,
    error: &AppError,
) -> Response {
    let detail = match error {
        AppError::Blocked(detail) => detail.as_str(),
        _ => "the configured snapshot could not be verified",
    };
    let readiness = store::blocked_backup_media_readiness(
        profile_id,
        backup_request_id,
        store::BackupMediaReadinessBlocker {
            code: "snapshot_unavailable".to_owned(),
            scope: "profile".to_owned(),
            resource_id: Some(profile_id),
            related_resource_id: None,
            message: format!("the saved snapshot is unavailable or no longer read-only: {detail}"),
            action: "choose_snapshot".to_owned(),
        },
    );
    backup_media_readiness_response(json!({
        "readiness": readiness,
        "safety_authority": false,
        "revalidated_on_assignment": true
    }))
}

async fn backup_profile_media_readiness(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    let profile_id = parse_uuid_path(&id, "backup profile")?;
    let profile = backup_ops::get_backup_profile(pool, profile_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    if let Err(error) = verify_backup_source_preflight(&state, &profile) {
        return Ok(backup_source_unavailable_response(profile_id, None, &error));
    }
    match store::backup_media_readiness(pool, profile_id, None).await? {
        store::BackupMediaReadinessResult::Readiness(readiness) => {
            Ok(backup_media_readiness_response(json!({
                "readiness": readiness,
                "safety_authority": false,
                "revalidated_on_assignment": true
            })))
        }
        store::BackupMediaReadinessResult::NotFound => Err(AppError::NotFound),
    }
}

async fn backup_run_media_readiness(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    let request_id = parse_uuid_path(&id, "backup run")?;
    let request = backup_ops::get_backup_request(pool, request_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    if let Some(data_run_id) = request.assigned_data_run_id {
        let Some(media_request) = store::open_data_run_media_request(pool, data_run_id).await?
        else {
            let readiness = store::assigned_backup_media_readiness(request.profile_id, request.id);
            return Ok(backup_media_readiness_response(json!({
                "readiness": readiness,
                "safety_authority": false,
                "revalidated_on_assignment": true
            })));
        };
        let data_run = store::get_single_tape_data_run(pool, data_run_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let volumes = store::data_run_volumes(pool, data_run_id).await?;
        if media_request.state != "pending" {
            let mut readiness =
                store::assigned_backup_media_readiness(request.profile_id, request.id);
            readiness.state = match media_request.state.as_str() {
                "authorized" => "media_authorized",
                "loaded" => "media_loaded",
                _ => "assigned",
            }
            .to_owned();
            let mut readiness_value = serde_json::to_value(readiness)
                .map_err(|error| AppError::Internal(error.to_string()))?;
            let readiness_object = readiness_value
                .as_object_mut()
                .expect("serialized readiness is an object");
            readiness_object.insert("media_request".to_owned(), json!(&media_request));
            readiness_object.insert("volumes".to_owned(), json!(&volumes));
            return Ok(backup_media_readiness_response(json!({
                "readiness": readiness_value,
                "safety_authority": false,
                "revalidated_on_assignment": true
            })));
        }
        let used_cartridges = volumes
            .iter()
            .map(|volume| volume.cartridge_id)
            .collect::<std::collections::HashSet<_>>();
        if media_request.phase == "backup_write" {
            let mut readiness = match store::backup_continuation_media_readiness(
                pool,
                request.profile_id,
                data_run_id,
            )
            .await?
            {
                store::BackupMediaReadinessResult::Readiness(readiness) => *readiness,
                store::BackupMediaReadinessResult::NotFound => return Err(AppError::NotFound),
            };
            readiness.candidates.retain(|candidate| {
                candidate.drive_id == data_run.drive_id
                    && !used_cartridges.contains(&candidate.cartridge_id)
            });
            for candidate in &mut readiness.candidates {
                candidate.required_confirmation = Some(format!(
                    "OVERWRITE {} FOR BACKUP {} VOLUME {}",
                    candidate.cartridge_label, request.id, media_request.volume_order
                ));
            }
            readiness.recommended_candidate = readiness.candidates.first().cloned();
            readiness.counts.candidates_returned = readiness.candidates.len();
            readiness.state = if readiness.candidates.is_empty() {
                "blocked".to_owned()
            } else {
                "waiting_for_media".to_owned()
            };
            let mut readiness_value = serde_json::to_value(readiness)
                .map_err(|error| AppError::Internal(error.to_string()))?;
            let readiness_object = readiness_value
                .as_object_mut()
                .expect("serialized readiness is an object");
            readiness_object.insert("media_request".to_owned(), json!(&media_request));
            readiness_object.insert("volumes".to_owned(), json!(&volumes));
            return Ok(backup_media_readiness_response(json!({
                "readiness": readiness_value,
                "safety_authority": false,
                "revalidated_on_assignment": true
            })));
        }
        if media_request.phase != "full_readback" {
            return Err(AppError::Conflict(format!(
                "unsupported backup media-request phase {}",
                media_request.phase
            )));
        }

        let mut readiness = match store::backup_readback_media_readiness(
            pool,
            request.profile_id,
            request.id,
            data_run_id,
            media_request.id,
        )
        .await?
        {
            store::BackupMediaReadinessResult::Readiness(readiness) => *readiness,
            store::BackupMediaReadinessResult::NotFound => return Err(AppError::NotFound),
        };
        if readiness.candidates.len() == 1 {
            readiness.state = "waiting_for_media".to_owned();
        }
        let mut readiness_value = serde_json::to_value(readiness)
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let readiness_object = readiness_value
            .as_object_mut()
            .expect("serialized readiness is an object");
        readiness_object.insert("media_request".to_owned(), json!(&media_request));
        readiness_object.insert("volumes".to_owned(), json!(&volumes));
        return Ok(backup_media_readiness_response(json!({
            "readiness": readiness_value,
            "safety_authority": false,
            "revalidated_on_assignment": true
        })));
    }
    let profile = backup_ops::get_backup_profile(pool, request.profile_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    if let Err(error) = verify_backup_source_preflight(&state, &profile) {
        return Ok(backup_source_unavailable_response(
            request.profile_id,
            Some(request.id),
            &error,
        ));
    }
    match store::backup_media_readiness(
        pool,
        request.profile_id,
        Some((request.id, request.job_run_id)),
    )
    .await?
    {
        store::BackupMediaReadinessResult::Readiness(readiness) => {
            Ok(backup_media_readiness_response(json!({
                "readiness": readiness,
                "safety_authority": false,
                "revalidated_on_assignment": true
            })))
        }
        store::BackupMediaReadinessResult::NotFound => Err(AppError::NotFound),
    }
}

#[derive(Debug, Deserialize)]
struct AssignBackupMediaRequest {
    drive_id: Uuid,
    expected_stable_id: String,
    cartridge_id: Uuid,
    #[serde(default)]
    volume_sequence: Option<i32>,
    typed_cartridge_label: String,
    loaded_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
    encryption_deviation_confirmed: bool,
    confirmation: String,
}

async fn assign_backup_media(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<AssignBackupMediaRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    let request_id = parse_uuid_path(&id, "backup run")?;
    let backup_request = backup_ops::get_backup_request(pool, request_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    let profile = backup_ops::get_backup_profile(pool, backup_request.profile_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    if !request.loaded_media_confirmed {
        return Err(AppError::Invalid(
            "loaded_media_confirmed must be true".to_owned(),
        ));
    }
    let stable_id = request.expected_stable_id.trim();
    let cartridge_label = request.typed_cartridge_label.trim();
    if stable_id.is_empty()
        || stable_id.len() > 512
        || stable_id.chars().any(char::is_control)
        || cartridge_label.is_empty()
        || cartridge_label.len() > 128
        || cartridge_label.chars().any(char::is_control)
    {
        return Err(AppError::Invalid(
            "expected_stable_id and typed_cartridge_label must be bounded printable values"
                .to_owned(),
        ));
    }
    if let Some(data_run_id) = backup_request.assigned_data_run_id {
        let data_run = store::get_single_tape_data_run(pool, data_run_id)
            .await?
            .ok_or_else(|| {
                AppError::Conflict(
                    "the retained backup assignment no longer resolves to its data run".to_owned(),
                )
            })?;
        let replayable_media_request = match request.volume_sequence {
            Some(volume_order) => {
                let confirmation_evidence = json!({
                    "confirmation": request.confirmation,
                    "volume_sequence": volume_order,
                    "drive_id": request.drive_id,
                    "expected_stable_id": stable_id,
                    "typed_cartridge_label": cartridge_label,
                    "loaded_media_confirmed": request.loaded_media_confirmed,
                    "destructive_overwrite_confirmed": request.destructive_overwrite_confirmed,
                    "encryption_deviation_confirmed": request.encryption_deviation_confirmed
                });
                store::replayable_data_run_media_request(
                    pool,
                    data_run_id,
                    volume_order,
                    request.cartridge_id,
                    &confirmation_evidence,
                )
                .await?
            }
            None => None,
        };
        let media_request = match replayable_media_request {
            Some(media_request) => Some(media_request),
            None => store::open_data_run_media_request(pool, data_run_id).await?,
        };
        if let Some(media_request) = media_request {
            if !matches!(
                media_request.phase.as_str(),
                "backup_write" | "full_readback"
            ) {
                return Err(AppError::Conflict(format!(
                    "unsupported backup media-request phase {}",
                    media_request.phase
                )));
            }
            if request.volume_sequence != Some(media_request.volume_order) {
                return Err(AppError::Conflict(format!(
                    "the backup is waiting for archive volume {}",
                    media_request.volume_order
                )));
            }
            let destructive = media_request.phase == "backup_write";
            if destructive
                && (!request.destructive_overwrite_confirmed
                    || !request.encryption_deviation_confirmed)
            {
                return Err(AppError::Invalid(
                    "a continuation write requires destructive overwrite and unencrypted-format confirmation"
                        .to_owned(),
                ));
            }
            let expected_confirmation = if destructive {
                format!(
                    "OVERWRITE {cartridge_label} FOR BACKUP {request_id} VOLUME {}",
                    media_request.volume_order
                )
            } else {
                format!(
                    "LOAD {cartridge_label} FOR BACKUP {request_id} READBACK VOLUME {}",
                    media_request.volume_order
                )
            };
            if request.confirmation != expected_confirmation {
                return Err(AppError::Invalid(format!(
                    "confirmation must exactly equal {expected_confirmation}"
                )));
            }
            let disposition = store::authorize_data_run_media(
                pool,
                &store::NewDataRunMediaAuthorization {
                    data_run_id,
                    request_id: media_request.id,
                    drive_id: request.drive_id,
                    expected_stable_id: stable_id,
                    cartridge_id: request.cartridge_id,
                    typed_cartridge_label: cartridge_label,
                    loaded_media_confirmed: request.loaded_media_confirmed,
                    destructive_overwrite_confirmed: request.destructive_overwrite_confirmed,
                    confirmation_evidence: json!({
                        "confirmation": request.confirmation,
                        "volume_sequence": media_request.volume_order,
                        "phase": media_request.phase,
                        "drive_id": request.drive_id,
                        "expected_stable_id": stable_id,
                        "typed_cartridge_label": cartridge_label,
                        "loaded_media_confirmed": request.loaded_media_confirmed,
                        "destructive_overwrite_confirmed": request.destructive_overwrite_confirmed,
                        "encryption_deviation_confirmed": request.encryption_deviation_confirmed
                    }),
                },
            )
            .await?;
            return match disposition {
                store::AuthorizeDataRunMediaResult::Authorized {
                    request: authorized_request,
                    volume,
                    idempotent_replay,
                } => {
                    let updated = backup_ops::get_backup_request(pool, request_id)
                        .await
                        .map_err(map_backup_ops_error)?
                        .ok_or(AppError::NotFound)?;
                    let run = build_backup_run_view(pool, &updated).await?;
                    Ok((
                        if idempotent_replay {
                            StatusCode::OK
                        } else {
                            StatusCode::ACCEPTED
                        },
                        Json(json!({
                            "run": run,
                            "media_request": authorized_request,
                            "volume": volume,
                            "durability": "postgresql",
                            "audit": if idempotent_replay { "existing" } else { "appended" },
                            "idempotent_replay": idempotent_replay,
                            "destructive": destructive,
                            "production_eligible": false
                        })),
                    ))
                }
                store::AuthorizeDataRunMediaResult::NotFound => Err(AppError::NotFound),
                store::AuthorizeDataRunMediaResult::Blocked(reason) => {
                    Err(AppError::Blocked(reason))
                }
            };
        }
        let expected_confirmation = format!("OVERWRITE {cartridge_label} FOR BACKUP {request_id}");
        if request.confirmation != expected_confirmation {
            return Err(AppError::Invalid(format!(
                "confirmation must exactly equal {expected_confirmation}"
            )));
        }
        let exact_replay = data_run.profile_id == backup_request.profile_id
            && data_run.job_run_id == backup_request.job_run_id
            && data_run.workflow_kind == "backup"
            && data_run.drive_id == request.drive_id
            && data_run.expected_stable_id == stable_id
            && data_run.cartridge_id == request.cartridge_id
            && data_run.expected_cartridge_label == cartridge_label
            && data_run.scratch_media_confirmed == request.loaded_media_confirmed
            && data_run.destructive_overwrite_confirmed == request.destructive_overwrite_confirmed
            && data_run.encryption_deviation_confirmed == request.encryption_deviation_confirmed;
        if !exact_replay {
            return Err(AppError::Conflict(
                "the backup request already has a different exact media assignment".to_owned(),
            ));
        }
        let run = build_backup_run_view(pool, &backup_request).await?;
        return Ok((
            StatusCode::OK,
            Json(json!({
                "run": run,
                "data_run_id": data_run.id,
                "durability": "postgresql",
                "audit": "existing",
                "idempotent_replay": true,
                "destructive": true,
                "production_eligible": false
            })),
        ));
    }
    if !request.destructive_overwrite_confirmed || !request.encryption_deviation_confirmed {
        return Err(AppError::Invalid(
            "destructive_overwrite_confirmed and encryption_deviation_confirmed must be true"
                .to_owned(),
        ));
    }
    if request
        .volume_sequence
        .is_some_and(|sequence| sequence != 1)
    {
        return Err(AppError::Invalid(
            "the first assigned tape must use volume_sequence 1".to_owned(),
        ));
    }
    let expected_confirmation = format!("OVERWRITE {cartridge_label} FOR BACKUP {request_id}");
    if request.confirmation != expected_confirmation {
        return Err(AppError::Invalid(format!(
            "confirmation must exactly equal {expected_confirmation}"
        )));
    }
    verify_backup_source_preflight(&state, &profile)?;
    match store::attach_single_tape_backup_run(
        pool,
        backup_request.profile_id,
        request_id,
        backup_request.job_run_id,
        &backup_request.trigger_kind,
        backup_request.scheduled_occurrence.as_deref(),
        profile.retention_days,
        request.drive_id,
        stable_id,
        request.cartridge_id,
        cartridge_label,
        request.loaded_media_confirmed,
        request.destructive_overwrite_confirmed,
        request.encryption_deviation_confirmed,
    )
    .await?
    {
        store::QueueSingleTapeDataRunResult::Queued(data_run) => {
            let updated = backup_ops::get_backup_request(pool, request_id)
                .await
                .map_err(map_backup_ops_error)?
                .ok_or(AppError::NotFound)?;
            let run = build_backup_run_view(pool, &updated).await?;
            Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "run": run,
                    "data_run_id": data_run.id,
                    "durability": "postgresql",
                    "audit": "appended",
                    "destructive": true,
                    "production_eligible": false
                })),
            ))
        }
        store::QueueSingleTapeDataRunResult::NotFound => Err(AppError::NotFound),
        store::QueueSingleTapeDataRunResult::Blocked(reason) => Err(AppError::Blocked(reason)),
    }
}

#[derive(Debug, Deserialize)]
struct CancelBackupRunRequest {
    reason: String,
    #[serde(default)]
    requested_by_label: Option<String>,
}

async fn cancel_backup_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CancelBackupRunRequest>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let request_id = parse_uuid_path(&id, "backup run")?;
    let reason = request.reason.trim();
    if reason.is_empty() || reason.chars().count() > 1000 || reason.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "reason must contain 1 to 1000 printable characters".to_owned(),
        ));
    }
    let actor = backup_actor_label(request.requested_by_label.as_deref())?;
    let current = backup_ops::get_backup_request(pool, request_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    let mut idempotent_replay = false;
    if let Some(data_run_id) = current.assigned_data_run_id {
        match store::cancel_single_tape_data_run(pool, data_run_id, &actor, reason).await? {
            store::CancelSingleTapeDataRunResult::Cancelled(_)
            | store::CancelSingleTapeDataRunResult::CancellationRequested(_) => {}
            store::CancelSingleTapeDataRunResult::NotFound => return Err(AppError::NotFound),
            store::CancelSingleTapeDataRunResult::Terminal(run) if run.state == "cancelled" => {
                idempotent_replay = true;
            }
            store::CancelSingleTapeDataRunResult::Terminal(_) => {
                return Err(AppError::Conflict(
                    "the backup run is already terminal".to_owned(),
                ))
            }
        }
    } else {
        let disposition =
            backup_ops::cancel_waiting_backup_request(pool, request_id, &actor, reason)
                .await
                .map_err(map_backup_ops_error)?;
        idempotent_replay = matches!(
            disposition,
            backup_ops::CancelBackupRequestResult::AlreadyCancelled(_)
        );
    }
    let updated = backup_ops::get_backup_request(pool, request_id)
        .await
        .map_err(map_backup_ops_error)?
        .ok_or(AppError::NotFound)?;
    let run = build_backup_run_view(pool, &updated).await?;
    Ok(Json(json!({
        "run": run,
        "reason": reason,
        "durability": "postgresql",
        "audit": if idempotent_replay { "existing" } else { "appended" },
        "idempotent_replay": idempotent_replay
    })))
}

async fn build_backup_run_view(
    pool: &PgPool,
    request: &backup_ops::BackupRequestRecord,
) -> Result<Value, AppError> {
    let profile = backup_ops::get_backup_profile(pool, request.profile_id)
        .await
        .map_err(map_backup_ops_error)?;
    let mut value =
        serde_json::to_value(request).map_err(|error| AppError::Internal(error.to_string()))?;
    let object = value
        .as_object_mut()
        .expect("serialized backup request is an object");
    let mut continuation_write_pending = false;
    let mut continuation_readback_pending = false;
    if let Some(profile) = profile {
        object.insert("source_subpath".to_owned(), json!(profile.source_subpath));
        object.insert("logical_root".to_owned(), json!(profile.logical_root));
        object.insert("retention_days".to_owned(), json!(profile.retention_days));
        object.insert(
            "schedule_enabled".to_owned(),
            json!(profile.schedule_enabled),
        );
        object.insert(
            "schedule_timezone".to_owned(),
            json!(profile.schedule_timezone),
        );
        object.insert("media_pool_id".to_owned(), json!(profile.media_pool_id));
        object.insert("media_pool_name".to_owned(), json!(profile.media_pool_name));
    }
    if let Some(data_run_id) = request.assigned_data_run_id {
        if let Some(data_run) = store::get_single_tape_data_run(pool, data_run_id).await? {
            let sanitized = bounded_restore_evidence_value(
                &serde_json::to_value(&data_run)
                    .map_err(|error| AppError::Internal(error.to_string()))?,
            );
            if let Some(data) = sanitized.as_object() {
                for key in [
                    "expected_stable_id",
                    "expected_path",
                    "expected_vendor",
                    "expected_model",
                    "expected_serial",
                    "expected_wwn",
                    "expected_firmware",
                    "expected_cartridge_label",
                    "expected_media_generation",
                    "expected_media_format",
                    "media_write_started",
                    "media_write_started_at",
                    "cancel_requested_at",
                    "cancel_requested_by_label",
                    "result",
                    "compression_mode",
                    "resolved_compression",
                    "spanning_enabled",
                ] {
                    if let Some(item) = data.get(key) {
                        object.insert(key.to_owned(), item.clone());
                    }
                }
                object.insert("data_run".to_owned(), sanitized);
            }
            let volumes = store::data_run_volumes(pool, data_run_id).await?;
            object.insert("volumes".to_owned(), json!(volumes));
            if let Some(media_request) =
                store::open_data_run_media_request(pool, data_run_id).await?
            {
                let media_pending = media_request.state == "pending";
                continuation_write_pending = media_pending
                    && media_request.phase == "backup_write"
                    && media_request.volume_order > 1;
                continuation_readback_pending =
                    media_pending && media_request.phase == "full_readback";
                object.insert("media_request".to_owned(), json!(media_request));
            }
        }
    }
    object.insert(
        "allowed_actions".to_owned(),
        json!({
            "assign_media": request.state == "waiting_for_media"
                && (request.assigned_data_run_id.is_none()
                    || continuation_readback_pending),
            "assign_next_media": request.state == "waiting_for_media"
                && continuation_write_pending,
            "cancel": !matches!(request.state.as_str(), "verified" | "failed" | "cancelled" | "completed")
        }),
    );
    object.insert("production_eligible".to_owned(), Value::Bool(false));
    Ok(value)
}

async fn media(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let records = match state.pool.as_ref() {
        Some(pool) => media_ops::list_media_inventory(pool)
            .await
            .map_err(map_media_ops_error)?,
        None => Vec::new(),
    };
    let media = records.iter().map(media_inventory_view).collect::<Vec<_>>();
    Ok(Json(json!({"media":media})))
}

#[derive(Debug, Deserialize)]
struct CreateMediaRequest {
    internal_label: String,
    #[serde(default)]
    barcode: Option<String>,
    media_generation: i16,
    #[serde(default)]
    media_format: Option<String>,
    #[serde(default)]
    worm: bool,
}

impl CreateMediaRequest {
    fn validate(self) -> Result<store::NewMedia, AppError> {
        let internal_label = self.internal_label.trim().to_owned();
        if internal_label.is_empty() {
            return Err(AppError::Invalid("internal_label is required".to_owned()));
        }
        if internal_label.len() > 128 {
            return Err(AppError::Invalid(
                "internal_label must be at most 128 bytes".to_owned(),
            ));
        }
        if !internal_label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._:".contains(character))
        {
            return Err(AppError::Invalid(
                "internal_label may contain only letters, numbers, hyphen, underscore, period, and colon"
                    .to_owned(),
            ));
        }

        let barcode = self
            .barcode
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if barcode
            .as_ref()
            .is_some_and(|value| value.chars().count() > 128)
        {
            return Err(AppError::Invalid(
                "barcode must be at most 128 characters".to_owned(),
            ));
        }
        if !(1..=10).contains(&self.media_generation) {
            return Err(AppError::Invalid(
                "media_generation must be between 1 and 10".to_owned(),
            ));
        }
        let media_format = self
            .media_format
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("LTO-{}", self.media_generation));
        if media_format.chars().count() > 64 {
            return Err(AppError::Invalid(
                "media_format must be at most 64 characters".to_owned(),
            ));
        }

        Ok(store::NewMedia {
            internal_label,
            barcode,
            media_generation: self.media_generation,
            media_format,
            worm: self.worm,
        })
    }
}

async fn create_media(
    State(state): State<AppState>,
    Json(request): Json<CreateMediaRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pool = require_pool(&state)?;
    let media = request.validate()?;
    let record = match store::create_media(pool, &media).await {
        Ok(record) => record,
        Err(error) if is_unique_violation(&error) => {
            return Err(AppError::Conflict(
                "a cartridge with this internal label or barcode already exists".to_owned(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    let detail = media_ops::get_media_detail(pool, record.id)
        .await
        .map_err(map_media_ops_error)?;
    let media = media_inventory_view(&detail.cartridge);
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "media":media,
            "durability":"postgresql",
            "audit":"appended"
        })),
    ))
}

async fn media_pools(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let pools = match state.pool.as_ref() {
        Some(pool) => media_ops::list_media_pools(pool)
            .await
            .map_err(map_media_ops_error)?,
        None => Vec::new(),
    };
    Ok(Json(json!({"pools":pools})))
}

#[derive(Debug, Deserialize)]
struct CreateMediaPoolApiRequest {
    name: String,
    purpose: media_ops::MediaPoolPurpose,
    #[serde(default)]
    indefinite_retention: bool,
    #[serde(default = "empty_json_object")]
    details: Value,
    #[serde(default)]
    actor: Option<String>,
}

async fn create_media_pool(
    State(state): State<AppState>,
    Json(request): Json<CreateMediaPoolApiRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pool = require_pool(&state)?;
    let media_pool = media_ops::create_media_pool(
        pool,
        &media_ops::CreateMediaPoolRequest {
            name: request.name,
            purpose: request.purpose,
            indefinite_retention: request.indefinite_retention,
            details: request.details,
            actor: media_actor(request.actor.as_deref()),
            correlation_id: Uuid::now_v7(),
        },
    )
    .await
    .map_err(map_media_ops_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "pool":media_pool,
            "durability":"postgresql",
            "audit":"appended"
        })),
    ))
}

async fn media_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "cartridge")?;
    let detail = media_ops::get_media_detail(pool, id)
        .await
        .map_err(map_media_ops_error)?;
    Ok(Json(json!({"media":detail})))
}

#[derive(Debug, Deserialize)]
struct AssignMediaPoolApiRequest {
    #[serde(default)]
    pool_id: Option<Uuid>,
    expected_state_version: i64,
    idempotency_key: String,
    #[serde(default)]
    actor: Option<String>,
}

async fn assign_media_pool(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<AssignMediaPoolApiRequest>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let cartridge_id = parse_uuid_path(&id, "cartridge")?;
    let receipt = media_ops::assign_media_pool(
        pool,
        cartridge_id,
        &media_ops::AssignMediaPoolRequest {
            pool_id: request.pool_id,
            expected_state_version: request.expected_state_version,
            idempotency_key: request.idempotency_key,
            actor: media_actor(request.actor.as_deref()),
            correlation_id: Uuid::now_v7(),
        },
    )
    .await
    .map_err(map_media_ops_error)?;
    let detail = media_ops::get_media_detail(pool, cartridge_id)
        .await
        .map_err(map_media_ops_error)?;
    Ok(Json(json!({
        "media":detail,
        "receipt":receipt,
        "durability":"postgresql",
        "audit":"appended"
    })))
}

#[derive(Debug, Deserialize)]
struct CreateMediaHoldApiRequest {
    kind: media_ops::MediaHoldKind,
    reason: String,
    #[serde(default)]
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    expected_state_version: i64,
    idempotency_key: String,
    #[serde(default)]
    actor: Option<String>,
}

async fn create_media_hold(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CreateMediaHoldApiRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pool = require_pool(&state)?;
    let cartridge_id = parse_uuid_path(&id, "cartridge")?;
    let receipt = media_ops::add_media_hold(
        pool,
        cartridge_id,
        &media_ops::CreateMediaHoldRequest {
            kind: request.kind,
            reason: request.reason,
            expires_at: request.expires_at,
            expected_state_version: request.expected_state_version,
            idempotency_key: request.idempotency_key,
            actor: media_actor(request.actor.as_deref()),
            correlation_id: Uuid::now_v7(),
        },
    )
    .await
    .map_err(map_media_ops_error)?;
    let detail = media_ops::get_media_detail(pool, cartridge_id)
        .await
        .map_err(map_media_ops_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "media":detail,
            "receipt":receipt,
            "durability":"postgresql",
            "audit":"appended"
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct ReleaseMediaHoldApiRequest {
    reason: String,
    expected_state_version: i64,
    idempotency_key: String,
    #[serde(default)]
    actor: Option<String>,
}

async fn release_media_hold(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ReleaseMediaHoldApiRequest>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let hold_id = parse_uuid_path(&id, "hold")?;
    let receipt = media_ops::release_media_hold(
        pool,
        hold_id,
        &media_ops::ReleaseMediaHoldRequest {
            reason: request.reason,
            expected_state_version: request.expected_state_version,
            idempotency_key: request.idempotency_key,
            actor: media_actor(request.actor.as_deref()),
            correlation_id: Uuid::now_v7(),
        },
    )
    .await
    .map_err(map_media_ops_error)?;
    let detail = media_ops::get_media_detail(pool, receipt.cartridge_id)
        .await
        .map_err(map_media_ops_error)?;
    Ok(Json(json!({
        "media":detail,
        "receipt":receipt,
        "durability":"postgresql",
        "audit":"appended"
    })))
}

#[derive(Debug, Deserialize)]
struct MediaReuseQuery {
    drive_id: Uuid,
}

async fn evaluate_media_reuse(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MediaReuseQuery>,
) -> Result<Response, AppError> {
    let pool = require_pool(&state)?;
    let cartridge_id = parse_uuid_path(&id, "cartridge")?;
    let evaluation = media_ops::evaluate_media_reuse(pool, cartridge_id, query.drive_id)
        .await
        .map_err(map_media_ops_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "evaluation":evaluation,
            "read_only":true,
            "destructive_operation_available":false
        })),
    )
        .into_response())
}

async fn drives(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let (drives, slots, operations) = match state.pool.as_ref() {
        Some(pool) => (
            store::list_drives(pool).await?,
            store::list_slots(pool).await?,
            store::list_drive_operations(pool).await?,
        ),
        None => (Vec::new(), Vec::new(), Vec::new()),
    };
    Ok(Json(
        json!({"drives":drives,"slots":slots,"operations":operations}),
    ))
}

#[derive(Debug, Deserialize)]
struct ReconcileDriveRequest {
    stable_id: String,
}

async fn reconcile_drive(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ReconcileDriveRequest>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| AppError::Invalid("drive id must be a UUID".to_owned()))?;
    let stable_id = request.stable_id.trim();
    if stable_id.is_empty()
        || stable_id.chars().count() > 512
        || stable_id.chars().any(char::is_control)
    {
        return Err(AppError::Invalid(
            "stable_id must contain 1 to 512 characters".to_owned(),
        ));
    }

    match store::reconcile_drive(pool, id, stable_id).await? {
        store::ReconcileDriveResult::Reconciled(drive) => Ok(Json(json!({
            "drive":drive,
            "audit":"appended",
            "physical_qualification":"not_established"
        }))),
        store::ReconcileDriveResult::NotFound => Err(AppError::NotFound),
        store::ReconcileDriveResult::Blocked => Err(AppError::Blocked(
            "drive reconciliation requires the exact observed stable identity, current device access, no identity conflict, and no outstanding tape I/O"
                .to_owned(),
        )),
    }
}

#[derive(Debug, Deserialize)]
struct DriveActionRequest {
    stable_id: String,
    action: SasMotionOperation,
    confirmation: String,
    scratch_media_confirmed: bool,
}

impl DriveActionRequest {
    fn expected_confirmation(&self) -> String {
        format!(
            "{} SCRATCH MEDIA; PRIOR WRITE MAY BE FINALIZED",
            self.action.as_str().to_ascii_uppercase()
        )
    }

    fn validate(&self) -> Result<&str, AppError> {
        let stable_id = self.stable_id.trim();
        if stable_id.is_empty()
            || stable_id.chars().count() > 512
            || stable_id.chars().any(char::is_control)
        {
            return Err(AppError::Invalid(
                "stable_id must contain 1 to 512 characters".to_owned(),
            ));
        }
        if !self.scratch_media_confirmed {
            return Err(AppError::Invalid(
                "scratch_media_confirmed must be true for unqualified lab motion".to_owned(),
            ));
        }
        if self.confirmation != self.expected_confirmation() {
            return Err(AppError::Invalid(format!(
                "confirmation must exactly equal {}",
                self.expected_confirmation()
            )));
        }
        Ok(stable_id)
    }
}

async fn queue_drive_action(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<DriveActionRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if !state.lab_tape_motion_enabled {
        return Err(AppError::Blocked(
            "unqualified lab tape motion is disabled; set TAPESCALE_LAB_TAPE_MOTION_ENABLED=true on the API and worker only for an attended test host"
                .to_owned(),
        ));
    }
    let pool = require_pool(&state)?;
    let drive_id = Uuid::parse_str(&id)
        .map_err(|_| AppError::Invalid("drive id must be a UUID".to_owned()))?;
    let stable_id = request.validate()?;
    match store::queue_drive_operation(
        pool,
        drive_id,
        stable_id,
        request.action,
        request.scratch_media_confirmed,
    )
    .await?
    {
        store::QueueDriveOperationResult::Queued(operation) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "operation": operation,
                "durability": "postgresql",
                "audit": "appended",
                "qualification": "unqualified_lab_motion",
                "data_operations": "blocked"
            })),
        )),
        store::QueueDriveOperationResult::NotFound => Err(AppError::NotFound),
        store::QueueDriveOperationResult::Blocked(reason) => Err(AppError::Blocked(reason)),
    }
}

async fn locations(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let locations = match state.pool.as_ref() {
        Some(pool) => custody_ops::list_locations(pool)
            .await
            .map_err(map_custody_error)?,
        None => Vec::new(),
    };
    Ok(Json(json!({"locations":locations})))
}

#[derive(Debug, Deserialize)]
struct CreateLocationApiRequest {
    name: String,
    kind: custody_ops::LocationKind,
    #[serde(default = "empty_json_object")]
    details: Value,
    #[serde(default)]
    actor: Option<String>,
}

async fn create_location(
    State(state): State<AppState>,
    Json(request): Json<CreateLocationApiRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pool = require_pool(&state)?;
    let location = custody_ops::create_location(
        pool,
        custody_ops::CreateLocationRequest {
            name: request.name,
            kind: request.kind,
            details: request.details,
            actor: custody_actor(request.actor.as_deref()),
            correlation_id: Uuid::now_v7(),
        },
    )
    .await
    .map_err(map_custody_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "location": location,
            "durability": "postgresql",
            "audit": "appended"
        })),
    ))
}

async fn movements(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let movements = match state.pool.as_ref() {
        Some(pool) => custody_ops::list_movements(pool, custody_ops::MovementListQuery::default())
            .await
            .map_err(map_custody_error)?,
        None => Vec::new(),
    };
    Ok(Json(json!({"movements":movements})))
}

#[derive(Debug, Deserialize)]
struct CreateMovementApiRequest {
    #[serde(default)]
    external_id: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
    kind: custody_ops::MovementKind,
    #[serde(default)]
    source_location_id: Option<Uuid>,
    destination_location_id: Uuid,
    #[serde(default)]
    transit_location_id: Option<Uuid>,
    cartridge_ids: Vec<Uuid>,
    #[serde(default)]
    readiness: Option<Value>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    assigned_user_id: Option<Uuid>,
    due_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    actor: Option<String>,
}

async fn create_movement(
    State(state): State<AppState>,
    Json(request): Json<CreateMovementApiRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pool = require_pool(&state)?;
    let request_id = Uuid::now_v7();
    let idempotency_key = request
        .idempotency_key
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("movement-create:{request_id}"));
    let external_id = request
        .external_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| generated_movement_external_id(&idempotency_key));
    let mut readiness = request.readiness.unwrap_or_else(empty_json_object);
    if !readiness.is_object() {
        return Err(AppError::Invalid(
            "readiness must be a JSON object".to_owned(),
        ));
    }
    if let Some(reason) = request
        .reason
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        readiness
            .as_object_mut()
            .expect("readiness was checked as an object")
            .insert("reason".to_owned(), json!(reason));
    }
    let result = custody_ops::create_movement(
        pool,
        custody_ops::CreateMovementRequest {
            external_id,
            idempotency_key,
            kind: request.kind,
            source_location_id: request.source_location_id,
            destination_location_id: request.destination_location_id,
            transit_location_id: request.transit_location_id,
            cartridge_ids: request.cartridge_ids,
            readiness,
            assigned_user_id: request.assigned_user_id,
            due_at: request.due_at,
            actor: custody_actor(request.actor.as_deref()),
            correlation_id: Uuid::now_v7(),
        },
    )
    .await
    .map_err(map_custody_error)?;
    let detail = custody_ops::get_movement(pool, result.movement_task_id)
        .await
        .map_err(map_custody_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "movement": detail,
            "receipt": result,
            "durability": "postgresql",
            "audit": "appended",
            "custody_changed": false
        })),
    ))
}

async fn movement(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let detail = resolve_movement(pool, &id).await?;
    Ok(Json(json!({"movement": detail})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MovementConfirmationAction {
    Departure,
    Arrival,
    Direct,
}

#[derive(Debug, Deserialize)]
struct MovementConfirmation {
    action: MovementConfirmationAction,
    cartridge_ids: Vec<Uuid>,
    expected_confirmation_version: i64,
    physically_confirmed: bool,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    actor: Option<String>,
}

async fn confirm_movement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(confirmation): Json<MovementConfirmation>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let movement_id = resolve_movement_id(pool, &id).await?;
    let operation_id = Uuid::now_v7();
    let request = custody_ops::PhysicalConfirmationRequest {
        cartridge_ids: confirmation.cartridge_ids,
        idempotency_key: confirmation
            .idempotency_key
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("movement-confirm:{operation_id}")),
        expected_confirmation_version: confirmation.expected_confirmation_version,
        physically_confirmed: confirmation.physically_confirmed,
        note: confirmation.note,
        actor: custody_actor(confirmation.actor.as_deref()),
        correlation_id: Uuid::now_v7(),
    };
    let receipt = match confirmation.action {
        MovementConfirmationAction::Departure => {
            custody_ops::confirm_departure(pool, movement_id, request).await
        }
        MovementConfirmationAction::Arrival => {
            custody_ops::confirm_arrival(pool, movement_id, request).await
        }
        MovementConfirmationAction::Direct => {
            custody_ops::confirm_direct_move(pool, movement_id, request).await
        }
    }
    .map_err(map_custody_error)?;
    let detail = custody_ops::get_movement(pool, movement_id)
        .await
        .map_err(map_custody_error)?;
    Ok(Json(json!({
        "movement": detail,
        "receipt": receipt,
        "durability": "postgresql",
        "audit": "appended",
        "custody_changed": true
    })))
}

#[derive(Debug, Deserialize)]
struct CancelMovementApiRequest {
    expected_confirmation_version: i64,
    reason: String,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    actor: Option<String>,
}

async fn cancel_movement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CancelMovementApiRequest>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let movement_id = resolve_movement_id(pool, &id).await?;
    let operation_id = Uuid::now_v7();
    let receipt = custody_ops::cancel_movement(
        pool,
        movement_id,
        custody_ops::CancelMovementRequest {
            idempotency_key: request
                .idempotency_key
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("movement-cancel:{operation_id}")),
            expected_confirmation_version: request.expected_confirmation_version,
            reason: request.reason,
            actor: custody_actor(request.actor.as_deref()),
            correlation_id: Uuid::now_v7(),
        },
    )
    .await
    .map_err(map_custody_error)?;
    let detail = custody_ops::get_movement(pool, movement_id)
        .await
        .map_err(map_custody_error)?;
    Ok(Json(
        json!({"movement":detail,"receipt":receipt,"custody_changed":false}),
    ))
}

#[derive(Debug, Deserialize)]
struct CorrectMovementCustodyApiRequest {
    corrects_event_id: Uuid,
    cartridge_id: Uuid,
    #[serde(default)]
    expected_current_location_id: Option<Uuid>,
    confirmed_location_id: Uuid,
    expected_confirmation_version: i64,
    physically_confirmed: bool,
    note: String,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    actor: Option<String>,
}

async fn correct_movement_custody(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CorrectMovementCustodyApiRequest>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let movement_id = resolve_movement_id(pool, &id).await?;
    let operation_id = Uuid::now_v7();
    let receipt = custody_ops::append_custody_correction(
        pool,
        movement_id,
        custody_ops::AppendCorrectionRequest {
            corrects_event_id: request.corrects_event_id,
            cartridge_id: request.cartridge_id,
            expected_current_location_id: request.expected_current_location_id,
            confirmed_location_id: request.confirmed_location_id,
            idempotency_key: request
                .idempotency_key
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("movement-correction:{operation_id}")),
            expected_confirmation_version: request.expected_confirmation_version,
            physically_confirmed: request.physically_confirmed,
            note: request.note,
            actor: custody_actor(request.actor.as_deref()),
            correlation_id: Uuid::now_v7(),
        },
    )
    .await
    .map_err(map_custody_error)?;
    let detail = custody_ops::get_movement(pool, movement_id)
        .await
        .map_err(map_custody_error)?;
    Ok(Json(
        json!({"movement":detail,"receipt":receipt,"custody_changed":true}),
    ))
}

async fn verification(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let verification = match state.pool.as_ref() {
        Some(pool) => store::list_verification(pool).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({"verification":verification})))
}

async fn recovery_points(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let recovery_points = match state.pool.as_ref() {
        Some(pool) => store::list_recovery_points(pool).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({"recovery_points":recovery_points})))
}

async fn recovery_point(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "recovery point")?;
    let aggregate = store::get_recovery_point_aggregate(pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(
        serde_json::to_value(aggregate).map_err(|error| AppError::Internal(error.to_string()))?,
    ))
}

async fn recovery_point_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<BrowserQuery>,
) -> Result<Response, AppError> {
    let pool = require_pool(&state)?;
    let recovery_point_id = parse_uuid_path(&id, "recovery point")?;
    let binding = store::recovery_point_manifest_binding(pool, recovery_point_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let reference = RetainedManifestReference {
        storage_ref: binding.storage_ref.ok_or_else(|| {
            AppError::Blocked("the recovery point has no retained source manifest".to_owned())
        })?,
        artifact_sha256: binding.artifact_sha256.ok_or_else(|| {
            AppError::Blocked("the recovery point has no retained source manifest".to_owned())
        })?,
        artifact_size_bytes: binding.artifact_size_bytes.ok_or_else(|| {
            AppError::Blocked("the recovery point has no retained source manifest".to_owned())
        })?,
        manifest_digest: binding.manifest_digest.ok_or_else(|| {
            AppError::Blocked("the recovery point has no retained source manifest".to_owned())
        })?,
    };
    // A retained manifest can be a large, valid artifact. Serialize this
    // memory-heavy read/hash/decode path so concurrent unauthenticated reads
    // cannot multiply its peak memory use inside the API container.
    let permit = state
        .retained_manifest_browser_slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::Internal("manifest browser concurrency gate closed".to_owned()))?;
    let evidence_root = state.evidence_root.clone();
    let listing = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        source_browser::browse_retained_manifest(&evidence_root, &reference, &query.path)
    })
    .await
    .map_err(|error| AppError::Internal(format!("manifest browser task failed: {error}")))?
    .map_err(map_source_browser_error)?;
    let mut response =
        serde_json::to_value(listing).map_err(|error| AppError::Internal(error.to_string()))?;
    let object = response
        .as_object_mut()
        .expect("serialized browser listing is an object");
    object.insert("recovery_point_id".to_owned(), json!(recovery_point_id));
    object.insert("browse_only".to_owned(), Value::Bool(true));
    object.insert(
        "path_selective_restore_supported".to_owned(),
        Value::Bool(false),
    );
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(response)).into_response())
}

async fn recovery_point_evidence(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    const MAX_EXPORT_BYTES: usize = 4 * 1024 * 1024;

    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "recovery point")?;
    let aggregate = store::get_recovery_point_aggregate(pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let evidence = json!({
        "format": "tapescale.recovery-point-evidence.v1",
        "canonicalization": "UTF-8 compact JSON with lexicographically ordered object keys",
        "execution_claim": "none; this export records retained recovery evidence and prerequisites",
        "bounds": {
            "maximum_serialized_bytes": MAX_EXPORT_BYTES,
            "row_and_value_caps": aggregate.caps
        },
        "recovery_point": aggregate.recovery_point,
        "volumes": aggregate.volumes,
        "dependencies": aggregate.dependencies,
        "holds": aggregate.holds,
        "verifications": aggregate.verifications,
        "origin": aggregate.origin,
        "events": aggregate.events,
        "artifacts": aggregate.artifacts
    });
    let body = canonical_json(&evidence)?;
    if body.len() > MAX_EXPORT_BYTES {
        return Err(AppError::Blocked(format!(
            "the bounded evidence export is {} bytes, exceeding the {} byte response limit",
            body.len(),
            MAX_EXPORT_BYTES
        )));
    }
    let digest = format!("{:x}", Sha256::digest(&body));
    let disposition =
        format!("attachment; filename=\"tapescale-recovery-point-{id}-evidence.json\"");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ETAG, format!("\"sha256-{digest}\""))
        .header("x-content-type-options", "nosniff")
        .body(Body::from(body))
        .map_err(|error| AppError::Internal(error.to_string()))
}

#[derive(Debug, Deserialize)]
struct CreateRecoveryExerciseProfileRequest {
    name: String,
    source_subpath: String,
    restore_subpath: String,
    immutable_snapshot_confirmed: bool,
}

async fn recovery_exercise_profiles(
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let profiles = match state.pool.as_ref() {
        Some(pool) => store::list_single_tape_data_profiles(pool).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({
        "profiles": profiles,
        "scope": "laboratory_single_tape_full_file_roundtrip",
        "production_eligible": false
    })))
}

async fn create_recovery_exercise_profile(
    State(state): State<AppState>,
    Json(request): Json<CreateRecoveryExerciseProfileRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    if !request.immutable_snapshot_confirmed {
        return Err(AppError::Invalid(
            "immutable_snapshot_confirmed must be true; mount an immutable TrueNAS snapshot beneath the configured source root"
                .to_owned(),
        ));
    }
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "name must contain 1 to 128 printable characters".to_owned(),
        ));
    }
    let source_subpath = normalized_relative_path(&request.source_subpath, "source_subpath")?;
    let restore_subpath = normalized_relative_path(&request.restore_subpath, "restore_subpath")?;
    let source_mount = resolve_existing_selection(&state.source_root, &source_subpath)
        .map_err(|error| AppError::Blocked(error.to_string()))?;
    verify_read_only_mount(&source_mount).map_err(|error| AppError::Blocked(error.to_string()))?;
    let restore_probe =
        isolated_restore_destination(&state.restore_root, &restore_subpath, Uuid::nil())
            .map_err(|error| AppError::Blocked(error.to_string()))?;
    let restore_root = restore_probe
        .parent()
        .ok_or_else(|| AppError::Invalid("restore_subpath is invalid".to_owned()))?;
    let source_mount_path = utf8_path(&source_mount, "resolved source path")?;
    let restore_root_path = utf8_path(restore_root, "resolved restore path")?;
    let profile = store::NewSingleTapeDataProfile {
        name: name.to_owned(),
        source_subpath,
        restore_subpath,
        source_mount_path,
        restore_root_path,
        immutable_snapshot_confirmed: true,
    };
    match store::create_single_tape_data_profile(pool, &profile).await? {
        store::CreateSingleTapeDataProfileResult::Created(profile) => Ok((
            StatusCode::CREATED,
            Json(json!({
                "profile": profile,
                "source_mount": "read_only_verified",
                "scope": "laboratory_single_tape_full_file_roundtrip",
                "production_eligible": false
            })),
        )),
        store::CreateSingleTapeDataProfileResult::NotFound => Err(AppError::NotFound),
        store::CreateSingleTapeDataProfileResult::Blocked(reason) => Err(AppError::Blocked(reason)),
    }
}

#[derive(Debug, Deserialize)]
struct QueueRecoveryExerciseRequest {
    profile_id: Uuid,
    drive_id: Uuid,
    stable_id: String,
    cartridge_id: Uuid,
    cartridge_label: String,
    confirmation: String,
    scratch_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
    encryption_deviation_confirmed: bool,
}

async fn recovery_exercises(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let exercises = match state.pool.as_ref() {
        Some(pool) => store::list_single_tape_data_runs(pool).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({
        "exercises": exercises,
        "scope": "laboratory_single_tape_full_file_roundtrip",
        "production_eligible": false
    })))
}

async fn recovery_exercise(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| AppError::Invalid("recovery exercise id must be a UUID".to_owned()))?;
    let exercise = store::get_single_tape_data_run(pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let events = store::single_tape_data_events(pool, id).await?;
    let artifacts = store::single_tape_data_artifacts(pool, id).await?;
    Ok(Json(json!({
        "exercise": exercise,
        "events": events,
        "artifacts": artifacts,
        "scope": "laboratory_single_tape_full_file_roundtrip",
        "production_eligible": false
    })))
}

async fn queue_recovery_exercise(
    State(state): State<AppState>,
    Json(request): Json<QueueRecoveryExerciseRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    let profile = store::list_single_tape_data_profiles(pool)
        .await?
        .into_iter()
        .find(|profile| profile.id == request.profile_id)
        .ok_or(AppError::NotFound)?;
    let stable_id = request.stable_id.trim();
    if stable_id.is_empty()
        || stable_id.chars().count() > 512
        || stable_id.chars().any(char::is_control)
    {
        return Err(AppError::Invalid(
            "stable_id must contain 1 to 512 characters".to_owned(),
        ));
    }
    let cartridge_label = request.cartridge_label.trim();
    if cartridge_label.is_empty()
        || cartridge_label.chars().count() > 128
        || cartridge_label.chars().any(char::is_control)
    {
        return Err(AppError::Invalid(
            "cartridge_label must contain 1 to 128 printable characters".to_owned(),
        ));
    }
    if !request.scratch_media_confirmed
        || !request.destructive_overwrite_confirmed
        || !request.encryption_deviation_confirmed
    {
        return Err(AppError::Invalid(
            "scratch_media_confirmed, destructive_overwrite_confirmed, and encryption_deviation_confirmed must all be true"
                .to_owned(),
        ));
    }
    let expected_confirmation = format!(
        "WRITE {} TO {} AND RESTORE VERIFY",
        profile.name, cartridge_label
    );
    if request.confirmation != expected_confirmation {
        return Err(AppError::Invalid(format!(
            "confirmation must exactly equal {expected_confirmation}"
        )));
    }
    match store::queue_single_tape_data_run(
        pool,
        request.profile_id,
        request.drive_id,
        stable_id,
        request.cartridge_id,
        cartridge_label,
        request.scratch_media_confirmed,
        request.destructive_overwrite_confirmed,
        request.encryption_deviation_confirmed,
    )
    .await?
    {
        store::QueueSingleTapeDataRunResult::Queued(exercise) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "exercise": exercise,
                "durability": "postgresql",
                "audit": "appended",
                "destructive": true,
                "encryption": "explicitly_disabled_for_g0_2_lab_deviation",
                "production_eligible": false
            })),
        )),
        store::QueueSingleTapeDataRunResult::NotFound => Err(AppError::NotFound),
        store::QueueSingleTapeDataRunResult::Blocked(reason) => Err(AppError::Blocked(reason)),
    }
}

fn normalized_relative_path(value: &str, field: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    let safe = validate_archive_path(std::path::Path::new(trimmed)).map_err(|_| {
        AppError::Invalid(format!(
            "{field} must be a non-empty contained relative path without traversal"
        ))
    })?;
    safe.as_path()
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Invalid(format!("{field} must be valid UTF-8")))
}

fn utf8_path(path: &std::path::Path, label: &str) -> Result<String, AppError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Blocked(format!("{label} must be valid UTF-8")))
}

fn require_file_roundtrip(state: &AppState) -> Result<(), AppError> {
    if state.lab_file_roundtrip_enabled {
        Ok(())
    } else {
        Err(AppError::Blocked(
            "the attended Bacula backup and recovery path is disabled; set TAPESCALE_LAB_FILE_ROUNDTRIP_ENABLED=true on the API and worker for operational full backups or the G0.2 single-tape recovery exercise"
                .to_owned(),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct RestorePlanRequest {
    #[serde(default)]
    recovery_point_id: Option<Uuid>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    conflict_policy: Option<String>,
    #[serde(default)]
    key_available: bool,
    #[serde(default)]
    key_recovery_package_tested: bool,
    #[serde(default)]
    destination_approved: bool,
    #[serde(default = "default_true", alias = "destination_is_new")]
    is_new_destination: bool,
    #[serde(default)]
    conflicts_found: usize,
    #[serde(default)]
    acl_semantics_supported: bool,
    #[serde(default)]
    identity_mapping_complete: bool,
    #[serde(default)]
    estimated_required_bytes: Option<u64>,
    #[serde(default)]
    available_bytes: Option<u64>,
    #[serde(default)]
    required_free_reserve_bytes: u64,
    #[serde(default)]
    administrator_reauthenticated: bool,
    #[serde(default)]
    destructive_preview_accepted: bool,
    #[serde(default)]
    automatic_publication_requested: bool,
}

async fn plan_restore(
    State(state): State<AppState>,
    Json(request): Json<RestorePlanRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pool = require_pool(&state)?;
    let recovery_point_id = request.recovery_point_id.ok_or_else(|| {
        AppError::Invalid("recovery_point_id is required and must be a UUID".to_owned())
    })?;
    let destination = request
        .destination
        .as_deref()
        .ok_or_else(|| AppError::Invalid("destination is required".to_owned()))?;
    let destination = validate_archive_path(std::path::Path::new(destination))
        .map_err(|_| {
            AppError::Invalid(
                "destination must be a non-empty contained dataset-relative path".to_owned(),
            )
        })?
        .as_path()
        .to_str()
        .ok_or_else(|| AppError::Invalid("destination must be valid UTF-8".to_owned()))?
        .to_owned();

    let mut aggregate = store::get_recovery_point_aggregate(pool, recovery_point_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mode = parse_restore_mode(
        request.mode.as_deref(),
        &aggregate.recovery_point.representation,
    )?;
    ensure_restore_mode_matches(mode, &aggregate.recovery_point.representation)?;
    let conflict_policy = parse_conflict_policy(request.conflict_policy.as_deref())?;

    let chain_ids = dedupe_preserving_order(
        aggregate
            .dependencies
            .iter()
            .map(|dependency| dependency.id)
            .chain(std::iter::once(recovery_point_id)),
    );
    aggregate.volumes.sort_by_key(|volume| {
        (
            chain_ids
                .iter()
                .position(|id| *id == volume.recovery_point_id)
                .unwrap_or(usize::MAX),
            volume.volume_order,
            volume.id,
        )
    });

    let mut missing_dependencies = Vec::new();
    if aggregate.caps.dependencies_truncated {
        missing_dependencies
            .push("dependency graph exceeds the 128-node planning bound".to_owned());
    }
    if aggregate.caps.volumes_truncated {
        missing_dependencies
            .push("required media exceeds the 256-volume planning bound".to_owned());
    }
    for dependency in &aggregate.dependencies {
        if dependency.status != "verified" {
            missing_dependencies.push(format!(
                "recovery point {} is {}",
                dependency.id, dependency.status
            ));
        }
    }
    if aggregate.recovery_point.status != "verified" {
        missing_dependencies.push(format!(
            "selected recovery point {} is {}",
            aggregate.recovery_point.id, aggregate.recovery_point.status
        ));
    }
    for chain_id in &chain_ids {
        if !aggregate
            .volumes
            .iter()
            .any(|volume| volume.recovery_point_id == *chain_id && volume.finalized)
        {
            missing_dependencies.push(format!(
                "recovery point {chain_id} has no finalized media record"
            ));
        }
    }
    missing_dependencies.sort();
    missing_dependencies.dedup();

    let mut seen_tape_ids = Vec::new();
    let tapes = aggregate
        .volumes
        .iter()
        .filter(|volume| {
            if seen_tape_ids.contains(&volume.cartridge_id) {
                false
            } else {
                seen_tape_ids.push(volume.cartridge_id);
                true
            }
        })
        .map(|volume| TapePrerequisite {
            tape_id: volume.internal_label.clone(),
            available: volume.available,
            confirmed_location: volume.location_name.clone(),
            compatible_qualified_drive_ids: volume
                .compatible_qualified_drives
                .iter()
                .map(|drive| drive.stable_id.clone())
                .collect(),
        })
        .collect::<Vec<_>>();

    let mut key_lineage_ids = aggregate
        .dependencies
        .iter()
        .filter_map(|dependency| dependency.key_lineage_id.clone())
        .collect::<Vec<_>>();
    if let Some(key) = &aggregate.recovery_point.key_lineage_id {
        key_lineage_ids.push(key.clone());
    }
    key_lineage_ids.sort();
    key_lineage_ids.dedup();
    let keys = key_lineage_ids
        .iter()
        .map(|key_id| KeyPrerequisite {
            key_id: key_id.clone(),
            available: request.key_available,
            recovery_package_tested: request.key_recovery_package_tested,
        })
        .collect::<Vec<_>>();

    let all_chain_full_readback = chain_ids.iter().all(|id| {
        let mut expected_media_ids = aggregate
            .volumes
            .iter()
            .filter(|volume| volume.recovery_point_id == *id)
            .collect::<Vec<_>>();
        expected_media_ids.sort_by_key(|volume| volume.volume_order);
        let expected_media_ids = expected_media_ids
            .iter()
            .map(|volume| volume.cartridge_id)
            .collect::<Vec<_>>();
        !expected_media_ids.is_empty()
            && aggregate.verifications.iter().any(|verification| {
                verification.recovery_point_id == *id
                    && verification.kind == "full_readback"
                    && verification.outcome == "passed"
                    && verification.finished_at.is_some()
                    && verification.media_ids == expected_media_ids
            })
    });
    let restore_test_passed = aggregate.verifications.iter().any(|verification| {
        verification.recovery_point_id == recovery_point_id
            && verification.kind == "restore_test"
            && verification.outcome == "passed"
    });
    let native_stream_validation_available = chain_ids.iter().all(|id| {
        aggregate.verifications.iter().any(|verification| {
            verification.recovery_point_id == *id
                && verification.outcome == "passed"
                && (verification
                    .evidence
                    .get("native_stream_validated")
                    .and_then(Value::as_bool)
                    == Some(true)
                    || verification
                        .scope
                        .get("native_stream_validated")
                        .and_then(Value::as_bool)
                        == Some(true))
        })
    });
    let per_file_digests_available = aggregate.recovery_point.manifest_digest.is_some()
        && aggregate
            .dependencies
            .iter()
            .all(|dependency| dependency.manifest_digest.is_some());

    let destination_assessment = DestinationAssessment {
        approved: request.destination_approved,
        is_new_destination: request.is_new_destination,
        conflicts_found: request.conflicts_found,
        conflict_policy,
        acl_semantics_supported: request.acl_semantics_supported,
        identity_mapping_complete: request.identity_mapping_complete,
        estimated_required_bytes: request.estimated_required_bytes,
        available_bytes: request.available_bytes,
        required_free_reserve_bytes: request.required_free_reserve_bytes,
        administrator_reauthenticated: request.administrator_reauthenticated,
        destructive_preview_accepted: request.destructive_preview_accepted,
    };
    let restore_request = RestoreRequest {
        recovery_point_id: recovery_point_id.to_string(),
        mode,
        dependency_chain: chain_ids.iter().map(ToString::to_string).collect(),
        missing_dependencies,
        tapes,
        keys,
        verification: VerificationEvidence {
            full_readback_passed: all_chain_full_readback,
            restore_test_passed,
            native_stream_validation_available,
            per_file_digests_available,
        },
        destination: destination_assessment,
        automatic_publication_requested: request.automatic_publication_requested,
    };
    let verification_prerequisites = json!({
        "full_readback_passed": restore_request.verification.full_readback_passed,
        "restore_test_passed": restore_request.verification.restore_test_passed,
        "native_stream_validation_available": restore_request.verification.native_stream_validation_available,
        "per_file_digests_available": restore_request.verification.per_file_digests_available
    });
    let preflight = preflight_restore(&restore_request);
    let source_manifest_artifact_id = aggregate
        .recovery_point
        .coverage
        .get("source_manifest_artifact_id")
        .and_then(Value::as_i64);
    let source_manifest_artifact_size_bytes = source_manifest_artifact_id.and_then(|artifact_id| {
        aggregate
            .artifacts
            .iter()
            .find(|artifact| {
                artifact.id == artifact_id && artifact.artifact_kind == "source_manifest"
            })
            .map(|artifact| artifact.size_bytes)
    });
    let source_manifest_limit_applies = mode == RestoreMode::FileArchive
        && matches!(
            aggregate.origin.workflow_kind.as_deref(),
            Some("recovery_exercise" | "backup")
        );
    let source_manifest_artifact_supported = !source_manifest_limit_applies
        || source_manifest_artifact_size_bytes
            .is_some_and(store::restore_manifest_artifact_size_supported);
    let source_manifest_artifact_blocker = if !source_manifest_limit_applies
        || source_manifest_artifact_supported
    {
        None
    } else if let Some(size_bytes) = source_manifest_artifact_size_bytes {
        let code = if size_bytes > 0 {
            "source_manifest_artifact_too_large"
        } else {
            "source_manifest_artifact_invalid_size"
        };
        Some(json!({
            "code": code,
            "message": format!(
                "The retained source manifest is {size_bytes} bytes; attended restore supports artifacts from 1 through {} bytes.",
                store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES
            ),
            "size_bytes": size_bytes,
            "maximum_supported_bytes": store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES
        }))
    } else {
        Some(json!({
            "code": "source_manifest_artifact_missing",
            "message": "The exact retained source-manifest artifact metadata is unavailable for attended restore.",
            "maximum_supported_bytes": store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES
        }))
    };
    let mut blockers = preflight
        .blockers
        .iter()
        .map(restore_blocker_view)
        .collect::<Vec<_>>();
    if let Some(blocker) = &source_manifest_artifact_blocker {
        blockers.push(blocker.clone());
    }
    let plan_ready = preflight.ready() && source_manifest_artifact_supported;
    let warnings = preflight
        .warnings
        .iter()
        .map(restore_warning_view)
        .collect::<Vec<_>>();
    let required_media_ids =
        dedupe_preserving_order(aggregate.volumes.iter().map(|volume| volume.cartridge_id));
    let required_tape_ids = dedupe_preserving_order(
        aggregate
            .volumes
            .iter()
            .map(|volume| volume.internal_label.clone()),
    );
    let encoded_bytes = aggregate
        .volumes
        .iter()
        .filter_map(|volume| volume.encoded_bytes)
        .filter_map(|bytes| u64::try_from(bytes).ok())
        .fold(0_u64, u64::saturating_add);
    let estimates = json!({
        "encoded_bytes": encoded_bytes,
        "media_count": required_media_ids.len(),
        "dependency_count": aggregate.dependencies.len(),
        "destination_required_bytes": request.estimated_required_bytes,
        "available_bytes": request.available_bytes,
        "required_free_reserve_bytes": request.required_free_reserve_bytes,
        "estimate_only": true
    });
    let destination_view = json!({
        "path": destination,
        "approved": request.destination_approved,
        "is_new_destination": request.is_new_destination,
        "conflicts_found": request.conflicts_found,
        "conflict_policy": conflict_policy_name(conflict_policy),
        "acl_semantics_supported": request.acl_semantics_supported,
        "identity_mapping_complete": request.identity_mapping_complete,
        "estimated_required_bytes": request.estimated_required_bytes,
        "available_bytes": request.available_bytes,
        "required_free_reserve_bytes": request.required_free_reserve_bytes,
        "administrator_reauthenticated": request.administrator_reauthenticated,
        "destructive_preview_accepted": request.destructive_preview_accepted,
        "automatic_publication_requested": request.automatic_publication_requested
    });
    let original_rehearsal_drive_id = aggregate
        .verifications
        .iter()
        .find(|verification| {
            verification.recovery_point_id == recovery_point_id
                && verification.outcome == "passed"
                && verification.kind == "full_readback"
                && verification.finished_at.is_some()
                && verification.media_ids == required_media_ids
        })
        .and_then(|verification| verification.drive_id);
    let rehearsal_capacity_ready = request
        .estimated_required_bytes
        .and_then(|required| {
            required
                .checked_add(request.required_free_reserve_bytes)
                .zip(request.available_bytes)
                .map(|(needed, available)| (required, needed, available))
        })
        .is_some_and(|(required, required_with_reserve, available)| {
            available >= required_with_reserve
                && required <= i64::MAX.unsigned_abs()
                && request.required_free_reserve_bytes <= i64::MAX.unsigned_abs()
                && available <= i64::MAX.unsigned_abs()
        });
    let compatible_execution_drives = aggregate
        .volumes
        .first()
        .map(|volume| {
            volume
                .compatible_qualified_drives
                .iter()
                .filter(|drive| Some(drive.id) == original_rehearsal_drive_id)
                .filter(|drive| {
                    aggregate.volumes.iter().all(|candidate| {
                        candidate
                            .compatible_qualified_drives
                            .iter()
                            .any(|compatible| compatible.id == drive.id)
                    })
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let supported_origin_workflow = matches!(
        aggregate.origin.workflow_kind.as_deref(),
        Some("recovery_exercise" | "backup")
    );
    let ordered_finalized_volumes = !aggregate.volumes.is_empty()
        && aggregate.volumes.iter().enumerate().all(|(index, volume)| {
            i32::try_from(index + 1) == Ok(volume.volume_order)
                && volume.recovery_point_id == recovery_point_id
                && volume.finalized
                && volume.on_tape_identity.is_some()
        });
    let origin_volume_shape_supported = match aggregate.origin.workflow_kind.as_deref() {
        Some("backup") => ordered_finalized_volumes,
        Some("recovery_exercise") => ordered_finalized_volumes && aggregate.volumes.len() == 1,
        _ => false,
    };
    let execution_supported = state.lab_file_roundtrip_enabled
        && plan_ready
        && mode == RestoreMode::FileArchive
        && aggregate.recovery_point.representation == "file"
        && aggregate.recovery_point.level == "full"
        && aggregate.recovery_point.status == "verified"
        && aggregate.recovery_point.key_lineage_id.is_none()
        && rehearsal_capacity_ready
        && aggregate.dependencies.is_empty()
        && ordered_finalized_volumes
        && required_media_ids.len() == aggregate.volumes.len()
        && aggregate.origin.data_run_id.is_some()
        && supported_origin_workflow
        && origin_volume_shape_supported
        && aggregate.origin.run_state == "verified"
        && aggregate.origin.engine_name.as_deref() == Some("Bacula Community")
        && aggregate.origin.engine_version.as_deref() == Some("15.0.3")
        && aggregate.origin.adapter_version.as_deref() == Some("g0.2-lab-v1")
        && aggregate.recovery_point.format_version == "bacula-community-15.0.3-lab-v1"
        && all_chain_full_readback
        && per_file_digests_available
        && !compatible_execution_drives.is_empty()
        && !aggregate.caps.dependencies_truncated
        && !aggregate.caps.volumes_truncated
        && !aggregate.caps.verifications_truncated
        && !aggregate.caps.artifacts_truncated
        && !aggregate.caps.values_truncated;
    let execution_detail = if let Some(blocker) = &source_manifest_artifact_blocker {
        blocker
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The retained source manifest is outside the attended restore read bound.")
            .to_owned()
    } else if execution_supported {
        "This verified full recovery point supports an attended, read-only ordered-media restore into the isolated destination.".to_owned()
    } else if !plan_ready {
        "Resolve every restore-planning blocker and generate a fresh plan before execution can be considered.".to_owned()
    } else {
        "This ready plan remains planning-only because its representation, dependency shape, retained format, origin workflow, ordered media, or physical evidence is outside the attended restore adapter.".to_owned()
    };
    let execution = json!({
        "kind": "attended_verified_point_recovery_rehearsal",
        "supported": execution_supported,
        "production_eligible": false,
        "recovery_point_id": recovery_point_id,
        "origin_data_run_id": aggregate.origin.data_run_id,
        "origin_workflow_kind": aggregate.origin.workflow_kind.clone(),
        "required_media_ids": required_media_ids,
        "required_tape_ids": required_tape_ids,
        "volume_count": required_media_ids.len(),
        "multi_volume": required_media_ids.len() > 1,
        "source_manifest_artifact": {
            "size_bytes": source_manifest_artifact_size_bytes,
            "maximum_supported_bytes": store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES,
            "supported": source_manifest_artifact_supported
        },
        "destination_subpath": destination,
        "compatible_drives": compatible_execution_drives,
        "original_drive_id": original_rehearsal_drive_id,
        "attended": true,
        "writes_tape": false,
        "writes_only_to_isolated_destination": true
    });
    let selection = json!({
        "recovery_point_id": recovery_point_id,
        "mode": restore_mode_name(mode),
        "representation": aggregate.recovery_point.representation.clone(),
        "scope": aggregate.recovery_point.coverage.clone()
    });
    let preview = json!({
        "selection": selection,
        "dependency_chain": chain_ids,
        "required_media_ids": required_media_ids,
        "required_tape_ids": required_tape_ids,
        "key_lineage_ids": key_lineage_ids,
        "destination": destination_view,
        "blockers": blockers,
        "warnings": warnings,
        "estimates": estimates,
        "verification": verification_prerequisites,
        "execution_supported": execution_supported,
        "execution": execution
    });
    let preview_hash = format!("{:x}", Sha256::digest(canonical_json(&preview)?));
    let persisted_selection = json!({
        "request": selection,
        "ready": plan_ready,
        "execution_supported": execution_supported,
        "execution": execution,
        "dependencies": &aggregate.dependencies,
        "volumes": &aggregate.volumes,
        "verifications": &aggregate.verifications,
        "verification_prerequisites": verification_prerequisites,
        "warnings": warnings,
        "caps": &aggregate.caps
    });
    let record = store::create_restore_plan(
        pool,
        &store::NewRestorePlan {
            selection: persisted_selection,
            dependency_chain: chain_ids.clone(),
            required_media_ids: required_media_ids.clone(),
            key_lineage_ids: key_lineage_ids.clone(),
            destination: destination_view.clone(),
            conflict_policy: conflict_policy_name(conflict_policy).to_owned(),
            blockers: Value::Array(blockers.clone()),
            estimates: estimates.clone(),
            preview_hash,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "plan": {
                "id": record.id,
                "preview_hash": record.preview_hash,
                "created_at": record.created_at,
                "expires_at": record.expires_at,
                "ready": plan_ready,
                "status": if plan_ready { "ready" } else { "blocked" },
                "execution_supported": execution_supported,
                "execution_detail": execution_detail,
                "execution": execution,
                "execution_drives": compatible_execution_drives,
                "confirmation_phrase": format!("RESTORE PLAN {} TO ISOLATED DESTINATION", record.id),
                "selection": selection,
                "recovery_point": aggregate.recovery_point,
                "dependencies": aggregate.dependencies,
                "dependency_chain": chain_ids,
                "volumes": aggregate.volumes,
                "required_tape_ids": required_tape_ids,
                "required_media_ids": required_media_ids,
                "required_key_ids": key_lineage_ids,
                "keys": key_lineage_ids.iter().map(|key_id| json!({
                    "key_id": key_id,
                    "available": request.key_available,
                    "recovery_package_tested": request.key_recovery_package_tested
                })).collect::<Vec<_>>(),
                "verifications": aggregate.verifications,
                "verification_prerequisites": verification_prerequisites,
                "destination": destination_view,
                "blockers": blockers,
                "warnings": warnings,
                "estimates": estimates,
                "caps": aggregate.caps
            }
        })),
    ))
}

const RESTORE_REHEARSAL_EVENT_LIMIT: usize = 256;
const RESTORE_REHEARSAL_ARTIFACT_LIMIT: usize = 64;
const RESTORE_REHEARSAL_EXPORT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct QueueRestoreRehearsalRequest {
    plan_id: Uuid,
    preview_hash: String,
    drive_id: Uuid,
    confirmation: String,
    tape_loaded_confirmed: bool,
    isolated_destination_confirmed: bool,
    attended_confirmed: bool,
    #[serde(default)]
    actor: Option<String>,
}

impl QueueRestoreRehearsalRequest {
    fn validate(&self) -> Result<String, AppError> {
        let expected = format!("RESTORE PLAN {} TO ISOLATED DESTINATION", self.plan_id);
        if self.confirmation != expected {
            return Err(AppError::Invalid(format!(
                "confirmation must exactly equal {expected}"
            )));
        }
        if !self.tape_loaded_confirmed
            || !self.isolated_destination_confirmed
            || !self.attended_confirmed
        {
            return Err(AppError::Invalid(
                "tape_loaded_confirmed, isolated_destination_confirmed, and attended_confirmed must all be true"
                    .to_owned(),
            ));
        }
        let actor = self.actor.as_deref().unwrap_or("Web operator").trim();
        if actor.is_empty() || actor.chars().count() > 128 || actor.chars().any(char::is_control) {
            return Err(AppError::Invalid(
                "actor must contain 1 to 128 printable characters".to_owned(),
            ));
        }
        Ok(actor.to_owned())
    }
}

#[derive(Debug, Deserialize)]
struct CancelRestoreRehearsalRequest {
    confirmation: String,
    #[serde(default)]
    actor: Option<String>,
}

impl CancelRestoreRehearsalRequest {
    fn validate(&self, id: Uuid) -> Result<String, AppError> {
        let expected = format!("CANCEL RESTORE REHEARSAL {id}");
        if self.confirmation != expected {
            return Err(AppError::Invalid(format!(
                "confirmation must exactly equal {expected}"
            )));
        }
        let actor = self.actor.as_deref().unwrap_or("Web operator").trim();
        if actor.is_empty() || actor.chars().count() > 128 || actor.chars().any(char::is_control) {
            return Err(AppError::Invalid(
                "actor must contain 1 to 128 printable characters".to_owned(),
            ));
        }
        Ok(actor.to_owned())
    }
}

#[derive(Debug, Deserialize)]
struct AuthorizeRestoreRehearsalMediaRequest {
    request_id: Uuid,
    drive_id: Uuid,
    expected_stable_id: String,
    cartridge_id: Uuid,
    typed_cartridge_label: String,
    volume_order: i32,
    loaded_media_confirmed: bool,
    attended_confirmed: bool,
    confirmation: String,
    #[serde(default)]
    actor: Option<String>,
}

impl AuthorizeRestoreRehearsalMediaRequest {
    fn actor(&self) -> Result<String, AppError> {
        let actor = self.actor.as_deref().unwrap_or("Web operator").trim();
        if actor.is_empty() || actor.chars().count() > 128 || actor.chars().any(char::is_control) {
            return Err(AppError::Invalid(
                "actor must contain 1 to 128 printable characters".to_owned(),
            ));
        }
        Ok(actor.to_owned())
    }
}

async fn restore_rehearsals(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let rehearsals = match state.pool.as_ref() {
        Some(pool) => {
            let records = store::list_restore_rehearsals(pool).await?;
            let mut views = Vec::with_capacity(records.len());
            for record in records {
                let request = store::open_restore_rehearsal_media_request(pool, record.id).await?;
                let mut view = restore_rehearsal_run_summary_view(&record);
                if let Some(object) = view.as_object_mut() {
                    object.insert(
                        "media_request".to_owned(),
                        request
                            .as_ref()
                            .map(restore_rehearsal_media_request_view)
                            .unwrap_or(Value::Null),
                    );
                    object.insert(
                        "media_handoff".to_owned(),
                        restore_rehearsal_media_handoff_view(&record, request.as_ref()),
                    );
                }
                views.push(view);
            }
            views
        }
        None => Vec::new(),
    };
    Ok(Json(json!({
        "rehearsals": rehearsals,
        "scope": "attended_verified_ordered_volume_physical_restore",
        "production_eligible": false
    })))
}

async fn restore_rehearsal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "restore rehearsal")?;
    let rehearsal = store::get_restore_rehearsal(pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let events = store::restore_rehearsal_events(pool, id).await?;
    let artifacts = store::restore_rehearsal_artifacts(pool, id).await?;
    let volumes = store::restore_rehearsal_volumes(pool, id).await?;
    let media_request = store::open_restore_rehearsal_media_request(pool, id).await?;
    let media_requests = store::restore_rehearsal_media_requests(pool, id).await?;
    let events_truncated = events.len() > RESTORE_REHEARSAL_EVENT_LIMIT;
    let artifacts_truncated = artifacts.len() > RESTORE_REHEARSAL_ARTIFACT_LIMIT;
    Ok(Json(json!({
        "rehearsal": restore_rehearsal_run_view(&rehearsal),
        "volumes": volumes.iter().map(restore_rehearsal_volume_view).collect::<Vec<_>>(),
        "media_request": media_request.as_ref().map(restore_rehearsal_media_request_view),
        "media_requests": media_requests.iter().map(restore_rehearsal_media_request_view).collect::<Vec<_>>(),
        "media_handoff": restore_rehearsal_media_handoff_view(&rehearsal, media_request.as_ref()),
        "events": events.iter().take(RESTORE_REHEARSAL_EVENT_LIMIT).map(restore_rehearsal_event_view).collect::<Vec<_>>(),
        "artifacts": artifacts.iter().take(RESTORE_REHEARSAL_ARTIFACT_LIMIT).map(restore_rehearsal_artifact_view).collect::<Vec<_>>(),
        "caps": {
            "events_maximum": RESTORE_REHEARSAL_EVENT_LIMIT,
            "events_truncated": events_truncated,
            "artifacts_maximum": RESTORE_REHEARSAL_ARTIFACT_LIMIT,
            "artifacts_truncated": artifacts_truncated
        },
        "scope": "attended_verified_ordered_volume_physical_restore",
        "production_eligible": false
    })))
}

async fn restore_rehearsal_media_readiness(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "restore rehearsal")?;
    let rehearsal = store::get_restore_rehearsal(pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let volumes = store::restore_rehearsal_volumes(pool, id).await?;
    let media_request = store::open_restore_rehearsal_media_request(pool, id).await?;
    let media_requests = store::restore_rehearsal_media_requests(pool, id).await?;
    let candidate = media_request.as_ref().and_then(|request| {
        volumes
            .iter()
            .find(|volume| volume.volume_order == request.volume_order)
            .map(|volume| {
                json!({
                    "cartridge_id": volume.cartridge_id,
                    "internal_label": volume.expected_cartridge_label,
                    "volume_order": volume.volume_order,
                    "expected_engine_volume_name": volume.expected_volume_label,
                    "media_generation": volume.expected_media_generation,
                    "media_format": volume.expected_media_format
                })
            })
    });
    let (ready, reason) = match media_request.as_ref() {
        Some(_) if rehearsal.state != "running" => (
            false,
            "the restore is not running; media authorization is closed",
        ),
        Some(request) if request.state == "pending" && candidate.is_some() => (
            true,
            "the durable handoff is actionable; load the exact retained cartridge and submit the attended confirmation, then the worker will perform live drive and media validation",
        ),
        Some(request) if request.state == "authorized" => (
            false,
            "the exact cartridge is authorized; the worker must revalidate and lease it",
        ),
        Some(request) if request.state == "loaded" => (
            false,
            "the worker has leased this volume; a restart at this boundary fails closed",
        ),
        Some(_) => (false, "the media request is not actionable"),
        None => (
            false,
            "the restore is not currently waiting for another volume",
        ),
    };
    Ok(Json(json!({
        "rehearsal": restore_rehearsal_run_view(&rehearsal),
        "volumes": volumes.iter().map(restore_rehearsal_volume_view).collect::<Vec<_>>(),
        "media_request": media_request.as_ref().map(restore_rehearsal_media_request_view),
        "media_requests": media_requests.iter().map(restore_rehearsal_media_request_view).collect::<Vec<_>>(),
        "media_handoff": restore_rehearsal_media_handoff_view(&rehearsal, media_request.as_ref()),
        "readiness": {
            "ready": ready,
            "advisory": true,
            "scope": "durable_handoff_precheck",
            "reason": reason,
            "drive": {
                "id": rehearsal.drive_id,
                "stable_id": rehearsal.expected_stable_id,
                "path": rehearsal.expected_path
            },
            "candidate": candidate,
            "live_drive_validated": false,
            "final_validation": "after authorization the worker atomically leases the exact retained cartridge, freshly re-probes the drive, verifies the loaded tape, rewinds it, and only then mounts it for Bacula"
        }
    })))
}

async fn authorize_restore_rehearsal_media(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<AuthorizeRestoreRehearsalMediaRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "restore rehearsal")?;
    let actor = request.actor()?;
    let authorization = store::RestoreRehearsalMediaAuthorization {
        request_id: request.request_id,
        drive_id: request.drive_id,
        expected_stable_id: request.expected_stable_id,
        cartridge_id: request.cartridge_id,
        typed_cartridge_label: request.typed_cartridge_label,
        volume_order: request.volume_order,
        loaded_media_confirmed: request.loaded_media_confirmed,
        attended_confirmed: request.attended_confirmed,
        confirmation: request.confirmation,
        actor_label: actor,
    };
    match store::authorize_restore_rehearsal_media(pool, id, &authorization).await? {
        store::AuthorizeRestoreRehearsalMediaResult::Authorized {
            request,
            volume,
            idempotent_replay,
        } => {
            let rehearsal = store::get_restore_rehearsal(pool, id)
                .await?
                .ok_or(AppError::NotFound)?;
            Ok((
                StatusCode::OK,
                Json(json!({
                    "rehearsal": restore_rehearsal_run_view(&rehearsal),
                    "media_request": restore_rehearsal_media_request_view(&request),
                    "volume": restore_rehearsal_volume_view(&volume),
                    "media_handoff": restore_rehearsal_media_handoff_view(&rehearsal, Some(&request)),
                    "idempotent_replay": idempotent_replay,
                    "attended": true,
                    "writes_tape": false
                })),
            ))
        }
        store::AuthorizeRestoreRehearsalMediaResult::NotFound => Err(AppError::NotFound),
        store::AuthorizeRestoreRehearsalMediaResult::Blocked(reason) => {
            Err(AppError::Blocked(reason))
        }
    }
}

async fn queue_restore_rehearsal(
    State(state): State<AppState>,
    Json(request): Json<QueueRestoreRehearsalRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_file_roundtrip(&state)?;
    let pool = require_pool(&state)?;
    let actor = request.validate()?;
    let attestations = store::RestoreRehearsalAttestations {
        confirmation: request.confirmation.clone(),
        tape_loaded_confirmed: request.tape_loaded_confirmed,
        isolated_destination_confirmed: request.isolated_destination_confirmed,
        attended_confirmed: request.attended_confirmed,
    };
    match store::queue_restore_rehearsal(
        pool,
        request.plan_id,
        request.preview_hash.trim(),
        request.drive_id,
        &actor,
        &attestations,
    )
    .await?
    {
        store::QueueRestoreRehearsalResult::Queued(rehearsal) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "rehearsal": restore_rehearsal_run_view(&rehearsal),
                "durability": "postgresql",
                "attended": true,
                "writes_tape": false,
                "production_eligible": false
            })),
        )),
        store::QueueRestoreRehearsalResult::NotFound => Err(AppError::NotFound),
        store::QueueRestoreRehearsalResult::Blocked(reason) => Err(AppError::Blocked(reason)),
    }
}

async fn cancel_restore_rehearsal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CancelRestoreRehearsalRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "restore rehearsal")?;
    let actor = request.validate(id)?;
    match store::request_cancel_restore_rehearsal(pool, id, &actor).await? {
        store::CancelRestoreRehearsalResult::Cancelled(rehearsal) => Ok((
            StatusCode::OK,
            Json(json!({
                "rehearsal": restore_rehearsal_run_view(&rehearsal),
                "cancel": "completed_before_worker_claim"
            })),
        )),
        store::CancelRestoreRehearsalResult::Requested(rehearsal) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "rehearsal": restore_rehearsal_run_view(&rehearsal),
                "cancel": "requested_for_safe_worker_boundary"
            })),
        )),
        store::CancelRestoreRehearsalResult::AlreadyTerminal(rehearsal) => Ok((
            StatusCode::OK,
            Json(json!({
                "rehearsal": restore_rehearsal_run_view(&rehearsal),
                "cancel": "already_terminal"
            })),
        )),
        store::CancelRestoreRehearsalResult::NotFound => Err(AppError::NotFound),
        store::CancelRestoreRehearsalResult::Blocked(reason) => Err(AppError::Blocked(reason)),
    }
}

async fn restore_rehearsal_evidence(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let pool = require_pool(&state)?;
    let id = parse_uuid_path(&id, "restore rehearsal")?;
    let rehearsal = store::get_restore_rehearsal(pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let events = store::restore_rehearsal_events(pool, id).await?;
    let artifacts = store::restore_rehearsal_artifacts(pool, id).await?;
    let volumes = store::restore_rehearsal_volumes(pool, id).await?;
    let media_request = store::open_restore_rehearsal_media_request(pool, id).await?;
    let evidence = json!({
        "format": "tapescale.restore-rehearsal-evidence.v1",
        "canonicalization": "UTF-8 compact JSON with lexicographically ordered object keys",
        "execution_claim": "attended verified-point ordered-volume physical-tape Restore run; not production qualification",
        "bounds": {
            "maximum_serialized_bytes": RESTORE_REHEARSAL_EXPORT_BYTES,
            "events_maximum": RESTORE_REHEARSAL_EVENT_LIMIT,
            "events_truncated": events.len() > RESTORE_REHEARSAL_EVENT_LIMIT,
            "artifacts_maximum": RESTORE_REHEARSAL_ARTIFACT_LIMIT,
            "artifacts_truncated": artifacts.len() > RESTORE_REHEARSAL_ARTIFACT_LIMIT,
            "nested_values": "depth, collection, and string bounded; worker-internal paths redacted"
        },
        "rehearsal": restore_rehearsal_run_view(&rehearsal),
        "volumes": volumes.iter().map(restore_rehearsal_volume_view).collect::<Vec<_>>(),
        "media_request": media_request.as_ref().map(restore_rehearsal_media_request_view),
        "events": events.iter().take(RESTORE_REHEARSAL_EVENT_LIMIT).map(restore_rehearsal_event_view).collect::<Vec<_>>(),
        "artifacts": artifacts.iter().take(RESTORE_REHEARSAL_ARTIFACT_LIMIT).map(restore_rehearsal_artifact_view).collect::<Vec<_>>()
    });
    let body = canonical_json(&evidence)?;
    if body.len() > RESTORE_REHEARSAL_EXPORT_BYTES {
        return Err(AppError::Blocked(format!(
            "the bounded Restore-run evidence export is {} bytes, exceeding the {} byte response limit",
            body.len(),
            RESTORE_REHEARSAL_EXPORT_BYTES
        )));
    }
    let digest = format!("{:x}", Sha256::digest(&body));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"tapescale-restore-run-{id}-evidence.json\""),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ETAG, format!("\"sha256-{digest}\""))
        .header("x-content-type-options", "nosniff")
        .body(Body::from(body))
        .map_err(|error| AppError::Internal(error.to_string()))
}

fn restore_rehearsal_run_view(run: &store::RestoreRehearsalRunRecord) -> Value {
    let mut view = serde_json::to_value(run)
        .expect("a serializable restore rehearsal record always converts to JSON");
    let object = view
        .as_object_mut()
        .expect("a restore rehearsal record serializes as an object");
    for internal in [
        "source_manifest_storage_ref",
        "source_mount_path",
        "drive_lease_id",
        "drive_fencing_token",
        "media_lease_id",
        "media_fencing_token",
    ] {
        object.remove(internal);
    }
    object.insert(
        "source_manifest".to_owned(),
        json!({
            "artifact_id": run.source_manifest_artifact_id,
            "sha256": run.source_manifest_sha256,
            "manifest_digest": run.source_manifest_digest,
            "size_bytes": run.source_manifest_size_bytes
        }),
    );
    object.insert(
        "result".to_owned(),
        bounded_restore_evidence_value(&run.result),
    );
    object.insert("multi_volume".to_owned(), json!(run.volume_count > 1));
    view
}

fn restore_rehearsal_run_summary_view(run: &store::RestoreRehearsalRunRecord) -> Value {
    let mut view = restore_rehearsal_run_view(run);
    if let Some(object) = view.as_object_mut() {
        object.remove("result");
        object.remove("source_manifest");
        object.remove("source_manifest_sha256");
        object.remove("source_manifest_digest");
    }
    view
}

fn restore_rehearsal_volume_view(volume: &store::RestoreRehearsalVolumeRecord) -> Value {
    let mut view = serde_json::to_value(volume)
        .expect("a serializable restore volume record always converts to JSON");
    if let Some(object) = view.as_object_mut() {
        object.remove("media_lease_id");
        object.remove("media_fencing_token");
    }
    view
}

fn restore_rehearsal_media_request_view(
    request: &store::RestoreRehearsalMediaRequestRecord,
) -> Value {
    let mut view = serde_json::to_value(request)
        .expect("a serializable restore media request always converts to JSON");
    if let Some(object) = view.as_object_mut() {
        object.insert(
            "confirmation_evidence".to_owned(),
            bounded_restore_evidence_value(&request.confirmation_evidence),
        );
    }
    view
}

fn restore_rehearsal_media_handoff_view(
    run: &store::RestoreRehearsalRunRecord,
    request: Option<&store::RestoreRehearsalMediaRequestRecord>,
) -> Value {
    let Some(request) = request else {
        return json!({
            "required": false,
            "waiting": false,
            "state": if run.state == "running" { "active" } else { run.state.as_str() },
            "volume_order": Value::Null,
            "confirmation_phrase": Value::Null
        });
    };
    json!({
        "required": true,
        "waiting": matches!(request.state.as_str(), "pending" | "authorized"),
        "state": request.state,
        "request_id": request.id,
        "volume_order": request.volume_order,
        "volume_count": run.volume_count,
        "expected_cartridge_id": request.expected_cartridge_id,
        "expected_cartridge_label": request.expected_cartridge_label,
        "expected_engine_volume_name": request.expected_engine_volume_name,
        "confirmation_phrase": format!(
            "LOAD {} FOR RESTORE {} VOLUME {}",
            request.expected_cartridge_label, run.id, request.volume_order
        )
    })
}

fn restore_rehearsal_event_view(event: &store::RestoreRehearsalEventRecord) -> Value {
    json!({
        "id": event.id,
        "event_type": event.event_type,
        "evidence": bounded_restore_evidence_value(&event.evidence),
        "correlation_id": event.correlation_id,
        "occurred_at": event.occurred_at
    })
}

fn restore_rehearsal_artifact_view(artifact: &store::RestoreRehearsalArtifactRecord) -> Value {
    json!({
        "id": artifact.id,
        "artifact_kind": artifact.artifact_kind,
        "sha256": artifact.sha256,
        "size_bytes": artifact.size_bytes,
        "metadata": bounded_restore_evidence_value(&artifact.metadata),
        "created_at": artifact.created_at
    })
}

fn bounded_restore_evidence_value(value: &Value) -> Value {
    const MAX_VALUE_BYTES: usize = 8 * 1024;
    let mut truncated = false;
    let bounded = sanitize_restore_evidence_value(value, 0, &mut truncated);
    let encoded = serde_json::to_vec(&bounded).expect("a JSON value always serializes");
    if encoded.len() <= MAX_VALUE_BYTES {
        bounded
    } else {
        json!({
            "_tapescale_truncated": "sanitized value exceeded the per-value export bound",
            "sanitized_size_bytes": encoded.len(),
            "sanitized_sha256": format!("{:x}", Sha256::digest(&encoded))
        })
    }
}

fn sanitize_restore_evidence_value(value: &Value, depth: usize, truncated: &mut bool) -> Value {
    const MAX_DEPTH: usize = 8;
    const MAX_MEMBERS: usize = 64;
    const MAX_STRING_CHARS: usize = 4 * 1024;
    if depth >= MAX_DEPTH {
        *truncated = true;
        return json!({"_tapescale_truncated": "maximum JSON depth reached"});
    }
    match value {
        Value::String(text) if text.chars().count() > MAX_STRING_CHARS => {
            *truncated = true;
            let mut bounded = text.chars().take(MAX_STRING_CHARS).collect::<String>();
            bounded.push_str("…[truncated]");
            Value::String(bounded)
        }
        Value::Array(values) => {
            let mut bounded = values
                .iter()
                .take(MAX_MEMBERS)
                .map(|item| sanitize_restore_evidence_value(item, depth + 1, truncated))
                .collect::<Vec<_>>();
            if values.len() > MAX_MEMBERS {
                *truncated = true;
                bounded.push(json!({"_tapescale_truncated_items": values.len() - MAX_MEMBERS}));
            }
            Value::Array(bounded)
        }
        Value::Object(values) => {
            let mut bounded = serde_json::Map::new();
            for (key, item) in values.iter().take(MAX_MEMBERS) {
                let normalized = key.to_ascii_lowercase();
                if normalized.contains("storage_ref")
                    || normalized.ends_with("_lease_id")
                    || normalized.ends_with("_fencing_token")
                    || matches!(
                        normalized.as_str(),
                        "source_mount_path"
                            | "restore_root_path"
                            | "daemon_logs"
                            | "working_directory"
                            | "state_directory"
                            | "runtime_root"
                    )
                    || normalized.ends_with("_config_path")
                {
                    continue;
                }
                bounded.insert(
                    key.clone(),
                    sanitize_restore_evidence_value(item, depth + 1, truncated),
                );
            }
            if values.len() > MAX_MEMBERS {
                *truncated = true;
                bounded.insert(
                    "_tapescale_truncated_members".to_owned(),
                    Value::from(values.len() - MAX_MEMBERS),
                );
            }
            Value::Object(bounded)
        }
        _ => value.clone(),
    }
}

async fn preflight(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let Some(pool) = state.pool.as_ref() else {
        let checks = vec![
            preflight_check(
                "database",
                "Database",
                "blocked",
                "PostgreSQL is not configured",
            ),
            preflight_check(
                "engine",
                "Tape backup engine",
                "blocked",
                "PostgreSQL is required before the attended Bacula backup path can retain requests and evidence",
            ),
        ];
        return Ok(Json(json!({
            "status":derived_preflight_status(&checks),
            "checked_at":chrono::Utc::now(),
            "checks":checks,
            "inventory":empty_inventory()
        })));
    };

    let inventory = store::inventory_counts(pool).await?;
    let prepared_pair_available = has_ready_prepared_tape_pair(pool).await?;
    let access_status = if inventory.accessible_drives > 0 {
        "passed"
    } else {
        "blocked"
    };
    let identity_status = if inventory.reconciled_drives > 0 {
        "passed"
    } else {
        "blocked"
    };
    let qualification_status = if prepared_pair_available {
        "passed"
    } else {
        "blocked"
    };
    let media_status = if inventory.cartridges > 0 {
        "passed"
    } else {
        "blocked"
    };
    let engine_status = if state.lab_file_roundtrip_enabled {
        "passed"
    } else {
        "blocked"
    };
    let checks = vec![
        preflight_check("database", "Database", "passed", "PostgreSQL responded"),
        direct_source_mount_preflight(&state.source_root),
        preflight_check(
            "sas_device_access",
            "SAS tape device access",
            access_status,
            if inventory.accessible_drives > 0 {
                "The worker can read status from a configured non-rewinding tape device"
            } else {
                "No configured SAS tape device is currently accessible"
            },
        ),
        preflight_check(
            "stable_drive_identity",
            "Stable drive identity",
            identity_status,
            if inventory.reconciled_drives > 0 {
                "An accessible drive is bound to its observed stable identity"
            } else {
                "No accessible drive identity has been reconciled"
            },
        ),
        preflight_check(
            "prepared_tape_path",
            "Prepared drive and tape",
            qualification_status,
            if prepared_pair_available {
                "A current passing Bacula setup test matches the exact ready drive and eligible loaded scratch cartridge; this is narrow setup evidence, not general physical qualification"
            } else {
                "No current passing Bacula setup test matches an exact ready drive and eligible loaded scratch cartridge"
            },
        ),
        preflight_check(
            "media_inventory",
            "Media inventory",
            media_status,
            if inventory.cartridges > 0 {
                "Cartridges are registered; on-tape identity still governs safe use"
            } else {
                "No cartridges are registered"
            },
        ),
        preflight_check(
            "engine",
            "Tape backup engine",
            engine_status,
            if state.lab_file_roundtrip_enabled {
                "Pinned Bacula 15.0.3 is enabled for attended compressed, ordered multi-tape Full backups and single-volume recovery exercises; this alpha path is not production-qualified"
            } else {
                "The attended Bacula backup and recovery data path is disabled"
            },
        ),
    ];
    Ok(Json(json!({
        "status":derived_preflight_status(&checks),
        "checked_at":chrono::Utc::now(),
        "checks":checks,
        "prepared_pair_available":prepared_pair_available,
        "physical_qualification":"not_established",
        "inventory":inventory
    })))
}

fn direct_source_mount_preflight(source_root: &FsPath) -> Value {
    match verify_read_only_mount(source_root) {
        Ok(()) => preflight_check(
            "source_mount",
            "Read-only source mount",
            "passed",
            "The configured direct source mount is readable and kernel-reported read-only; a separate TrueNAS API registration is not required for this deployment mode",
        ),
        Err(_) => preflight_check(
            "source_mount",
            "Read-only source mount",
            "blocked",
            "The configured direct source mount is missing, inaccessible, or not kernel-reported read-only",
        ),
    }
}

async fn has_ready_prepared_tape_pair(pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1
               FROM qualification_runs qualification
               JOIN drives drive ON drive.id = qualification.drive_id
               JOIN cartridges cartridge ON cartridge.id = qualification.cartridge_id
              WHERE qualification.state = 'passed'
                AND qualification.attempt > 0
                AND qualification.engine_name = 'Bacula Community'
                AND qualification.engine_version = $1
                AND qualification.test_kind = 'btape_test'
                AND qualification.id = (
                    SELECT latest.id
                      FROM qualification_runs latest
                     WHERE latest.drive_id = qualification.drive_id
                       AND latest.cartridge_id = qualification.cartridge_id
                       AND latest.state IN ('passed', 'failed', 'interrupted')
                       AND latest.attempt > 0
                     ORDER BY latest.created_at DESC, latest.id DESC
                     LIMIT 1)
                AND qualification.expected_stable_id = drive.stable_id
                AND qualification.expected_path IS NOT DISTINCT FROM drive.observed_path
                AND qualification.expected_vendor IS NOT DISTINCT FROM drive.vendor
                AND qualification.expected_model IS NOT DISTINCT FROM drive.model
                AND qualification.expected_serial IS NOT DISTINCT FROM drive.serial
                AND qualification.expected_wwn IS NOT DISTINCT FROM drive.wwn
                AND qualification.expected_firmware IS NOT DISTINCT FROM drive.firmware
                AND qualification.expected_cartridge_label = cartridge.internal_label
                AND qualification.expected_media_generation
                    IS NOT DISTINCT FROM cartridge.media_generation
                AND qualification.expected_media_format
                    IS NOT DISTINCT FROM cartridge.media_format
                AND drive.reconciled
                AND drive.expected_path IS NOT DISTINCT FROM drive.observed_path
                AND drive.health->>'present' = 'true'
                AND drive.health->>'accessible' = 'true'
                AND drive.health->>'identity_verified' = 'true'
                AND COALESCE(drive.health->>'identity_conflict', 'false') <> 'true'
                AND COALESCE(drive.health->>'motion_position_uncertain', 'false') <> 'true'
                AND drive.health->>'immediate_mode' = 'false'
                AND drive.health->>'in_flight' = '0'
                AND drive.health->>'ready' = 'true'
                AND drive.health->>'medium_loaded' = 'true'
                AND drive.last_observed_at >= now() - interval '2 minutes'
                AND NOT cartridge.worm
                AND cartridge.reserved_by IS NULL
                AND cartridge.health_status = 'qualified_scratch'
                AND NOT EXISTS (
                    SELECT 1 FROM volume_records volume
                     WHERE volume.cartridge_id = cartridge.id)
                AND NOT EXISTS (
                    SELECT 1 FROM holds active_hold
                     WHERE active_hold.cartridge_id = cartridge.id
                       AND active_hold.released_at IS NULL)
         )",
    )
    .bind(BACULA_VERSION)
    .fetch_one(pool)
    .await
}

async fn qualification_runs(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let runs = match state.pool.as_ref() {
        Some(pool) => store::list_qualification_runs(pool).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({
        "runs": runs,
        "engine_candidate": {"name":"Bacula Community","version":"15.0.3"},
        "physical_qualification":"not_established"
    })))
}

async fn qualification_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = require_pool(&state)?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| AppError::Invalid("qualification run id must be a UUID".to_owned()))?;
    let run = store::get_qualification_run(pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let events = store::qualification_events(pool, id).await?;
    Ok(Json(json!({
        "run":run,
        "events":events,
        "physical_qualification":"not_established"
    })))
}

#[derive(Clone, Debug, Deserialize)]
struct QueueQualificationRunRequest {
    drive_id: Uuid,
    stable_id: String,
    cartridge_id: Uuid,
    cartridge_label: String,
    confirmation: String,
    scratch_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
    #[serde(default)]
    idempotency_key: String,
}

impl QueueQualificationRunRequest {
    fn expected_confirmation(&self) -> String {
        format!("DESTROY {} WITH BACULA BTAPE TEST", self.cartridge_label)
    }

    fn validate(&self) -> Result<(&str, &str, &str), AppError> {
        let stable_id = self.stable_id.trim();
        if stable_id.is_empty()
            || stable_id.chars().count() > 512
            || stable_id.chars().any(char::is_control)
        {
            return Err(AppError::Invalid(
                "stable_id must contain 1 to 512 characters".to_owned(),
            ));
        }
        if self.cartridge_label.is_empty()
            || self.cartridge_label.chars().count() > 128
            || self.cartridge_label.chars().any(char::is_control)
        {
            return Err(AppError::Invalid(
                "cartridge_label must contain 1 to 128 characters".to_owned(),
            ));
        }
        if !self.scratch_media_confirmed || !self.destructive_overwrite_confirmed {
            return Err(AppError::Invalid(
                "scratch_media_confirmed and destructive_overwrite_confirmed must both be true"
                    .to_owned(),
            ));
        }
        if self.confirmation != self.expected_confirmation() {
            return Err(AppError::Invalid(format!(
                "confirmation must exactly equal {}",
                self.expected_confirmation()
            )));
        }
        let idempotency_key = self.idempotency_key.trim();
        if idempotency_key.is_empty()
            || idempotency_key.len() > 256
            || idempotency_key.chars().any(char::is_control)
        {
            return Err(AppError::Invalid(
                "idempotency_key must contain 1 to 256 printable bytes".to_owned(),
            ));
        }
        Ok((stable_id, &self.cartridge_label, idempotency_key))
    }

    fn request_fingerprint(&self, stable_id: &str, cartridge_label: &str) -> String {
        let payload = json!([
            "tapescale.qualification-request.v1",
            self.drive_id,
            stable_id,
            self.cartridge_id,
            cartridge_label,
            self.scratch_media_confirmed,
            self.destructive_overwrite_confirmed,
            "Bacula Community",
            "15.0.3",
            "btape_test"
        ]);
        format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&payload).expect("qualification fingerprint JSON is infallible")
            )
        )
    }
}

async fn queue_qualification_run(
    State(state): State<AppState>,
    Json(request): Json<QueueQualificationRunRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pool = require_pool(&state)?;
    let (stable_id, cartridge_label, idempotency_key) = request.validate()?;
    let fingerprint = request.request_fingerprint(stable_id, cartridge_label);
    let disposition = store::queue_qualification_run_idempotent(
        pool,
        request.drive_id,
        stable_id,
        request.cartridge_id,
        cartridge_label,
        request.scratch_media_confirmed,
        request.destructive_overwrite_confirmed,
        idempotency_key,
        &fingerprint,
        state.lab_tape_qualification_enabled,
    )
    .await?;
    match disposition {
        store::QueueQualificationRunResult::Queued(run) => {
            info!(
                qualification_run_id = %run.id,
                drive_id = %run.drive_id,
                cartridge_id = %run.cartridge_id,
                cartridge_label = %run.expected_cartridge_label,
                state = %run.state,
                "destructive Bacula tape qualification was durably queued"
            );
            Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "run":run,
                    "durability":"postgresql",
                    "audit":"appended",
                    "disposition":"queued",
                    "idempotent_replay":false,
                    "destructive":true,
                    "engine_candidate":"Bacula Community 15.0.3",
                    "physical_qualification":"not_established"
                })),
            ))
        }
        store::QueueQualificationRunResult::Existing(run) => Ok((
            StatusCode::OK,
            Json(json!({
                "run":run,
                "durability":"postgresql",
                "audit":"existing",
                "disposition":"existing",
                "idempotent_replay":true,
                "destructive":true,
                "engine_candidate":"Bacula Community 15.0.3",
                "physical_qualification":"not_established"
            })),
        )),
        store::QueueQualificationRunResult::IdempotencyConflict => Err(AppError::Conflict(
            "idempotency_key was already used for a different qualification request".to_owned(),
        )),
        store::QueueQualificationRunResult::NotFound => Err(AppError::NotFound),
        store::QueueQualificationRunResult::Blocked(reason) => Err(AppError::Blocked(reason)),
    }
}

async fn qualification_report() -> Json<Value> {
    Json(qualification::report())
}

fn require_pool(state: &AppState) -> Result<&PgPool, AppError> {
    state.pool.as_ref().ok_or_else(|| {
        AppError::Blocked(
            "PostgreSQL is required for this operation and is not configured".to_owned(),
        )
    })
}

fn empty_json_object() -> Value {
    json!({})
}

fn generated_movement_external_id(idempotency_key: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(idempotency_key.as_bytes()));
    format!("TS-MOVE-{}", &digest[..24])
}

fn custody_actor(label: Option<&str>) -> CustodyActor {
    let label = label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("local-operator")
        .to_owned();
    CustodyActor {
        user_id: None,
        label,
    }
}

fn media_actor(label: Option<&str>) -> media_ops::MediaActor {
    let label = label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("local-operator")
        .to_owned();
    media_ops::MediaActor {
        user_id: None,
        label,
    }
}

/// Preserve the original flat `/api/v1/media` fields while exposing the
/// richer nested Media Control Center contract.  Exact-drive evaluation is
/// still required, so the deprecated inventory-only overwrite flag remains
/// false and cannot be mistaken for authorization.
fn media_inventory_view(record: &media_ops::MediaInventorySummary) -> Value {
    let blocker = match record.reuse_state {
        media_ops::InventoryReuseState::PhysicallyBlocked => {
            "physical media protection blocks overwrite"
        }
        media_ops::InventoryReuseState::Reserved => "the cartridge is reserved",
        media_ops::InventoryReuseState::NeedsReconciliation => {
            "media identity, catalog, location, health, or dependency state requires reconciliation"
        }
        media_ops::InventoryReuseState::Protected => {
            "retention, dependency, archive, or hold protection is active"
        }
        media_ops::InventoryReuseState::DriveEvaluationRequired => {
            "an exact-drive reuse evaluation is required"
        }
    };
    let mut value =
        serde_json::to_value(record).expect("serializing a media inventory summary cannot fail");
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "pool_id".to_owned(),
            json!(record.pool.as_ref().map(|pool| pool.id)),
        );
        object.insert(
            "pool_name".to_owned(),
            json!(record.pool.as_ref().map(|pool| pool.name.as_str())),
        );
        object.insert(
            "confirmed_location_id".to_owned(),
            json!(record
                .confirmed_location
                .as_ref()
                .map(|location| location.id)),
        );
        object.insert(
            "confirmed_location_name".to_owned(),
            json!(record
                .confirmed_location
                .as_ref()
                .map(|location| location.name.as_str())),
        );
        object.insert(
            "location_name".to_owned(),
            json!(record
                .confirmed_location
                .as_ref()
                .map(|location| location.name.as_str())),
        );
        object.insert("overwrite_eligible".to_owned(), json!(false));
        object.insert("overwrite_blockers".to_owned(), json!([blocker]));
    }
    value
}

fn map_source_browser_error(error: SourceBrowserError) -> AppError {
    match error {
        SourceBrowserError::InvalidPath
        | SourceBrowserError::PathTooLong
        | SourceBrowserError::NotDirectory
        | SourceBrowserError::SymlinkTraversal => AppError::Invalid(error.to_string()),
        SourceBrowserError::NotFound => AppError::NotFound,
        SourceBrowserError::InvalidRoot
        | SourceBrowserError::NonUtf8Name
        | SourceBrowserError::UnsafeEntryName
        | SourceBrowserError::ManifestUnavailable
        | SourceBrowserError::ManifestOutsideRoot
        | SourceBrowserError::InvalidManifestBinding
        | SourceBrowserError::ManifestIntegrity
        | SourceBrowserError::Io(_) => AppError::Blocked(error.to_string()),
    }
}

fn map_backup_ops_error(error: backup_ops::BackupOpsError) -> AppError {
    match error {
        backup_ops::BackupOpsError::Invalid(message) => AppError::Invalid(message),
        backup_ops::BackupOpsError::NotFound => AppError::NotFound,
        backup_ops::BackupOpsError::Conflict(message) => AppError::Conflict(message),
        backup_ops::BackupOpsError::Database(error) => AppError::Database(error),
    }
}

fn map_media_ops_error(error: media_ops::MediaOpsError) -> AppError {
    match error {
        media_ops::MediaOpsError::NotFound { .. } => AppError::NotFound,
        media_ops::MediaOpsError::Invalid { field, message } => {
            AppError::Invalid(format!("{field}: {message}"))
        }
        media_ops::MediaOpsError::Conflict { code, message } => {
            AppError::Conflict(format!("{code}: {message}"))
        }
        media_ops::MediaOpsError::StaleVersion { expected, actual } => AppError::Conflict(format!(
            "stale cartridge state version: expected {expected}, current {actual}"
        )),
        media_ops::MediaOpsError::IdempotencyConflict { key } => AppError::Conflict(format!(
            "idempotency key {key} was already used for a different media operation"
        )),
        media_ops::MediaOpsError::Invariant(message) => AppError::Internal(message),
        media_ops::MediaOpsError::Database(error) => AppError::Database(error),
    }
}

fn map_custody_error(error: CustodyOpsError) -> AppError {
    match error {
        CustodyOpsError::NotFound { .. } => AppError::NotFound,
        CustodyOpsError::Invalid { field, message } => {
            AppError::Invalid(format!("{field}: {message}"))
        }
        CustodyOpsError::Conflict { code, message } => {
            AppError::Conflict(format!("{code}: {message}"))
        }
        CustodyOpsError::StaleVersion { expected, actual } => AppError::Conflict(format!(
            "movement changed after it was opened (expected version {expected}, current version {actual}); refresh before confirming"
        )),
        CustodyOpsError::WrongSource {
            cartridge_id,
            expected_location_id,
            actual_location_id,
        } => AppError::Conflict(format!(
            "cartridge {cartridge_id} is at {actual_location_id:?}, not the movement source {expected_location_id:?}"
        )),
        CustodyOpsError::ReservationConflict {
            cartridge_id,
            reserved_by,
        } => AppError::Conflict(format!(
            "cartridge {cartridge_id} is reserved by {reserved_by}"
        )),
        CustodyOpsError::IdempotencyConflict { key } => AppError::Conflict(format!(
            "idempotency key {key:?} was already used for a different request"
        )),
        CustodyOpsError::Invariant(message) => AppError::Internal(message),
        CustodyOpsError::Database(source) => AppError::Database(source),
    }
}

async fn resolve_movement(
    pool: &PgPool,
    id: &str,
) -> Result<custody_ops::MovementDetail, AppError> {
    match Uuid::parse_str(id) {
        Ok(id) => custody_ops::get_movement(pool, id).await,
        Err(_) => custody_ops::get_movement_by_external_id(pool, id).await,
    }
    .map_err(map_custody_error)
}

async fn resolve_movement_id(pool: &PgPool, id: &str) -> Result<Uuid, AppError> {
    Ok(resolve_movement(pool, id).await?.summary.id)
}

fn runtime_mode(state: &AppState) -> &'static str {
    if state.demo_mode {
        "simulation"
    } else if state.lab_file_roundtrip_enabled {
        "attended_backup_and_recovery_alpha"
    } else {
        "alpha_control_plane"
    }
}

const fn default_true() -> bool {
    true
}

fn parse_uuid_path(value: &str, label: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::Invalid(format!("{label} id must be a UUID")))
}

fn canonical_json(value: &Value) -> Result<Vec<u8>, AppError> {
    serde_json::to_vec(value).map_err(|error| AppError::Internal(error.to_string()))
}

fn dedupe_preserving_order<T: PartialEq>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut unique = Vec::new();
    for item in items {
        if !unique.contains(&item) {
            unique.push(item);
        }
    }
    unique
}

fn parse_restore_mode(value: Option<&str>, representation: &str) -> Result<RestoreMode, AppError> {
    match value {
        Some("file_archive") => Ok(RestoreMode::FileArchive),
        Some("native_dataset") => Ok(RestoreMode::NativeDataset),
        Some("files_from_native") => Ok(RestoreMode::FilesFromNative),
        Some(_) => Err(AppError::Invalid(
            "mode must be file_archive, native_dataset, or files_from_native".to_owned(),
        )),
        None if representation == "file" => Ok(RestoreMode::FileArchive),
        None if representation == "native_zfs" => Ok(RestoreMode::NativeDataset),
        None => Err(AppError::Invalid(format!(
            "recovery-point representation {representation} has no supported restore mode"
        ))),
    }
}

fn ensure_restore_mode_matches(mode: RestoreMode, representation: &str) -> Result<(), AppError> {
    let matches = matches!(
        (mode, representation),
        (RestoreMode::FileArchive, "file")
            | (
                RestoreMode::NativeDataset | RestoreMode::FilesFromNative,
                "native_zfs"
            )
    );
    if matches {
        Ok(())
    } else {
        Err(AppError::Invalid(format!(
            "restore mode {} is incompatible with {representation} recovery media",
            restore_mode_name(mode)
        )))
    }
}

fn parse_conflict_policy(value: Option<&str>) -> Result<ConflictPolicy, AppError> {
    match value.unwrap_or("skip") {
        "skip" => Ok(ConflictPolicy::Skip),
        "rename" => Ok(ConflictPolicy::Rename),
        "replace" => Ok(ConflictPolicy::Replace),
        _ => Err(AppError::Invalid(
            "conflict_policy must be skip, rename, or replace".to_owned(),
        )),
    }
}

const fn restore_mode_name(mode: RestoreMode) -> &'static str {
    match mode {
        RestoreMode::FileArchive => "file_archive",
        RestoreMode::NativeDataset => "native_dataset",
        RestoreMode::FilesFromNative => "files_from_native",
    }
}

const fn conflict_policy_name(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Skip => "skip",
        ConflictPolicy::Rename => "rename",
        ConflictPolicy::Replace => "replace",
    }
}

fn restore_blocker_view(blocker: &RestoreBlocker) -> Value {
    match blocker {
        RestoreBlocker::MissingRecoveryDependency(dependency) => json!({
            "code": "missing_recovery_dependency",
            "message": format!("Resolve required recovery dependency: {dependency}."),
            "dependency": dependency
        }),
        RestoreBlocker::TapeUnavailable(tape) => json!({
            "code": "tape_unavailable",
            "message": format!("Reconcile and make tape {tape} available before restore."),
            "tape_id": tape
        }),
        RestoreBlocker::TapeLocationUnknown(tape) => json!({
            "code": "tape_location_unknown",
            "message": format!("Confirm the physical location of tape {tape}."),
            "tape_id": tape
        }),
        RestoreBlocker::NoQualifiedCompatibleDrive(tape) => json!({
            "code": "no_qualified_compatible_drive",
            "message": format!("Connect, reconcile, and qualify a read-compatible drive for tape {tape}."),
            "tape_id": tape
        }),
        RestoreBlocker::RequiredKeyUnavailable(key) => json!({
            "code": "required_key_unavailable",
            "message": format!("Make encryption key lineage {key} available."),
            "key_id": key
        }),
        RestoreBlocker::DestinationNotApproved => json!({
            "code": "destination_not_approved",
            "message": "Confirm that the isolated destination is approved for recovery output."
        }),
        RestoreBlocker::DestinationAclSemanticsUnsupported => json!({
            "code": "destination_acl_semantics_unsupported",
            "message": "Use a destination whose ACL semantics can represent the recorded metadata."
        }),
        RestoreBlocker::IdentityMappingIncomplete => json!({
            "code": "identity_mapping_incomplete",
            "message": "Complete the destination user and group identity mapping."
        }),
        RestoreBlocker::InsufficientDestinationCapacity {
            required_with_reserve,
            available,
        } => json!({
            "code": "insufficient_destination_capacity",
            "message": format!("Provide at least {required_with_reserve} bytes including reserve; {available} bytes were declared available."),
            "required_with_reserve": required_with_reserve,
            "available": available
        }),
        RestoreBlocker::DestructiveReplacementRequiresAdministratorReauthentication => json!({
            "code": "administrator_reauthentication_required",
            "message": "Reauthenticate as an administrator before planning replacement in an existing destination."
        }),
        RestoreBlocker::DestructiveReplacementRequiresPreview => json!({
            "code": "destructive_preview_required",
            "message": "Review and accept the destructive replacement preview."
        }),
        RestoreBlocker::NativeDestinationMustBeNewOrExplicitlyReplaced => json!({
            "code": "native_destination_must_be_new_or_replaced",
            "message": "Choose a new native destination or explicitly select replacement."
        }),
        RestoreBlocker::AutomaticNativePublicationForbidden => json!({
            "code": "automatic_native_publication_forbidden",
            "message": "Disable automatic publication; recovered datasets must remain isolated for review."
        }),
        RestoreBlocker::FileDigestEvidenceUnavailable => json!({
            "code": "file_digest_evidence_unavailable",
            "message": "Select a recovery point with retained per-file digest evidence."
        }),
        RestoreBlocker::NativeStreamValidationUnavailable => json!({
            "code": "native_stream_validation_unavailable",
            "message": "Select a recovery chain with validated native stream evidence."
        }),
    }
}

fn restore_warning_view(warning: &RestoreWarning) -> Value {
    match warning {
        RestoreWarning::FullReadbackNotPassed => json!({
            "code": "full_readback_not_passed",
            "message": "No complete passing physical-media readback covers this recovery chain."
        }),
        RestoreWarning::RestoreTestNotPassed => json!({
            "code": "restore_test_not_passed",
            "message": "The selected recovery point has no passing isolated restore test."
        }),
        RestoreWarning::RecoveryKeyPackageNotTested(key) => json!({
            "code": "recovery_key_package_not_tested",
            "message": format!("Recovery packaging for key lineage {key} has not been declared tested."),
            "key_id": key
        }),
        RestoreWarning::CapacityEstimateUnavailable => json!({
            "code": "capacity_estimate_unavailable",
            "message": "Destination capacity was not fully declared; verify capacity before execution."
        }),
        RestoreWarning::CapacityIsAnEstimate => json!({
            "code": "capacity_is_an_estimate",
            "message": "Capacity values are planning estimates, not an execution-time reservation."
        }),
        RestoreWarning::ExistingConflictsWillBeSkipped(count) => json!({
            "code": "existing_conflicts_will_be_skipped",
            "message": format!("{count} declared destination conflicts will be skipped."),
            "count": count
        }),
        RestoreWarning::ExistingConflictsWillBeRenamed(count) => json!({
            "code": "existing_conflicts_will_be_renamed",
            "message": format!("{count} declared destination conflicts will be renamed."),
            "count": count
        }),
        RestoreWarning::NativeExtractionHasStreamLevelEvidenceOnly => json!({
            "code": "native_extraction_has_stream_level_evidence_only",
            "message": "Native extraction has stream-level evidence but no independent per-file digest proof."
        }),
    }
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
}

fn empty_summary() -> Value {
    json!({"protected_sources":0,"active_jobs":0,"required_actions":0})
}

fn empty_inventory() -> Value {
    json!({
        "installations":0,
        "truenas_instances":0,
        "drives":0,
        "accessible_drives":0,
        "reconciled_drives":0,
        "qualified_drives":0,
        "cartridges":0
    })
}

fn preflight_check(id: &str, label: &str, status: &str, detail: &str) -> Value {
    json!({"id":id,"label":label,"status":status,"detail":detail})
}

fn derived_preflight_status(checks: &[Value]) -> &'static str {
    if !checks.is_empty()
        && checks
            .iter()
            .all(|check| check.get("status").and_then(Value::as_str) == Some("passed"))
    {
        "ready"
    } else {
        "blocked"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_request() -> CreateMediaRequest {
        CreateMediaRequest {
            internal_label: " TS-L9-000001 ".to_owned(),
            barcode: Some(" L90001 ".to_owned()),
            media_generation: 9,
            media_format: None,
            worm: false,
        }
    }

    #[test]
    fn preflight_status_is_derived_from_all_checks() {
        let ready = vec![
            preflight_check("database", "Database", "passed", "ready"),
            preflight_check("source_mount", "Source", "passed", "ready"),
            preflight_check("engine", "Engine", "passed", "ready"),
        ];
        assert_eq!(derived_preflight_status(&ready), "ready");

        let mut blocked = ready;
        blocked[1] = preflight_check("source_mount", "Source", "blocked", "not ready");
        assert_eq!(derived_preflight_status(&blocked), "blocked");
        assert_eq!(derived_preflight_status(&[]), "blocked");
    }

    #[tokio::test]
    async fn operational_system_metadata_reports_the_attended_bacula_path() {
        let state = AppState::new(
            None,
            false,
            true,
            true,
            true,
            PathBuf::from("/source"),
            PathBuf::from("/restore"),
        );
        let Json(value) = system(State(state)).await.expect("system metadata");

        assert_eq!(value["engine"]["adapter"], "tapescale-bacula-attended");
        assert_eq!(value["engine"]["engine_name"], "Bacula Community");
        assert_eq!(value["engine"]["engine_version"], BACULA_VERSION);
        assert_eq!(value["engine"]["qualification"], "detected_unqualified");
        assert_eq!(value["engine"]["format_selected"], false);
        assert_eq!(value["capabilities"]["file_backup"], true);
        assert_eq!(value["capabilities"]["native_stream"], false);
        assert_eq!(value["capabilities"]["multi_volume"], true);
        assert_eq!(value["capabilities"]["changer_control"], false);
        assert_eq!(
            value["operational_backup_data_path"]["production_eligible"],
            false
        );
    }

    #[tokio::test]
    async fn demo_system_metadata_keeps_the_simulator_identity() {
        let state = AppState::new(
            None,
            true,
            false,
            false,
            false,
            PathBuf::from("/source"),
            PathBuf::from("/restore"),
        );
        let Json(value) = system(State(state)).await.expect("system metadata");

        assert_eq!(value["engine"]["adapter"], "tapescale-simulator");
        assert_eq!(value["engine"]["qualification"], "simulated");
        assert_eq!(value["mode"], "simulation");
        assert_eq!(
            value["operational_backup_data_path"]["production_eligible"],
            false
        );
    }

    #[test]
    fn media_registration_is_normalized_and_defaults_format() {
        let media = valid_request().validate().expect("request is valid");
        assert_eq!(media.internal_label, "TS-L9-000001");
        assert_eq!(media.barcode.as_deref(), Some("L90001"));
        assert_eq!(media.media_generation, 9);
        assert_eq!(media.media_format, "LTO-9");
    }

    #[test]
    fn media_registration_rejects_invalid_generation() {
        let mut request = valid_request();
        request.media_generation = 11;
        assert!(matches!(request.validate(), Err(AppError::Invalid(_))));
    }

    #[test]
    fn media_registration_rejects_ambiguous_label_characters() {
        let mut request = valid_request();
        request.internal_label = "../../tape".to_owned();
        assert!(matches!(request.validate(), Err(AppError::Invalid(_))));
    }

    #[test]
    fn blank_barcode_is_not_persisted() {
        let mut request = valid_request();
        request.barcode = Some("   ".to_owned());
        assert_eq!(request.validate().expect("request is valid").barcode, None);
    }

    #[test]
    fn backup_plan_legacy_acknowledgements_are_optional_and_backward_compatible() {
        let without_legacy_fields: CreateBackupProfileRequest = serde_json::from_value(json!({
            "name": "Radio archive",
            "source_subpath": "pool/archive/.zfs/snapshot/nightly"
        }))
        .expect("the simplified request should deserialize");
        assert_eq!(
            without_legacy_fields._legacy_source_snapshot_attested_read_only,
            None
        );
        assert_eq!(without_legacy_fields._legacy_limitations_acknowledged, None);

        let old_client_payload: CreateBackupProfileRequest = serde_json::from_value(json!({
            "name": "Radio archive",
            "source_subpath": "pool/archive/.zfs/snapshot/nightly",
            "source_snapshot_attested_read_only": false,
            "limitations_acknowledged": true
        }))
        .expect("legacy acknowledgement fields should remain accepted");
        assert_eq!(
            old_client_payload._legacy_source_snapshot_attested_read_only,
            Some(false)
        );
        assert_eq!(
            old_client_payload._legacy_limitations_acknowledged,
            Some(true)
        );

        let run_without_attestation: CreateBackupRunRequest =
            serde_json::from_value(json!({"idempotency_key": "manual-radio-001"}))
                .expect("the simplified run request should deserialize");
        assert_eq!(
            run_without_attestation._legacy_source_snapshot_attested_read_only,
            None
        );

        let legacy_run: CreateBackupRunRequest = serde_json::from_value(json!({
            "source_snapshot_attested_read_only": false
        }))
        .expect("the legacy run field should remain accepted");
        assert_eq!(
            legacy_run._legacy_source_snapshot_attested_read_only,
            Some(false)
        );
    }

    #[test]
    fn drive_motion_requires_exact_closed_confirmation() {
        let request = DriveActionRequest {
            stable_id: "scsi:naa.5001".to_owned(),
            action: SasMotionOperation::Rewind,
            confirmation: "REWIND SCRATCH MEDIA; PRIOR WRITE MAY BE FINALIZED".to_owned(),
            scratch_media_confirmed: true,
        };
        assert_eq!(
            request.validate().expect("valid confirmation"),
            request.stable_id
        );

        let invalid = DriveActionRequest {
            confirmation: "rewind".to_owned(),
            ..request
        };
        assert!(matches!(invalid.validate(), Err(AppError::Invalid(_))));
    }

    #[test]
    fn tape_qualification_requires_exact_destructive_confirmation() {
        let request = QueueQualificationRunRequest {
            drive_id: Uuid::now_v7(),
            stable_id: "scsi:naa.5001".to_owned(),
            cartridge_id: Uuid::now_v7(),
            cartridge_label: "SCRATCH-L9-001".to_owned(),
            confirmation: "DESTROY SCRATCH-L9-001 WITH BACULA BTAPE TEST".to_owned(),
            scratch_media_confirmed: true,
            destructive_overwrite_confirmed: true,
            idempotency_key: "  qualification-test-001  ".to_owned(),
        };
        let (stable_id, cartridge_label, idempotency_key) =
            request.validate().expect("qualification request is valid");
        assert_eq!(idempotency_key, "qualification-test-001");
        let fingerprint = request.request_fingerprint(stable_id, cartridge_label);
        assert_eq!(fingerprint.len(), 64);
        let changed = QueueQualificationRunRequest {
            cartridge_id: Uuid::now_v7(),
            ..request.clone()
        };
        assert_ne!(
            fingerprint,
            changed.request_fingerprint(stable_id, cartridge_label)
        );

        let invalid = QueueQualificationRunRequest {
            confirmation: "DESTROY THE TAPE".to_owned(),
            ..request.clone()
        };
        assert!(matches!(invalid.validate(), Err(AppError::Invalid(_))));
        let invalid_key = QueueQualificationRunRequest {
            idempotency_key: "qualification\nretry".to_owned(),
            ..request.clone()
        };
        assert!(matches!(invalid_key.validate(), Err(AppError::Invalid(_))));
        let missing_key = QueueQualificationRunRequest {
            idempotency_key: String::new(),
            ..request
        };
        assert!(matches!(missing_key.validate(), Err(AppError::Invalid(_))));
    }

    #[test]
    fn restore_contract_uses_explicit_modes_and_safe_defaults() {
        assert_eq!(
            parse_restore_mode(None, "file").expect("file mode"),
            RestoreMode::FileArchive
        );
        assert_eq!(
            parse_restore_mode(Some("files_from_native"), "native_zfs")
                .expect("native extraction mode"),
            RestoreMode::FilesFromNative
        );
        assert!(ensure_restore_mode_matches(RestoreMode::FileArchive, "native_zfs").is_err());
        assert_eq!(
            parse_conflict_policy(None).expect("safe conflict default"),
            ConflictPolicy::Skip
        );
    }

    #[test]
    fn restore_prerequisite_ids_are_deduplicated_without_reordering() {
        assert_eq!(
            dedupe_preserving_order(["tape-b", "tape-a", "tape-b", "tape-c"]),
            vec!["tape-b", "tape-a", "tape-c"]
        );
    }

    #[test]
    fn restore_rehearsal_requires_exact_attended_confirmation() {
        let plan_id = Uuid::now_v7();
        let mut request = QueueRestoreRehearsalRequest {
            plan_id,
            preview_hash: "a".repeat(64),
            drive_id: Uuid::now_v7(),
            confirmation: format!("RESTORE PLAN {plan_id} TO ISOLATED DESTINATION"),
            tape_loaded_confirmed: true,
            isolated_destination_confirmed: true,
            attended_confirmed: true,
            actor: Some("attending operator".to_owned()),
        };
        assert_eq!(
            request.validate().expect("valid request"),
            "attending operator"
        );

        request.confirmation = "RESTORE IT".to_owned();
        assert!(matches!(request.validate(), Err(AppError::Invalid(_))));
        request.confirmation = format!("RESTORE PLAN {plan_id} TO ISOLATED DESTINATION");
        request.tape_loaded_confirmed = false;
        assert!(matches!(request.validate(), Err(AppError::Invalid(_))));
    }

    #[test]
    fn rehearsal_evidence_redacts_worker_paths_and_bounds_values() {
        let sanitized = bounded_restore_evidence_value(&json!({
            "runtime": {
                "director_config_sha256": "a".repeat(64),
                "daemon_logs": ["/var/lib/tapescale/private/director.log"]
            },
            "source_mount_path": "/source/private-snapshot",
            "lease": {
                "drive_lease_id": Uuid::now_v7(),
                "drive_fencing_token": 42,
                "media_lease_id": Uuid::now_v7(),
                "media_fencing_token": 43
            },
            "artifact": {
                "storage_ref": "/var/lib/tapescale/evidence/private.json",
                "sha256": "b".repeat(64)
            },
            "restore_destination": "planned-restores/run-1",
            "oversized": "x".repeat(5000)
        }));
        assert!(sanitized["runtime"].get("daemon_logs").is_none());
        assert!(sanitized.get("source_mount_path").is_none());
        assert!(sanitized["lease"].get("drive_lease_id").is_none());
        assert!(sanitized["lease"].get("drive_fencing_token").is_none());
        assert!(sanitized["lease"].get("media_lease_id").is_none());
        assert!(sanitized["lease"].get("media_fencing_token").is_none());
        assert!(sanitized["artifact"].get("storage_ref").is_none());
        assert_eq!(sanitized["restore_destination"], "planned-restores/run-1");
        assert!(sanitized["oversized"]
            .as_str()
            .is_some_and(|value| value.ends_with("…[truncated]")));
    }

    #[test]
    fn canonical_evidence_json_is_compact_and_key_sorted() {
        let encoded =
            canonical_json(&json!({"z": 1, "a": {"y": 2, "b": 3}})).expect("JSON should serialize");
        assert_eq!(
            String::from_utf8(encoded).expect("JSON is UTF-8"),
            r#"{"a":{"b":3,"y":2},"z":1}"#
        );
    }

    #[test]
    fn generated_movement_reference_is_stable_for_idempotent_retries() {
        let first = generated_movement_external_id("movement-create:browser-operation-42");
        let retry = generated_movement_external_id("movement-create:browser-operation-42");
        let other = generated_movement_external_id("movement-create:browser-operation-43");

        assert_eq!(first, retry);
        assert_ne!(first, other);
        assert!(first.starts_with("TS-MOVE-"));
        assert_eq!(first.len(), 32);
    }

    #[test]
    fn retained_manifest_browser_capacity_is_shared_across_state_clones() {
        let state = AppState::new(
            None,
            false,
            false,
            false,
            false,
            PathBuf::from("/source"),
            PathBuf::from("/restore"),
        );
        let all_slots = state
            .retained_manifest_browser_slots
            .clone()
            .try_acquire_many_owned(
                u32::try_from(RETAINED_MANIFEST_BROWSER_CONCURRENCY)
                    .expect("concurrency limit fits u32"),
            )
            .expect("configured browser slots are available");

        assert!(state
            .clone()
            .retained_manifest_browser_slots
            .try_acquire_owned()
            .is_err());
        drop(all_slots);
        assert!(state
            .retained_manifest_browser_slots
            .try_acquire_owned()
            .is_ok());
    }
}
