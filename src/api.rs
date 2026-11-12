use std::{path::PathBuf, sync::Arc};

use axum::{
    extract::{Path, State},
    http::{HeaderName, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::info;
use uuid::Uuid;

use crate::{
    data_path::{isolated_restore_destination, resolve_existing_selection, verify_read_only_mount},
    device::SasMotionOperation,
    domain::path::validate_archive_path,
    engine::{simulator::SimulatorEngine, TapeEngine},
    error::AppError,
    qualification, store, VERSION,
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
}

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
        }
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
        .route("/api/v1/media", get(media).post(create_media))
        .route("/api/v1/drives", get(drives))
        .route("/api/v1/drives/{id}/reconcile", post(reconcile_drive))
        .route("/api/v1/drives/{id}/actions", post(queue_drive_action))
        .route("/api/v1/movements", get(movements))
        .route("/api/v1/movements/{id}/confirm", post(confirm_movement))
        .route("/api/v1/verification", get(verification))
        .route("/api/v1/recovery-points", get(recovery_points))
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
    Ok(Json(json!({
        "product":"TapeScale",
        "version":VERSION,
        "mode":runtime_mode(&state),
        "backend_language":"Rust",
        "engine":identity,
        "capabilities":capabilities,
        "lab_tape_motion_enabled":state.lab_tape_motion_enabled,
        "lab_tape_qualification_enabled":state.lab_tape_qualification_enabled,
        "lab_file_roundtrip_enabled":state.lab_file_roundtrip_enabled,
        "file_roundtrip_scope": if state.lab_file_roundtrip_enabled {
            "laboratory_single_tape_full_file_backup_and_physical_restore"
        } else {
            "disabled"
        },
        "laboratory_data_path": {
            "enabled": state.lab_file_roundtrip_enabled,
            "engine": "Bacula Community",
            "engine_version": "15.0.3",
            "adapter_version": "g0.2-lab-v1",
            "production_eligible": false,
            "encryption": "disabled_with_explicit_per_run_deviation"
        },
        "operational_data":"postgresql_only"
    })))
}

