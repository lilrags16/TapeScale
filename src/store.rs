use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{pool::PoolConnection, postgres::PgPoolOptions, FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::device::{MissingSasDrive, SasDriveObservation, SasMotionOperation};

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct OverviewCounts {
    pub protected_sources: i64,
    pub active_jobs: i64,
    pub required_actions: i64,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SourceRecord {
    pub id: Uuid,
    pub dataset_name: String,
    pub source_type: String,
    pub logical_root: String,
    pub representation: Option<String>,
    pub recovery_time: Option<DateTime<Utc>>,
    pub recovery_status: Option<String>,
    pub recovery_level: Option<String>,
    pub verified_points: i64,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct JobRecord {
    pub job_id: Uuid,
    pub external_id: String,
    pub kind: String,
    pub priority: i32,
    pub job_created_at: DateTime<Utc>,
    pub policy_name: Option<String>,
    pub representation: Option<String>,
    pub dataset_name: Option<String>,
    pub run_id: Option<Uuid>,
    pub state: Option<String>,
    pub attempt: Option<i32>,
    pub correlation_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub failure_class: Option<String>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct JobTransitionRecord {
    pub occurred_at: DateTime<Utc>,
    pub from_state: Option<String>,
    pub to_state: String,
    pub reason: String,
    pub evidence: Value,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct MediaRecord {
    pub id: Uuid,
    pub internal_label: String,
    pub barcode: Option<String>,
    pub media_generation: Option<i16>,
    pub media_format: Option<String>,
    pub identity_status: String,
    pub health_status: String,
    pub worm: bool,
    pub reserved_by: Option<String>,
    pub pool_name: Option<String>,
    pub location_name: Option<String>,
    pub active_holds: i64,
    pub has_content: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMedia {
    pub internal_label: String,
    pub barcode: Option<String>,
    pub media_generation: i16,
    pub media_format: String,
    pub worm: bool,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct DriveRecord {
    pub id: Uuid,
    pub stable_id: String,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub wwn: Option<String>,
    pub firmware: Option<String>,
    pub generation: Option<i16>,
    pub observed_path: Option<String>,
    pub expected_path: Option<String>,
    pub reconciled: bool,
    pub qualification_status: String,
    pub health: Value,
    pub last_observed_at: Option<DateTime<Utc>>,
    pub changer_stable_id: Option<String>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SlotRecord {
    pub id: Uuid,
    pub element_address: i32,
    pub kind: String,
    pub observed_barcode: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub changer_stable_id: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct MovementRecord {
    pub id: Uuid,
    pub external_id: String,
    pub kind: String,
    pub required_media_ids: Vec<Uuid>,
    pub state: String,
    pub readiness: Value,
    pub due_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub source_name: Option<String>,
    pub destination_name: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct VerificationRecord {
    pub id: Uuid,
    pub kind: String,
    pub scope: Value,
    pub outcome: String,
    pub evidence: Value,
    pub correlation_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub recovery_point_id: Uuid,
    pub recovery_time: DateTime<Utc>,
    pub dataset_name: String,
    pub drive_stable_id: Option<String>,
    pub media_labels: Vec<String>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RecoveryPointRecord {
    pub id: Uuid,
    pub representation: String,
    pub level: String,
    pub status: String,
    pub recovery_time: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub indefinite: bool,
    pub coverage: Value,
    pub manifest_digest: Option<String>,
    pub dataset_name: String,
    pub media_labels: Vec<String>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct InventoryCounts {
    pub installations: i64,
    pub truenas_instances: i64,
    pub drives: i64,
    pub accessible_drives: i64,
    pub reconciled_drives: i64,
    pub qualified_drives: i64,
    pub cartridges: i64,
}

#[derive(Clone, Debug)]
pub enum ReconcileDriveResult {
    Reconciled(Box<DriveRecord>),
    NotFound,
    Blocked,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct DriveOperationRecord {
    pub id: Uuid,
    pub drive_id: Uuid,
    pub expected_stable_id: String,
    pub expected_path: String,
    pub expected_vendor: Option<String>,
    pub expected_model: Option<String>,
    pub expected_serial: Option<String>,
    pub expected_wwn: Option<String>,
    pub expected_firmware: Option<String>,
    pub action: String,
    pub scratch_media_confirmed: bool,
    pub state: String,
    pub requested_by_label: String,
    pub correlation_id: Uuid,
    pub lease_id: Option<Uuid>,
    pub fencing_token: Option<i64>,
    pub attempt: i32,
    pub result: Value,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct QualificationRunRecord {
    pub id: Uuid,
    pub drive_id: Uuid,
    pub cartridge_id: Uuid,
    pub expected_stable_id: String,
    pub expected_path: String,
    pub expected_vendor: Option<String>,
    pub expected_model: Option<String>,
    pub expected_serial: Option<String>,
    pub expected_wwn: Option<String>,
    pub expected_firmware: Option<String>,
    pub expected_cartridge_label: String,
    pub expected_media_generation: Option<i16>,
    pub expected_media_format: Option<String>,
    pub engine_name: String,
    pub engine_version: String,
    pub test_kind: String,
    pub scratch_media_confirmed: bool,
    pub destructive_overwrite_confirmed: bool,
    pub state: String,
    pub requested_by_label: String,
    pub correlation_id: Uuid,
    pub lease_id: Option<Uuid>,
    pub fencing_token: Option<i64>,
    pub attempt: i32,
    pub result: Value,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct QualificationEventRecord {
    pub id: i64,
    pub event_type: String,
    pub evidence: Value,
    pub correlation_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow)]
struct QualificationCartridge {
    id: Uuid,
    internal_label: String,
    media_generation: Option<i16>,
    media_format: Option<String>,
    identity_status: String,
    worm: bool,
    reserved_by: Option<String>,
    active_holds: i64,
    has_content: bool,
}

#[derive(Clone, Debug)]
pub struct ClaimedQualificationRun {
    pub run: QualificationRunRecord,
    pub lease_id: Uuid,
    pub fencing_token: i64,
    pub worker_id: Uuid,
}

#[derive(Clone, Debug)]
pub enum QueueQualificationRunResult {
    Queued(Box<QualificationRunRecord>),
    NotFound,
    Blocked(String),
}

#[derive(Clone, Debug)]
pub enum ClaimQualificationRunResult {
    Claimed(Box<ClaimedQualificationRun>),
    Blocked { id: Uuid, reason: String },
    Empty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualificationTerminalState {
    Passed,
    Failed,
    Blocked,
    Interrupted,
}

impl QualificationTerminalState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClaimedDriveOperation {
    pub operation: DriveOperationRecord,
    pub lease_id: Uuid,
    pub fencing_token: i64,
    pub worker_id: Uuid,
}

#[derive(Clone, Debug)]
pub enum QueueDriveOperationResult {
    Queued(Box<DriveOperationRecord>),
    NotFound,
    Blocked(String),
}

#[derive(Clone, Debug)]
pub enum ClaimDriveOperationResult {
    Claimed(Box<ClaimedDriveOperation>),
    Blocked { id: Uuid, reason: String },
    Empty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveOperationTerminalState {
    Succeeded,
    Failed,
    Blocked,
}

impl DriveOperationTerminalState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

pub async fn connect(database_url: &str, timeout: Duration) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .min_connections(1)
        .acquire_timeout(timeout)
        .connect(database_url)
        .await
        .context("failed to connect to PostgreSQL")?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("failed to apply database migrations")?;

    Ok(pool)
}

pub async fn ready(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await
        .is_ok()
}

pub async fn acquire_worker_singleton(pool: &PgPool) -> Result<PoolConnection<Postgres>> {
    const WORKER_LOCK_ID: i64 = 0x5441_5045_574f_524b;
    let mut connection = pool
        .acquire()
        .await
        .context("failed to acquire the worker singleton connection")?;
    let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
        .bind(WORKER_LOCK_ID)
        .fetch_one(&mut *connection)
        .await
        .context("failed to acquire the worker singleton lock")?;
    anyhow::ensure!(
        acquired,
        "another TapeScale worker is already active for this database"
    );
    Ok(connection)
}

pub async fn release_worker_singleton(connection: &mut PoolConnection<Postgres>) -> Result<()> {
    const WORKER_LOCK_ID: i64 = 0x5441_5045_574f_524b;
    let released = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
        .bind(WORKER_LOCK_ID)
        .fetch_one(&mut **connection)
        .await
        .context("failed to release the worker singleton lock")?;
    anyhow::ensure!(released, "the worker singleton lock was not held");
    Ok(())
}

pub async fn overview_counts(pool: &PgPool) -> Result<OverviewCounts, sqlx::Error> {
    sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM sources WHERE enabled) AS protected_sources,
            (SELECT count(*) FROM job_runs
             WHERE state IN ('queued','preparing','waiting_for_media','writing','written','verifying','blocked','interrupted')
               AND finished_at IS NULL) AS active_jobs,
            ((SELECT count(*) FROM job_runs
              WHERE state IN ('waiting_for_media','blocked','interrupted','failed')
                AND finished_at IS NULL) +
             (SELECT count(*) FROM movement_tasks
              WHERE state IN ('ready','departed','partially_confirmed','overdue','blocked'))) AS required_actions",
    )
    .fetch_one(pool)
    .await
}

pub async fn list_sources(pool: &PgPool) -> Result<Vec<SourceRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.id, s.dataset_name, s.source_type, s.logical_root,
                latest.representation, latest.recovery_time,
                latest.status AS recovery_status, latest.level AS recovery_level,
                (SELECT count(*) FROM recovery_points counted
                 WHERE counted.source_id = s.id AND counted.status = 'verified') AS verified_points
         FROM sources s
         LEFT JOIN LATERAL (
             SELECT rp.representation, rp.recovery_time, rp.status, rp.level
             FROM recovery_points rp
             WHERE rp.source_id = s.id
             ORDER BY rp.recovery_time DESC
             LIMIT 1
         ) latest ON true
         WHERE s.enabled
         ORDER BY s.dataset_name",
    )
    .fetch_all(pool)
    .await
}

const JOB_SELECT: &str = "SELECT j.id AS job_id, j.external_id, j.kind, j.priority,
            j.created_at AS job_created_at, p.name AS policy_name,
            p.representation, s.dataset_name, latest.id AS run_id,
            latest.state, latest.attempt, latest.correlation_id,
            latest.started_at, latest.finished_at, latest.failure_class
     FROM jobs j
     LEFT JOIN protection_policies p ON p.id = j.policy_id
     LEFT JOIN sources s ON s.id = p.source_id
     LEFT JOIN LATERAL (
         SELECT jr.id, jr.state, jr.attempt, jr.correlation_id,
                jr.started_at, jr.finished_at, jr.failure_class, jr.created_at
         FROM job_runs jr
         WHERE jr.job_id = j.id
         ORDER BY jr.created_at DESC
         LIMIT 1
     ) latest ON true";

pub async fn list_jobs(pool: &PgPool) -> Result<Vec<JobRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{JOB_SELECT} ORDER BY COALESCE(latest.created_at, j.created_at) DESC"
    ))
    .fetch_all(pool)
    .await
}

pub async fn get_job(pool: &PgPool, id: &str) -> Result<Option<JobRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{JOB_SELECT} WHERE j.external_id = $1 OR j.id::text = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn job_transitions(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<JobTransitionRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT occurred_at, from_state, to_state, reason, evidence, correlation_id
         FROM job_transitions WHERE run_id = $1 ORDER BY occurred_at",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
}

const MEDIA_SELECT: &str =
    "SELECT c.id, c.internal_label, c.barcode, c.media_generation, c.media_format,
            c.identity_status, c.health_status, c.worm, c.reserved_by,
            p.name AS pool_name, l.name AS location_name,
            (SELECT count(*) FROM holds h
             WHERE h.cartridge_id = c.id AND h.released_at IS NULL) AS active_holds,
            EXISTS (SELECT 1 FROM volume_records vr WHERE vr.cartridge_id = c.id) AS has_content,
            c.created_at, c.updated_at
     FROM cartridges c
     LEFT JOIN media_pools p ON p.id = c.pool_id
     LEFT JOIN locations l ON l.id = c.confirmed_location_id";

pub async fn list_media(pool: &PgPool) -> Result<Vec<MediaRecord>, sqlx::Error> {
    sqlx::query_as(&format!("{MEDIA_SELECT} ORDER BY c.internal_label"))
        .fetch_all(pool)
        .await
}

pub async fn create_media(pool: &PgPool, media: &NewMedia) -> Result<MediaRecord, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO cartridges
            (internal_label, barcode, media_generation, media_format, worm,
             identity_status, health_status)
         VALUES ($1, $2, $3, $4, $5, 'unknown', 'unknown')
         RETURNING id",
    )
    .bind(&media.internal_label)
    .bind(&media.barcode)
    .bind(media.media_generation)
    .bind(&media.media_format)
    .bind(media.worm)
    .fetch_one(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         VALUES ($1, 'local-api', 'media.register', 'cartridge', $2,
                 $3, 'created', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(id.to_string())
    .bind(json!({
        "internal_label": media.internal_label,
        "barcode": media.barcode,
        "media_generation": media.media_generation,
        "media_format": media.media_format,
        "worm": media.worm
    }))
    .bind(Uuid::now_v7())
    .execute(&mut *transaction)
    .await?;

    let record = sqlx::query_as(&format!("{MEDIA_SELECT} WHERE c.id = $1"))
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(record)
}

const DRIVE_SELECT: &str = "SELECT d.id, d.stable_id, d.vendor, d.model, d.serial, d.wwn,
                d.firmware, d.generation, d.observed_path, d.expected_path,
                d.reconciled, d.qualification_status, d.health,
                d.last_observed_at, c.stable_id AS changer_stable_id
         FROM drives d
         LEFT JOIN changers c ON c.id = d.changer_id";

pub async fn list_drives(pool: &PgPool) -> Result<Vec<DriveRecord>, sqlx::Error> {
    sqlx::query_as(&format!("{DRIVE_SELECT} ORDER BY d.stable_id"))
        .fetch_all(pool)
        .await
}

pub async fn record_drive_observation(
    pool: &PgPool,
    observation: &SasDriveObservation,
) -> Result<DriveRecord, sqlx::Error> {
    let mut transaction = pool.begin().await?;

    sqlx::query(
        "UPDATE drives
         SET reconciled = false,
             qualification_status = 'detected_unqualified',
             health = health || jsonb_build_object(
                 'present', false,
                 'accessible', false,
                 'status', 'identity_replaced',
                 'detail', 'A different stable identity was observed at this device path',
                 'observed_at', $1::timestamptz),
             last_observed_at = $1
         WHERE observed_path = $2 AND stable_id <> $3",
    )
    .bind(observation.observed_at)
    .bind(&observation.observed_path)
    .bind(&observation.stable_id)
    .execute(&mut *transaction)
    .await?;

    let health = observation.health();
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO drives
            (stable_id, vendor, model, serial, wwn, firmware, generation,
             observed_path, reconciled, qualification_status, health, last_observed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7,
                 $8, false, 'detected_unqualified', $9, $10)
         ON CONFLICT (stable_id) DO UPDATE SET
             vendor = EXCLUDED.vendor,
             model = EXCLUDED.model,
             serial = EXCLUDED.serial,
             wwn = EXCLUDED.wwn,
             firmware = EXCLUDED.firmware,
             generation = COALESCE(EXCLUDED.generation, drives.generation),
             observed_path = EXCLUDED.observed_path,
             reconciled = drives.reconciled
                 AND drives.expected_path = EXCLUDED.observed_path
                 AND drives.vendor IS NOT DISTINCT FROM EXCLUDED.vendor
                 AND drives.model IS NOT DISTINCT FROM EXCLUDED.model
                 AND drives.firmware IS NOT DISTINCT FROM EXCLUDED.firmware
                 AND $11
                 AND NOT $12
                 AND $13
                 AND $14
                 AND $15,
             qualification_status = CASE
                 WHEN drives.vendor IS DISTINCT FROM EXCLUDED.vendor
                   OR drives.model IS DISTINCT FROM EXCLUDED.model
                   OR drives.firmware IS DISTINCT FROM EXCLUDED.firmware
                   OR NOT $11
                   OR $12
                 THEN 'detected_unqualified'
                 ELSE drives.qualification_status
             END,
             health = EXCLUDED.health,
             last_observed_at = EXCLUDED.last_observed_at
         RETURNING id",
    )
    .bind(&observation.stable_id)
    .bind(&observation.vendor)
    .bind(&observation.model)
    .bind(&observation.serial)
    .bind(&observation.wwn)
    .bind(&observation.firmware)
    .bind(observation.generation)
    .bind(&observation.observed_path)
    .bind(health)
    .bind(observation.observed_at)
    .bind(observation.identity_verified)
    .bind(observation.identity_conflict)
    .bind(observation.accessible)
    .bind(observation.immediate_mode == Some(false))
    .bind(observation.in_flight == Some(0))
    .fetch_one(&mut *transaction)
    .await?;

    let record = sqlx::query_as(&format!("{DRIVE_SELECT} WHERE d.id = $1"))
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(record)
}

pub async fn mark_drive_missing(
    pool: &PgPool,
    missing: &MissingSasDrive,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE drives
         SET reconciled = false,
             health = health || jsonb_build_object(
                 'present', false,
                 'accessible', false,
                 'ready', NULL,
                 'medium_loaded', NULL,
                 'status', 'missing',
                 'detail', $1::text,
                 'observed_at', $2::timestamptz),
             last_observed_at = $2
         WHERE observed_path = $3",
    )
    .bind(&missing.reason)
    .bind(missing.observed_at)
    .bind(&missing.observed_path)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn reconcile_drive(
    pool: &PgPool,
    id: Uuid,
    stable_id: &str,
) -> Result<ReconcileDriveResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let updated_id = sqlx::query_scalar::<_, Uuid>(
        "UPDATE drives
         SET reconciled = true, expected_path = observed_path
         WHERE id = $1 AND stable_id = $2
           AND observed_path IS NOT NULL
           AND health->>'present' = 'true'
           AND health->>'accessible' = 'true'
           AND health->>'identity_verified' = 'true'
           AND COALESCE(health->>'identity_conflict', 'false') <> 'true'
           AND COALESCE(health->>'motion_position_uncertain', 'false') <> 'true'
           AND health->>'immediate_mode' = 'false'
           AND health->>'in_flight' = '0'
           AND last_observed_at >= now() - interval '2 minutes'
         RETURNING id",
    )
    .bind(id)
    .bind(stable_id)
    .fetch_optional(&mut *transaction)
    .await?;

    let Some(updated_id) = updated_id else {
        let exists =
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM drives WHERE id = $1)")
                .bind(id)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.rollback().await?;
        return Ok(if exists {
            ReconcileDriveResult::Blocked
        } else {
            ReconcileDriveResult::NotFound
        });
    };

    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         SELECT $1, 'local-api', 'drive.reconcile', 'drive', d.id::text,
                jsonb_build_object('stable_id', d.stable_id,
                                   'observed_path', d.observed_path),
                'reconciled', $2
         FROM drives d WHERE d.id = $3",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(updated_id)
    .execute(&mut *transaction)
    .await?;

    let record = sqlx::query_as(&format!("{DRIVE_SELECT} WHERE d.id = $1"))
        .bind(updated_id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(ReconcileDriveResult::Reconciled(Box::new(record)))
}

const DRIVE_OPERATION_SELECT: &str = "SELECT id, drive_id, expected_stable_id, expected_path,
            expected_vendor, expected_model, expected_serial, expected_wwn, expected_firmware,
            action, scratch_media_confirmed, state, requested_by_label,
            correlation_id, lease_id, fencing_token,
            attempt, result, created_at, started_at, finished_at
     FROM drive_operations";

pub async fn list_drive_operations(
    pool: &PgPool,
) -> Result<Vec<DriveOperationRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{DRIVE_OPERATION_SELECT}
         WHERE state IN ('queued', 'running')
            OR id IN (SELECT id FROM drive_operations ORDER BY created_at DESC LIMIT 100)
         ORDER BY created_at DESC"
    ))
    .fetch_all(pool)
    .await
}

pub async fn running_drive_operation_paths(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT expected_path FROM (
             SELECT expected_path FROM drive_operations WHERE state = 'running'
             UNION ALL
             SELECT expected_path FROM qualification_runs WHERE state = 'running'
         ) active_paths ORDER BY expected_path",
    )
    .fetch_all(pool)
    .await
}

pub async fn queue_drive_operation(
    pool: &PgPool,
    drive_id: Uuid,
    expected_stable_id: &str,
    action: SasMotionOperation,
    scratch_media_confirmed: bool,
) -> Result<QueueDriveOperationResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let drive = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT} WHERE d.id = $1 FOR UPDATE OF d"
    ))
    .bind(drive_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(drive) = drive else {
        transaction.rollback().await?;
        return Ok(QueueDriveOperationResult::NotFound);
    };

    if drive.stable_id != expected_stable_id {
        transaction.rollback().await?;
        return Ok(QueueDriveOperationResult::Blocked(
            "the requested stable identity does not match the current drive".to_owned(),
        ));
    }
    if !scratch_media_confirmed {
        transaction.rollback().await?;
        return Ok(QueueDriveOperationResult::Blocked(
            "lab tape motion requires explicit confirmation that the loaded cartridge is designated scratch media"
                .to_owned(),
        ));
    }
    if let Some(reason) = drive_motion_blocker(&drive) {
        transaction.rollback().await?;
        return Ok(QueueDriveOperationResult::Blocked(reason));
    }

    let busy = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM drive_operations
             WHERE drive_id = $1 AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM qualification_runs
             WHERE drive_id = $1 AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM resource_leases
             WHERE resource_kind = 'drive' AND resource_id = $1::text
               AND released_at IS NULL AND expires_at > now()
         )",
    )
    .bind(drive_id)
    .fetch_one(&mut *transaction)
    .await?;
    if busy {
        transaction.rollback().await?;
        return Ok(QueueDriveOperationResult::Blocked(
            "the drive already has queued work or an active controller lease".to_owned(),
        ));
    }

    let expected_path = drive
        .observed_path
        .as_deref()
        .expect("the motion gate requires an observed path");
    let operation_id = Uuid::now_v7();
    let correlation_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO drive_operations
            (id, drive_id, expected_stable_id, expected_path,
             expected_vendor, expected_model, expected_serial, expected_wwn, expected_firmware,
             action, scratch_media_confirmed, requested_by_label, correlation_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'local-api', $12)",
    )
    .bind(operation_id)
    .bind(drive_id)
    .bind(expected_stable_id)
    .bind(expected_path)
    .bind(&drive.vendor)
    .bind(&drive.model)
    .bind(&drive.serial)
    .bind(&drive.wwn)
    .bind(&drive.firmware)
    .bind(action.as_str())
    .bind(scratch_media_confirmed)
    .bind(correlation_id)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         VALUES ($1, 'local-api', 'drive.motion.request', 'drive_operation', $2,
                 jsonb_build_object('drive_id', $3::uuid,
                                    'stable_id', $4::text,
                                    'path', $5::text,
                                    'action', $6::text,
                                    'scratch_media_confirmed', $7::boolean),
                 'queued', $8)",
    )
    .bind(Uuid::now_v7())
    .bind(operation_id.to_string())
    .bind(drive_id)
    .bind(expected_stable_id)
    .bind(expected_path)
    .bind(action.as_str())
    .bind(scratch_media_confirmed)
    .bind(correlation_id)
    .execute(&mut *transaction)
    .await?;

    let operation = sqlx::query_as(&format!("{DRIVE_OPERATION_SELECT} WHERE id = $1"))
        .bind(operation_id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(QueueDriveOperationResult::Queued(Box::new(operation)))
}

