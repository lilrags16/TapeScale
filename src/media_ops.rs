//! Durable media inventory, protection, and reuse-readiness operations.
//!
//! This module intentionally has no destructive media primitive.  A reuse
//! evaluation is evidence only: it cannot erase, relabel, recycle, or override
//! a protection claim.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::domain::{
    lto::{DriveGeneration, MediaFormat},
    media::{
        evaluate_overwrite, IdentityState, MediaHealth, OverwriteBlocker, OverwriteContext,
        OverwriteIntent, ProtectionClaims, Reservation,
    },
};

pub const MEDIA_DETAIL_VOLUME_LIMIT: usize = 256;
pub const MEDIA_DETAIL_RECOVERY_LIMIT: usize = 256;
pub const MEDIA_DETAIL_DEPENDENCY_LIMIT: usize = 256;
pub const MEDIA_DETAIL_HOLD_LIMIT: usize = 256;
pub const MEDIA_DETAIL_VERIFICATION_LIMIT: usize = 128;
pub const MEDIA_DETAIL_CUSTODY_LIMIT: usize = 256;
pub const MEDIA_DETAIL_OPERATION_LIMIT: usize = 256;
pub const MEDIA_DETAIL_AUDIT_LIMIT: usize = 256;

fn empty_object() -> Value {
    json!({})
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaPoolPurpose {
    Routine,
    Archive,
    Scratch,
    Cleaning,
}

impl MediaPoolPurpose {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Routine => "routine",
            Self::Archive => "archive",
            Self::Scratch => "scratch",
            Self::Cleaning => "cleaning",
        }
    }

    fn parse(value: &str) -> Result<Self, MediaOpsError> {
        match value {
            "routine" => Ok(Self::Routine),
            "archive" => Ok(Self::Archive),
            "scratch" => Ok(Self::Scratch),
            "cleaning" => Ok(Self::Cleaning),
            other => Err(MediaOpsError::Invariant(format!(
                "database contains unknown media pool purpose {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaHoldKind {
    Manual,
    Legal,
    Archive,
}

impl MediaHoldKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Legal => "legal",
            Self::Archive => "archive",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaIdentityStatus {
    Unknown,
    Foreign,
    Reconciled,
    Mismatch,
}

impl MediaIdentityStatus {
    fn parse(value: &str) -> Result<Self, MediaOpsError> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "foreign" => Ok(Self::Foreign),
            "reconciled" => Ok(Self::Reconciled),
            "mismatch" => Ok(Self::Mismatch),
            other => Err(MediaOpsError::Invariant(format!(
                "database contains unknown cartridge identity status {other:?}"
            ))),
        }
    }

    const fn domain(self) -> IdentityState {
        match self {
            Self::Reconciled => IdentityState::Confirmed,
            Self::Unknown => IdentityState::Unknown,
            Self::Foreign => IdentityState::Foreign,
            Self::Mismatch => IdentityState::Mismatch,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaLocationKind {
    Onsite,
    Offsite,
    InTransit,
    Unknown,
    Device,
}

impl MediaLocationKind {
    fn parse(value: &str) -> Result<Self, MediaOpsError> {
        match value {
            "onsite" => Ok(Self::Onsite),
            "offsite" => Ok(Self::Offsite),
            "in_transit" => Ok(Self::InTransit),
            "unknown" => Ok(Self::Unknown),
            "device" => Ok(Self::Device),
            other => Err(MediaOpsError::Invariant(format!(
                "database contains unknown location kind {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryReuseState {
    PhysicallyBlocked,
    Reserved,
    NeedsReconciliation,
    Protected,
    DriveEvaluationRequired,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MediaActor {
    pub user_id: Option<Uuid>,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CreateMediaPoolRequest {
    pub name: String,
    pub purpose: MediaPoolPurpose,
    #[serde(default)]
    pub indefinite_retention: bool,
    #[serde(default = "empty_object")]
    pub details: Value,
    pub actor: MediaActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MediaPoolRecord {
    pub id: Uuid,
    pub name: String,
    pub purpose: MediaPoolPurpose,
    pub indefinite_retention: bool,
    pub details: Value,
    pub cartridge_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MediaPoolReference {
    pub id: Uuid,
    pub name: String,
    pub purpose: MediaPoolPurpose,
    pub indefinite_retention: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MediaLocationReference {
    pub id: Uuid,
    pub name: String,
    pub kind: MediaLocationKind,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MediaVerificationSummary {
    pub id: Uuid,
    pub recovery_point_id: Uuid,
    pub kind: String,
    pub outcome: String,
    pub drive_id: Option<Uuid>,
    pub drive_stable_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MediaInventorySummary {
    pub id: Uuid,
    pub internal_label: String,
    pub barcode: Option<String>,
    pub media_generation: Option<i16>,
    pub media_format: Option<String>,
    pub on_tape_identity: Option<String>,
    pub identity_status: MediaIdentityStatus,
    pub health_status: String,
    pub worm: bool,
    pub reserved_by: Option<String>,
    pub state_version: i64,
    pub pool: Option<MediaPoolReference>,
    pub confirmed_location: Option<MediaLocationReference>,
    pub active_holds: i64,
    pub has_content: bool,
    pub content_volume_count: i64,
    pub catalog_reconciled: bool,
    pub retained_recovery_points: i64,
    pub dependent_recovery_points: i64,
    pub indefinite_archive_claims: i64,
    pub dependency_graph_truncated: bool,
    pub latest_verification: Option<MediaVerificationSummary>,
    pub reuse_state: InventoryReuseState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AssignMediaPoolRequest {
    pub pool_id: Option<Uuid>,
    pub expected_state_version: i64,
    pub idempotency_key: String,
    pub actor: MediaActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CreateMediaHoldRequest {
    pub kind: MediaHoldKind,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub expected_state_version: i64,
    pub idempotency_key: String,
    pub actor: MediaActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseMediaHoldRequest {
    pub reason: String,
    pub expected_state_version: i64,
    pub idempotency_key: String,
    pub actor: MediaActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MediaMutationResult {
    pub cartridge_id: Uuid,
    pub operation_id: Uuid,
    pub object_id: Option<Uuid>,
    pub state_version: i64,
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, FromRow)]
pub struct MediaVolumeRecord {
    pub id: Uuid,
    pub recovery_point_id: Uuid,
    pub volume_order: i32,
    pub first_record: String,
    pub last_record: Option<String>,
    pub encoded_bytes: Option<i64>,
    pub finalized: bool,
    pub engine_record_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MediaRecoveryPointRecord {
    pub id: Uuid,
    pub source_id: Uuid,
    pub dataset_name: String,
    pub representation: String,
    pub level: String,
    pub status: String,
    pub recovery_time: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub indefinite: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MediaDependentRecoveryPointRecord {
    #[serde(flatten)]
    pub recovery_point: MediaRecoveryPointRecord,
    pub dependency_kind: String,
    pub depth: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, FromRow)]
pub struct MediaHoldRecord {
    pub id: Uuid,
    pub cartridge_id: Option<Uuid>,
    pub recovery_point_id: Option<Uuid>,
    pub kind: String,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub active: bool,
    pub created_by: Option<Uuid>,
    pub created_by_label: String,
    pub created_correlation_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
    pub released_by: Option<Uuid>,
    pub released_by_label: Option<String>,
    pub release_reason: Option<String>,
    pub release_correlation_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, FromRow)]
pub struct MediaVerificationRecord {
    pub id: Uuid,
    pub recovery_point_id: Uuid,
    pub kind: String,
    pub outcome: String,
    pub drive_id: Option<Uuid>,
    pub drive_stable_id: Option<String>,
    pub media_ids: Vec<Uuid>,
    pub correlation_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, FromRow)]
pub struct MediaCustodyEventRecord {
    pub id: Uuid,
    pub movement_task_id: Option<Uuid>,
    pub movement_task_external_id: Option<String>,
    pub event_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_label: String,
    pub source_location_id: Option<Uuid>,
    pub source_location_name: Option<String>,
    pub destination_location_id: Option<Uuid>,
    pub destination_location_name: Option<String>,
    pub destination_label: Option<String>,
    pub note: String,
    pub corrects_event_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, FromRow)]
pub struct MediaOperationRecord {
    pub id: Uuid,
    pub operation_kind: String,
    pub expected_state_version: i64,
    pub resulting_state_version: i64,
    pub object_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub actor_label: String,
    pub correlation_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, FromRow)]
pub struct MediaAuditRecord {
    pub id: Uuid,
    pub actor_id: Option<Uuid>,
    pub actor_label: String,
    pub action: String,
    pub object_type: String,
    pub object_id: String,
    pub result: String,
    pub correlation_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct MediaDetailTruncation {
    pub volumes: bool,
    pub recovery_points: bool,
    pub dependent_recovery_points: bool,
    pub holds: bool,
    pub verifications: bool,
    pub custody_events: bool,
    pub operations: bool,
    pub audit_events: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MediaDetailAggregate {
    pub cartridge: MediaInventorySummary,
    pub volumes: Vec<MediaVolumeRecord>,
    pub recovery_points: Vec<MediaRecoveryPointRecord>,
    pub dependent_recovery_points: Vec<MediaDependentRecoveryPointRecord>,
    pub holds: Vec<MediaHoldRecord>,
    pub verifications: Vec<MediaVerificationRecord>,
    pub custody_events: Vec<MediaCustodyEventRecord>,
    pub operations: Vec<MediaOperationRecord>,
    pub audit_events: Vec<MediaAuditRecord>,
    pub truncation: MediaDetailTruncation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MediaReuseDrive {
    pub id: Uuid,
    pub stable_id: String,
    pub generation: Option<i16>,
    pub qualification_status: String,
    pub reconciled: bool,
    pub observation_current: bool,
    pub motion_safe: bool,
    pub exact_qualification_current: bool,
    pub last_observed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MediaReuseBlocker {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AffectedRecoveryPoint {
    pub id: Uuid,
    pub dataset_name: String,
    pub status: String,
    pub recovery_time: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub indefinite: bool,
    pub relation: String,
    pub dependency_depth: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MediaReuseEvaluation {
    pub cartridge_id: Uuid,
    pub state_version: i64,
    pub selected_drive: MediaReuseDrive,
    pub allowed: bool,
    pub blockers: Vec<MediaReuseBlocker>,
    pub affected_recovery_points: Vec<AffectedRecoveryPoint>,
    /// Always false.  This service exposes evidence, never a destructive action.
    pub destructive_operation_available: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MediaOpsError {
    #[error("{resource} {id} was not found")]
    NotFound { resource: &'static str, id: String },
    #[error("invalid {field}: {message}")]
    Invalid {
        field: &'static str,
        message: String,
    },
    #[error("{code}: {message}")]
    Conflict { code: &'static str, message: String },
    #[error("cartridge state version is stale: expected {expected}, current {actual}")]
    StaleVersion { expected: i64, actual: i64 },
    #[error("idempotency key {key:?} was already used for a different request")]
    IdempotencyConflict { key: String },
    #[error("media database invariant failed: {0}")]
    Invariant(String),
    #[error("media database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, FromRow)]
struct MediaPoolRow {
    id: Uuid,
    name: String,
    purpose: String,
    indefinite_retention: bool,
    details: Value,
    cartridge_count: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<MediaPoolRow> for MediaPoolRecord {
    type Error = MediaOpsError;

    fn try_from(row: MediaPoolRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            name: row.name,
            purpose: MediaPoolPurpose::parse(&row.purpose)?,
            indefinite_retention: row.indefinite_retention,
            details: row.details,
            cartridge_count: row.cartridge_count,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Debug, FromRow)]
struct InventoryRow {
    id: Uuid,
    internal_label: String,
    barcode: Option<String>,
    media_generation: Option<i16>,
    media_format: Option<String>,
    on_tape_identity: Option<String>,
    identity_status: String,
    health_status: String,
    worm: bool,
    reserved_by: Option<String>,
    state_version: i64,
    pool_id: Option<Uuid>,
    pool_name: Option<String>,
    pool_purpose: Option<String>,
    pool_indefinite_retention: Option<bool>,
    location_id: Option<Uuid>,
    location_name: Option<String>,
    location_kind: Option<String>,
    active_holds: i64,
    content_volume_count: i64,
    unreconciled_content_records: i64,
    retained_recovery_points: i64,
    dependent_recovery_points: i64,
    indefinite_archive_claims: i64,
    dependency_graph_truncated: bool,
    latest_verification_id: Option<Uuid>,
    latest_verification_recovery_point_id: Option<Uuid>,
    latest_verification_kind: Option<String>,
    latest_verification_outcome: Option<String>,
    latest_verification_drive_id: Option<Uuid>,
    latest_verification_drive_stable_id: Option<String>,
    latest_verification_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<InventoryRow> for MediaInventorySummary {
    type Error = MediaOpsError;

    fn try_from(row: InventoryRow) -> Result<Self, Self::Error> {
        let identity_status = MediaIdentityStatus::parse(&row.identity_status)?;
        let pool = match (
            row.pool_id,
            row.pool_name,
            row.pool_purpose,
            row.pool_indefinite_retention,
        ) {
            (Some(id), Some(name), Some(purpose), Some(indefinite_retention)) => {
                Some(MediaPoolReference {
                    id,
                    name,
                    purpose: MediaPoolPurpose::parse(&purpose)?,
                    indefinite_retention,
                })
            }
            (None, None, None, None) => None,
            _ => {
                return Err(MediaOpsError::Invariant(
                    "cartridge has a partially joined media pool".to_owned(),
                ))
            }
        };
        let confirmed_location = match (row.location_id, row.location_name, row.location_kind) {
            (Some(id), Some(name), Some(kind)) => Some(MediaLocationReference {
                id,
                name,
                kind: MediaLocationKind::parse(&kind)?,
            }),
            (None, None, None) => None,
            _ => {
                return Err(MediaOpsError::Invariant(
                    "cartridge has a partially joined confirmed location".to_owned(),
                ))
            }
        };
        let latest_verification = match (
            row.latest_verification_id,
            row.latest_verification_recovery_point_id,
            row.latest_verification_kind,
            row.latest_verification_outcome,
            row.latest_verification_at,
        ) {
            (Some(id), Some(recovery_point_id), Some(kind), Some(outcome), Some(occurred_at)) => {
                Some(MediaVerificationSummary {
                    id,
                    recovery_point_id,
                    kind,
                    outcome,
                    drive_id: row.latest_verification_drive_id,
                    drive_stable_id: row.latest_verification_drive_stable_id,
                    occurred_at,
                })
            }
            (None, None, None, None, None) => None,
            _ => {
                return Err(MediaOpsError::Invariant(
                    "cartridge has a partially joined latest verification".to_owned(),
                ))
            }
        };

        let has_content = row.content_volume_count > 0;
        let catalog_reconciled = row.unreconciled_content_records == 0;
        let parsed_format = media_format(row.media_generation, row.media_format.as_deref());
        let format_reconciled = parsed_format
            .is_some_and(|format| Some(media_format_generation(format)) == row.media_generation);
        let protected = row.active_holds > 0
            || row.retained_recovery_points > 0
            || row.dependent_recovery_points > 0
            || row.indefinite_archive_claims > 0
            || pool.as_ref().is_some_and(|pool| {
                pool.indefinite_retention
                    || matches!(
                        pool.purpose,
                        MediaPoolPurpose::Archive | MediaPoolPurpose::Cleaning
                    )
            });
        let healthy = matches!(
            row.health_status.as_str(),
            "healthy" | "verified" | "written_verified"
        );
        let reuse_state = if row.worm {
            InventoryReuseState::PhysicallyBlocked
        } else if row.reserved_by.is_some() {
            InventoryReuseState::Reserved
        } else if identity_status != MediaIdentityStatus::Reconciled
            || row
                .on_tape_identity
                .as_deref()
                .is_none_or(|identity| identity.trim().is_empty())
            || confirmed_location.is_none()
            || row.media_generation.is_none()
            || !format_reconciled
            || !healthy
            || !catalog_reconciled
            || row.dependency_graph_truncated
        {
            InventoryReuseState::NeedsReconciliation
        } else if protected {
            InventoryReuseState::Protected
        } else {
            InventoryReuseState::DriveEvaluationRequired
        };

        Ok(Self {
            id: row.id,
            internal_label: row.internal_label,
            barcode: row.barcode,
            media_generation: row.media_generation,
            media_format: row.media_format,
            on_tape_identity: row.on_tape_identity,
            identity_status,
            health_status: row.health_status,
            worm: row.worm,
            reserved_by: row.reserved_by,
            state_version: row.state_version,
            pool,
            confirmed_location,
            active_holds: row.active_holds,
            has_content,
            content_volume_count: row.content_volume_count,
            catalog_reconciled,
            retained_recovery_points: row.retained_recovery_points,
            dependent_recovery_points: row.dependent_recovery_points,
            indefinite_archive_claims: row.indefinite_archive_claims,
            dependency_graph_truncated: row.dependency_graph_truncated,
            latest_verification,
            reuse_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Debug, FromRow)]
struct RecoveryPointRow {
    id: Uuid,
    source_id: Uuid,
    dataset_name: String,
    representation: String,
    level: String,
    status: String,
    recovery_time: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    indefinite: bool,
    created_at: DateTime<Utc>,
}

impl From<RecoveryPointRow> for MediaRecoveryPointRecord {
    fn from(row: RecoveryPointRow) -> Self {
        Self {
            id: row.id,
            source_id: row.source_id,
            dataset_name: row.dataset_name,
            representation: row.representation,
            level: row.level,
            status: row.status,
            recovery_time: row.recovery_time,
            expires_at: row.expires_at,
            indefinite: row.indefinite,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, FromRow)]
struct DependentRecoveryPointRow {
    id: Uuid,
    source_id: Uuid,
    dataset_name: String,
    representation: String,
    level: String,
    status: String,
    recovery_time: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    indefinite: bool,
    created_at: DateTime<Utc>,
    dependency_kind: String,
    depth: i32,
}

impl From<DependentRecoveryPointRow> for MediaDependentRecoveryPointRecord {
    fn from(row: DependentRecoveryPointRow) -> Self {
        Self {
            recovery_point: MediaRecoveryPointRecord {
                id: row.id,
                source_id: row.source_id,
                dataset_name: row.dataset_name,
                representation: row.representation,
                level: row.level,
                status: row.status,
                recovery_time: row.recovery_time,
                expires_at: row.expires_at,
                indefinite: row.indefinite,
                created_at: row.created_at,
            },
            dependency_kind: row.dependency_kind,
            depth: row.depth,
        }
    }
}

#[derive(Debug, FromRow)]
struct ReceiptRow {
    id: Uuid,
    cartridge_id: Uuid,
    operation_kind: String,
    request_fingerprint: String,
    resulting_state_version: i64,
    object_id: Option<Uuid>,
}

#[derive(Debug, FromRow)]
struct CartridgeLockRow {
    state_version: i64,
    pool_id: Option<Uuid>,
    reserved_by: Option<String>,
}

#[derive(Debug, FromRow)]
struct HoldLockRow {
    id: Uuid,
    cartridge_id: Option<Uuid>,
    kind: String,
    released_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct ReuseDriveRow {
    id: Uuid,
    stable_id: String,
    generation: Option<i16>,
    qualification_status: String,
    reconciled: bool,
    health: Value,
    last_observed_at: Option<DateTime<Utc>>,
    exact_qualification_current: bool,
}

const POOL_SELECT: &str = "SELECT p.id, p.name, p.purpose, p.indefinite_retention, p.details,
            count(c.id) AS cartridge_count, p.created_at, p.updated_at
     FROM media_pools p
     LEFT JOIN cartridges c ON c.pool_id = p.id";

// PostgreSQL's jsonb text form inserts separator whitespace that compact JSON
// does not contain.  Keeping the API ceiling below half of the database's
// 1 MiB defensive constraint guarantees ordinary nested JSON cannot pass API
// validation and then fail the CHECK merely because of that normalization.
const POOL_DETAILS_MAX_ENCODED_BYTES: usize = 500_000;

/// Lists all pools with a transactionally computed cartridge count.
pub async fn list_media_pools(pool: &PgPool) -> Result<Vec<MediaPoolRecord>, MediaOpsError> {
    let rows = sqlx::query_as::<_, MediaPoolRow>(&format!(
        "{POOL_SELECT}
         GROUP BY p.id
         ORDER BY lower(p.name), p.id"
    ))
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// Creates a pool and its audit/outbox records in one transaction.
pub async fn create_media_pool(
    pool: &PgPool,
    request: &CreateMediaPoolRequest,
) -> Result<MediaPoolRecord, MediaOpsError> {
    validate_actor(&request.actor)?;
    let name = validate_text("name", &request.name, 1, 256)?;
    validate_object("details", &request.details, POOL_DETAILS_MAX_ENCODED_BYTES)?;

    let mut transaction = pool.begin().await?;
    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM media_pools WHERE lower(name) = lower($1))",
    )
    .bind(&name)
    .fetch_one(&mut *transaction)
    .await?;
    if duplicate {
        return Err(MediaOpsError::Conflict {
            code: "pool_name_exists",
            message: format!("a media pool named {name:?} already exists"),
        });
    }

    // Archive is a protection policy, not a cosmetic label.  It is always
    // indefinite even if an older client omits the flag.
    let indefinite_retention =
        effective_pool_indefinite_retention(request.purpose, request.indefinite_retention);
    let id = Uuid::now_v7();
    let insert = sqlx::query(
        "INSERT INTO media_pools (
            id, name, purpose, indefinite_retention, details, updated_at
         ) VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(id)
    .bind(&name)
    .bind(request.purpose.as_str())
    .bind(indefinite_retention)
    .bind(&request.details)
    .execute(&mut *transaction)
    .await;
    if let Err(error) = insert {
        if is_unique_violation(&error) {
            return Err(MediaOpsError::Conflict {
                code: "pool_name_exists",
                message: format!("a media pool named {name:?} already exists"),
            });
        }
        return Err(error.into());
    }

    append_audit(
        &mut transaction,
        &request.actor,
        "media_pool.create",
        "media_pool",
        &id.to_string(),
        json!({
            "name": name,
            "purpose": request.purpose,
            "indefinite_retention": indefinite_retention,
        }),
        "created",
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("media-pool:{id}:created"),
        json!({
            "event": "media_pool_created",
            "media_pool_id": id,
            "name": name,
            "purpose": request.purpose,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;

    let row =
        sqlx::query_as::<_, MediaPoolRow>(&format!("{POOL_SELECT} WHERE p.id = $1 GROUP BY p.id"))
            .bind(id)
            .fetch_one(&mut *transaction)
            .await?;
    transaction.commit().await?;
    row.try_into()
}

// Recursive graph work is conservatively capped at five million candidate
// states/edge expansions.  Exceeding the cap marks affected media incomplete
// and therefore protected; it never turns missing traversal into eligibility.
const INVENTORY_SELECT: &str = r#"
WITH RECURSIVE direct_points AS (
    SELECT DISTINCT vr.cartridge_id, rp.id AS recovery_point_id,
           rp.status, rp.expires_at, rp.indefinite
    FROM volume_records vr
    JOIN recovery_points rp ON rp.id = vr.recovery_point_id
    WHERE ($1::uuid IS NULL OR vr.cartridge_id = $1)
), graph_sizes AS MATERIALIZED (
    SELECT
        (SELECT count(*) FROM (
            SELECT 1 FROM recovery_points LIMIT 50001
        ) bounded_nodes) AS node_count,
        (SELECT count(*) FROM (
            SELECT 1 FROM recovery_dependencies LIMIT 200001
        ) bounded_edges) AS edge_count,
        (SELECT count(*) FROM (
            SELECT DISTINCT cartridge_id FROM direct_points LIMIT 1001
        ) bounded_cartridges) AS seed_cartridge_count
), graph_guard AS MATERIALIZED (
    SELECT node_count <= 50000
       AND edge_count <= 200000
       AND seed_cartridge_count <= 1000
       AND (seed_cartridge_count = 0 OR
            node_count::numeric * seed_cartridge_count::numeric * 130
                <= 5000000)
       AND (seed_cartridge_count = 0 OR
            edge_count::numeric * seed_cartridge_count::numeric * 129
                <= 5000000) AS allowed
    FROM graph_sizes
), dependency_walk AS (
    SELECT direct.cartridge_id,
           direct.recovery_point_id AS point_id,
           0::integer AS depth
    FROM direct_points direct
    CROSS JOIN graph_guard guard
    WHERE guard.allowed
    UNION
    SELECT walk.cartridge_id,
           dependency.recovery_point_id AS point_id,
           walk.depth + 1
    FROM dependency_walk walk
    JOIN recovery_dependencies dependency
      ON dependency.depends_on_id = walk.point_id
    CROSS JOIN graph_guard guard
    WHERE guard.allowed AND walk.depth < 129
), reachable_dependents AS (
    SELECT cartridge_id, point_id AS dependent_id, min(depth) AS depth
    FROM dependency_walk
    WHERE depth BETWEEN 1 AND 128
    GROUP BY cartridge_id, point_id
), volume_stats AS (
    SELECT volume.cartridge_id,
           count(*) AS content_volume_count,
           count(*) FILTER (
               WHERE NOT volume.finalized
                  OR recovery.status NOT IN ('written', 'verified')
           ) AS unreconciled_content_records
    FROM volume_records volume
    JOIN recovery_points recovery ON recovery.id = volume.recovery_point_id
    WHERE ($1::uuid IS NULL OR volume.cartridge_id = $1)
    GROUP BY volume.cartridge_id
), direct_stats AS (
    SELECT cartridge_id,
           count(*) FILTER (
               WHERE status IN ('written', 'verified')
                 AND NOT indefinite
                 AND (expires_at IS NULL OR expires_at > now())
           ) AS retained_recovery_points,
           count(*) FILTER (
               WHERE status IN ('written', 'verified') AND indefinite
           ) AS indefinite_archive_claims
    FROM direct_points
    GROUP BY cartridge_id
), dependent_stats AS (
    SELECT walk.cartridge_id,
           count(DISTINCT walk.dependent_id) FILTER (
               WHERE rp.status IN ('written', 'verified')
                 AND (rp.indefinite OR rp.expires_at IS NULL OR rp.expires_at > now())
                 AND NOT EXISTS (
                     SELECT 1 FROM direct_points direct
                     WHERE direct.cartridge_id = walk.cartridge_id
                       AND direct.recovery_point_id = walk.dependent_id
                 )
           ) AS dependent_recovery_points,
           count(DISTINCT walk.dependent_id) FILTER (
               WHERE rp.status IN ('written', 'verified')
                 AND rp.indefinite
                 AND NOT EXISTS (
                     SELECT 1 FROM direct_points direct
                     WHERE direct.cartridge_id = walk.cartridge_id
                       AND direct.recovery_point_id = walk.dependent_id
                 )
           ) AS dependent_indefinite_archives
    FROM reachable_dependents walk
    JOIN recovery_points rp ON rp.id = walk.dependent_id
    GROUP BY walk.cartridge_id
), dependency_overflow AS (
    SELECT DISTINCT walk.cartridge_id
    FROM dependency_walk walk
    WHERE walk.depth = 129
), hold_links AS (
    SELECT h.cartridge_id, h.id
    FROM holds h
    WHERE h.cartridge_id IS NOT NULL
      AND ($1::uuid IS NULL OR h.cartridge_id = $1)
      AND h.released_at IS NULL
      AND (h.expires_at IS NULL OR h.expires_at > now())
    UNION
    SELECT direct.cartridge_id, h.id
    FROM direct_points direct
    JOIN holds h ON h.recovery_point_id = direct.recovery_point_id
    WHERE h.released_at IS NULL
      AND (h.expires_at IS NULL OR h.expires_at > now())
    UNION
    SELECT walk.cartridge_id, h.id
    FROM reachable_dependents walk
    JOIN holds h ON h.recovery_point_id = walk.dependent_id
    WHERE h.released_at IS NULL
      AND (h.expires_at IS NULL OR h.expires_at > now())
), hold_stats AS (
    SELECT cartridge_id, count(DISTINCT id) AS active_holds
    FROM hold_links
    GROUP BY cartridge_id
)
SELECT c.id, c.internal_label, c.barcode, c.media_generation, c.media_format,
       c.on_tape_identity, c.identity_status, c.health_status, c.worm,
       c.reserved_by, c.state_version,
       p.id AS pool_id, p.name AS pool_name, p.purpose AS pool_purpose,
       p.indefinite_retention AS pool_indefinite_retention,
       l.id AS location_id, l.name AS location_name, l.kind AS location_kind,
       COALESCE(holds.active_holds, 0) AS active_holds,
       COALESCE(volumes.content_volume_count, 0) AS content_volume_count,
       COALESCE(volumes.unreconciled_content_records, 0)
           AS unreconciled_content_records,
       COALESCE(direct.retained_recovery_points, 0) AS retained_recovery_points,
       COALESCE(dependent.dependent_recovery_points, 0) AS dependent_recovery_points,
       (COALESCE(direct.indefinite_archive_claims, 0)
        + COALESCE(dependent.dependent_indefinite_archives, 0))
           AS indefinite_archive_claims,
       ((NOT guard.allowed AND COALESCE(volumes.content_volume_count, 0) > 0)
        OR overflow.cartridge_id IS NOT NULL) AS dependency_graph_truncated,
       latest.id AS latest_verification_id,
       latest.recovery_point_id AS latest_verification_recovery_point_id,
       latest.kind AS latest_verification_kind,
       latest.outcome AS latest_verification_outcome,
       latest.drive_id AS latest_verification_drive_id,
       latest.drive_stable_id AS latest_verification_drive_stable_id,
       COALESCE(latest.finished_at, latest.started_at) AS latest_verification_at,
       c.created_at, c.updated_at
FROM cartridges c
CROSS JOIN graph_guard guard
LEFT JOIN media_pools p ON p.id = c.pool_id
LEFT JOIN locations l ON l.id = c.confirmed_location_id
LEFT JOIN volume_stats volumes ON volumes.cartridge_id = c.id
LEFT JOIN direct_stats direct ON direct.cartridge_id = c.id
LEFT JOIN dependent_stats dependent ON dependent.cartridge_id = c.id
LEFT JOIN dependency_overflow overflow ON overflow.cartridge_id = c.id
LEFT JOIN hold_stats holds ON holds.cartridge_id = c.id
LEFT JOIN LATERAL (
    SELECT verification.id, verification.recovery_point_id,
           verification.kind, verification.outcome, verification.drive_id,
           drive.stable_id AS drive_stable_id,
           verification.started_at, verification.finished_at
    FROM verification_records verification
    LEFT JOIN drives drive ON drive.id = verification.drive_id
    WHERE verification.media_ids @> ARRAY[c.id]::uuid[]
       OR EXISTS (
           SELECT 1 FROM volume_records volume
           WHERE volume.cartridge_id = c.id
             AND volume.recovery_point_id = verification.recovery_point_id
       )
    ORDER BY COALESCE(verification.finished_at, verification.started_at) DESC,
             verification.id DESC
    LIMIT 1
) latest ON true
WHERE ($1::uuid IS NULL OR c.id = $1)
"#;

/// Returns operator-facing media inventory without any internal artifact path.
pub async fn list_media_inventory(
    pool: &PgPool,
) -> Result<Vec<MediaInventorySummary>, MediaOpsError> {
    let rows = sqlx::query_as::<_, InventoryRow>(&format!(
        "{INVENTORY_SELECT} ORDER BY c.internal_label, c.id"
    ))
    .bind(Option::<Uuid>::None)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(TryInto::try_into).collect()
}

async fn get_inventory_in_connection(
    connection: &mut PgConnection,
    cartridge_id: Uuid,
) -> Result<MediaInventorySummary, MediaOpsError> {
    let row = sqlx::query_as::<_, InventoryRow>(INVENTORY_SELECT)
        .bind(Some(cartridge_id))
        .fetch_optional(connection)
        .await?
        .ok_or_else(|| MediaOpsError::NotFound {
            resource: "cartridge",
            id: cartridge_id.to_string(),
        })?;
    row.try_into()
}

/// Loads a bounded, repeatable-read media aggregate.  Evidence blobs and
/// artifact/storage paths are deliberately excluded from every record.
pub async fn get_media_detail(
    pool: &PgPool,
    cartridge_id: Uuid,
) -> Result<MediaDetailAggregate, MediaOpsError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await?;

    let cartridge = get_inventory_in_connection(&mut transaction, cartridge_id).await?;
    let dependency_graph_truncated = cartridge.dependency_graph_truncated;

    let mut volumes = sqlx::query_as::<_, MediaVolumeRecord>(
        "SELECT id, recovery_point_id, volume_order, first_record, last_record,
                encoded_bytes, finalized, engine_record_id
         FROM volume_records
         WHERE cartridge_id = $1
         ORDER BY recovery_point_id, volume_order, id
         LIMIT $2",
    )
    .bind(cartridge_id)
    .bind(fixed_limit(MEDIA_DETAIL_VOLUME_LIMIT))
    .fetch_all(&mut *transaction)
    .await?;
    let volumes_truncated = truncate_rows(&mut volumes, MEDIA_DETAIL_VOLUME_LIMIT);

    let mut recovery_rows = sqlx::query_as::<_, RecoveryPointRow>(
        "SELECT DISTINCT rp.id, rp.source_id, source.dataset_name,
                rp.representation, rp.level, rp.status, rp.recovery_time,
                rp.expires_at, rp.indefinite, rp.created_at
         FROM volume_records volume
         JOIN recovery_points rp ON rp.id = volume.recovery_point_id
         JOIN sources source ON source.id = rp.source_id
         WHERE volume.cartridge_id = $1
         ORDER BY rp.recovery_time DESC, rp.id
         LIMIT $2",
    )
    .bind(cartridge_id)
    .bind(fixed_limit(MEDIA_DETAIL_RECOVERY_LIMIT))
    .fetch_all(&mut *transaction)
    .await?;
    let recovery_points_truncated = truncate_rows(&mut recovery_rows, MEDIA_DETAIL_RECOVERY_LIMIT);
    let recovery_points = recovery_rows.into_iter().map(Into::into).collect();

    let mut dependent_rows = sqlx::query_as::<_, DependentRecoveryPointRow>(
        "WITH RECURSIVE direct_points AS (
             SELECT DISTINCT recovery_point_id
             FROM volume_records
             WHERE cartridge_id = $1
         ), dependency_walk(id, depth) AS (
             SELECT direct.recovery_point_id, 0::integer
             FROM direct_points direct
             WHERE NOT $3
             UNION
             SELECT dependency.recovery_point_id, walk.depth + 1
             FROM dependency_walk walk
             JOIN recovery_dependencies dependency ON dependency.depends_on_id = walk.id
             WHERE walk.depth < 129
         ), unique_walk AS (
             SELECT walk.id, min(walk.depth)::integer AS depth
             FROM dependency_walk walk
             WHERE walk.depth BETWEEN 1 AND 128
               AND NOT EXISTS (
                 SELECT 1 FROM direct_points direct
                 WHERE direct.recovery_point_id = walk.id
             )
             GROUP BY walk.id
         )
         SELECT rp.id, rp.source_id, source.dataset_name, rp.representation,
                rp.level, rp.status, rp.recovery_time, rp.expires_at,
                rp.indefinite, rp.created_at, relation.dependency_kind, walk.depth
         FROM unique_walk walk
         JOIN recovery_points rp ON rp.id = walk.id
         JOIN sources source ON source.id = rp.source_id
         JOIN LATERAL (
             SELECT min(dependency.dependency_kind) AS dependency_kind
             FROM dependency_walk parent
             JOIN recovery_dependencies dependency
               ON dependency.depends_on_id = parent.id
              AND dependency.recovery_point_id = walk.id
             WHERE parent.depth = walk.depth - 1
         ) relation ON relation.dependency_kind IS NOT NULL
         ORDER BY walk.depth, rp.recovery_time DESC, rp.id
         LIMIT $2",
    )
    .bind(cartridge_id)
    .bind(fixed_limit(MEDIA_DETAIL_DEPENDENCY_LIMIT))
    .bind(dependency_graph_truncated)
    .fetch_all(&mut *transaction)
    .await?;
    let dependent_recovery_points_truncated =
        truncate_rows(&mut dependent_rows, MEDIA_DETAIL_DEPENDENCY_LIMIT);
    let dependent_recovery_points = dependent_rows.into_iter().map(Into::into).collect();

    let mut holds = sqlx::query_as::<_, MediaHoldRecord>(
        "WITH RECURSIVE direct_points AS (
             SELECT DISTINCT recovery_point_id AS id
             FROM volume_records WHERE cartridge_id = $1
         ), related_points(id, depth) AS (
             SELECT direct.id, 0::integer
             FROM direct_points direct
             WHERE NOT $3
             UNION
             SELECT dependency.recovery_point_id, related.depth + 1
             FROM related_points related
             JOIN recovery_dependencies dependency ON dependency.depends_on_id = related.id
             WHERE related.depth < 129
         )
         SELECT h.id, h.cartridge_id, h.recovery_point_id, h.kind, h.reason,
                h.expires_at,
                (h.released_at IS NULL AND
                 (h.expires_at IS NULL OR h.expires_at > now())) AS active,
                h.created_by, h.created_by_label, h.created_correlation_id,
                h.created_at, h.released_at, h.released_by,
                h.released_by_label, h.release_reason, h.release_correlation_id
         FROM holds h
         WHERE h.cartridge_id = $1
            OR h.recovery_point_id IN (SELECT id FROM direct_points)
            OR h.recovery_point_id IN (
                SELECT id FROM related_points WHERE depth BETWEEN 1 AND 128
            )
         ORDER BY active DESC, h.created_at DESC, h.id
         LIMIT $2",
    )
    .bind(cartridge_id)
    .bind(fixed_limit(MEDIA_DETAIL_HOLD_LIMIT))
    .bind(dependency_graph_truncated)
    .fetch_all(&mut *transaction)
    .await?;
    let holds_truncated = truncate_rows(&mut holds, MEDIA_DETAIL_HOLD_LIMIT);

    let mut verifications = sqlx::query_as::<_, MediaVerificationRecord>(
        "SELECT verification.id, verification.recovery_point_id,
                verification.kind, verification.outcome, verification.drive_id,
                drive.stable_id AS drive_stable_id,
                verification.media_ids, verification.correlation_id,
                verification.started_at, verification.finished_at
         FROM verification_records verification
         LEFT JOIN drives drive ON drive.id = verification.drive_id
         WHERE verification.media_ids @> ARRAY[$1]::uuid[]
            OR verification.recovery_point_id IN (
                SELECT recovery_point_id FROM volume_records WHERE cartridge_id = $1
            )
         ORDER BY COALESCE(verification.finished_at, verification.started_at) DESC,
                  verification.id DESC
         LIMIT $2",
    )
    .bind(cartridge_id)
    .bind(fixed_limit(MEDIA_DETAIL_VERIFICATION_LIMIT))
    .fetch_all(&mut *transaction)
    .await?;
    let verifications_truncated =
        truncate_rows(&mut verifications, MEDIA_DETAIL_VERIFICATION_LIMIT);

    let mut custody_events = sqlx::query_as::<_, MediaCustodyEventRecord>(
        "SELECT event.id, event.movement_task_id,
                event.movement_task_external_id, event.event_type,
                event.actor_id, event.actor_label, event.source_location_id,
                source.name AS source_location_name,
                event.destination_location_id,
                destination.name AS destination_location_name,
                event.destination_label, event.note, event.corrects_event_id,
                event.occurred_at, event.correlation_id
         FROM custody_events event
         LEFT JOIN locations source ON source.id = event.source_location_id
         LEFT JOIN locations destination ON destination.id = event.destination_location_id
         WHERE event.cartridge_id = $1
         ORDER BY event.occurred_at DESC, event.id DESC
         LIMIT $2",
    )
    .bind(cartridge_id)
    .bind(fixed_limit(MEDIA_DETAIL_CUSTODY_LIMIT))
    .fetch_all(&mut *transaction)
    .await?;
    let custody_events_truncated = truncate_rows(&mut custody_events, MEDIA_DETAIL_CUSTODY_LIMIT);

    let mut operations = sqlx::query_as::<_, MediaOperationRecord>(
        "SELECT id, operation_kind, expected_state_version,
                resulting_state_version, object_id, actor_id, actor_label,
                correlation_id, occurred_at
         FROM media_operations
         WHERE cartridge_id = $1
         ORDER BY occurred_at DESC, id DESC
         LIMIT $2",
    )
    .bind(cartridge_id)
    .bind(fixed_limit(MEDIA_DETAIL_OPERATION_LIMIT))
    .fetch_all(&mut *transaction)
    .await?;
    let operations_truncated = truncate_rows(&mut operations, MEDIA_DETAIL_OPERATION_LIMIT);

    let mut audit_events = sqlx::query_as::<_, MediaAuditRecord>(
        "SELECT id, actor_id, actor_label, action, object_type, object_id,
                result, correlation_id, occurred_at
         FROM audit_events
         WHERE object_type = 'cartridge' AND object_id = $1
         ORDER BY occurred_at DESC, id DESC
         LIMIT $2",
    )
    .bind(cartridge_id.to_string())
    .bind(fixed_limit(MEDIA_DETAIL_AUDIT_LIMIT))
    .fetch_all(&mut *transaction)
    .await?;
    let audit_events_truncated = truncate_rows(&mut audit_events, MEDIA_DETAIL_AUDIT_LIMIT);

    transaction.commit().await?;
    Ok(MediaDetailAggregate {
        cartridge,
        volumes,
        recovery_points,
        dependent_recovery_points,
        holds,
        verifications,
        custody_events,
        operations,
        audit_events,
        truncation: MediaDetailTruncation {
            volumes: volumes_truncated,
            recovery_points: recovery_points_truncated,
            dependent_recovery_points: dependent_recovery_points_truncated
                || dependency_graph_truncated,
            holds: holds_truncated || dependency_graph_truncated,
            verifications: verifications_truncated,
            custody_events: custody_events_truncated,
            operations: operations_truncated,
            audit_events: audit_events_truncated,
        },
    })
}

#[derive(Debug, FromRow)]
struct PoolSafetyRow {
    recorded_content: bool,
    live_protection: bool,
    archive_protection: bool,
    dependency_graph_truncated: bool,
}

/// Assigns (or clears) a pool with optimistic locking and retry-safe receipts.
/// The assignment never mutates tape content.
pub async fn assign_media_pool(
    pool: &PgPool,
    cartridge_id: Uuid,
    request: &AssignMediaPoolRequest,
) -> Result<MediaMutationResult, MediaOpsError> {
    validate_mutation_common(
        request.expected_state_version,
        &request.idempotency_key,
        &request.actor,
    )?;
    let idempotency_key = request.idempotency_key.trim();
    let request_fingerprint = assignment_request_fingerprint(cartridge_id, request)?;

    let mut transaction = pool.begin().await?;
    lock_idempotency_key(&mut transaction, idempotency_key).await?;
    if let Some(receipt) = find_receipt(&mut transaction, idempotency_key).await? {
        ensure_receipt_match(
            &receipt,
            cartridge_id,
            "pool_assigned",
            &request_fingerprint,
            idempotency_key,
        )?;
        transaction.commit().await?;
        return Ok(result_from_receipt(receipt, true));
    }

    let cartridge = lock_cartridge(&mut transaction, cartridge_id).await?;
    ensure_expected_version(request.expected_state_version, cartridge.state_version)?;
    if let Some(reserved_by) = &cartridge.reserved_by {
        return Err(MediaOpsError::Conflict {
            code: "cartridge_reserved",
            message: format!("cartridge is reserved by {reserved_by}"),
        });
    }

    let target_pool = if let Some(pool_id) = request.pool_id {
        Some(
            sqlx::query_as::<_, (String, String, bool)>(
                "SELECT name, purpose, indefinite_retention
                 FROM media_pools WHERE id = $1",
            )
            .bind(pool_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| MediaOpsError::NotFound {
                resource: "media_pool",
                id: pool_id.to_string(),
            })?,
        )
    } else {
        None
    };

    let safety = sqlx::query_as::<_, PoolSafetyRow>(
        "WITH RECURSIVE direct_points AS (
             SELECT DISTINCT rp.id, rp.status, rp.expires_at, rp.indefinite
             FROM volume_records volume
             JOIN recovery_points rp ON rp.id = volume.recovery_point_id
             WHERE volume.cartridge_id = $1
         ), graph_sizes AS MATERIALIZED (
             SELECT
                 (SELECT count(*) FROM (
                     SELECT 1 FROM recovery_points LIMIT 50001
                 ) bounded_nodes) AS node_count,
                 (SELECT count(*) FROM (
                     SELECT 1 FROM recovery_dependencies LIMIT 200001
                 ) bounded_edges) AS edge_count
         ), graph_guard AS MATERIALIZED (
             SELECT node_count <= 50000
                AND edge_count <= 200000
                AND node_count::numeric * 130 <= 5000000
                AND edge_count::numeric * 129 <= 5000000 AS allowed
             FROM graph_sizes
         ), dependency_walk(id, depth) AS (
             SELECT direct.id, 0::integer
             FROM direct_points direct
             CROSS JOIN graph_guard guard
             WHERE guard.allowed
             UNION
             SELECT dependency.recovery_point_id, walk.depth + 1
             FROM dependency_walk walk
             JOIN recovery_dependencies dependency ON dependency.depends_on_id = walk.id
             CROSS JOIN graph_guard guard
             WHERE guard.allowed AND walk.depth < 129
         ), reachable_dependents AS (
             SELECT id, min(depth)::integer AS depth
             FROM dependency_walk
             WHERE depth BETWEEN 1 AND 128
             GROUP BY id
         ), graph_overflow AS (
             SELECT ((NOT guard.allowed AND EXISTS(SELECT 1 FROM direct_points))
                     OR EXISTS(
                         SELECT 1 FROM dependency_walk WHERE depth = 129
                     )) AS truncated
             FROM graph_guard guard
         ), direct_holds AS (
             SELECT h.kind
             FROM holds h
             WHERE (h.cartridge_id = $1
                    OR h.recovery_point_id IN (SELECT id FROM direct_points)
                    OR h.recovery_point_id IN (SELECT id FROM reachable_dependents))
               AND h.released_at IS NULL
               AND (h.expires_at IS NULL OR h.expires_at > now())
         )
         SELECT
             EXISTS(
                 SELECT 1 FROM volume_records
                 WHERE cartridge_id = $1
             ) AS recorded_content,
             (EXISTS(SELECT 1 FROM direct_holds)
              OR EXISTS(
                  SELECT 1 FROM direct_points
                  WHERE status IN ('written', 'verified')
                    AND (indefinite OR expires_at IS NULL OR expires_at > now())
              )
              OR EXISTS(
                  SELECT 1
                  FROM reachable_dependents walk
                  JOIN recovery_points dependent ON dependent.id = walk.id
                  WHERE dependent.status IN ('written', 'verified')
                    AND (dependent.indefinite OR dependent.expires_at IS NULL
                         OR dependent.expires_at > now())
              )
              OR (SELECT truncated FROM graph_overflow)) AS live_protection,
             (EXISTS(SELECT 1 FROM direct_holds WHERE kind = 'archive')
              OR EXISTS(
                  SELECT 1 FROM direct_points
                  WHERE status IN ('written', 'verified') AND indefinite
              )
              OR EXISTS(
                  SELECT 1 FROM reachable_dependents walk
                  JOIN recovery_points dependent ON dependent.id = walk.id
                  WHERE dependent.status IN ('written', 'verified')
                    AND dependent.indefinite
              )
              OR EXISTS(
                  SELECT 1 FROM media_pools current_pool
                  WHERE current_pool.id = $2
                    AND (current_pool.purpose = 'archive'
                         OR current_pool.indefinite_retention)
                    AND EXISTS(
                        SELECT 1 FROM volume_records protected_volume
                        WHERE protected_volume.cartridge_id = $1
                    )
              )) AS archive_protection,
             (SELECT truncated FROM graph_overflow) AS dependency_graph_truncated",
    )
    .bind(cartridge_id)
    .bind(cartridge.pool_id)
    .fetch_one(&mut *transaction)
    .await?;

    if safety.dependency_graph_truncated {
        return Err(MediaOpsError::Conflict {
            code: "dependency_graph_truncated",
            message: "the recovery dependency graph exceeds the bounded media-policy traversal; pool assignment is blocked"
                .to_owned(),
        });
    }

    if let Some((target_name, target_purpose, _)) = &target_pool {
        if matches!(target_purpose.as_str(), "scratch" | "cleaning")
            && (safety.recorded_content || safety.live_protection)
        {
            return Err(MediaOpsError::Conflict {
                code: "protected_media_pool_assignment",
                message: format!("pool {target_name:?} cannot receive recorded or protected media"),
            });
        }
        if target_purpose == "routine" && safety.archive_protection {
            return Err(MediaOpsError::Conflict {
                code: "archive_protection_transition",
                message: "archive-protected media cannot be moved into a reusable routine pool"
                    .to_owned(),
            });
        }
    } else if safety.archive_protection {
        return Err(MediaOpsError::Conflict {
            code: "archive_protection_transition",
            message: "archive-protected media cannot be detached from its protected pool"
                .to_owned(),
        });
    }

    let resulting_state_version = sqlx::query_scalar::<_, i64>(
        "UPDATE cartridges
         SET pool_id = $2, updated_at = now()
         WHERE id = $1
         RETURNING state_version",
    )
    .bind(cartridge_id)
    .bind(request.pool_id)
    .fetch_one(&mut *transaction)
    .await?;
    ensure_single_version_advance(cartridge.state_version, resulting_state_version)?;

    let operation_id = Uuid::now_v7();
    let result = json!({
        "cartridge_id": cartridge_id,
        "pool_id": request.pool_id,
        "state_version": resulting_state_version,
    });
    insert_receipt(
        &mut transaction,
        operation_id,
        cartridge_id,
        "pool_assigned",
        idempotency_key,
        &request_fingerprint,
        request.expected_state_version,
        resulting_state_version,
        request.pool_id,
        &request.actor,
        request.correlation_id,
        result.clone(),
    )
    .await?;
    append_audit(
        &mut transaction,
        &request.actor,
        "media.pool_assign",
        "cartridge",
        &cartridge_id.to_string(),
        json!({
            "pool_id": request.pool_id,
            "expected_state_version": request.expected_state_version,
        }),
        "assigned",
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("media-operation:{operation_id}"),
        json!({
            "event": "media_pool_assigned",
            "operation_id": operation_id,
            "cartridge_id": cartridge_id,
            "pool_id": request.pool_id,
            "state_version": resulting_state_version,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;
    transaction.commit().await?;

    Ok(MediaMutationResult {
        cartridge_id,
        operation_id,
        object_id: request.pool_id,
        state_version: resulting_state_version,
        idempotent_replay: false,
    })
}

/// Adds one of the three operator-managed hold kinds.  System-generated holds
/// remain outside this API so callers cannot forge dependency/job state.
pub async fn add_media_hold(
    pool: &PgPool,
    cartridge_id: Uuid,
    request: &CreateMediaHoldRequest,
) -> Result<MediaMutationResult, MediaOpsError> {
    validate_mutation_common(
        request.expected_state_version,
        &request.idempotency_key,
        &request.actor,
    )?;
    let reason = validate_text("reason", &request.reason, 1, 4096)?;
    validate_hold_expiration(request.kind, request.expires_at, Utc::now())?;
    let idempotency_key = request.idempotency_key.trim();
    let request_fingerprint = create_hold_request_fingerprint(cartridge_id, request, &reason)?;

    let mut transaction = pool.begin().await?;
    lock_idempotency_key(&mut transaction, idempotency_key).await?;
    if let Some(receipt) = find_receipt(&mut transaction, idempotency_key).await? {
        ensure_receipt_match(
            &receipt,
            cartridge_id,
            "hold_created",
            &request_fingerprint,
            idempotency_key,
        )?;
        transaction.commit().await?;
        return Ok(result_from_receipt(receipt, true));
    }
    let cartridge = lock_cartridge(&mut transaction, cartridge_id).await?;
    ensure_expected_version(request.expected_state_version, cartridge.state_version)?;

    let hold_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO holds (
            id, cartridge_id, kind, reason, expires_at, created_by,
            created_by_label, created_correlation_id
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(hold_id)
    .bind(cartridge_id)
    .bind(request.kind.as_str())
    .bind(&reason)
    .bind(request.expires_at)
    .bind(request.actor.user_id)
    .bind(request.actor.label.trim())
    .bind(request.correlation_id)
    .execute(&mut *transaction)
    .await?;
    let resulting_state_version = touch_cartridge(&mut transaction, cartridge_id).await?;
    ensure_single_version_advance(cartridge.state_version, resulting_state_version)?;

    let operation_id = Uuid::now_v7();
    insert_receipt(
        &mut transaction,
        operation_id,
        cartridge_id,
        "hold_created",
        idempotency_key,
        &request_fingerprint,
        request.expected_state_version,
        resulting_state_version,
        Some(hold_id),
        &request.actor,
        request.correlation_id,
        json!({
            "cartridge_id": cartridge_id,
            "hold_id": hold_id,
            "kind": request.kind,
            "state_version": resulting_state_version,
        }),
    )
    .await?;
    append_audit(
        &mut transaction,
        &request.actor,
        "media.hold_create",
        "cartridge",
        &cartridge_id.to_string(),
        json!({
            "hold_id": hold_id,
            "kind": request.kind,
            "expires_at": request.expires_at,
            "expected_state_version": request.expected_state_version,
        }),
        "created",
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("media-operation:{operation_id}"),
        json!({
            "event": "media_hold_created",
            "operation_id": operation_id,
            "cartridge_id": cartridge_id,
            "hold_id": hold_id,
            "kind": request.kind,
            "state_version": resulting_state_version,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(MediaMutationResult {
        cartridge_id,
        operation_id,
        object_id: Some(hold_id),
        state_version: resulting_state_version,
        idempotent_replay: false,
    })
}

/// Explicitly releases an operator-managed hold, retaining the release actor
/// and reason.  A released hold is never deleted.
pub async fn release_media_hold(
    pool: &PgPool,
    hold_id: Uuid,
    request: &ReleaseMediaHoldRequest,
) -> Result<MediaMutationResult, MediaOpsError> {
    validate_mutation_common(
        request.expected_state_version,
        &request.idempotency_key,
        &request.actor,
    )?;
    let reason = validate_text("reason", &request.reason, 1, 4096)?;
    let idempotency_key = request.idempotency_key.trim();
    let request_fingerprint = release_hold_request_fingerprint(hold_id, request, &reason)?;

    let mut transaction = pool.begin().await?;
    let initial_hold = sqlx::query_as::<_, HoldLockRow>(
        "SELECT id, cartridge_id, kind, released_at FROM holds WHERE id = $1",
    )
    .bind(hold_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| MediaOpsError::NotFound {
        resource: "hold",
        id: hold_id.to_string(),
    })?;
    let cartridge_id = initial_hold
        .cartridge_id
        .ok_or_else(|| MediaOpsError::Conflict {
            code: "recovery_point_hold",
            message: "this endpoint only releases cartridge holds".to_owned(),
        })?;

    lock_idempotency_key(&mut transaction, idempotency_key).await?;
    if let Some(receipt) = find_receipt(&mut transaction, idempotency_key).await? {
        ensure_receipt_match(
            &receipt,
            cartridge_id,
            "hold_released",
            &request_fingerprint,
            idempotency_key,
        )?;
        transaction.commit().await?;
        return Ok(result_from_receipt(receipt, true));
    }

    let cartridge = lock_cartridge(&mut transaction, cartridge_id).await?;
    ensure_expected_version(request.expected_state_version, cartridge.state_version)?;
    let hold = sqlx::query_as::<_, HoldLockRow>(
        "SELECT id, cartridge_id, kind, released_at
         FROM holds WHERE id = $1 FOR UPDATE",
    )
    .bind(hold_id)
    .fetch_one(&mut *transaction)
    .await?;
    if hold.id != hold_id || hold.cartridge_id != Some(cartridge_id) {
        return Err(MediaOpsError::Invariant(
            "hold target changed while locked".to_owned(),
        ));
    }
    if !matches!(hold.kind.as_str(), "manual" | "legal" | "archive") {
        return Err(MediaOpsError::Conflict {
            code: "system_hold_not_releasable",
            message: format!(
                "{} holds are derived from system state and cannot be manually released",
                hold.kind
            ),
        });
    }
    if hold.released_at.is_some() {
        return Err(MediaOpsError::Conflict {
            code: "hold_already_released",
            message: "the hold has already been released under a different operation".to_owned(),
        });
    }

    sqlx::query(
        "UPDATE holds
         SET released_at = now(), released_by = $2, released_by_label = $3,
             release_reason = $4, release_correlation_id = $5
         WHERE id = $1",
    )
    .bind(hold_id)
    .bind(request.actor.user_id)
    .bind(request.actor.label.trim())
    .bind(&reason)
    .bind(request.correlation_id)
    .execute(&mut *transaction)
    .await?;
    let resulting_state_version = touch_cartridge(&mut transaction, cartridge_id).await?;
    ensure_single_version_advance(cartridge.state_version, resulting_state_version)?;

    let operation_id = Uuid::now_v7();
    insert_receipt(
        &mut transaction,
        operation_id,
        cartridge_id,
        "hold_released",
        idempotency_key,
        &request_fingerprint,
        request.expected_state_version,
        resulting_state_version,
        Some(hold_id),
        &request.actor,
        request.correlation_id,
        json!({
            "cartridge_id": cartridge_id,
            "hold_id": hold_id,
            "state_version": resulting_state_version,
        }),
    )
    .await?;
    append_audit(
        &mut transaction,
        &request.actor,
        "media.hold_release",
        "cartridge",
        &cartridge_id.to_string(),
        json!({
            "hold_id": hold_id,
            "reason": reason,
            "expected_state_version": request.expected_state_version,
        }),
        "released",
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("media-operation:{operation_id}"),
        json!({
            "event": "media_hold_released",
            "operation_id": operation_id,
            "cartridge_id": cartridge_id,
            "hold_id": hold_id,
            "state_version": resulting_state_version,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(MediaMutationResult {
        cartridge_id,
        operation_id,
        object_id: Some(hold_id),
        state_version: resulting_state_version,
        idempotent_replay: false,
    })
}

#[derive(Debug, FromRow)]
struct AffectedRecoveryPointRow {
    id: Uuid,
    dataset_name: String,
    status: String,
    recovery_time: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    indefinite: bool,
    relation: String,
    dependency_depth: Option<i32>,
}

impl From<AffectedRecoveryPointRow> for AffectedRecoveryPoint {
    fn from(row: AffectedRecoveryPointRow) -> Self {
        Self {
            id: row.id,
            dataset_name: row.dataset_name,
            status: row.status,
            recovery_time: row.recovery_time,
            expires_at: row.expires_at,
            indefinite: row.indefinite,
            relation: row.relation,
            dependency_depth: row.dependency_depth,
        }
    }
}

/// Evaluates automatic reuse against one selected drive.  Even an `allowed`
/// result is informational; no destructive operation is exposed.
pub async fn evaluate_media_reuse(
    pool: &PgPool,
    cartridge_id: Uuid,
    drive_id: Uuid,
) -> Result<MediaReuseEvaluation, MediaOpsError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let cartridge = get_inventory_in_connection(&mut transaction, cartridge_id).await?;

    let drive = sqlx::query_as::<_, ReuseDriveRow>(
        "SELECT drive.id, drive.stable_id, drive.generation,
                drive.qualification_status, drive.reconciled, drive.health,
                drive.last_observed_at,
                COALESCE((
                    SELECT qualification.state = 'passed'
                    FROM qualification_runs qualification
                    JOIN cartridges qualified_cartridge
                      ON qualified_cartridge.id = qualification.cartridge_id
                    WHERE qualification.drive_id = drive.id
                      AND qualification.cartridge_id = $2
                      AND qualification.state IN ('passed', 'failed', 'interrupted')
                      AND qualification.expected_stable_id = drive.stable_id
                      AND qualification.expected_path
                          IS NOT DISTINCT FROM drive.observed_path
                      AND qualification.expected_vendor
                          IS NOT DISTINCT FROM drive.vendor
                      AND qualification.expected_model
                          IS NOT DISTINCT FROM drive.model
                      AND qualification.expected_serial
                          IS NOT DISTINCT FROM drive.serial
                      AND qualification.expected_wwn
                          IS NOT DISTINCT FROM drive.wwn
                      AND qualification.expected_firmware
                          IS NOT DISTINCT FROM drive.firmware
                      AND qualification.expected_cartridge_label
                          = qualified_cartridge.internal_label
                      AND qualification.expected_media_generation
                          IS NOT DISTINCT FROM qualified_cartridge.media_generation
                      AND qualification.expected_media_format
                          IS NOT DISTINCT FROM qualified_cartridge.media_format
                      AND qualification.engine_name = 'Bacula Community'
                      AND qualification.engine_version = '15.0.3'
                      AND qualification.test_kind = 'btape_test'
                      AND qualification.attempt > 0
                    ORDER BY qualification.created_at DESC, qualification.id DESC
                    LIMIT 1
                ), false) AS exact_qualification_current
         FROM drives drive
         WHERE drive.id = $1",
    )
    .bind(drive_id)
    .bind(cartridge_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| MediaOpsError::NotFound {
        resource: "drive",
        id: drive_id.to_string(),
    })?;

    let mut affected_rows = sqlx::query_as::<_, AffectedRecoveryPointRow>(
        "WITH RECURSIVE direct_points AS (
             SELECT DISTINCT recovery_point_id AS id
             FROM volume_records
             WHERE cartridge_id = $1
         ), dependent_walk(id, depth) AS (
             SELECT direct.id, 0::integer
             FROM direct_points direct
             UNION
             SELECT dependency.recovery_point_id, walk.depth + 1
             FROM dependent_walk walk
             JOIN recovery_dependencies dependency ON dependency.depends_on_id = walk.id
             WHERE NOT $2 AND walk.depth < 129
         ), unique_dependents AS (
             SELECT walk.id, min(walk.depth)::integer AS depth
             FROM dependent_walk walk
             WHERE walk.depth BETWEEN 1 AND 128
               AND NOT EXISTS (SELECT 1 FROM direct_points direct WHERE direct.id = walk.id)
             GROUP BY walk.id
         ), affected AS (
             SELECT direct.id, 'direct'::text AS relation, NULL::integer AS depth
             FROM direct_points direct
             UNION ALL
             SELECT dependent.id, 'dependent'::text, dependent.depth
             FROM unique_dependents dependent
         )
         SELECT rp.id, source.dataset_name, rp.status, rp.recovery_time,
                rp.expires_at, rp.indefinite, affected.relation,
                affected.depth AS dependency_depth
         FROM affected
         JOIN recovery_points rp ON rp.id = affected.id
         JOIN sources source ON source.id = rp.source_id
         ORDER BY affected.relation, affected.depth NULLS FIRST,
                  rp.recovery_time DESC, rp.id
         LIMIT 513",
    )
    .bind(cartridge_id)
    .bind(cartridge.dependency_graph_truncated)
    .fetch_all(&mut *transaction)
    .await?;
    let affected_truncated = truncate_rows(&mut affected_rows, 512);
    let affected_recovery_points = affected_rows.into_iter().map(Into::into).collect();

    transaction.commit().await?;

    let observation_current = drive
        .last_observed_at
        .is_some_and(|observed| observed >= Utc::now() - chrono::Duration::minutes(2));
    let motion_safe = drive_health_true(&drive.health, "present")
        && drive_health_true(&drive.health, "accessible")
        && drive_health_true(&drive.health, "identity_verified")
        && !drive_health_true(&drive.health, "identity_conflict")
        && !drive_health_true(&drive.health, "motion_position_uncertain")
        && drive_health_false(&drive.health, "immediate_mode")
        && drive_health_i64(&drive.health, "in_flight") == Some(0);
    let selected_drive = MediaReuseDrive {
        id: drive.id,
        stable_id: drive.stable_id,
        generation: drive.generation,
        qualification_status: drive.qualification_status.clone(),
        reconciled: drive.reconciled,
        observation_current,
        motion_safe,
        exact_qualification_current: drive.exact_qualification_current,
        last_observed_at: drive.last_observed_at,
    };

    let parsed_drive_generation = drive.generation.and_then(drive_generation);
    // As with an unknown media format, use a placeholder solely to collect
    // every shared-domain blocker, then add an explicit fail-closed blocker.
    let domain_drive_generation = parsed_drive_generation.unwrap_or(DriveGeneration::Lto10);
    let parsed_media_format = media_format(
        cartridge.media_generation,
        cartridge.media_format.as_deref(),
    );
    let generation_format_consistent = parsed_media_format
        .is_some_and(|format| Some(media_format_generation(format)) == cartridge.media_generation);
    // A placeholder is needed to run the shared evaluator and collect all
    // other blockers.  The explicit unsupported-format blocker keeps this
    // fail-closed and the final result cannot become allowed.
    let domain_media_format = parsed_media_format.unwrap_or(MediaFormat::Lto10);
    let policy_allows_reuse = cartridge.pool.as_ref().is_some_and(|pool| {
        !pool.indefinite_retention
            && matches!(
                pool.purpose,
                MediaPoolPurpose::Routine | MediaPoolPurpose::Scratch
            )
    });
    let physical_state_reconciled = cartridge
        .confirmed_location
        .as_ref()
        .is_some_and(|location| {
            !matches!(
                location.kind,
                MediaLocationKind::Unknown | MediaLocationKind::InTransit
            )
        });
    let context = OverwriteContext {
        identity: cartridge.identity_status.domain(),
        physical_state_reconciled,
        engine_state_reconciled: cartridge
            .on_tape_identity
            .as_deref()
            .is_some_and(|identity| !identity.trim().is_empty())
            && cartridge.catalog_reconciled,
        health: media_health(&cartridge.health_status),
        is_worm: cartridge.worm,
        reservation: reservation(cartridge.reserved_by.as_deref()),
        protection: ProtectionClaims {
            retained_recovery_points: usize_count(cartridge.retained_recovery_points),
            dependent_recovery_points: usize_count(cartridge.dependent_recovery_points),
            explicit_holds: usize_count(cartridge.active_holds),
            indefinite_archives: usize_count(cartridge.indefinite_archive_claims),
        },
        policy_allows_reuse,
        drive: domain_drive_generation,
        media_format: domain_media_format,
        intent: OverwriteIntent::AutomaticReuse,
    };
    let decision = evaluate_overwrite(&context);
    let mut blockers = decision
        .blockers
        .into_iter()
        .map(domain_blocker)
        .collect::<Vec<_>>();

    if parsed_media_format.is_none() {
        blockers.push(blocker(
            "unsupported_media_format",
            "The cartridge generation/format is missing or unsupported.",
        ));
    }
    if parsed_media_format.is_some() && !generation_format_consistent {
        blockers.push(blocker(
            "media_generation_format_mismatch",
            "The cartridge generation and recorded media format disagree.",
        ));
    }
    if !cartridge.catalog_reconciled {
        blockers.push(blocker(
            "incomplete_catalog_records",
            "Unfinalized volume records or incomplete/failed recovery points still reference this cartridge.",
        ));
    }
    if parsed_drive_generation.is_none() {
        blockers.push(blocker(
            "drive_generation_unknown",
            "The selected drive generation is missing or unsupported.",
        ));
    }
    if !drive.reconciled {
        blockers.push(blocker(
            "drive_not_reconciled",
            "The selected drive identity has not been reconciled.",
        ));
    }
    if !observation_current {
        blockers.push(blocker(
            "drive_observation_stale",
            "The selected drive has not produced a current observation.",
        ));
    }
    if !motion_safe {
        blockers.push(blocker(
            "drive_not_motion_safe",
            "The selected drive is absent, inaccessible, busy, or position-uncertain.",
        ));
    }
    if !drive.exact_qualification_current {
        blockers.push(blocker(
            "exact_drive_media_qualification_missing",
            "No current passing physical qualification matches this exact drive and cartridge tuple.",
        ));
    }
    if affected_truncated {
        blockers.push(blocker(
            "affected_recovery_points_truncated",
            "More than 512 affected recovery points exist; the preview is incomplete.",
        ));
    }
    if cartridge.dependency_graph_truncated {
        blockers.push(blocker(
            "dependency_graph_traversal_incomplete",
            "The recovery dependency graph exceeds the bounded depth or catalog safety budget.",
        ));
    }
    blockers.sort_by(|left, right| left.code.cmp(&right.code));
    blockers.dedup_by(|left, right| left.code == right.code);
    let allowed = decision.allowed && blockers.is_empty();

    Ok(MediaReuseEvaluation {
        cartridge_id,
        state_version: cartridge.state_version,
        selected_drive,
        allowed,
        blockers,
        affected_recovery_points,
        destructive_operation_available: false,
    })
}

fn validate_actor(actor: &MediaActor) -> Result<(), MediaOpsError> {
    validate_text("actor.label", &actor.label, 1, 256).map(|_| ())
}

fn validate_mutation_common(
    expected_state_version: i64,
    idempotency_key: &str,
    actor: &MediaActor,
) -> Result<(), MediaOpsError> {
    if expected_state_version < 0 {
        return Err(MediaOpsError::Invalid {
            field: "expected_state_version",
            message: "must be zero or greater".to_owned(),
        });
    }
    validate_text("idempotency_key", idempotency_key, 1, 256)?;
    validate_actor(actor)
}

fn validate_hold_expiration(
    kind: MediaHoldKind,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(), MediaOpsError> {
    if expires_at.is_some_and(|expires_at| expires_at <= now) {
        return Err(MediaOpsError::Invalid {
            field: "expires_at",
            message: "must be in the future".to_owned(),
        });
    }
    if expires_at.is_some() && matches!(kind, MediaHoldKind::Legal | MediaHoldKind::Archive) {
        return Err(MediaOpsError::Invalid {
            field: "expires_at",
            message: "legal and archive holds require an explicit release".to_owned(),
        });
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    input: &str,
    minimum: usize,
    maximum: usize,
) -> Result<String, MediaOpsError> {
    let value = input.trim();
    // The matching PostgreSQL constraints use octet_length. Validate encoded
    // UTF-8 bytes here as well so multibyte input produces a useful 4xx error
    // instead of reaching the database and surfacing as an internal error.
    let length = value.len();
    if length < minimum || length > maximum || value.chars().any(char::is_control) {
        return Err(MediaOpsError::Invalid {
            field,
            message: format!(
                "must contain {minimum} to {maximum} printable UTF-8 bytes after trimming"
            ),
        });
    }
    Ok(value.to_owned())
}

fn validate_object(
    field: &'static str,
    value: &Value,
    maximum_bytes: usize,
) -> Result<(), MediaOpsError> {
    if !value.is_object() {
        return Err(MediaOpsError::Invalid {
            field,
            message: "must be a JSON object".to_owned(),
        });
    }
    let size = serde_json::to_vec(value)
        .map_err(|error| MediaOpsError::Invariant(format!("could not encode JSON: {error}")))?
        .len();
    if size > maximum_bytes {
        return Err(MediaOpsError::Invalid {
            field,
            message: format!("must be at most {maximum_bytes} encoded bytes"),
        });
    }
    Ok(())
}

fn fingerprint(value: &Value) -> Result<String, MediaOpsError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        MediaOpsError::Invariant(format!("could not encode request fingerprint: {error}"))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

const fn effective_pool_indefinite_retention(purpose: MediaPoolPurpose, requested: bool) -> bool {
    requested || matches!(purpose, MediaPoolPurpose::Archive)
}

fn assignment_request_fingerprint(
    cartridge_id: Uuid,
    request: &AssignMediaPoolRequest,
) -> Result<String, MediaOpsError> {
    fingerprint(&json!({
        "operation": "pool_assigned",
        "cartridge_id": cartridge_id,
        "pool_id": request.pool_id,
        "expected_state_version": request.expected_state_version,
        "actor": request.actor,
    }))
}

fn create_hold_request_fingerprint(
    cartridge_id: Uuid,
    request: &CreateMediaHoldRequest,
    normalized_reason: &str,
) -> Result<String, MediaOpsError> {
    fingerprint(&json!({
        "operation": "hold_created",
        "cartridge_id": cartridge_id,
        "kind": request.kind,
        "reason": normalized_reason,
        "expires_at": request.expires_at,
        "expected_state_version": request.expected_state_version,
        "actor": request.actor,
    }))
}

fn release_hold_request_fingerprint(
    hold_id: Uuid,
    request: &ReleaseMediaHoldRequest,
    normalized_reason: &str,
) -> Result<String, MediaOpsError> {
    fingerprint(&json!({
        "operation": "hold_released",
        "hold_id": hold_id,
        "reason": normalized_reason,
        "expected_state_version": request.expected_state_version,
        "actor": request.actor,
    }))
}

fn fixed_limit(limit: usize) -> i64 {
    i64::try_from(limit + 1).expect("fixed media detail limit fits in i64")
}

fn truncate_rows<T>(rows: &mut Vec<T>, limit: usize) -> bool {
    let truncated = rows.len() > limit;
    rows.truncate(limit);
    truncated
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23505")
}

async fn lock_idempotency_key(
    transaction: &mut Transaction<'_, Postgres>,
    key: &str,
) -> Result<(), MediaOpsError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn find_receipt(
    transaction: &mut Transaction<'_, Postgres>,
    idempotency_key: &str,
) -> Result<Option<ReceiptRow>, MediaOpsError> {
    sqlx::query_as::<_, ReceiptRow>(
        "SELECT id, cartridge_id, operation_kind, request_fingerprint,
                resulting_state_version, object_id
         FROM media_operations
         WHERE idempotency_key = $1",
    )
    .bind(idempotency_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(Into::into)
}

fn ensure_receipt_match(
    receipt: &ReceiptRow,
    cartridge_id: Uuid,
    operation_kind: &str,
    request_fingerprint: &str,
    idempotency_key: &str,
) -> Result<(), MediaOpsError> {
    if receipt.cartridge_id != cartridge_id
        || receipt.operation_kind != operation_kind
        || receipt.request_fingerprint != request_fingerprint
    {
        return Err(MediaOpsError::IdempotencyConflict {
            key: idempotency_key.to_owned(),
        });
    }
    Ok(())
}

fn result_from_receipt(receipt: ReceiptRow, idempotent_replay: bool) -> MediaMutationResult {
    MediaMutationResult {
        cartridge_id: receipt.cartridge_id,
        operation_id: receipt.id,
        object_id: receipt.object_id,
        state_version: receipt.resulting_state_version,
        idempotent_replay,
    }
}

async fn lock_cartridge(
    transaction: &mut Transaction<'_, Postgres>,
    cartridge_id: Uuid,
) -> Result<CartridgeLockRow, MediaOpsError> {
    sqlx::query_as::<_, CartridgeLockRow>(
        "SELECT state_version, pool_id, reserved_by
         FROM cartridges WHERE id = $1 FOR UPDATE",
    )
    .bind(cartridge_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or_else(|| MediaOpsError::NotFound {
        resource: "cartridge",
        id: cartridge_id.to_string(),
    })
}

fn ensure_expected_version(expected: i64, actual: i64) -> Result<(), MediaOpsError> {
    if expected != actual {
        return Err(MediaOpsError::StaleVersion { expected, actual });
    }
    Ok(())
}

fn ensure_single_version_advance(prior: i64, resulting: i64) -> Result<(), MediaOpsError> {
    if resulting != prior + 1 {
        return Err(MediaOpsError::Invariant(format!(
            "cartridge version trigger advanced from {prior} to {resulting}, expected exactly one step"
        )));
    }
    Ok(())
}

async fn touch_cartridge(
    transaction: &mut Transaction<'_, Postgres>,
    cartridge_id: Uuid,
) -> Result<i64, MediaOpsError> {
    sqlx::query_scalar::<_, i64>(
        "UPDATE cartridges SET updated_at = now() WHERE id = $1 RETURNING state_version",
    )
    .bind(cartridge_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
async fn insert_receipt(
    transaction: &mut Transaction<'_, Postgres>,
    operation_id: Uuid,
    cartridge_id: Uuid,
    operation_kind: &str,
    idempotency_key: &str,
    request_fingerprint: &str,
    expected_state_version: i64,
    resulting_state_version: i64,
    object_id: Option<Uuid>,
    actor: &MediaActor,
    correlation_id: Uuid,
    result: Value,
) -> Result<(), MediaOpsError> {
    sqlx::query(
        "INSERT INTO media_operations (
            id, cartridge_id, operation_kind, idempotency_key,
            request_fingerprint, expected_state_version,
            resulting_state_version, object_id, actor_id, actor_label,
            correlation_id, result
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(operation_id)
    .bind(cartridge_id)
    .bind(operation_kind)
    .bind(idempotency_key)
    .bind(request_fingerprint)
    .bind(expected_state_version)
    .bind(resulting_state_version)
    .bind(object_id)
    .bind(actor.user_id)
    .bind(actor.label.trim())
    .bind(correlation_id)
    .bind(result)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn append_audit(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MediaActor,
    action: &str,
    object_type: &str,
    object_id: &str,
    requested: Value,
    result: &str,
    correlation_id: Uuid,
) -> Result<(), MediaOpsError> {
    sqlx::query(
        "INSERT INTO audit_events (
            id, actor_id, actor_label, action, object_type, object_id,
            requested, result, correlation_id
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::now_v7())
    .bind(actor.user_id)
    .bind(actor.label.trim())
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

async fn append_outbox(
    transaction: &mut Transaction<'_, Postgres>,
    event_key: String,
    payload: Value,
) -> Result<(), MediaOpsError> {
    sqlx::query(
        "INSERT INTO notification_outbox (id, event_key, channel, payload)
         VALUES ($1, $2, 'in_app', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(event_key)
    .bind(payload)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

const fn drive_generation(generation: i16) -> Option<DriveGeneration> {
    match generation {
        1 => Some(DriveGeneration::Lto1),
        2 => Some(DriveGeneration::Lto2),
        3 => Some(DriveGeneration::Lto3),
        4 => Some(DriveGeneration::Lto4),
        5 => Some(DriveGeneration::Lto5),
        6 => Some(DriveGeneration::Lto6),
        7 => Some(DriveGeneration::Lto7),
        8 => Some(DriveGeneration::Lto8),
        9 => Some(DriveGeneration::Lto9),
        10 => Some(DriveGeneration::Lto10),
        _ => None,
    }
}

fn normalize_media_format(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn media_format(generation: Option<i16>, format: Option<&str>) -> Option<MediaFormat> {
    if let Some(format) = format {
        return match normalize_media_format(format).as_str() {
            "LTO1" => Some(MediaFormat::Lto1),
            "LTO2" => Some(MediaFormat::Lto2),
            "LTO3" => Some(MediaFormat::Lto3),
            "LTO4" => Some(MediaFormat::Lto4),
            "LTO5" => Some(MediaFormat::Lto5),
            "LTO6" => Some(MediaFormat::Lto6),
            "LTO7" => Some(MediaFormat::Lto7),
            "LTO7TYPEM" | "LTO7M" | "M8" => Some(MediaFormat::Lto7TypeM),
            "LTO8" => Some(MediaFormat::Lto8),
            "LTO9" => Some(MediaFormat::Lto9),
            "LTO10" => Some(MediaFormat::Lto10),
            _ => None,
        };
    }
    match generation {
        Some(1) => Some(MediaFormat::Lto1),
        Some(2) => Some(MediaFormat::Lto2),
        Some(3) => Some(MediaFormat::Lto3),
        Some(4) => Some(MediaFormat::Lto4),
        Some(5) => Some(MediaFormat::Lto5),
        Some(6) => Some(MediaFormat::Lto6),
        // Generation 7 without an explicit format is ambiguous: it could be
        // ordinary LTO-7 or an M8-format cartridge.
        Some(7) => None,
        Some(8) => Some(MediaFormat::Lto8),
        Some(9) => Some(MediaFormat::Lto9),
        Some(10) => Some(MediaFormat::Lto10),
        _ => None,
    }
}

const fn media_format_generation(format: MediaFormat) -> i16 {
    match format {
        MediaFormat::Lto1 => 1,
        MediaFormat::Lto2 => 2,
        MediaFormat::Lto3 => 3,
        MediaFormat::Lto4 => 4,
        MediaFormat::Lto5 => 5,
        MediaFormat::Lto6 => 6,
        MediaFormat::Lto7 | MediaFormat::Lto7TypeM => 7,
        MediaFormat::Lto8 => 8,
        MediaFormat::Lto9 => 9,
        MediaFormat::Lto10 => 10,
    }
}

fn media_health(status: &str) -> MediaHealth {
    match status {
        "healthy" | "verified" | "written_verified" => MediaHealth::Healthy,
        "quarantined" | "write_unverified" => MediaHealth::Quarantined,
        "failed" => MediaHealth::Failed,
        _ => MediaHealth::Degraded,
    }
}

fn reservation(reserved_by: Option<&str>) -> Reservation {
    match reserved_by {
        None => Reservation::None,
        Some(value) if value.starts_with("movement:") => Reservation::Export,
        Some(value) if value.starts_with("restore:") => Reservation::Restore,
        Some(_) => Reservation::ActiveJob,
    }
}

fn drive_health_true(health: &Value, key: &str) -> bool {
    health
        .get(key)
        .is_some_and(|value| value.as_bool() == Some(true) || value.as_str() == Some("true"))
}

fn drive_health_false(health: &Value, key: &str) -> bool {
    health
        .get(key)
        .is_some_and(|value| value.as_bool() == Some(false) || value.as_str() == Some("false"))
}

fn drive_health_i64(health: &Value, key: &str) -> Option<i64> {
    health.get(key).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn usize_count(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}

fn blocker(code: &str, message: &str) -> MediaReuseBlocker {
    MediaReuseBlocker {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

fn domain_blocker(blocker_value: OverwriteBlocker) -> MediaReuseBlocker {
    match blocker_value {
        OverwriteBlocker::IdentityUnknown => blocker(
            "identity_unknown",
            "The cartridge identity has not been reconciled.",
        ),
        OverwriteBlocker::ForeignMedia => blocker(
            "foreign_media",
            "The cartridge is identified as foreign media.",
        ),
        OverwriteBlocker::IdentityMismatch => blocker(
            "identity_mismatch",
            "Catalog, physical, and on-tape identities do not agree.",
        ),
        OverwriteBlocker::PhysicalStateUnreconciled => blocker(
            "physical_state_unreconciled",
            "Physical custody/location state is not reconciled.",
        ),
        OverwriteBlocker::EngineStateUnreconciled => blocker(
            "engine_state_unreconciled",
            "The backup engine identity is not reconciled with the cartridge.",
        ),
        OverwriteBlocker::MediaNotHealthy => blocker(
            "media_not_healthy",
            "The cartridge is not in a healthy state.",
        ),
        OverwriteBlocker::PhysicalWormMedia => blocker(
            "physical_worm_media",
            "Physical WORM media cannot be overwritten.",
        ),
        OverwriteBlocker::ReservedForActiveJob => blocker(
            "reserved_for_active_job",
            "The cartridge is reserved for an active job.",
        ),
        OverwriteBlocker::ReservedForExport => blocker(
            "reserved_for_export",
            "The cartridge is reserved for a custody movement/export.",
        ),
        OverwriteBlocker::ReservedForRestore => blocker(
            "reserved_for_restore",
            "The cartridge is reserved for a restore.",
        ),
        OverwriteBlocker::DriveCannotWriteMediaFormat => blocker(
            "drive_cannot_write_media_format",
            "The selected drive cannot write this media format.",
        ),
        OverwriteBlocker::PolicyDisallowsReuse => blocker(
            "policy_disallows_reuse",
            "The assigned pool policy does not allow automatic reuse.",
        ),
        OverwriteBlocker::RetainedRecoveryPoints => blocker(
            "retained_recovery_points",
            "One or more retained recovery points use this cartridge.",
        ),
        OverwriteBlocker::DependentRecoveryPoints => blocker(
            "dependent_recovery_points",
            "Retained recovery points depend on content on this cartridge.",
        ),
        OverwriteBlocker::ExplicitHolds => blocker(
            "explicit_holds",
            "Active holds protect this cartridge or its recovery points.",
        ),
        OverwriteBlocker::IndefiniteArchive => blocker(
            "indefinite_archive",
            "Indefinite archive recovery points protect this cartridge.",
        ),
        OverwriteBlocker::AdministratorReauthenticationRequired => blocker(
            "administrator_reauthentication_required",
            "Administrator reauthentication would be required.",
        ),
        OverwriteBlocker::AffectedObjectPreviewRequired => blocker(
            "affected_object_preview_required",
            "An affected-object preview would be required.",
        ),
        OverwriteBlocker::AuditReasonRequired => blocker(
            "audit_reason_required",
            "An audit reason would be required.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn actor() -> MediaActor {
        MediaActor {
            user_id: None,
            label: "tape operator".to_owned(),
        }
    }

    fn inventory_row() -> InventoryRow {
        InventoryRow {
            id: Uuid::now_v7(),
            internal_label: "AA0001".to_owned(),
            barcode: Some("AA0001L8".to_owned()),
            media_generation: Some(8),
            media_format: Some("LTO-8".to_owned()),
            on_tape_identity: Some("AA0001".to_owned()),
            identity_status: "reconciled".to_owned(),
            health_status: "healthy".to_owned(),
            worm: false,
            reserved_by: None,
            state_version: 7,
            pool_id: Some(Uuid::now_v7()),
            pool_name: Some("Routine".to_owned()),
            pool_purpose: Some("routine".to_owned()),
            pool_indefinite_retention: Some(false),
            location_id: Some(Uuid::now_v7()),
            location_name: Some("Vault".to_owned()),
            location_kind: Some("onsite".to_owned()),
            active_holds: 0,
            content_volume_count: 0,
            unreconciled_content_records: 0,
            retained_recovery_points: 0,
            dependent_recovery_points: 0,
            indefinite_archive_claims: 0,
            dependency_graph_truncated: false,
            latest_verification_id: None,
            latest_verification_recovery_point_id: None,
            latest_verification_kind: None,
            latest_verification_outcome: None,
            latest_verification_drive_id: None,
            latest_verification_drive_stable_id: None,
            latest_verification_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn public_enums_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_value(MediaPoolPurpose::Archive).unwrap(),
            json!("archive")
        );
        assert_eq!(
            serde_json::to_value(MediaHoldKind::Manual).unwrap(),
            json!("manual")
        );
        assert_eq!(
            serde_json::to_value(InventoryReuseState::DriveEvaluationRequired).unwrap(),
            json!("drive_evaluation_required")
        );
        assert_eq!(
            serde_json::to_value(MediaLocationKind::InTransit).unwrap(),
            json!("in_transit")
        );
    }

    #[test]
    fn pool_details_leave_headroom_for_postgres_jsonb_text() {
        let accepted = json!({"payload": "x".repeat(400_000)});
        validate_object("details", &accepted, POOL_DETAILS_MAX_ENCODED_BYTES).unwrap();

        let rejected = json!({"payload": "x".repeat(POOL_DETAILS_MAX_ENCODED_BYTES)});
        let error =
            validate_object("details", &rejected, POOL_DETAILS_MAX_ENCODED_BYTES).unwrap_err();
        assert!(matches!(
            error,
            MediaOpsError::Invalid {
                field: "details",
                ..
            }
        ));
    }

    #[test]
    fn archive_pool_always_becomes_indefinite() {
        assert!(effective_pool_indefinite_retention(
            MediaPoolPurpose::Archive,
            false
        ));
        assert!(!effective_pool_indefinite_retention(
            MediaPoolPurpose::Routine,
            false
        ));
        assert!(effective_pool_indefinite_retention(
            MediaPoolPurpose::Routine,
            true
        ));
    }

    #[test]
    fn mutation_text_limits_are_enforced_in_utf8_bytes() {
        let valid_actor = MediaActor {
            user_id: None,
            label: "é".repeat(128),
        };
        assert!(validate_actor(&valid_actor).is_ok());

        let oversized_actor = MediaActor {
            user_id: None,
            label: "é".repeat(129),
        };
        assert!(matches!(
            validate_actor(&oversized_actor),
            Err(MediaOpsError::Invalid {
                field: "actor.label",
                ..
            })
        ));

        let valid_idempotency_key = "é".repeat(128);
        assert!(validate_mutation_common(0, &valid_idempotency_key, &actor()).is_ok());

        let oversized_idempotency_key = "é".repeat(129);
        assert!(matches!(
            validate_mutation_common(0, &oversized_idempotency_key, &actor()),
            Err(MediaOpsError::Invalid {
                field: "idempotency_key",
                ..
            })
        ));

        let valid_release_reason = "🧵".repeat(1024);
        assert!(validate_text("reason", &valid_release_reason, 1, 4096).is_ok());

        let oversized_release_reason = "🧵".repeat(1025);
        assert!(matches!(
            validate_text("reason", &oversized_release_reason, 1, 4096),
            Err(MediaOpsError::Invalid {
                field: "reason",
                ..
            })
        ));
    }

    #[test]
    fn idempotency_fingerprint_ignores_transport_metadata() {
        let cartridge_id = Uuid::now_v7();
        let pool_id = Uuid::now_v7();
        let mut request = AssignMediaPoolRequest {
            pool_id: Some(pool_id),
            expected_state_version: 4,
            idempotency_key: "first-key".to_owned(),
            actor: actor(),
            correlation_id: Uuid::now_v7(),
        };
        let first = assignment_request_fingerprint(cartridge_id, &request).unwrap();
        request.idempotency_key = "same-operation-retry".to_owned();
        request.correlation_id = Uuid::now_v7();
        let retried = assignment_request_fingerprint(cartridge_id, &request).unwrap();
        assert_eq!(first, retried);

        request.pool_id = None;
        assert_ne!(
            first,
            assignment_request_fingerprint(cartridge_id, &request).unwrap()
        );
    }

    #[test]
    fn hold_fingerprints_ignore_retry_correlation_but_bind_semantics() {
        let cartridge_id = Uuid::now_v7();
        let hold_id = Uuid::now_v7();
        let mut create = CreateMediaHoldRequest {
            kind: MediaHoldKind::Legal,
            reason: " preserve for litigation ".to_owned(),
            expires_at: None,
            expected_state_version: 2,
            idempotency_key: "create-1".to_owned(),
            actor: actor(),
            correlation_id: Uuid::now_v7(),
        };
        let first =
            create_hold_request_fingerprint(cartridge_id, &create, "preserve for litigation")
                .unwrap();
        create.correlation_id = Uuid::now_v7();
        create.idempotency_key = "ignored-here".to_owned();
        assert_eq!(
            first,
            create_hold_request_fingerprint(cartridge_id, &create, "preserve for litigation")
                .unwrap()
        );
        create.kind = MediaHoldKind::Archive;
        assert_ne!(
            first,
            create_hold_request_fingerprint(cartridge_id, &create, "preserve for litigation")
                .unwrap()
        );

        let mut release = ReleaseMediaHoldRequest {
            reason: "case closed".to_owned(),
            expected_state_version: 3,
            idempotency_key: "release-1".to_owned(),
            actor: actor(),
            correlation_id: Uuid::now_v7(),
        };
        let first_release =
            release_hold_request_fingerprint(hold_id, &release, "case closed").unwrap();
        release.correlation_id = Uuid::now_v7();
        assert_eq!(
            first_release,
            release_hold_request_fingerprint(hold_id, &release, "case closed").unwrap()
        );
        assert_ne!(
            first_release,
            release_hold_request_fingerprint(hold_id, &release, "different reason").unwrap()
        );
    }

    #[test]
    fn expired_content_is_not_misrepresented_as_current_protection() {
        let mut row = inventory_row();
        row.content_volume_count = 3;
        let inventory = MediaInventorySummary::try_from(row).unwrap();
        assert!(inventory.has_content);
        assert_eq!(
            inventory.reuse_state,
            InventoryReuseState::DriveEvaluationRequired
        );
    }

    #[test]
    fn incomplete_catalog_content_requires_reconciliation() {
        let mut row = inventory_row();
        row.content_volume_count = 1;
        row.unreconciled_content_records = 1;
        let inventory = MediaInventorySummary::try_from(row).unwrap();
        assert!(!inventory.catalog_reconciled);
        assert_eq!(
            inventory.reuse_state,
            InventoryReuseState::NeedsReconciliation
        );
    }

    #[test]
    fn incomplete_dependency_graph_never_looks_reusable() {
        let mut row = inventory_row();
        row.dependency_graph_truncated = true;
        let inventory = MediaInventorySummary::try_from(row).unwrap();
        assert_eq!(
            inventory.reuse_state,
            InventoryReuseState::NeedsReconciliation
        );
    }

    #[test]
    fn inventory_reuse_summary_is_fail_closed_and_precedence_ordered() {
        let mut row = inventory_row();
        row.active_holds = 1;
        assert_eq!(
            MediaInventorySummary::try_from(row).unwrap().reuse_state,
            InventoryReuseState::Protected
        );

        let mut row = inventory_row();
        row.reserved_by = Some("job:123".to_owned());
        row.active_holds = 1;
        assert_eq!(
            MediaInventorySummary::try_from(row).unwrap().reuse_state,
            InventoryReuseState::Reserved
        );

        let mut row = inventory_row();
        row.worm = true;
        row.reserved_by = Some("job:123".to_owned());
        assert_eq!(
            MediaInventorySummary::try_from(row).unwrap().reuse_state,
            InventoryReuseState::PhysicallyBlocked
        );

        let mut row = inventory_row();
        row.on_tape_identity = Some("   ".to_owned());
        assert_eq!(
            MediaInventorySummary::try_from(row).unwrap().reuse_state,
            InventoryReuseState::NeedsReconciliation
        );

        let mut row = inventory_row();
        row.media_format = Some("LTO-9".to_owned());
        assert_eq!(
            MediaInventorySummary::try_from(row).unwrap().reuse_state,
            InventoryReuseState::NeedsReconciliation
        );
    }

    #[test]
    fn m8_is_not_collapsed_into_ordinary_lto7() {
        assert_eq!(
            media_format(Some(7), Some("LTO-7 Type M")),
            Some(MediaFormat::Lto7TypeM)
        );
        assert_eq!(
            media_format(Some(7), Some("M8")),
            Some(MediaFormat::Lto7TypeM)
        );
        assert_eq!(
            media_format(Some(7), Some("LTO-7")),
            Some(MediaFormat::Lto7)
        );
        assert_eq!(media_format(Some(7), None), None);
    }

    #[test]
    fn legal_and_archive_holds_cannot_silently_expire() {
        let now = Utc::now();
        let future = now + chrono::Duration::days(1);
        assert!(validate_hold_expiration(MediaHoldKind::Manual, Some(future), now).is_ok());
        assert!(matches!(
            validate_hold_expiration(MediaHoldKind::Legal, Some(future), now),
            Err(MediaOpsError::Invalid {
                field: "expires_at",
                ..
            })
        ));
        assert!(validate_hold_expiration(MediaHoldKind::Archive, None, now).is_ok());
    }

    #[test]
    fn missing_immediate_mode_evidence_is_not_motion_safe() {
        assert!(!drive_health_false(&json!({}), "immediate_mode"));
        assert!(drive_health_false(
            &json!({"immediate_mode": false}),
            "immediate_mode"
        ));
        assert!(drive_health_false(
            &json!({"immediate_mode": "false"}),
            "immediate_mode"
        ));
    }

    #[test]
    fn every_domain_blocker_has_a_unique_stable_api_code() {
        let values = [
            OverwriteBlocker::IdentityUnknown,
            OverwriteBlocker::ForeignMedia,
            OverwriteBlocker::IdentityMismatch,
            OverwriteBlocker::PhysicalStateUnreconciled,
            OverwriteBlocker::EngineStateUnreconciled,
            OverwriteBlocker::MediaNotHealthy,
            OverwriteBlocker::PhysicalWormMedia,
            OverwriteBlocker::ReservedForActiveJob,
            OverwriteBlocker::ReservedForExport,
            OverwriteBlocker::ReservedForRestore,
            OverwriteBlocker::DriveCannotWriteMediaFormat,
            OverwriteBlocker::PolicyDisallowsReuse,
            OverwriteBlocker::RetainedRecoveryPoints,
            OverwriteBlocker::DependentRecoveryPoints,
            OverwriteBlocker::ExplicitHolds,
            OverwriteBlocker::IndefiniteArchive,
            OverwriteBlocker::AdministratorReauthenticationRequired,
            OverwriteBlocker::AffectedObjectPreviewRequired,
            OverwriteBlocker::AuditReasonRequired,
        ];
        let codes = values
            .into_iter()
            .map(domain_blocker)
            .map(|blocker| blocker.code)
            .collect::<HashSet<_>>();
        assert_eq!(codes.len(), 19);
    }

    #[test]
    fn optimistic_version_checks_never_accept_stale_or_multi_step_updates() {
        assert!(ensure_expected_version(9, 9).is_ok());
        assert!(matches!(
            ensure_expected_version(8, 9),
            Err(MediaOpsError::StaleVersion {
                expected: 8,
                actual: 9
            })
        ));
        assert!(ensure_single_version_advance(9, 10).is_ok());
        assert!(matches!(
            ensure_single_version_advance(9, 11),
            Err(MediaOpsError::Invariant(_))
        ));
    }

    #[test]
    fn idempotency_receipts_bind_target_kind_and_payload() {
        let cartridge_id = Uuid::now_v7();
        let receipt = ReceiptRow {
            id: Uuid::now_v7(),
            cartridge_id,
            operation_kind: "pool_assigned".to_owned(),
            request_fingerprint: "a".repeat(64),
            resulting_state_version: 2,
            object_id: None,
        };
        assert!(ensure_receipt_match(
            &receipt,
            cartridge_id,
            "pool_assigned",
            &"a".repeat(64),
            "key"
        )
        .is_ok());
        assert!(matches!(
            ensure_receipt_match(
                &receipt,
                Uuid::now_v7(),
                "pool_assigned",
                &"a".repeat(64),
                "key"
            ),
            Err(MediaOpsError::IdempotencyConflict { .. })
        ));
    }
}
