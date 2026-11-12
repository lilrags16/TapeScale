use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, FromRow, PgPool};
use uuid::Uuid;

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
    pub generation: i16,
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
    pub qualified_drives: i64,
    pub cartridges: i64,
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

pub async fn list_drives(pool: &PgPool) -> Result<Vec<DriveRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT d.id, d.stable_id, d.vendor, d.model, d.serial, d.wwn,
                d.firmware, d.generation, d.observed_path, d.expected_path,
                d.reconciled, d.qualification_status, d.health,
                d.last_observed_at, c.stable_id AS changer_stable_id
         FROM drives d
         LEFT JOIN changers c ON c.id = d.changer_id
         ORDER BY d.stable_id",
    )
    .fetch_all(pool)
    .await
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
             WHERE qualification_status = 'physically_qualified') AS qualified_drives,
            (SELECT count(*) FROM cartridges) AS cartridges",
    )
    .fetch_one(pool)
    .await
}