pub async fn block_queued_drive_operations(
    pool: &PgPool,
    reason: &str,
) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let queued = sqlx::query_as::<_, DriveOperationRecord>(&format!(
        "{DRIVE_OPERATION_SELECT} WHERE state = 'queued' FOR UPDATE"
    ))
    .fetch_all(&mut *transaction)
    .await?;
    for operation in &queued {
        sqlx::query(
            "UPDATE drive_operations
             SET state = 'blocked', started_at = now(), finished_at = now(),
                 result = jsonb_build_object('detail', $1::text)
             WHERE id = $2",
        )
        .bind(reason)
        .bind(operation.id)
        .execute(&mut *transaction)
        .await?;
        append_drive_operation_audit(
            &mut transaction,
            operation,
            "drive.motion.block",
            "blocked",
            json!({"detail": reason}),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(queued.len() as u64)
}

pub async fn recover_expired_drive_operations(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let expired = sqlx::query_as::<_, DriveOperationRecord>(
        "SELECT o.id, o.drive_id, o.expected_stable_id, o.expected_path,
                o.expected_vendor, o.expected_model, o.expected_serial,
                o.expected_wwn, o.expected_firmware,
                o.action, o.scratch_media_confirmed, o.state,
                o.requested_by_label, o.correlation_id,
                o.lease_id, o.fencing_token, o.attempt, o.result,
                o.created_at, o.started_at, o.finished_at
         FROM drive_operations o
         LEFT JOIN resource_leases l ON l.id = o.lease_id
         WHERE o.state = 'running'
           AND (l.id IS NULL OR l.released_at IS NOT NULL OR l.expires_at <= now())
         FOR UPDATE OF o",
    )
    .fetch_all(&mut *transaction)
    .await?;
    for operation in &expired {
        let detail = "the worker lease expired during tape motion; physical position requires reconciliation";
        sqlx::query(
            "UPDATE drive_operations
             SET state = 'blocked', finished_at = now(),
                 result = result || jsonb_build_object('detail', $1::text,
                                                       'position_uncertain', true)
             WHERE id = $2",
        )
        .bind(detail)
        .bind(operation.id)
        .execute(&mut *transaction)
        .await?;
        mark_drive_motion_uncertain(&mut transaction, operation.drive_id, detail).await?;
        if let Some(lease_id) = operation.lease_id {
            sqlx::query("SELECT id FROM resource_leases WHERE id = $1 FOR UPDATE")
                .bind(lease_id)
                .fetch_optional(&mut *transaction)
                .await?;
            sqlx::query(
                "UPDATE resource_leases SET released_at = now()
                 WHERE id = $1 AND released_at IS NULL",
            )
            .bind(lease_id)
            .execute(&mut *transaction)
            .await?;
        }
        append_drive_operation_audit(
            &mut transaction,
            operation,
            "drive.motion.lease_expired",
            "blocked",
            json!({"detail": detail, "position_uncertain": true}),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(expired.len() as u64)
}

pub async fn claim_next_drive_operation(
    pool: &PgPool,
    worker_id: Uuid,
) -> Result<ClaimDriveOperationResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let operation = sqlx::query_as::<_, DriveOperationRecord>(&format!(
        "{DRIVE_OPERATION_SELECT}
         WHERE state = 'queued'
           AND NOT EXISTS (
               SELECT 1 FROM resource_leases l
               WHERE l.resource_kind = 'drive'
                 AND l.resource_id = drive_operations.drive_id::text
                 AND l.released_at IS NULL AND l.expires_at > now())
           AND NOT EXISTS (
               SELECT 1 FROM qualification_runs q
               WHERE q.drive_id = drive_operations.drive_id
                 AND q.state IN ('queued', 'running'))
         ORDER BY created_at
         FOR UPDATE SKIP LOCKED LIMIT 1"
    ))
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(operation) = operation else {
        transaction.rollback().await?;
        return Ok(ClaimDriveOperationResult::Empty);
    };

    let drive = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT} WHERE d.id = $1 FOR UPDATE OF d"
    ))
    .bind(operation.drive_id)
    .fetch_one(&mut *transaction)
    .await?;
    let blocker = if !operation.scratch_media_confirmed {
        Some(
            "the persisted request does not confirm designated scratch media; execution is denied"
                .to_owned(),
        )
    } else if drive.stable_id != operation.expected_stable_id {
        Some("the observed stable identity changed after this request was queued".to_owned())
    } else if drive.observed_path.as_deref() != Some(operation.expected_path.as_str()) {
        Some("the observed device path changed after this request was queued".to_owned())
    } else if drive.vendor != operation.expected_vendor
        || drive.model != operation.expected_model
        || drive.serial != operation.expected_serial
        || drive.wwn != operation.expected_wwn
        || drive.firmware != operation.expected_firmware
    {
        Some(
            "drive model, serial, WWN, or firmware changed after this request was queued"
                .to_owned(),
        )
    } else {
        drive_motion_blocker(&drive)
    };
    if let Some(reason) = blocker {
        sqlx::query(
            "UPDATE drive_operations
             SET state = 'blocked', started_at = now(), finished_at = now(),
                 result = jsonb_build_object('detail', $1::text)
             WHERE id = $2",
        )
        .bind(&reason)
        .bind(operation.id)
        .execute(&mut *transaction)
        .await?;
        append_drive_operation_audit(
            &mut transaction,
            &operation,
            "drive.motion.block",
            "blocked",
            json!({"detail": reason}),
        )
        .await?;
        transaction.commit().await?;
        return Ok(ClaimDriveOperationResult::Blocked {
            id: operation.id,
            reason,
        });
    }

    sqlx::query(
        "UPDATE resource_leases SET released_at = now()
         WHERE resource_kind = 'drive' AND resource_id = $1::text
           AND released_at IS NULL AND expires_at <= now()",
    )
    .bind(operation.drive_id)
    .execute(&mut *transaction)
    .await?;
    let lease_id = Uuid::now_v7();
    let fencing_token = sqlx::query_scalar::<_, i64>(
        "INSERT INTO resource_leases
            (id, resource_kind, resource_id, owner_id, fencing_token, expires_at)
         VALUES ($1, 'drive', $2::text, $3, nextval('resource_fencing_token_seq'),
                 now() + interval '16 minutes')
         ON CONFLICT DO NOTHING
         RETURNING fencing_token",
    )
    .bind(lease_id)
    .bind(operation.drive_id)
    .bind(worker_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(fencing_token) = fencing_token else {
        transaction.rollback().await?;
        return Ok(ClaimDriveOperationResult::Empty);
    };

    sqlx::query(
        "UPDATE drive_operations
         SET state = 'running', lease_id = $1, fencing_token = $2,
             attempt = attempt + 1, started_at = now()
         WHERE id = $3 AND state = 'queued'",
    )
    .bind(lease_id)
    .bind(fencing_token)
    .bind(operation.id)
    .execute(&mut *transaction)
    .await?;
    append_drive_operation_audit(
        &mut transaction,
        &operation,
        "drive.motion.start",
        "running",
        json!({"lease_id": lease_id, "fencing_token": fencing_token}),
    )
    .await?;
    let operation = sqlx::query_as(&format!("{DRIVE_OPERATION_SELECT} WHERE id = $1"))
        .bind(operation.id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(ClaimDriveOperationResult::Claimed(Box::new(
        ClaimedDriveOperation {
            operation,
            lease_id,
            fencing_token,
            worker_id,
        },
    )))
}

pub async fn finish_drive_operation(
    pool: &PgPool,
    claim: &ClaimedDriveOperation,
    state: DriveOperationTerminalState,
    result: Value,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE drive_operations o
         SET state = $1, result = $2, finished_at = now()
         FROM resource_leases l
         WHERE o.id = $3 AND o.state = 'running'
           AND o.lease_id = $4 AND o.fencing_token = $5
           AND l.id = o.lease_id AND l.owner_id = $6
           AND l.fencing_token = o.fencing_token
           AND l.released_at IS NULL AND l.expires_at > now()",
    )
    .bind(state.as_str())
    .bind(&result)
    .bind(claim.operation.id)
    .bind(claim.lease_id)
    .bind(claim.fencing_token)
    .bind(claim.worker_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }

    let released = sqlx::query(
        "UPDATE resource_leases SET released_at = now()
         WHERE id = $1 AND owner_id = $2 AND fencing_token = $3 AND released_at IS NULL",
    )
    .bind(claim.lease_id)
    .bind(claim.worker_id)
    .bind(claim.fencing_token)
    .execute(&mut *transaction)
    .await?;
    if released.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    let position_uncertain = result
        .get("position_uncertain")
        .and_then(Value::as_bool)
        .unwrap_or(state != DriveOperationTerminalState::Succeeded);
    if position_uncertain {
        let detail = result
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("tape motion did not complete with a verified postcondition");
        mark_drive_motion_uncertain(&mut transaction, claim.operation.drive_id, detail).await?;
    }
    append_drive_operation_audit(
        &mut transaction,
        &claim.operation,
        "drive.motion.finish",
        state.as_str(),
        result,
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

pub async fn renew_drive_operation_lease(
    pool: &PgPool,
    claim: &ClaimedDriveOperation,
) -> Result<bool, sqlx::Error> {
    let renewed = sqlx::query(
        "UPDATE resource_leases l
         SET expires_at = now() + interval '16 minutes'
         FROM drive_operations o
         WHERE l.id = $1 AND l.owner_id = $2 AND l.fencing_token = $3
           AND l.released_at IS NULL AND l.expires_at > now()
           AND o.id = $4 AND o.state = 'running'
           AND o.lease_id = l.id AND o.fencing_token = l.fencing_token",
    )
    .bind(claim.lease_id)
    .bind(claim.worker_id)
    .bind(claim.fencing_token)
    .bind(claim.operation.id)
    .execute(pool)
    .await?;
    Ok(renewed.rows_affected() == 1)
}

fn drive_motion_blocker(drive: &DriveRecord) -> Option<String> {
    let health = &drive.health;
    if !drive.reconciled {
        return Some("the drive identity has not been reconciled".to_owned());
    }
    if drive.observed_path.is_none() || drive.observed_path != drive.expected_path {
        return Some("the current device path does not match the trusted path".to_owned());
    }
    if drive
        .last_observed_at
        .is_none_or(|time| time < Utc::now() - chrono::TimeDelta::minutes(2))
    {
        return Some("the latest drive observation is stale".to_owned());
    }
    for (key, detail) in [
        ("present", "the configured tape device is not present"),
        ("accessible", "the worker cannot access the tape device"),
        (
            "identity_verified",
            "the drive does not have a verified stable SCSI identity",
        ),
        ("ready", "the loaded tape drive is not ready"),
        ("medium_loaded", "no ready cartridge is loaded"),
    ] {
        if health.get(key).and_then(Value::as_bool) != Some(true) {
            return Some(detail.to_owned());
        }
    }
    if health.get("identity_conflict").and_then(Value::as_bool) == Some(true) {
        return Some("the stable identity is present at more than one configured path".to_owned());
    }
    if health
        .get("motion_position_uncertain")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Some("a prior tape-motion result still requires reconciliation".to_owned());
    }
    if health.get("immediate_mode").and_then(Value::as_bool) != Some(false) {
        return Some("kernel immediate tape motion is enabled or unknown".to_owned());
    }
    if health.get("in_flight").and_then(Value::as_u64) != Some(0) {
        return Some("the kernel reports outstanding or unknown tape I/O".to_owned());
    }
    None
}

async fn mark_drive_motion_uncertain(
    transaction: &mut Transaction<'_, Postgres>,
    drive_id: Uuid,
    detail: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE drives
         SET reconciled = false,
             last_observed_at = NULL,
             health = health || jsonb_build_object(
                 'ready', NULL,
                 'medium_loaded', NULL,
                 'at_bot', NULL,
                 'position_lost', NULL,
                 'status', 'motion_reconciliation_required',
                 'status_detail', $1::text,
                 'motion_position_uncertain', true,
                 'observed_at', now())
         WHERE id = $2",
    )
    .bind(detail)
    .bind(drive_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn append_drive_operation_audit(
    transaction: &mut Transaction<'_, Postgres>,
    operation: &DriveOperationRecord,
    action: &str,
    result_label: &str,
    evidence: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         VALUES ($1, 'tapescale-worker', $2, 'drive_operation', $3,
                 jsonb_build_object('drive_id', $4::uuid,
                                    'stable_id', $5::text,
                                    'path', $6::text,
                                    'action', $7::text,
                                    'scratch_media_confirmed', $8::boolean,
                                    'evidence', $9::jsonb),
                 $10, $11)",
    )
    .bind(Uuid::now_v7())
    .bind(action)
    .bind(operation.id.to_string())
    .bind(operation.drive_id)
    .bind(&operation.expected_stable_id)
    .bind(&operation.expected_path)
    .bind(&operation.action)
    .bind(operation.scratch_media_confirmed)
    .bind(evidence)
    .bind(result_label)
    .bind(operation.correlation_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

const QUALIFICATION_RUN_SELECT: &str = "SELECT id, drive_id, cartridge_id,
            expected_stable_id, expected_path, expected_vendor, expected_model,
            expected_serial, expected_wwn, expected_firmware,
            expected_cartridge_label, expected_media_generation, expected_media_format,
            engine_name, engine_version, test_kind,
            scratch_media_confirmed, destructive_overwrite_confirmed,
            state, requested_by_label, correlation_id, lease_id, fencing_token,
            attempt, result, created_at, started_at, finished_at
     FROM qualification_runs";

const QUALIFICATION_CARTRIDGE_SELECT: &str = "SELECT c.id, c.internal_label,
            c.media_generation, c.media_format, c.identity_status, c.worm, c.reserved_by,
            (SELECT count(*) FROM holds h
             WHERE h.cartridge_id = c.id AND h.released_at IS NULL) AS active_holds,
            EXISTS (SELECT 1 FROM volume_records vr WHERE vr.cartridge_id = c.id) AS has_content
     FROM cartridges c";

pub async fn list_qualification_runs(
    pool: &PgPool,
) -> Result<Vec<QualificationRunRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{QUALIFICATION_RUN_SELECT}
         WHERE state IN ('queued', 'running')
            OR id IN (SELECT id FROM qualification_runs ORDER BY created_at DESC LIMIT 100)
         ORDER BY created_at DESC"
    ))
    .fetch_all(pool)
    .await
}

