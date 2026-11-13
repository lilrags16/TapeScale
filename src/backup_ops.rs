//! Durable attended full-file backup policies and pre-assignment requests.
//!
//! The physical executor remains the fenced, pinned single-tape data path.  This
//! module adds the missing operational boundary in front of it: a policy-backed
//! request and `job_runs` identity are committed in `waiting_for_media` before
//! an operator selects destructive hardware.  Nothing in this module promotes
//! the laboratory adapter, unencrypted format, or a hardware tuple to
//! production qualification.

use chrono::{DateTime, Days, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::domain::schedule::{default_occurrence, RunKind};

pub const DEFAULT_FULL_SCHEDULE: &str = "0 2 * * 6";
pub const DEFAULT_RETENTION_DAYS: i32 = 28;
const MAX_LABEL_BYTES: usize = 256;
const MAX_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum BackupOpsError {
    #[error("invalid backup request: {0}")]
    Invalid(String),
    #[error("backup resource was not found")]
    NotFound,
    #[error("backup operation conflicts with durable state: {0}")]
    Conflict(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct BackupProfileRecord {
    pub id: Uuid,
    pub policy_id: Uuid,
    pub job_id: Uuid,
    pub source_id: Uuid,
    pub source_snapshot_id: Uuid,
    pub name: String,
    pub source_subpath: String,
    pub source_mount_path: String,
    pub logical_root: String,
    pub media_pool_id: Option<Uuid>,
    pub media_pool_name: Option<String>,
    pub schedule_enabled: bool,
    pub schedule_timezone: String,
    pub full_schedule: String,
    pub incremental_schedule: String,
    pub retention_days: i32,
    pub representation: String,
    pub encryption_enabled: bool,
    pub compression_mode: String,
    pub verification_mode: String,
    pub engine_name: String,
    pub engine_version: String,
    pub adapter_version: String,
    pub format_version: String,
    pub enabled: bool,
    pub production_eligible: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const BACKUP_PROFILE_SELECT: &str = r#"
    SELECT data_profile.id,
           policy.id AS policy_id,
           data_profile.job_id,
           data_profile.source_id,
           data_profile.source_snapshot_id,
           data_profile.name,
           data_profile.source_subpath,
           data_profile.source_mount_path,
           data_profile.logical_root,
           data_profile.media_pool_id,
           media_pool.name AS media_pool_name,
           data_profile.schedule_enabled,
           policy.schedule_timezone,
           policy.full_schedule,
           policy.incremental_schedule,
           data_profile.retention_days,
           policy.representation,
           policy.encryption_enabled,
           policy.compression_mode,
           policy.verification_mode,
           data_profile.engine_name,
           data_profile.engine_version,
           data_profile.adapter_version,
           data_profile.format_version,
           data_profile.enabled,
           data_profile.production_eligible,
           data_profile.created_at,
           data_profile.updated_at
      FROM single_tape_data_profiles data_profile
      JOIN protection_policies policy ON policy.id = data_profile.policy_id
      LEFT JOIN media_pools media_pool ON media_pool.id = data_profile.media_pool_id
     WHERE data_profile.workflow_kind = 'backup'
"#;

#[derive(Clone, Debug, Deserialize)]
pub struct NewBackupProfile {
    pub source_profile_id: Uuid,
    #[serde(default)]
    pub media_pool_id: Option<Uuid>,
    #[serde(default = "default_timezone")]
    pub schedule_timezone: String,
    #[serde(default = "default_full_schedule")]
    pub full_schedule: String,
    #[serde(default)]
    pub schedule_enabled: bool,
    #[serde(default = "default_retention_days")]
    pub retention_days: i32,
    #[serde(default = "default_file_representation")]
    pub representation: String,
    #[serde(default = "default_full_level")]
    pub requested_level: String,
    #[serde(default = "default_full_readback")]
    pub verification_mode: String,
    #[serde(default)]
    pub encryption_enabled: bool,
    #[serde(default)]
    pub spanning_enabled: bool,
    #[serde(default = "default_actor")]
    pub actor_label: String,
}

impl NewBackupProfile {
    #[must_use]
    pub fn attended_full_file(source_profile_id: Uuid) -> Self {
        Self {
            source_profile_id,
            media_pool_id: None,
            schedule_timezone: default_timezone(),
            full_schedule: default_full_schedule(),
            schedule_enabled: false,
            retention_days: DEFAULT_RETENTION_DAYS,
            representation: default_file_representation(),
            requested_level: default_full_level(),
            verification_mode: default_full_readback(),
            encryption_enabled: false,
            spanning_enabled: false,
            actor_label: default_actor(),
        }
    }

    fn validate(&self) -> Result<(), BackupOpsError> {
        validate_label(&self.actor_label, "actor_label")?;
        if self.representation != "file" {
            return Err(BackupOpsError::Invalid(
                "only the file representation is executable; native_zfs and both remain blocked"
                    .to_owned(),
            ));
        }
        if self.requested_level != "full" {
            return Err(BackupOpsError::Invalid(
                "only full backups are executable; incremental backup claims remain blocked"
                    .to_owned(),
            ));
        }
        if self.verification_mode != "full_readback" {
            return Err(BackupOpsError::Invalid(
                "the attended backup workflow requires full_readback verification".to_owned(),
            ));
        }
        if self.encryption_enabled {
            return Err(BackupOpsError::Invalid(
                "encryption is not implemented by the pinned laboratory adapter".to_owned(),
            ));
        }
        if self.spanning_enabled {
            return Err(BackupOpsError::Invalid(
                "multi-volume spanning is not implemented by the pinned single-tape adapter"
                    .to_owned(),
            ));
        }
        if !(1..=36_500).contains(&self.retention_days) {
            return Err(BackupOpsError::Invalid(
                "retention_days must be between 1 and 36500".to_owned(),
            ));
        }
        self.schedule_timezone
            .parse::<Tz>()
            .map_err(|_| BackupOpsError::Invalid("schedule_timezone is invalid".to_owned()))?;
        if self.full_schedule != DEFAULT_FULL_SCHEDULE {
            return Err(BackupOpsError::Invalid(format!(
                "the full-only scheduler currently supports exactly {DEFAULT_FULL_SCHEDULE} (Saturday at 02:00)"
            )));
        }
        Ok(())
    }
}

fn default_timezone() -> String {
    "UTC".to_owned()
}

fn default_full_schedule() -> String {
    DEFAULT_FULL_SCHEDULE.to_owned()
}

const fn default_retention_days() -> i32 {
    DEFAULT_RETENTION_DAYS
}

fn default_file_representation() -> String {
    "file".to_owned()
}

fn default_full_level() -> String {
    "full".to_owned()
}

fn default_full_readback() -> String {
    "full_readback".to_owned()
}

fn default_actor() -> String {
    "Web operator".to_owned()
}

#[derive(Debug, FromRow)]
struct SourceProfileBinding {
    id: Uuid,
    job_id: Uuid,
    source_id: Uuid,
    name: String,
    immutable_snapshot_confirmed: bool,
    enabled: bool,
    production_eligible: bool,
    workflow_kind: String,
    policy_id: Option<Uuid>,
}

pub async fn create_backup_profile(
    pool: &PgPool,
    request: &NewBackupProfile,
) -> Result<BackupProfileRecord, BackupOpsError> {
    request.validate()?;
    let mut transaction = pool.begin().await?;
    let source = sqlx::query_as::<_, SourceProfileBinding>(
        "SELECT id, job_id, source_id, name, immutable_snapshot_confirmed,
                enabled, production_eligible, workflow_kind, policy_id
           FROM single_tape_data_profiles
          WHERE id = $1 FOR UPDATE",
    )
    .bind(request.source_profile_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(BackupOpsError::NotFound)?;

    if source.workflow_kind == "backup" {
        transaction.rollback().await?;
        return get_backup_profile(pool, source.id).await?.ok_or_else(|| {
            BackupOpsError::Conflict(
                "the source profile is already linked to an incomplete backup policy".to_owned(),
            )
        });
    }
    if source.policy_id.is_some() || source.workflow_kind != "recovery_exercise" {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(
            "the source profile is already owned by another workflow".to_owned(),
        ));
    }
    if !source.enabled || !source.immutable_snapshot_confirmed || source.production_eligible {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(
            "the source profile must be enabled, immutable-snapshot confirmed, and laboratory-only"
                .to_owned(),
        ));
    }

    if let Some(media_pool_id) = request.media_pool_id {
        let purpose = sqlx::query_scalar::<_, String>(
            "SELECT purpose FROM media_pools WHERE id = $1 FOR SHARE",
        )
        .bind(media_pool_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(BackupOpsError::NotFound)?;
        if purpose == "cleaning" {
            transaction.rollback().await?;
            return Err(BackupOpsError::Invalid(
                "a cleaning-media pool cannot be a backup target".to_owned(),
            ));
        }
    }

    let job_policy = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT policy_id FROM jobs WHERE id = $1 AND kind = 'backup' FOR UPDATE",
    )
    .bind(source.job_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| {
        BackupOpsError::Conflict("the source profile has no durable backup job".to_owned())
    })?;
    if job_policy.is_some() {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(
            "the source profile job is already attached to a protection policy".to_owned(),
        ));
    }

    let policy_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO protection_policies
            (id, name, source_id, representation, schedule_timezone,
             full_schedule, incremental_schedule, retention_days,
             encryption_enabled, compression_mode, verification_mode,
             config, enabled)
         VALUES ($1, $2, $3, 'file', $4, $5, 'disabled', $6,
                 false, 'none', 'full_readback', $7, true)",
    )
    .bind(policy_id)
    .bind(&source.name)
    .bind(source.source_id)
    .bind(&request.schedule_timezone)
    .bind(&request.full_schedule)
    .bind(request.retention_days)
    .bind(json!({
        "workflow": "attended_full_file_backup",
        "requested_level": "full",
        "incremental_supported": false,
        "spanning_supported": false,
        "encryption_supported": false,
        "production_eligible": false,
        "schedule_enabled": request.schedule_enabled
    }))
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE jobs SET policy_id = $1, external_id = $2 WHERE id = $3")
        .bind(policy_id)
        .bind(format!("backup-full-{}", source.id))
        .bind(source.job_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE single_tape_data_profiles
            SET workflow_kind = 'backup', policy_id = $1, media_pool_id = $2,
                schedule_enabled = $3, retention_days = $4, updated_at = now()
          WHERE id = $5",
    )
    .bind(policy_id)
    .bind(request.media_pool_id)
    .bind(request.schedule_enabled)
    .bind(request.retention_days)
    .bind(source.id)
    .execute(&mut *transaction)
    .await?;
    append_audit(
        &mut transaction,
        &request.actor_label,
        "backup.profile.create",
        "single_tape_data_profile",
        source.id.to_string(),
        json!({
            "policy_id": policy_id,
            "media_pool_id": request.media_pool_id,
            "schedule_enabled": request.schedule_enabled,
            "schedule_timezone": request.schedule_timezone,
            "full_schedule": request.full_schedule,
            "retention_days": request.retention_days,
            "representation": "file",
            "requested_level": "full",
            "verification_mode": "full_readback",
            "encryption_enabled": false,
            "spanning_enabled": false,
            "production_eligible": false
        }),
        "created",
        Uuid::now_v7(),
    )
    .await?;
    let record = fetch_backup_profile(&mut transaction, source.id)
        .await?
        .ok_or(BackupOpsError::NotFound)?;
    transaction.commit().await?;
    Ok(record)
}

pub async fn list_backup_profiles(
    pool: &PgPool,
) -> Result<Vec<BackupProfileRecord>, BackupOpsError> {
    Ok(sqlx::query_as::<_, BackupProfileRecord>(&format!(
        "{BACKUP_PROFILE_SELECT} ORDER BY data_profile.created_at DESC"
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn get_backup_profile(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<BackupProfileRecord>, BackupOpsError> {
    Ok(sqlx::query_as::<_, BackupProfileRecord>(&format!(
        "{BACKUP_PROFILE_SELECT} AND data_profile.id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

async fn fetch_backup_profile(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<BackupProfileRecord>, sqlx::Error> {
    sqlx::query_as::<_, BackupProfileRecord>(&format!(
        "{BACKUP_PROFILE_SELECT} AND data_profile.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTriggerKind {
    Manual,
    Scheduled,
}

impl BackupTriggerKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
        }
    }
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct BackupRequestRecord {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub profile_name: String,
    pub policy_id: Uuid,
    pub job_run_id: Uuid,
    pub trigger_kind: String,
    pub requested_level: String,
    pub scheduled_occurrence: Option<String>,
    pub assigned_data_run_id: Option<Uuid>,
    pub requested_by_label: String,
    pub correlation_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub cancelled_by_label: Option<String>,
    pub state: String,
    pub attempt: i32,
    pub source_snapshot_id: Option<Uuid>,
    pub engine_operation_id: Option<String>,
    pub failure_class: Option<String>,
    pub failure_detail: Option<Value>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

const BACKUP_REQUEST_SELECT: &str = r#"
    SELECT request.id,
           request.profile_id,
           profile.name AS profile_name,
           profile.policy_id,
           request.job_run_id,
           request.trigger_kind,
           request.requested_level,
           request.scheduled_occurrence,
           request.assigned_data_run_id,
           request.requested_by_label,
           request.correlation_id,
           request.created_at,
           request.cancelled_at,
           request.cancelled_by_label,
           job_run.state,
           job_run.attempt,
           job_run.source_snapshot_id,
           job_run.engine_operation_id,
           job_run.failure_class,
           job_run.failure_detail,
           job_run.started_at,
           job_run.finished_at
      FROM backup_requests request
      JOIN single_tape_data_profiles profile ON profile.id = request.profile_id
      JOIN job_runs job_run ON job_run.id = request.job_run_id
     WHERE profile.workflow_kind = 'backup'
"#;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "disposition", content = "request", rename_all = "snake_case")]
pub enum CreateBackupRequestResult {
    Created(BackupRequestRecord),
    Existing(BackupRequestRecord),
}

impl CreateBackupRequestResult {
    #[must_use]
    pub const fn record(&self) -> &BackupRequestRecord {
        match self {
            Self::Created(record) | Self::Existing(record) => record,
        }
    }
}

pub async fn create_manual_backup_request(
    pool: &PgPool,
    profile_id: Uuid,
    idempotency_key: &str,
    requested_by_label: &str,
) -> Result<CreateBackupRequestResult, BackupOpsError> {
    create_backup_request(
        pool,
        profile_id,
        BackupTriggerKind::Manual,
        None,
        idempotency_key,
        requested_by_label,
    )
    .await
}

pub async fn create_scheduled_backup_request(
    pool: &PgPool,
    profile_id: Uuid,
    scheduled_occurrence: &str,
) -> Result<CreateBackupRequestResult, BackupOpsError> {
    let idempotency_key = format!("scheduled:{scheduled_occurrence}");
    create_backup_request(
        pool,
        profile_id,
        BackupTriggerKind::Scheduled,
        Some(scheduled_occurrence),
        &idempotency_key,
        "tapescale-scheduler",
    )
    .await
}

async fn create_backup_request(
    pool: &PgPool,
    profile_id: Uuid,
    trigger_kind: BackupTriggerKind,
    scheduled_occurrence: Option<&str>,
    idempotency_key: &str,
    requested_by_label: &str,
) -> Result<CreateBackupRequestResult, BackupOpsError> {
    validate_label(idempotency_key, "idempotency_key")?;
    validate_label(requested_by_label, "requested_by_label")?;
    if let Some(occurrence) = scheduled_occurrence {
        validate_label(occurrence, "scheduled_occurrence")?;
    }
    if (trigger_kind == BackupTriggerKind::Scheduled) != scheduled_occurrence.is_some() {
        return Err(BackupOpsError::Invalid(
            "scheduled requests require a scheduled_occurrence and manual requests forbid one"
                .to_owned(),
        ));
    }

    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("tapescale:backup-request:{profile_id}"))
        .execute(&mut *transaction)
        .await?;

    #[derive(FromRow)]
    struct RequestProfile {
        job_id: Uuid,
        source_snapshot_id: Uuid,
        schedule_enabled: bool,
        enabled: bool,
        production_eligible: bool,
        representation: String,
        encryption_enabled: bool,
        verification_mode: String,
    }
    let profile = sqlx::query_as::<_, RequestProfile>(
        "SELECT data_profile.job_id, data_profile.source_snapshot_id,
                data_profile.schedule_enabled, data_profile.enabled,
                data_profile.production_eligible, policy.representation,
                policy.encryption_enabled, policy.verification_mode
           FROM single_tape_data_profiles data_profile
           JOIN protection_policies policy ON policy.id = data_profile.policy_id
          WHERE data_profile.id = $1 AND data_profile.workflow_kind = 'backup'
          FOR UPDATE OF data_profile, policy",
    )
    .bind(profile_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(BackupOpsError::NotFound)?;
    if !profile.enabled
        || profile.production_eligible
        || profile.representation != "file"
        || profile.encryption_enabled
        || profile.verification_mode != "full_readback"
    {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(
            "the profile no longer satisfies the laboratory full-file backup contract".to_owned(),
        ));
    }
    if trigger_kind == BackupTriggerKind::Scheduled && !profile.schedule_enabled {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(
            "scheduled execution is disabled for this backup profile".to_owned(),
        ));
    }

    let existing_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM backup_requests
          WHERE profile_id = $1 AND idempotency_key = $2",
    )
    .bind(profile_id)
    .bind(idempotency_key)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(existing_id) = existing_id {
        let record = fetch_backup_request(&mut transaction, existing_id)
            .await?
            .ok_or(BackupOpsError::NotFound)?;
        transaction.commit().await?;
        return Ok(CreateBackupRequestResult::Existing(record));
    }

    let active = sqlx::query_scalar::<_, Uuid>(
        "SELECT request.id
           FROM backup_requests request
           JOIN job_runs job_run ON job_run.id = request.job_run_id
          WHERE request.profile_id = $1
            AND job_run.state IN (
                'queued', 'preparing', 'waiting_for_media', 'writing',
                'written', 'verifying', 'restoring', 'comparing'
            )
          ORDER BY request.created_at DESC LIMIT 1",
    )
    .bind(profile_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(active) = active {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(format!(
            "backup request {active} is already active for this profile"
        )));
    }

    let id = Uuid::now_v7();
    let job_run_id = Uuid::now_v7();
    let correlation_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO job_runs
            (id, job_id, scheduled_occurrence, attempt, state, correlation_id,
             source_snapshot_id)
         VALUES ($1, $2, $3, 1, 'waiting_for_media', $4, $5)",
    )
    .bind(job_run_id)
    .bind(profile.job_id)
    .bind(scheduled_occurrence)
    .bind(correlation_id)
    .bind(profile.source_snapshot_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO backup_requests
            (id, profile_id, job_run_id, trigger_kind, requested_level,
             scheduled_occurrence, idempotency_key, requested_by_label,
             correlation_id)
         VALUES ($1, $2, $3, $4, 'full', $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(profile_id)
    .bind(job_run_id)
    .bind(trigger_kind.as_str())
    .bind(scheduled_occurrence)
    .bind(idempotency_key)
    .bind(requested_by_label)
    .bind(correlation_id)
    .execute(&mut *transaction)
    .await?;
    append_job_transition(
        &mut transaction,
        job_run_id,
        None,
        "waiting_for_media",
        "full-file backup request retained before hardware assignment",
        json!({
            "backup_request_id": id,
            "trigger_kind": trigger_kind,
            "requested_level": "full",
            "production_eligible": false
        }),
        correlation_id,
    )
    .await?;
    append_request_event(
        &mut transaction,
        id,
        "requested",
        json!({
            "job_run_id": job_run_id,
            "trigger_kind": trigger_kind,
            "scheduled_occurrence": scheduled_occurrence,
            "requested_level": "full",
            "production_eligible": false
        }),
        requested_by_label,
        correlation_id,
    )
    .await?;
    append_request_event(
        &mut transaction,
        id,
        "waiting_for_media",
        json!({
            "detail": "select an exact eligible drive and cartridge to continue",
            "safety_authority": false
        }),
        requested_by_label,
        correlation_id,
    )
    .await?;
    append_audit(
        &mut transaction,
        requested_by_label,
        "backup.request.create",
        "backup_request",
        id.to_string(),
        json!({
            "profile_id": profile_id,
            "job_run_id": job_run_id,
            "trigger_kind": trigger_kind,
            "scheduled_occurrence": scheduled_occurrence,
            "requested_level": "full"
        }),
        "waiting_for_media",
        correlation_id,
    )
    .await?;
    let record = fetch_backup_request(&mut transaction, id)
        .await?
        .ok_or(BackupOpsError::NotFound)?;
    transaction.commit().await?;
    Ok(CreateBackupRequestResult::Created(record))
}

pub async fn list_backup_requests(
    pool: &PgPool,
    profile_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<BackupRequestRecord>, BackupOpsError> {
    let limit = limit.clamp(1, 250);
    Ok(sqlx::query_as::<_, BackupRequestRecord>(&format!(
        "{BACKUP_REQUEST_SELECT}
         AND ($1::uuid IS NULL OR request.profile_id = $1)
         ORDER BY request.created_at DESC LIMIT $2"
    ))
    .bind(profile_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn get_backup_request(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<BackupRequestRecord>, BackupOpsError> {
    Ok(sqlx::query_as::<_, BackupRequestRecord>(&format!(
        "{BACKUP_REQUEST_SELECT} AND request.id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

async fn fetch_backup_request(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<BackupRequestRecord>, sqlx::Error> {
    sqlx::query_as::<_, BackupRequestRecord>(&format!(
        "{BACKUP_REQUEST_SELECT} AND request.id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct BackupRequestEventRecord {
    pub id: i64,
    pub event_type: String,
    pub evidence: Value,
    pub actor_label: String,
    pub correlation_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

pub async fn backup_request_events(
    pool: &PgPool,
    id: Uuid,
) -> Result<Vec<BackupRequestEventRecord>, BackupOpsError> {
    Ok(sqlx::query_as(
        "SELECT id, event_type, evidence, actor_label, correlation_id, occurred_at
           FROM backup_request_events
          WHERE backup_request_id = $1
          ORDER BY occurred_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?)
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupRequestDetail {
    pub request: BackupRequestRecord,
    pub events: Vec<BackupRequestEventRecord>,
}

pub async fn get_backup_request_detail(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<BackupRequestDetail>, BackupOpsError> {
    let Some(request) = get_backup_request(pool, id).await? else {
        return Ok(None);
    };
    let events = backup_request_events(pool, id).await?;
    Ok(Some(BackupRequestDetail { request, events }))
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "disposition", content = "request", rename_all = "snake_case")]
pub enum AssignBackupRequestResult {
    Assigned(BackupRequestRecord),
    AlreadyAssigned(BackupRequestRecord),
}

/// Link the exact fenced data run to a retained waiting request.
///
/// The caller must first insert `single_tape_data_runs` with this request's
/// existing `job_run_id`, profile, workflow kind, trigger kind, level, and
/// occurrence.  This function verifies those bindings, changes the existing
/// job run from `waiting_for_media` to `queued`, and appends evidence.  It does
/// not treat assignment metadata as overwrite authority.
pub async fn assign_backup_request(
    pool: &PgPool,
    request_id: Uuid,
    data_run_id: Uuid,
    actor_label: &str,
) -> Result<AssignBackupRequestResult, BackupOpsError> {
    validate_label(actor_label, "actor_label")?;
    let mut transaction = pool.begin().await?;
    #[derive(FromRow)]
    struct AssignmentRequest {
        profile_id: Uuid,
        job_run_id: Uuid,
        assigned_data_run_id: Option<Uuid>,
        correlation_id: Uuid,
        job_state: String,
    }
    let request = sqlx::query_as::<_, AssignmentRequest>(
        "SELECT request.profile_id, request.job_run_id,
                request.assigned_data_run_id, request.correlation_id,
                job_run.state AS job_state
           FROM backup_requests request
           JOIN job_runs job_run ON job_run.id = request.job_run_id
          WHERE request.id = $1
          FOR UPDATE OF request, job_run",
    )
    .bind(request_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(BackupOpsError::NotFound)?;
    if request.assigned_data_run_id == Some(data_run_id) {
        let record = fetch_backup_request(&mut transaction, request_id)
            .await?
            .ok_or(BackupOpsError::NotFound)?;
        transaction.commit().await?;
        return Ok(AssignBackupRequestResult::AlreadyAssigned(record));
    }
    if request.assigned_data_run_id.is_some() || request.job_state != "waiting_for_media" {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(
            "the backup request is no longer waiting for its first hardware assignment".to_owned(),
        ));
    }

    #[derive(FromRow)]
    struct DataRunBinding {
        profile_id: Uuid,
        job_run_id: Uuid,
        workflow_kind: String,
        trigger_kind: String,
        requested_level: String,
    }
    let data_run = sqlx::query_as::<_, DataRunBinding>(
        "SELECT profile_id, job_run_id, workflow_kind, trigger_kind, requested_level
           FROM single_tape_data_runs WHERE id = $1 FOR UPDATE",
    )
    .bind(data_run_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(BackupOpsError::NotFound)?;
    if data_run.profile_id != request.profile_id
        || data_run.job_run_id != request.job_run_id
        || data_run.workflow_kind != "backup"
        || !matches!(data_run.trigger_kind.as_str(), "manual" | "scheduled")
        || data_run.requested_level != "full"
    {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(
            "the fenced data run does not match the retained profile, job run, trigger, and full level"
                .to_owned(),
        ));
    }

    sqlx::query("UPDATE backup_requests SET assigned_data_run_id = $1 WHERE id = $2")
        .bind(data_run_id)
        .bind(request_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE job_runs SET state = 'queued' WHERE id = $1 AND state = 'waiting_for_media'",
    )
    .bind(request.job_run_id)
    .execute(&mut *transaction)
    .await?;
    append_job_transition(
        &mut transaction,
        request.job_run_id,
        Some("waiting_for_media"),
        "queued",
        "operator assigned an exact fenced drive and cartridge",
        json!({
            "backup_request_id": request_id,
            "single_tape_data_run_id": data_run_id,
            "assignment_only": true,
            "safety_authority": false
        }),
        request.correlation_id,
    )
    .await?;
    append_request_event(
        &mut transaction,
        request_id,
        "assignment_recorded",
        json!({
            "single_tape_data_run_id": data_run_id,
            "assignment_only": true,
            "safety_authority": false
        }),
        actor_label,
        request.correlation_id,
    )
    .await?;
    append_audit(
        &mut transaction,
        actor_label,
        "backup.request.assign",
        "backup_request",
        request_id.to_string(),
        json!({"single_tape_data_run_id": data_run_id}),
        "queued",
        request.correlation_id,
    )
    .await?;
    let record = fetch_backup_request(&mut transaction, request_id)
        .await?
        .ok_or(BackupOpsError::NotFound)?;
    transaction.commit().await?;
    Ok(AssignBackupRequestResult::Assigned(record))
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "disposition", content = "request", rename_all = "snake_case")]
pub enum CancelBackupRequestResult {
    Cancelled(BackupRequestRecord),
    AlreadyCancelled(BackupRequestRecord),
}

pub async fn cancel_waiting_backup_request(
    pool: &PgPool,
    request_id: Uuid,
    actor_label: &str,
    reason: &str,
) -> Result<CancelBackupRequestResult, BackupOpsError> {
    validate_label(actor_label, "actor_label")?;
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > 1000 || reason.chars().any(char::is_control) {
        return Err(BackupOpsError::Invalid(
            "reason must contain 1 to 1000 printable bytes".to_owned(),
        ));
    }
    let mut transaction = pool.begin().await?;
    #[derive(FromRow)]
    struct CancellationRequest {
        job_run_id: Uuid,
        assigned_data_run_id: Option<Uuid>,
        correlation_id: Uuid,
        cancelled_at: Option<DateTime<Utc>>,
        job_state: String,
    }
    let request = sqlx::query_as::<_, CancellationRequest>(
        "SELECT request.job_run_id, request.assigned_data_run_id,
                request.correlation_id, request.cancelled_at,
                job_run.state AS job_state
           FROM backup_requests request
           JOIN job_runs job_run ON job_run.id = request.job_run_id
          WHERE request.id = $1
          FOR UPDATE OF request, job_run",
    )
    .bind(request_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(BackupOpsError::NotFound)?;
    if request.cancelled_at.is_some() && request.job_state == "cancelled" {
        let record = fetch_backup_request(&mut transaction, request_id)
            .await?
            .ok_or(BackupOpsError::NotFound)?;
        transaction.commit().await?;
        return Ok(CancelBackupRequestResult::AlreadyCancelled(record));
    }
    if request.job_state != "waiting_for_media" || request.assigned_data_run_id.is_some() {
        transaction.rollback().await?;
        return Err(BackupOpsError::Conflict(
            "only an unassigned waiting-for-media backup request can use immediate cancellation"
                .to_owned(),
        ));
    }
    sqlx::query(
        "UPDATE backup_requests
            SET cancelled_at = now(), cancelled_by_label = $1
          WHERE id = $2",
    )
    .bind(actor_label)
    .bind(request_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE job_runs
            SET state = 'cancelled', failure_class = 'operator_cancelled',
                failure_detail = $1, started_at = COALESCE(started_at, now()),
                finished_at = now()
          WHERE id = $2 AND state = 'waiting_for_media'",
    )
    .bind(json!({
        "detail": "cancelled before hardware assignment",
        "reason": reason
    }))
    .bind(request.job_run_id)
    .execute(&mut *transaction)
    .await?;
    append_job_transition(
        &mut transaction,
        request.job_run_id,
        Some("waiting_for_media"),
        "cancelled",
        "operator cancelled before hardware assignment",
        json!({
            "backup_request_id": request_id,
            "reason": reason,
            "tape_command_dispatched": false
        }),
        request.correlation_id,
    )
    .await?;
    append_request_event(
        &mut transaction,
        request_id,
        "cancelled",
        json!({
            "detail": "cancelled before hardware assignment",
            "reason": reason,
            "tape_command_dispatched": false
        }),
        actor_label,
        request.correlation_id,
    )
    .await?;
    append_audit(
        &mut transaction,
        actor_label,
        "backup.request.cancel",
        "backup_request",
        request_id.to_string(),
        json!({"job_run_id": request.job_run_id, "reason": reason}),
        "cancelled",
        request.correlation_id,
    )
    .await?;
    let record = fetch_backup_request(&mut transaction, request_id)
        .await?
        .ok_or(BackupOpsError::NotFound)?;
    transaction.commit().await?;
    Ok(CancelBackupRequestResult::Cancelled(record))
}

#[derive(Clone, Debug, Serialize)]
pub struct DueBackupBlocker {
    pub profile_id: Uuid,
    pub occurrence_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DueBackupEnqueueReport {
    pub created: Vec<BackupRequestRecord>,
    pub existing: Vec<BackupRequestRecord>,
    pub blocked: Vec<DueBackupBlocker>,
}

/// Enqueue the latest due weekly Full occurrence for each enabled profile.
///
/// The function intentionally supports only the declared Saturday 02:00 Full
/// schedule.  Multiple missed Saturdays coalesce to the latest occurrence, and
/// the profile/idempotency uniqueness makes repeated scheduler evaluation safe.
pub async fn enqueue_due_backup_requests(
    pool: &PgPool,
    as_of: DateTime<Utc>,
) -> Result<DueBackupEnqueueReport, BackupOpsError> {
    #[derive(FromRow)]
    struct ScheduledProfile {
        id: Uuid,
        schedule_timezone: String,
        full_schedule: String,
        created_at: DateTime<Utc>,
    }
    let profiles = sqlx::query_as::<_, ScheduledProfile>(
        "SELECT data_profile.id, policy.schedule_timezone,
                policy.full_schedule, data_profile.created_at
           FROM single_tape_data_profiles data_profile
           JOIN protection_policies policy ON policy.id = data_profile.policy_id
          WHERE data_profile.workflow_kind = 'backup'
            AND data_profile.enabled
            AND data_profile.schedule_enabled
            AND policy.enabled
          ORDER BY data_profile.created_at, data_profile.id",
    )
    .fetch_all(pool)
    .await?;
    let mut report = DueBackupEnqueueReport::default();
    for profile in profiles {
        if profile.full_schedule != DEFAULT_FULL_SCHEDULE {
            report.blocked.push(DueBackupBlocker {
                profile_id: profile.id,
                occurrence_id: "unsupported-schedule".to_owned(),
                reason: format!(
                    "unsupported full schedule {}; expected {DEFAULT_FULL_SCHEDULE}",
                    profile.full_schedule
                ),
            });
            continue;
        }
        let timezone = match profile.schedule_timezone.parse::<Tz>() {
            Ok(timezone) => timezone,
            Err(_) => {
                report.blocked.push(DueBackupBlocker {
                    profile_id: profile.id,
                    occurrence_id: "invalid-timezone".to_owned(),
                    reason: format!("invalid schedule timezone {}", profile.schedule_timezone),
                });
                continue;
            }
        };
        let Some(occurrence) = latest_due_full(as_of, timezone) else {
            continue;
        };
        if occurrence.scheduled_for < profile.created_at {
            continue;
        }
        match create_scheduled_backup_request(pool, profile.id, &occurrence.id).await {
            Ok(CreateBackupRequestResult::Created(record)) => report.created.push(record),
            Ok(CreateBackupRequestResult::Existing(record)) => report.existing.push(record),
            Err(BackupOpsError::Conflict(reason)) => report.blocked.push(DueBackupBlocker {
                profile_id: profile.id,
                occurrence_id: occurrence.id,
                reason,
            }),
            Err(error) => return Err(error),
        }
    }
    Ok(report)
}

fn latest_due_full(
    as_of: DateTime<Utc>,
    timezone: Tz,
) -> Option<crate::domain::schedule::ScheduledOccurrence> {
    let local_date = as_of.with_timezone(&timezone).date_naive();
    for days_back in 0..=7 {
        let date = local_date.checked_sub_days(Days::new(days_back))?;
        let occurrence = default_occurrence(date, timezone);
        if occurrence.kind == RunKind::Full && occurrence.scheduled_for <= as_of {
            return Some(occurrence);
        }
    }
    None
}

async fn append_job_transition(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    from_state: Option<&str>,
    to_state: &str,
    reason: &str,
    evidence: Value,
    correlation_id: Uuid,
) -> Result<(), BackupOpsError> {
    validate_json_object(&evidence, "job transition evidence", MAX_EVENT_BYTES)?;
    sqlx::query(
        "INSERT INTO job_transitions
            (run_id, from_state, to_state, reason, evidence, correlation_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(run_id)
    .bind(from_state)
    .bind(to_state)
    .bind(reason)
    .bind(evidence)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn append_request_event(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
    event_type: &str,
    evidence: Value,
    actor_label: &str,
    correlation_id: Uuid,
) -> Result<(), BackupOpsError> {
    validate_label(actor_label, "actor_label")?;
    validate_json_object(&evidence, "backup request evidence", MAX_EVENT_BYTES)?;
    sqlx::query(
        "INSERT INTO backup_request_events
            (backup_request_id, event_type, evidence, actor_label, correlation_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(request_id)
    .bind(event_type)
    .bind(evidence)
    .bind(actor_label)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn append_audit(
    transaction: &mut Transaction<'_, Postgres>,
    actor_label: &str,
    action: &str,
    object_type: &str,
    object_id: String,
    requested: Value,
    result: &str,
    correlation_id: Uuid,
) -> Result<(), BackupOpsError> {
    validate_label(actor_label, "actor_label")?;
    validate_json_object(&requested, "audit request", MAX_EVENT_BYTES)?;
    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(actor_label)
    .bind(action)
    .bind(object_type)
    .bind(object_id)
    .bind(requested)
    .bind(result)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn validate_label(value: &str, field: &str) -> Result<(), BackupOpsError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_LABEL_BYTES
        || trimmed.chars().any(char::is_control)
    {
        return Err(BackupOpsError::Invalid(format!(
            "{field} must contain 1 to {MAX_LABEL_BYTES} printable bytes"
        )));
    }
    Ok(())
}

fn validate_json_object(value: &Value, field: &str, limit: usize) -> Result<(), BackupOpsError> {
    if !value.is_object() {
        return Err(BackupOpsError::Invalid(format!(
            "{field} must be a JSON object"
        )));
    }
    let length = serde_json::to_vec(value)
        .map_err(|error| BackupOpsError::Invalid(error.to_string()))?
        .len();
    if length > limit {
        return Err(BackupOpsError::Invalid(format!(
            "{field} exceeds the {limit}-byte limit"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attended_profile_defaults_are_full_file_and_schedule_disabled() {
        let profile = NewBackupProfile::attended_full_file(Uuid::nil());
        assert_eq!(profile.representation, "file");
        assert_eq!(profile.requested_level, "full");
        assert_eq!(profile.verification_mode, "full_readback");
        assert_eq!(profile.retention_days, 28);
        assert!(!profile.schedule_enabled);
        assert!(!profile.encryption_enabled);
        assert!(!profile.spanning_enabled);
        profile.validate().expect("safe defaults should validate");
    }

    #[test]
    fn unsupported_backup_claims_fail_closed() {
        let mut profile = NewBackupProfile::attended_full_file(Uuid::nil());
        profile.requested_level = "incremental".to_owned();
        assert!(profile.validate().is_err());

        let mut profile = NewBackupProfile::attended_full_file(Uuid::nil());
        profile.representation = "native_zfs".to_owned();
        assert!(profile.validate().is_err());

        let mut profile = NewBackupProfile::attended_full_file(Uuid::nil());
        profile.encryption_enabled = true;
        assert!(profile.validate().is_err());

        let mut profile = NewBackupProfile::attended_full_file(Uuid::nil());
        profile.spanning_enabled = true;
        assert!(profile.validate().is_err());
    }

    #[test]
    fn scheduler_selects_only_due_saturday_full() {
        let timezone: Tz = "UTC".parse().unwrap();
        let before = "2026-11-14T01:59:59Z".parse().unwrap();
        let after = "2026-11-14T02:00:00Z".parse().unwrap();
        let previous = latest_due_full(before, timezone).unwrap();
        let current = latest_due_full(after, timezone).unwrap();
        assert!(previous.scheduled_for < before);
        assert_eq!(current.scheduled_for, after);
        assert_eq!(current.kind, RunKind::Full);
    }

    #[test]
    fn labels_and_evidence_are_bounded() {
        assert!(validate_label("operator", "actor").is_ok());
        assert!(validate_label("\n", "actor").is_err());
        assert!(validate_label(&"x".repeat(257), "actor").is_err());
        assert!(validate_json_object(&json!({"safe": true}), "evidence", 64).is_ok());
        assert!(validate_json_object(&json!(["not", "object"]), "evidence", 64).is_err());
    }
}