async fn overview(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let Some(pool) = state.pool.as_ref() else {
        return Ok(Json(json!({
            "summary": empty_summary(),
            "sources": [],
            "actions": [],
            "storage": "not_configured"
        })));
    };

    let summary = store::overview_counts(pool).await?;
    let sources = store::list_sources(pool).await?;
    Ok(Json(json!({
        "summary": summary,
        "sources": sources,
        "actions": [],
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

async fn media(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let records = match state.pool.as_ref() {
        Some(pool) => store::list_media(pool).await?,
        None => Vec::new(),
    };
    let media = records.iter().map(media_view).collect::<Vec<_>>();
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
        if internal_label.chars().count() > 128 {
            return Err(AppError::Invalid(
                "internal_label must be at most 128 characters".to_owned(),
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
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "media":media_view(&record),
            "durability":"postgresql",
            "audit":"appended"
        })),
    ))
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

async fn movements(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let movements = match state.pool.as_ref() {
        Some(pool) => store::list_movements(pool).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({"movements":movements})))
}

#[derive(Debug, Deserialize)]
struct MovementConfirmation {
    actor: String,
    destination: String,
    tape_ids: Vec<String>,
    #[serde(default)]
    note: String,
}

async fn confirm_movement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(confirmation): Json<MovementConfirmation>,
) -> Result<Json<Value>, AppError> {
    if confirmation.actor.trim().is_empty() || confirmation.destination.trim().is_empty() {
        return Err(AppError::Invalid(
            "actor and destination are required".to_owned(),
        ));
    }
    if confirmation.tape_ids.is_empty() {
        return Err(AppError::Blocked(
            "custody cannot advance without physically confirmed tape identities".to_owned(),
        ));
    }
    let pool = require_pool(&state)?;
    let exists = store::list_movements(pool)
        .await?
        .into_iter()
        .any(|movement| movement.external_id == id || movement.id.to_string() == id);
    if !exists {
        return Err(AppError::NotFound);
    }

    let _ = confirmation.note;
    Err(AppError::Blocked(
        "durable movement confirmation is not implemented yet; no custody state was changed"
            .to_owned(),
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
            "the laboratory file roundtrip is disabled; set TAPESCALE_LAB_FILE_ROUNDTRIP_ENABLED=true on the API and worker only for an attended single-tape recovery exercise"
                .to_owned(),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct RestorePlanRequest {
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    key_available: bool,
}

async fn plan_restore(Json(request): Json<RestorePlanRequest>) -> Result<Json<Value>, AppError> {
    if let Some(destination) = &request.destination {
        if validate_archive_path(std::path::Path::new(destination)).is_err() {
            return Err(AppError::Invalid(
                "destination must be a contained dataset-relative path".to_owned(),
            ));
        }
    }
    let _ = request.key_available;
    Err(AppError::Blocked(
        "restore planning requires a persisted recovery-point selection and is not implemented yet"
            .to_owned(),
    ))
}

async fn preflight(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let Some(pool) = state.pool.as_ref() else {
        return Ok(Json(json!({
            "status":"blocked",
            "checked_at":chrono::Utc::now(),
            "checks":[
                preflight_check("database", "Database", "blocked", "PostgreSQL is not configured"),
                preflight_check("engine", "Tape backup engine", "blocked", "Backup jobs still use the simulator; Bacula btape is qualification-only")
            ],
            "inventory":empty_inventory()
        })));
    };

    let inventory = store::inventory_counts(pool).await?;
    let truenas_status = if inventory.truenas_instances > 0 {
        "registered"
    } else {
        "blocked"
    };
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
    let qualification_status = if inventory.qualified_drives > 0 {
        "passed"
    } else {
        "blocked"
    };
    let media_status = if inventory.cartridges > 0 {
        "registered"
    } else {
        "blocked"
    };
    let engine_status = if state.lab_file_roundtrip_enabled {
        "passed"
    } else {
        "blocked"
    };
    Ok(Json(json!({
        "status":"blocked",
        "checked_at":chrono::Utc::now(),
        "checks":[
            preflight_check("database", "Database", "passed", "PostgreSQL responded"),
            preflight_check("truenas_registration", "TrueNAS registration", truenas_status,
                if inventory.truenas_instances > 0 {"A registration exists; live connectivity is not checked yet"} else {"No TrueNAS system is registered"}),
            preflight_check("sas_device_access", "SAS tape device access", access_status,
                if inventory.accessible_drives > 0 {"The worker can read status from a configured non-rewinding tape device"} else {"No configured SAS tape device is currently accessible"}),
            preflight_check("stable_drive_identity", "Stable drive identity", identity_status,
                if inventory.reconciled_drives > 0 {"An accessible drive is bound to its observed stable identity"} else {"No accessible drive identity has been reconciled"}),
            preflight_check("physical_drive", "Qualified tape drive", qualification_status,
                if inventory.qualified_drives > 0 {"A physically qualified drive is recorded"} else {"No physically qualified drive is recorded"}),
            preflight_check("media_inventory", "Media inventory", media_status,
                if inventory.cartridges > 0 {"Cartridges are registered; on-tape identity still governs safe use"} else {"No cartridges are registered"}),
            preflight_check("engine", "Tape backup engine", engine_status,
                if state.lab_file_roundtrip_enabled {"Pinned Bacula 15.0.3 is enabled only for attended G0.2 single-tape recovery exercises; general backup scheduling remains blocked"} else {"General backup jobs use the simulator and the G0.2 Bacula data path is disabled"})
        ],
        "inventory":inventory
    })))
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

#[derive(Debug, Deserialize)]
struct QueueQualificationRunRequest {
    drive_id: Uuid,
    stable_id: String,
    cartridge_id: Uuid,
    cartridge_label: String,
    confirmation: String,
    scratch_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
}

impl QueueQualificationRunRequest {
    fn expected_confirmation(&self) -> String {
        format!("DESTROY {} WITH BACULA BTAPE TEST", self.cartridge_label)
    }

    fn validate(&self) -> Result<(&str, &str), AppError> {
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
        Ok((stable_id, &self.cartridge_label))
    }
}

async fn queue_qualification_run(
    State(state): State<AppState>,
    Json(request): Json<QueueQualificationRunRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if !state.lab_tape_qualification_enabled {
        return Err(AppError::Blocked(
            "destructive lab tape qualification is disabled; set TAPESCALE_LAB_TAPE_QUALIFICATION_ENABLED=true on the API and worker only for an attended scratch-media test host"
                .to_owned(),
        ));
    }
    let pool = require_pool(&state)?;
    let (stable_id, cartridge_label) = request.validate()?;
    match store::queue_qualification_run(
        pool,
        request.drive_id,
        stable_id,
        request.cartridge_id,
        cartridge_label,
        request.scratch_media_confirmed,
        request.destructive_overwrite_confirmed,
    )
    .await?
    {
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
                    "destructive":true,
                    "engine_candidate":"Bacula Community 15.0.3",
                    "physical_qualification":"not_established"
                })),
            ))
        }
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

fn runtime_mode(state: &AppState) -> &'static str {
    if state.demo_mode {
        "simulation"
    } else if state.lab_file_roundtrip_enabled {
        "laboratory_recovery_exercise"
    } else {
        "unqualified_preview"
    }
}

fn media_view(record: &store::MediaRecord) -> Value {
    let overwrite_blocker = match record.identity_status.as_str() {
        "unknown" => "on-tape identity has not been reconciled",
        "foreign" => "foreign media is protected",
        "mismatch" => "catalog and on-tape identity do not match",
        _ => "overwrite eligibility evaluation is not implemented",
    };
    let mut value = serde_json::to_value(record).expect("serializing a media record cannot fail");
    if let Some(object) = value.as_object_mut() {
        object.insert("overwrite_eligible".to_owned(), json!(false));
        object.insert("overwrite_blockers".to_owned(), json!([overwrite_blocker]));
    }
    value
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
        };
        assert!(request.validate().is_ok());

        let invalid = QueueQualificationRunRequest {
            confirmation: "DESTROY THE TAPE".to_owned(),
            ..request
        };
        assert!(matches!(invalid.validate(), Err(AppError::Invalid(_))));
    }
}