pub async fn get_qualification_run(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<QualificationRunRecord>, sqlx::Error> {
    sqlx::query_as(&format!("{QUALIFICATION_RUN_SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn qualification_events(
    pool: &PgPool,
    id: Uuid,
) -> Result<Vec<QualificationEventRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, event_type, evidence, correlation_id, occurred_at
         FROM qualification_events
         WHERE qualification_run_id = $1 ORDER BY occurred_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

pub async fn running_qualification_paths(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT expected_path FROM qualification_runs
         WHERE state = 'running' ORDER BY expected_path",
    )
    .fetch_all(pool)
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn queue_qualification_run(
    pool: &PgPool,
    drive_id: Uuid,
    expected_stable_id: &str,
    cartridge_id: Uuid,
    typed_cartridge_label: &str,
    scratch_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
) -> Result<QueueQualificationRunResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let drive = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT} WHERE d.id = $1 FOR UPDATE OF d"
    ))
    .bind(drive_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(drive) = drive else {
        transaction.rollback().await?;
        return Ok(QueueQualificationRunResult::NotFound);
    };
    let cartridge = sqlx::query_as::<_, QualificationCartridge>(&format!(
        "{QUALIFICATION_CARTRIDGE_SELECT} WHERE c.id = $1 FOR UPDATE OF c"
    ))
    .bind(cartridge_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(cartridge) = cartridge else {
        transaction.rollback().await?;
        return Ok(QueueQualificationRunResult::NotFound);
    };

    let blocker = if drive.stable_id != expected_stable_id {
        Some("the requested stable identity does not match the current drive".to_owned())
    } else if !scratch_media_confirmed || !destructive_overwrite_confirmed {
        Some(
            "qualification requires explicit scratch-media and destructive-overwrite confirmations"
                .to_owned(),
        )
    } else if cartridge.internal_label != typed_cartridge_label {
        Some("the typed cartridge label does not exactly match the selected cartridge".to_owned())
    } else if let Some(reason) = drive_motion_blocker(&drive) {
        Some(reason)
    } else {
        qualification_cartridge_blocker(&cartridge, None)
    };
    if let Some(reason) = blocker {
        transaction.rollback().await?;
        return Ok(QueueQualificationRunResult::Blocked(reason));
    }

    let busy = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM drive_operations
             WHERE drive_id = $1 AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM qualification_runs
             WHERE (drive_id = $1 OR cartridge_id = $2) AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM resource_leases
             WHERE resource_kind = 'drive' AND resource_id = $1::text
               AND released_at IS NULL AND expires_at > now()
         )",
    )
    .bind(drive_id)
    .bind(cartridge_id)
    .fetch_one(&mut *transaction)
    .await?;
    if busy {
        transaction.rollback().await?;
        return Ok(QueueQualificationRunResult::Blocked(
            "the drive or cartridge already has queued work or an active controller lease"
                .to_owned(),
        ));
    }

    let expected_path = drive
        .observed_path
        .as_deref()
        .expect("the qualification gate requires an observed path");
    let id = Uuid::now_v7();
    let correlation_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO qualification_runs
            (id, drive_id, cartridge_id, expected_stable_id, expected_path,
             expected_vendor, expected_model, expected_serial, expected_wwn, expected_firmware,
             expected_cartridge_label, expected_media_generation, expected_media_format,
             scratch_media_confirmed, destructive_overwrite_confirmed,
             requested_by_label, correlation_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 $11, $12, $13, $14, $15, 'local-api', $16)",
    )
    .bind(id)
    .bind(drive_id)
    .bind(cartridge_id)
    .bind(expected_stable_id)
    .bind(expected_path)
    .bind(&drive.vendor)
    .bind(&drive.model)
    .bind(&drive.serial)
    .bind(&drive.wwn)
    .bind(&drive.firmware)
    .bind(&cartridge.internal_label)
    .bind(cartridge.media_generation)
    .bind(&cartridge.media_format)
    .bind(scratch_media_confirmed)
    .bind(destructive_overwrite_confirmed)
    .bind(correlation_id)
    .execute(&mut *transaction)
    .await?;
    let reservation = format!("qualification:{id}");
    sqlx::query("UPDATE cartridges SET reserved_by = $1, updated_at = now() WHERE id = $2")
        .bind(&reservation)
        .bind(cartridge_id)
        .execute(&mut *transaction)
        .await?;
    append_qualification_event(
        &mut transaction,
        id,
        correlation_id,
        "queued",
        json!({
            "drive_id": drive_id,
            "stable_id": expected_stable_id,
            "path": expected_path,
            "cartridge_id": cartridge_id,
            "cartridge_label": cartridge.internal_label,
            "engine": "Bacula Community",
            "engine_version": "15.0.3",
            "test_kind": "btape_test",
            "destructive": true
        }),
        "local-api",
    )
    .await?;
    let run = sqlx::query_as(&format!("{QUALIFICATION_RUN_SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(QueueQualificationRunResult::Queued(Box::new(run)))
}

pub async fn recover_expired_qualification_runs(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let expired = sqlx::query_as::<_, QualificationRunRecord>(&format!(
        "{QUALIFICATION_RUN_SELECT}
         WHERE id IN (
             SELECT q.id FROM qualification_runs q
             LEFT JOIN resource_leases l ON l.id = q.lease_id
             WHERE q.state = 'running'
               AND (l.id IS NULL OR l.released_at IS NOT NULL OR l.expires_at <= now())
         ) FOR UPDATE"
    ))
    .fetch_all(&mut *transaction)
    .await?;
    let mut recovered = 0_u64;
    for run in &expired {
        let lease_expired = match run.lease_id {
            Some(lease_id) => sqlx::query_scalar::<_, bool>(
                "SELECT released_at IS NOT NULL OR expires_at <= now()
                 FROM resource_leases WHERE id = $1 FOR UPDATE",
            )
            .bind(lease_id)
            .fetch_optional(&mut *transaction)
            .await?
            .unwrap_or(true),
            None => true,
        };
        if !lease_expired {
            continue;
        }
        let detail =
            "the worker lease expired during destructive tape qualification; tape position and contents require reconciliation";
        sqlx::query(
            "UPDATE qualification_runs
             SET state = 'interrupted', finished_at = now(),
                 result = result || jsonb_build_object(
                     'detail', $1::text, 'position_uncertain', true)
             WHERE id = $2",
        )
        .bind(detail)
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        release_qualification_resources(&mut transaction, run).await?;
        mark_drive_motion_uncertain(&mut transaction, run.drive_id, detail).await?;
        append_qualification_event(
            &mut transaction,
            run.id,
            run.correlation_id,
            "interrupted",
            json!({"detail": detail, "position_uncertain": true}),
            "tapescale-worker",
        )
        .await?;
        recovered += 1;
    }
    transaction.commit().await?;
    Ok(recovered)
}

pub async fn block_queued_qualification_runs(
    pool: &PgPool,
    reason: &str,
) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let queued = sqlx::query_as::<_, QualificationRunRecord>(&format!(
        "{QUALIFICATION_RUN_SELECT} WHERE state = 'queued' FOR UPDATE"
    ))
    .fetch_all(&mut *transaction)
    .await?;
    for run in &queued {
        sqlx::query(
            "UPDATE qualification_runs
             SET state = 'blocked', started_at = now(), finished_at = now(),
                 result = jsonb_build_object('detail', $1::text)
             WHERE id = $2",
        )
        .bind(reason)
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        clear_qualification_reservation(&mut transaction, run).await?;
        append_qualification_event(
            &mut transaction,
            run.id,
            run.correlation_id,
            "blocked",
            json!({"detail": reason}),
            "tapescale-worker",
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(queued.len() as u64)
}

pub async fn claim_next_qualification_run(
    pool: &PgPool,
    worker_id: Uuid,
) -> Result<ClaimQualificationRunResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let run = sqlx::query_as::<_, QualificationRunRecord>(&format!(
        "{QUALIFICATION_RUN_SELECT}
         WHERE state = 'queued'
           AND NOT EXISTS (
               SELECT 1 FROM resource_leases l
               WHERE l.resource_kind = 'drive'
                 AND l.resource_id = qualification_runs.drive_id::text
                 AND l.released_at IS NULL AND l.expires_at > now())
         ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1"
    ))
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(run) = run else {
        transaction.rollback().await?;
        return Ok(ClaimQualificationRunResult::Empty);
    };
    let drive = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT} WHERE d.id = $1 FOR UPDATE OF d"
    ))
    .bind(run.drive_id)
    .fetch_one(&mut *transaction)
    .await?;
    let cartridge = sqlx::query_as::<_, QualificationCartridge>(&format!(
        "{QUALIFICATION_CARTRIDGE_SELECT} WHERE c.id = $1 FOR UPDATE OF c"
    ))
    .bind(run.cartridge_id)
    .fetch_one(&mut *transaction)
    .await?;
    let expected_reservation = format!("qualification:{}", run.id);
    let motion_waiting = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM drive_operations
                       WHERE drive_id = $1 AND state IN ('queued', 'running'))",
    )
    .bind(run.drive_id)
    .fetch_one(&mut *transaction)
    .await?;
    let blocker = if motion_waiting {
        Some("a tape-motion request is already queued for this drive".to_owned())
    } else {
        qualification_run_blocker(&run, &drive, &cartridge, &expected_reservation)
    };
    if let Some(reason) = blocker {
        sqlx::query(
            "UPDATE qualification_runs SET state = 'blocked', started_at = now(),
                    finished_at = now(), result = jsonb_build_object('detail', $1::text)
             WHERE id = $2",
        )
        .bind(&reason)
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        clear_qualification_reservation(&mut transaction, &run).await?;
        append_qualification_event(
            &mut transaction,
            run.id,
            run.correlation_id,
            "blocked",
            json!({"detail": reason}),
            "tapescale-worker",
        )
        .await?;
        transaction.commit().await?;
        return Ok(ClaimQualificationRunResult::Blocked { id: run.id, reason });
    }

    sqlx::query(
        "UPDATE resource_leases SET released_at = now()
         WHERE resource_kind = 'drive' AND resource_id = $1::text
           AND released_at IS NULL AND expires_at <= now()",
    )
    .bind(run.drive_id)
    .execute(&mut *transaction)
    .await?;
    let lease_id = Uuid::now_v7();
    let fencing_token = sqlx::query_scalar::<_, i64>(
        "INSERT INTO resource_leases
            (id, resource_kind, resource_id, owner_id, fencing_token, expires_at)
         VALUES ($1, 'drive', $2::text, $3, nextval('resource_fencing_token_seq'),
                 now() + interval '30 minutes')
         ON CONFLICT DO NOTHING RETURNING fencing_token",
    )
    .bind(lease_id)
    .bind(run.drive_id)
    .bind(worker_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(fencing_token) = fencing_token else {
        transaction.rollback().await?;
        return Ok(ClaimQualificationRunResult::Empty);
    };
    sqlx::query(
        "UPDATE qualification_runs
         SET state = 'running', lease_id = $1, fencing_token = $2,
             attempt = attempt + 1, started_at = now()
         WHERE id = $3 AND state = 'queued'",
    )
    .bind(lease_id)
    .bind(fencing_token)
    .bind(run.id)
    .execute(&mut *transaction)
    .await?;
    append_qualification_event(
        &mut transaction,
        run.id,
        run.correlation_id,
        "started",
        json!({"lease_id": lease_id, "fencing_token": fencing_token}),
        "tapescale-worker",
    )
    .await?;
    let run = sqlx::query_as(&format!("{QUALIFICATION_RUN_SELECT} WHERE id = $1"))
        .bind(run.id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(ClaimQualificationRunResult::Claimed(Box::new(
        ClaimedQualificationRun {
            run,
            lease_id,
            fencing_token,
            worker_id,
        },
    )))
}

pub async fn renew_qualification_run_lease(
    pool: &PgPool,
    claim: &ClaimedQualificationRun,
) -> Result<bool, sqlx::Error> {
    let renewed = sqlx::query(
        "UPDATE resource_leases l SET expires_at = now() + interval '30 minutes'
         FROM qualification_runs q
         WHERE l.id = $1 AND l.owner_id = $2 AND l.fencing_token = $3
           AND l.released_at IS NULL AND l.expires_at > now()
           AND q.id = $4 AND q.state = 'running'
           AND q.lease_id = l.id AND q.fencing_token = l.fencing_token",
    )
    .bind(claim.lease_id)
    .bind(claim.worker_id)
    .bind(claim.fencing_token)
    .bind(claim.run.id)
    .execute(pool)
    .await?;
    Ok(renewed.rows_affected() == 1)
}

pub async fn finish_qualification_run(
    pool: &PgPool,
    claim: &ClaimedQualificationRun,
    state: QualificationTerminalState,
    result: Value,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE qualification_runs q
         SET state = $1, result = $2, finished_at = now()
         FROM resource_leases l
         WHERE q.id = $3 AND q.state = 'running'
           AND q.lease_id = $4 AND q.fencing_token = $5
           AND l.id = q.lease_id AND l.owner_id = $6
           AND l.fencing_token = q.fencing_token
           AND l.released_at IS NULL AND l.expires_at > now()",
    )
    .bind(state.as_str())
    .bind(&result)
    .bind(claim.run.id)
    .bind(claim.lease_id)
    .bind(claim.fencing_token)
    .bind(claim.worker_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    release_qualification_resources(&mut transaction, &claim.run).await?;
    let position_uncertain = result
        .get("position_uncertain")
        .and_then(Value::as_bool)
        .unwrap_or(state != QualificationTerminalState::Passed);
    if position_uncertain {
        let detail = result
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("destructive tape qualification did not leave a verified tape position");
        mark_drive_motion_uncertain(&mut transaction, claim.run.drive_id, detail).await?;
    }
    append_qualification_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        state.as_str(),
        result,
        "tapescale-worker",
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

fn qualification_run_blocker(
    run: &QualificationRunRecord,
    drive: &DriveRecord,
    cartridge: &QualificationCartridge,
    expected_reservation: &str,
) -> Option<String> {
    if run.created_at < Utc::now() - chrono::TimeDelta::minutes(5) {
        return Some(
            "the attended destructive confirmation expired before worker dispatch; inspect the loaded cartridge and queue a new run"
                .to_owned(),
        );
    }
    if !run.scratch_media_confirmed || !run.destructive_overwrite_confirmed {
        return Some("persisted destructive scratch-media confirmations are missing".to_owned());
    }
    if drive.stable_id != run.expected_stable_id
        || drive.observed_path.as_deref() != Some(run.expected_path.as_str())
        || drive.vendor != run.expected_vendor
        || drive.model != run.expected_model
        || drive.serial != run.expected_serial
        || drive.wwn != run.expected_wwn
        || drive.firmware != run.expected_firmware
    {
        return Some("the drive identity, path, model, serial, WWN, or firmware changed after the request was queued".to_owned());
    }
    if let Some(reason) = drive_motion_blocker(drive) {
        return Some(reason);
    }
    if cartridge.id != run.cartridge_id
        || cartridge.internal_label != run.expected_cartridge_label
        || cartridge.media_generation != run.expected_media_generation
        || cartridge.media_format != run.expected_media_format
    {
        return Some(
            "the registered cartridge metadata changed after the request was queued".to_owned(),
        );
    }
    qualification_cartridge_blocker(cartridge, Some(expected_reservation))
}

fn qualification_cartridge_blocker(
    cartridge: &QualificationCartridge,
    expected_reservation: Option<&str>,
) -> Option<String> {
    if cartridge.worm {
        return Some("WORM media cannot be used for a destructive qualification test".to_owned());
    }
    if cartridge.active_holds > 0 || cartridge.has_content {
        return Some("the cartridge has protected content or an active hold".to_owned());
    }
    if matches!(cartridge.identity_status.as_str(), "foreign" | "mismatch") {
        return Some(
            "foreign or identity-mismatched media cannot be selected as scratch".to_owned(),
        );
    }
    match (expected_reservation, cartridge.reserved_by.as_deref()) {
        (None, Some(_)) => Some("the cartridge is already reserved".to_owned()),
        (Some(expected), Some(actual)) if expected == actual => None,
        (Some(_), _) => Some("the cartridge reservation changed after queuing".to_owned()),
        (None, None) => None,
    }
}

async fn clear_qualification_reservation(
    transaction: &mut Transaction<'_, Postgres>,
    run: &QualificationRunRecord,
) -> Result<(), sqlx::Error> {
    let reservation = format!("qualification:{}", run.id);
    sqlx::query(
        "UPDATE cartridges SET reserved_by = NULL, updated_at = now()
         WHERE id = $1 AND reserved_by = $2",
    )
    .bind(run.cartridge_id)
    .bind(reservation)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn release_qualification_resources(
    transaction: &mut Transaction<'_, Postgres>,
    run: &QualificationRunRecord,
) -> Result<(), sqlx::Error> {
    if let Some(lease_id) = run.lease_id {
        sqlx::query(
            "UPDATE resource_leases SET released_at = now()
             WHERE id = $1 AND released_at IS NULL",
        )
        .bind(lease_id)
        .execute(&mut **transaction)
        .await?;
    }
    clear_qualification_reservation(transaction, run).await
}

async fn append_qualification_event(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    correlation_id: Uuid,
    event_type: &str,
    evidence: Value,
    actor_label: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO qualification_events
            (qualification_run_id, event_type, evidence, correlation_id)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(run_id)
    .bind(event_type)
    .bind(&evidence)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         VALUES ($1, $2, $3, 'qualification_run', $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(actor_label)
    .bind(format!("qualification.{event_type}"))
    .bind(run_id.to_string())
    .bind(evidence)
    .bind(event_type)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub async fn list_slots(pool: &PgPool) -> Result<Vec<SlotRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.id, s.element_address, s.kind, s.observed_barcode,
                s.observed_at, c.stable_id AS changer_stable_id
         FROM slots s JOIN changers c ON c.id = s.changer_id
         ORDER BY c.stable_id, s.element_address",
    )
    .fetch_all(pool)
    .await
}

pub async fn list_movements(pool: &PgPool) -> Result<Vec<MovementRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT mt.id, mt.external_id, mt.kind, mt.required_media_ids,
                mt.state, mt.readiness, mt.due_at, mt.created_at,
                source.name AS source_name, destination.name AS destination_name
         FROM movement_tasks mt
         LEFT JOIN locations source ON source.id = mt.source_location_id
         JOIN locations destination ON destination.id = mt.destination_location_id
         ORDER BY mt.due_at",
    )
    .fetch_all(pool)
    .await
}

pub async fn list_verification(pool: &PgPool) -> Result<Vec<VerificationRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT vr.id, vr.kind, vr.scope, vr.outcome, vr.evidence,
                vr.correlation_id, vr.started_at, vr.finished_at,
                rp.id AS recovery_point_id, rp.recovery_time, s.dataset_name,
                d.stable_id AS drive_stable_id,
                COALESCE(array_agg(c.internal_label) FILTER (WHERE c.id IS NOT NULL), '{}'::text[]) AS media_labels
         FROM verification_records vr
         JOIN recovery_points rp ON rp.id = vr.recovery_point_id
         JOIN sources s ON s.id = rp.source_id
         LEFT JOIN drives d ON d.id = vr.drive_id
         LEFT JOIN LATERAL unnest(vr.media_ids) AS media(media_id) ON true
         LEFT JOIN cartridges c ON c.id = media.media_id
         GROUP BY vr.id, rp.id, rp.recovery_time, s.dataset_name, d.stable_id
         ORDER BY vr.started_at DESC",
    )
    .fetch_all(pool)
    .await
}

pub async fn list_recovery_points(pool: &PgPool) -> Result<Vec<RecoveryPointRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT rp.id, rp.representation, rp.level, rp.status,
                rp.recovery_time, rp.expires_at, rp.indefinite,
                rp.coverage, rp.manifest_digest, s.dataset_name,
                COALESCE(array_agg(c.internal_label ORDER BY vr.volume_order)
                         FILTER (WHERE c.id IS NOT NULL), '{}'::text[]) AS media_labels
         FROM recovery_points rp
         JOIN sources s ON s.id = rp.source_id
         LEFT JOIN volume_records vr ON vr.recovery_point_id = rp.id
         LEFT JOIN cartridges c ON c.id = vr.cartridge_id
         GROUP BY rp.id, s.dataset_name
         ORDER BY rp.recovery_time DESC",
    )
    .fetch_all(pool)
    .await
}

pub async fn inventory_counts(pool: &PgPool) -> Result<InventoryCounts, sqlx::Error> {
    sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM installations) AS installations,
            (SELECT count(*) FROM truenas_instances) AS truenas_instances,
            (SELECT count(*) FROM drives) AS drives,
            (SELECT count(*) FROM drives
             WHERE health->>'present' = 'true'
               AND health->>'accessible' = 'true'
               AND last_observed_at >= now() - interval '2 minutes') AS accessible_drives,
            (SELECT count(*) FROM drives
             WHERE reconciled
               AND health->>'present' = 'true'
               AND health->>'accessible' = 'true'
               AND health->>'identity_verified' = 'true'
               AND COALESCE(health->>'identity_conflict', 'false') <> 'true'
               AND last_observed_at >= now() - interval '2 minutes') AS reconciled_drives,
            (SELECT count(*) FROM drives
             WHERE qualification_status = 'physically_qualified') AS qualified_drives,
            (SELECT count(*) FROM cartridges) AS cartridges",
    )
    .fetch_one(pool)
    .await
}
