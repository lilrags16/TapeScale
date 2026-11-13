use std::{
    path::{Component, Path},
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{pool::PoolConnection, postgres::PgPoolOptions, FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    device::{MissingSasDrive, SasDriveObservation, SasMotionOperation},
    domain::lto::{can_read, DriveGeneration, MediaFormat},
};

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct OverviewCounts {
    pub protected_sources: i64,
    pub active_jobs: i64,
    pub required_actions: i64,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RequiredActionRecord {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub state: String,
    pub due_at: Option<DateTime<Utc>>,
    pub route: String,
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
pub struct RecoveryPointDetailRecord {
    pub id: Uuid,
    pub source_id: Uuid,
    pub run_id: Uuid,
    pub representation: String,
    pub level: String,
    pub source_scope_hash: String,
    pub format_version: String,
    pub key_lineage_id: Option<String>,
    pub status: String,
    pub recovery_time: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub indefinite: bool,
    pub coverage: Value,
    pub manifest_digest: Option<String>,
    pub created_at: DateTime<Utc>,
    pub dataset_name: String,
    pub source_type: String,
    pub logical_root: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RecoveryDependencyRecord {
    pub id: Uuid,
    pub representation: String,
    pub level: String,
    pub status: String,
    pub recovery_time: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub indefinite: bool,
    pub format_version: String,
    pub key_lineage_id: Option<String>,
    pub manifest_digest: Option<String>,
    pub dependency_kind: String,
    pub depth: i32,
}

#[derive(Clone, Debug, FromRow)]
struct RecoveryVolumeRow {
    id: Uuid,
    recovery_point_id: Uuid,
    volume_order: i32,
    first_record: String,
    last_record: Option<String>,
    encoded_bytes: Option<i64>,
    finalized: bool,
    engine_record_id: String,
    cartridge_id: Uuid,
    internal_label: String,
    barcode: Option<String>,
    media_generation: Option<i16>,
    media_format: Option<String>,
    on_tape_identity: Option<String>,
    identity_status: String,
    health_status: String,
    worm: bool,
    reserved_by: Option<String>,
    location_id: Option<Uuid>,
    location_name: Option<String>,
    location_kind: Option<String>,
}

#[derive(Clone, Debug, FromRow)]
struct RecoveryDriveRow {
    id: Uuid,
    stable_id: String,
    generation: Option<i16>,
    vendor: Option<String>,
    model: Option<String>,
    serial: Option<String>,
    wwn: Option<String>,
    firmware: Option<String>,
    observed_path: Option<String>,
    last_observed_at: Option<DateTime<Utc>>,
    qualified_media: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecoveryDrivePrerequisite {
    pub id: Uuid,
    pub stable_id: String,
    pub generation: Option<i16>,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub wwn: Option<String>,
    pub firmware: Option<String>,
    pub observed_path: Option<String>,
    pub last_observed_at: Option<DateTime<Utc>>,
    pub compatibility_basis: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecoveryVolumeRecord {
    pub id: Uuid,
    pub recovery_point_id: Uuid,
    pub volume_order: i32,
    pub first_record: String,
    pub last_record: Option<String>,
    pub encoded_bytes: Option<i64>,
    pub finalized: bool,
    pub engine_record_id: String,
    pub cartridge_id: Uuid,
    pub internal_label: String,
    pub barcode: Option<String>,
    pub media_generation: Option<i16>,
    pub media_format: Option<String>,
    pub on_tape_identity: Option<String>,
    pub identity_status: String,
    pub health_status: String,
    pub worm: bool,
    pub reserved_by: Option<String>,
    pub location_id: Option<Uuid>,
    pub location_name: Option<String>,
    pub location_kind: Option<String>,
    pub available: bool,
    pub compatible_qualified_drives: Vec<RecoveryDrivePrerequisite>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RecoveryHoldRecord {
    pub id: Uuid,
    pub cartridge_id: Option<Uuid>,
    pub recovery_point_id: Option<Uuid>,
    pub kind: String,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub released_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub active: bool,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RecoveryVerificationRecord {
    pub id: Uuid,
    pub recovery_point_id: Uuid,
    pub kind: String,
    pub scope: Value,
    pub outcome: String,
    pub evidence: Value,
    pub correlation_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub drive_id: Option<Uuid>,
    pub drive_stable_id: Option<String>,
    pub media_ids: Vec<Uuid>,
    pub media_labels: Vec<String>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RecoveryOriginRecord {
    pub job_id: Uuid,
    pub job_external_id: String,
    pub job_kind: String,
    pub run_id: Uuid,
    pub run_state: String,
    pub attempt: i32,
    pub correlation_id: Uuid,
    pub engine_operation_id: Option<String>,
    pub run_created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub data_run_id: Option<Uuid>,
    pub workflow_kind: Option<String>,
    pub profile_name: Option<String>,
    pub engine_name: Option<String>,
    pub engine_version: Option<String>,
    pub adapter_version: Option<String>,
    pub data_run_result: Option<Value>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RecoveryEventRecord {
    pub source: String,
    pub event_id: String,
    pub event_type: String,
    pub detail: String,
    pub evidence: Value,
    pub correlation_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RecoveryArtifactRecord {
    pub id: i64,
    pub artifact_kind: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow)]
pub struct RecoveryPointManifestBinding {
    pub storage_ref: Option<String>,
    pub artifact_sha256: Option<String>,
    pub artifact_size_bytes: Option<i64>,
    pub manifest_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecoveryAggregateCaps {
    pub dependencies_truncated: bool,
    pub volumes_truncated: bool,
    pub holds_truncated: bool,
    pub verifications_truncated: bool,
    pub events_truncated: bool,
    pub artifacts_truncated: bool,
    pub values_truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecoveryPointAggregate {
    pub recovery_point: RecoveryPointDetailRecord,
    pub volumes: Vec<RecoveryVolumeRecord>,
    pub dependencies: Vec<RecoveryDependencyRecord>,
    pub holds: Vec<RecoveryHoldRecord>,
    pub verifications: Vec<RecoveryVerificationRecord>,
    pub origin: RecoveryOriginRecord,
    pub events: Vec<RecoveryEventRecord>,
    pub artifacts: Vec<RecoveryArtifactRecord>,
    pub caps: RecoveryAggregateCaps,
}

#[derive(Clone, Debug)]
pub struct NewRestorePlan {
    pub selection: Value,
    pub dependency_chain: Vec<Uuid>,
    pub required_media_ids: Vec<Uuid>,
    pub key_lineage_ids: Vec<String>,
    pub destination: Value,
    pub conflict_policy: String,
    pub blockers: Value,
    pub estimates: Value,
    pub preview_hash: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RestorePlanRecord {
    pub id: Uuid,
    pub selection: Value,
    pub dependency_chain: Vec<Uuid>,
    pub required_media_ids: Vec<Uuid>,
    pub key_lineage_ids: Vec<String>,
    pub destination: Value,
    pub conflict_policy: String,
    pub blockers: Value,
    pub estimates: Value,
    pub preview_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RestoreRehearsalAttestations {
    pub confirmation: String,
    pub tape_loaded_confirmed: bool,
    pub isolated_destination_confirmed: bool,
    pub attended_confirmed: bool,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RestoreRehearsalRunRecord {
    pub id: Uuid,
    pub restore_plan_id: Uuid,
    pub job_id: Uuid,
    pub job_run_id: Uuid,
    pub recovery_point_id: Uuid,
    pub original_data_run_id: Uuid,
    pub drive_id: Uuid,
    pub cartridge_id: Uuid,
    pub expected_stable_id: String,
    pub expected_path: String,
    pub expected_vendor: Option<String>,
    pub expected_model: Option<String>,
    pub expected_serial: Option<String>,
    pub expected_wwn: Option<String>,
    pub expected_firmware: Option<String>,
    pub expected_volume_label: String,
    pub expected_media_generation: Option<i16>,
    pub expected_media_format: Option<String>,
    pub expected_on_tape_identity: String,
    pub backup_job_id: i64,
    pub expected_engine_media_id: i64,
    pub expected_engine_record_id: String,
    pub source_manifest_artifact_id: i64,
    pub source_manifest_storage_ref: String,
    pub source_manifest_sha256: String,
    pub source_manifest_digest: String,
    pub source_manifest_size_bytes: i64,
    pub source_mount_path: String,
    pub logical_root: String,
    pub destination_subpath: String,
    pub estimated_required_bytes: i64,
    pub available_bytes: i64,
    pub required_free_reserve_bytes: i64,
    pub engine_name: String,
    pub engine_version: String,
    pub adapter_version: String,
    pub format_version: String,
    pub preview_hash: String,
    pub production_eligible: bool,
    pub state: String,
    pub phase: String,
    pub requested_by_label: String,
    pub correlation_id: Uuid,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub cancel_requested_by_label: Option<String>,
    pub drive_lease_id: Option<Uuid>,
    pub drive_fencing_token: Option<i64>,
    pub media_lease_id: Option<Uuid>,
    pub media_fencing_token: Option<i64>,
    pub attempt: i32,
    pub result: Value,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RestoreRehearsalEventRecord {
    pub id: i64,
    pub event_type: String,
    pub evidence: Value,
    pub correlation_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct RestoreRehearsalArtifactRecord {
    pub id: i64,
    pub artifact_kind: String,
    pub storage_ref: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ClaimedRestoreRehearsalRun {
    pub run: RestoreRehearsalRunRecord,
    pub drive_lease_id: Uuid,
    pub drive_fencing_token: i64,
    pub media_lease_id: Uuid,
    pub media_fencing_token: i64,
    pub worker_id: Uuid,
}

#[derive(Clone, Debug)]
pub enum QueueRestoreRehearsalResult {
    Queued(Box<RestoreRehearsalRunRecord>),
    NotFound,
    Blocked(String),
}

#[derive(Clone, Debug)]
pub enum ClaimRestoreRehearsalRunResult {
    Claimed(Box<ClaimedRestoreRehearsalRun>),
    Blocked { id: Uuid, reason: String },
    Empty,
}

#[derive(Clone, Debug)]
pub enum CancelRestoreRehearsalResult {
    Cancelled(Box<RestoreRehearsalRunRecord>),
    Requested(Box<RestoreRehearsalRunRecord>),
    AlreadyTerminal(Box<RestoreRehearsalRunRecord>),
    NotFound,
    Blocked(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreRehearsalPhase {
    Restoring,
    Comparing,
}

impl RestoreRehearsalPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Restoring => "restoring",
            Self::Comparing => "comparing",
        }
    }

    const fn allowed_previous(self) -> &'static [&'static str] {
        match self {
            Self::Restoring => &["preparing", "waiting_for_media"],
            Self::Comparing => &["restoring"],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreRehearsalTerminalState {
    Failed,
    Blocked,
    Interrupted,
    Cancelled,
}

impl RestoreRehearsalTerminalState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinalizeRestoreRehearsalResult {
    Passed { verification_record_id: Uuid },
    Rejected(String),
    FencedOut,
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
    health_status: String,
    worm: bool,
    pool_id: Option<Uuid>,
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
    Existing(Box<QualificationRunRecord>),
    IdempotencyConflict,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSingleTapeDataProfile {
    pub name: String,
    pub source_subpath: String,
    pub restore_subpath: String,
    pub source_mount_path: String,
    pub restore_root_path: String,
    pub immutable_snapshot_confirmed: bool,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SingleTapeDataProfileRecord {
    pub id: Uuid,
    pub job_id: Uuid,
    pub source_id: Uuid,
    pub source_snapshot_id: Uuid,
    pub name: String,
    pub source_subpath: String,
    pub restore_subpath: String,
    pub source_mount_path: String,
    pub restore_root_path: String,
    pub logical_root: String,
    pub engine_name: String,
    pub engine_version: String,
    pub adapter_version: String,
    pub format_version: String,
    pub immutable_snapshot_confirmed: bool,
    pub enabled: bool,
    pub production_eligible: bool,
    pub workflow_kind: String,
    pub policy_id: Option<Uuid>,
    pub media_pool_id: Option<Uuid>,
    pub schedule_enabled: bool,
    pub retention_days: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SingleTapeDataRunRecord {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub job_run_id: Uuid,
    pub source_snapshot_id: Uuid,
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
    pub source_mount_path: String,
    pub restore_root_path: String,
    pub logical_root: String,
    pub engine_name: String,
    pub engine_version: String,
    pub adapter_version: String,
    pub format_version: String,
    pub scratch_media_confirmed: bool,
    pub destructive_overwrite_confirmed: bool,
    pub encryption_deviation_confirmed: bool,
    pub media_write_started: bool,
    pub media_write_started_at: Option<DateTime<Utc>>,
    pub workflow_kind: String,
    pub trigger_kind: String,
    pub requested_level: String,
    pub scheduled_occurrence: Option<String>,
    pub retention_days: i32,
    pub compression_mode: String,
    pub resolved_compression: String,
    pub spanning_enabled: bool,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub cancel_requested_by_label: Option<String>,
    pub state: String,
    pub requested_by_label: String,
    pub correlation_id: Uuid,
    pub drive_lease_id: Option<Uuid>,
    pub drive_fencing_token: Option<i64>,
    pub media_lease_id: Option<Uuid>,
    pub media_fencing_token: Option<i64>,
    pub attempt: i32,
    pub result: Value,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, FromRow, Serialize, PartialEq, Eq)]
pub struct DataRunVolumeRecord {
    pub id: Uuid,
    pub single_tape_data_run_id: Uuid,
    pub volume_order: i32,
    pub cartridge_id: Uuid,
    pub expected_cartridge_label: String,
    pub expected_media_generation: Option<i16>,
    pub expected_media_format: Option<String>,
    pub engine_volume_name: Option<String>,
    pub engine_media_id: Option<i64>,
    pub state: String,
    pub media_lease_id: Option<Uuid>,
    pub media_fencing_token: Option<i64>,
    pub media_write_started_at: Option<DateTime<Utc>>,
    pub written_at: Option<DateTime<Utc>>,
    pub verified_at: Option<DateTime<Utc>>,
    pub encoded_bytes: Option<i64>,
    pub first_record: Option<String>,
    pub last_record: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow, Serialize, PartialEq, Eq)]
pub struct DataRunMediaRequestRecord {
    pub id: Uuid,
    pub single_tape_data_run_id: Uuid,
    pub request_sequence: i32,
    pub phase: String,
    pub volume_order: i32,
    pub expected_cartridge_id: Option<Uuid>,
    pub expected_engine_volume_name: Option<String>,
    pub state: String,
    pub authorized_cartridge_id: Option<Uuid>,
    pub requested_at: DateTime<Utc>,
    pub authorized_at: Option<DateTime<Utc>>,
    pub loaded_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub confirmation_evidence: Value,
}

#[derive(Clone, Debug)]
pub struct NewDataRunMediaAuthorization<'a> {
    pub data_run_id: Uuid,
    pub request_id: Uuid,
    pub drive_id: Uuid,
    pub expected_stable_id: &'a str,
    pub cartridge_id: Uuid,
    pub typed_cartridge_label: &'a str,
    pub loaded_media_confirmed: bool,
    pub destructive_overwrite_confirmed: bool,
    pub confirmation_evidence: Value,
}

#[derive(Clone, Debug)]
pub enum DataRunMediaRequestResult {
    Ready(Box<DataRunMediaRequestRecord>),
    FencedOut,
    Blocked(String),
}

#[derive(Clone, Debug)]
pub enum AuthorizeDataRunMediaResult {
    Authorized {
        request: Box<DataRunMediaRequestRecord>,
        volume: Box<DataRunVolumeRecord>,
        idempotent_replay: bool,
    },
    NotFound,
    Blocked(String),
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SingleTapeDataArtifactRecord {
    pub id: i64,
    pub artifact_kind: String,
    pub storage_ref: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SingleTapeDataEventRecord {
    pub id: i64,
    pub event_type: String,
    pub evidence: Value,
    pub correlation_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ClaimedSingleTapeDataRun {
    pub run: SingleTapeDataRunRecord,
    pub drive_lease_id: Uuid,
    pub drive_fencing_token: i64,
    pub media_lease_id: Uuid,
    pub media_fencing_token: i64,
    pub worker_id: Uuid,
}

#[derive(Clone, Debug)]
pub enum CreateSingleTapeDataProfileResult {
    Created(Box<SingleTapeDataProfileRecord>),
    NotFound,
    Blocked(String),
}

#[derive(Clone, Debug)]
pub enum QueueSingleTapeDataRunResult {
    Queued(Box<SingleTapeDataRunRecord>),
    NotFound,
    Blocked(String),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupMediaReadinessCandidate {
    pub drive_id: Uuid,
    pub stable_id: String,
    pub device_path: String,
    pub cartridge_id: Uuid,
    pub cartridge_label: String,
    pub media_generation: Option<i16>,
    pub media_format: Option<String>,
    pub qualification_run_id: Uuid,
    pub required_confirmation: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupMediaReadinessBlocker {
    pub code: String,
    pub scope: String,
    pub resource_id: Option<Uuid>,
    pub related_resource_id: Option<Uuid>,
    pub message: String,
    pub action: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupMediaReadinessCounts {
    pub drives_total: i64,
    pub drives_evaluated: usize,
    pub cartridges_total: i64,
    pub cartridges_evaluated: usize,
    pub pairs_evaluated: usize,
    pub candidates_matched: usize,
    pub candidates_returned: usize,
    pub blockers_matched: usize,
    pub blockers_returned: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupMediaReadinessLimits {
    pub drives: usize,
    pub cartridges: usize,
    pub pairs: usize,
    pub candidates: usize,
    pub blockers: usize,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct BackupMediaReadinessTruncation {
    pub drives: bool,
    pub cartridges: bool,
    pub pairs: bool,
    pub candidates: bool,
    pub blockers: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupMediaReadiness {
    pub state: String,
    pub profile_id: Uuid,
    pub backup_request_id: Option<Uuid>,
    pub candidates: Vec<BackupMediaReadinessCandidate>,
    pub recommended_candidate: Option<BackupMediaReadinessCandidate>,
    pub blockers: Vec<BackupMediaReadinessBlocker>,
    pub counts: BackupMediaReadinessCounts,
    pub limits: BackupMediaReadinessLimits,
    pub truncated: BackupMediaReadinessTruncation,
}

#[derive(Clone, Debug)]
pub enum BackupMediaReadinessResult {
    Readiness(Box<BackupMediaReadiness>),
    NotFound,
}

#[derive(Clone, Debug)]
pub enum CancelSingleTapeDataRunResult {
    Cancelled(Box<SingleTapeDataRunRecord>),
    CancellationRequested(Box<SingleTapeDataRunRecord>),
    NotFound,
    Terminal(Box<SingleTapeDataRunRecord>),
}

#[derive(Clone, Debug)]
pub enum ClaimSingleTapeDataRunResult {
    Claimed(Box<ClaimedSingleTapeDataRun>),
    Blocked { id: Uuid, reason: String },
    Empty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleTapeDataPhase {
    Writing,
    Written,
    Verifying,
    Restoring,
    Comparing,
}

impl SingleTapeDataPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Writing => "writing",
            Self::Written => "written",
            Self::Verifying => "verifying",
            Self::Restoring => "restoring",
            Self::Comparing => "comparing",
        }
    }

    const fn allowed_previous(self) -> &'static [&'static str] {
        match self {
            Self::Writing => &["preparing", "waiting_for_media"],
            Self::Written => &["writing"],
            Self::Verifying => &["written"],
            Self::Restoring => &["verifying"],
            Self::Comparing => &["restoring"],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleTapeDataTerminalState {
    Failed,
    Blocked,
    Interrupted,
    Cancelled,
}

impl SingleTapeDataTerminalState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SingleTapePassEvidence {
    pub engine_operation_id: String,
    pub engine_volume_name: String,
    pub engine_record_id: String,
    pub first_record: String,
    pub last_record: Option<String>,
    pub logical_bytes: u64,
    pub encoded_bytes: u64,
    pub file_count: u64,
    pub coverage: Value,
    pub full_readback_evidence: Value,
    pub restore_test_evidence: Value,
    pub result: Value,
}

#[derive(Clone, Debug)]
struct FinalizedBackupVolume {
    run_volume_id: Uuid,
    volume_order: i32,
    cartridge_id: Uuid,
    engine_volume_name: String,
    engine_media_id: i64,
    first_record: String,
    last_record: Option<String>,
    encoded_bytes: i64,
    engine_record_id: String,
}

fn validated_backup_volume_evidence(
    run_volumes: &[DataRunVolumeRecord],
    engine_write_metadata: &Value,
    evidence: &SingleTapePassEvidence,
) -> std::result::Result<Vec<FinalizedBackupVolume>, String> {
    if run_volumes.is_empty() {
        return Err("the backup has no ordered archive volumes".to_owned());
    }
    if run_volumes
        .iter()
        .enumerate()
        .any(|(index, volume)| volume.volume_order != i32::try_from(index + 1).unwrap_or(i32::MAX))
    {
        return Err("archive volume order is incomplete or non-contiguous".to_owned());
    }
    if run_volumes.iter().any(|volume| {
        volume.media_write_started_at.is_none()
            || matches!(
                volume.state.as_str(),
                "authorized" | "write_unverified" | "failed"
            )
    }) {
        return Err(
            "one or more archive volumes lack a durable successful write boundary".to_owned(),
        );
    }

    let Some(catalog_volumes) = engine_write_metadata
        .get("volumes")
        .and_then(Value::as_array)
    else {
        // Compatibility for pre-spanning, single-volume evidence retained by
        // older test fixtures and installations upgraded in place.
        if run_volumes.len() != 1 {
            return Err(
                "multi-volume engine evidence is missing its ordered volume list".to_owned(),
            );
        }
        let run_volume = &run_volumes[0];
        if run_volume.engine_volume_name.as_deref() != Some(&evidence.engine_volume_name) {
            return Err("the engine volume does not match the authorized first tape".to_owned());
        }
        let engine_media_id = evidence
            .engine_record_id
            .strip_prefix("bacula-media:")
            .and_then(|value| value.split(':').next())
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| "the Bacula MediaId is invalid".to_owned())?;
        return Ok(vec![FinalizedBackupVolume {
            run_volume_id: run_volume.id,
            volume_order: 1,
            cartridge_id: run_volume.cartridge_id,
            engine_volume_name: evidence.engine_volume_name.clone(),
            engine_media_id,
            first_record: evidence.first_record.clone(),
            last_record: evidence.last_record.clone(),
            encoded_bytes: i64::try_from(evidence.encoded_bytes)
                .map_err(|_| "encoded byte count exceeds the catalog range".to_owned())?,
            engine_record_id: evidence.engine_record_id.clone(),
        }]);
    };
    if catalog_volumes.len() != run_volumes.len() {
        return Err(format!(
            "Bacula reported {} archive volumes but TapeScale authorized {}",
            catalog_volumes.len(),
            run_volumes.len()
        ));
    }
    let segments = engine_write_metadata
        .get("segments")
        .and_then(Value::as_array)
        .ok_or_else(|| "multi-volume engine evidence is missing JobMedia segments".to_owned())?;
    let backup_job_id = evidence
        .engine_operation_id
        .strip_prefix("bacula-job:")
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| "the Bacula backup operation identity is invalid".to_owned())?;
    let mut finalized = Vec::with_capacity(run_volumes.len());
    for (index, (run_volume, catalog_volume)) in
        run_volumes.iter().zip(catalog_volumes.iter()).enumerate()
    {
        let volume_name = catalog_volume
            .get("volume_name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
            .ok_or_else(|| format!("archive volume {} has no valid engine label", index + 1))?;
        if run_volume.engine_volume_name.as_deref() != Some(volume_name) {
            return Err(format!(
                "archive volume {} does not match its authorized engine label",
                index + 1
            ));
        }
        let engine_media_id = catalog_volume
            .get("media_id")
            .and_then(Value::as_u64)
            .and_then(|value| i64::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("archive volume {} has no valid Bacula MediaId", index + 1))?;
        let encoded_bytes = catalog_volume
            .get("volume_bytes")
            .and_then(Value::as_u64)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(|| format!("archive volume {} has an invalid byte count", index + 1))?;
        let volume_segments = segments
            .iter()
            .filter(|segment| {
                segment.get("volume_name").and_then(Value::as_str) == Some(volume_name)
            })
            .collect::<Vec<_>>();
        let first_segment = volume_segments
            .first()
            .ok_or_else(|| format!("archive volume {} has no JobMedia segment", index + 1))?;
        let last_segment = volume_segments
            .last()
            .expect("the first segment established a non-empty list");
        let coordinate = |segment: &Value, file_key: &str, block_key: &str| {
            let file = segment.get(file_key).and_then(Value::as_u64)?;
            let block = segment.get(block_key).and_then(Value::as_u64)?;
            Some(format!("file={file}:block={block}"))
        };
        let first_record = coordinate(first_segment, "start_file", "start_block")
            .ok_or_else(|| format!("archive volume {} has invalid start coordinates", index + 1))?;
        let last_record = coordinate(last_segment, "end_file", "end_block")
            .ok_or_else(|| format!("archive volume {} has invalid end coordinates", index + 1))?;
        finalized.push(FinalizedBackupVolume {
            run_volume_id: run_volume.id,
            volume_order: run_volume.volume_order,
            cartridge_id: run_volume.cartridge_id,
            engine_volume_name: volume_name.to_owned(),
            engine_media_id,
            first_record,
            last_record: Some(last_record),
            encoded_bytes,
            engine_record_id: format!("bacula-media:{engine_media_id}:job:{backup_job_id}"),
        });
    }
    let total_encoded = finalized.iter().try_fold(0_i64, |total, volume| {
        total.checked_add(volume.encoded_bytes)
    });
    if total_encoded != i64::try_from(evidence.encoded_bytes).ok() {
        return Err("ordered archive bytes do not match the retained backup evidence".to_owned());
    }
    let first = finalized.first().expect("the volume list is non-empty");
    let last = finalized.last().expect("the volume list is non-empty");
    if evidence.engine_volume_name != first.engine_volume_name
        || evidence.engine_record_id != first.engine_record_id
        || evidence.first_record != first.first_record
        || evidence.last_record != last.last_record
    {
        return Err("summary evidence does not match the ordered Bacula volume chain".to_owned());
    }
    Ok(finalized)
}

fn validate_catalog_written_volume_chain(
    run_volumes: &[DataRunVolumeRecord],
    catalog_volume_names: &[String],
) -> std::result::Result<(), String> {
    if run_volumes.is_empty() || run_volumes.len() != catalog_volume_names.len() {
        return Err(
            "the catalog-proven volume count does not match the authorized archive".to_owned(),
        );
    }
    for (index, (volume, catalog_name)) in run_volumes
        .iter()
        .zip(catalog_volume_names.iter())
        .enumerate()
    {
        let expected_order = i32::try_from(index + 1).unwrap_or(i32::MAX);
        if volume.volume_order != expected_order
            || volume.media_write_started_at.is_none()
            || !matches!(volume.state.as_str(), "write_started" | "written")
            || volume.engine_volume_name.as_deref() != Some(catalog_name.as_str())
        {
            return Err(format!(
                "archive volume {} does not match the strict completed-backup catalog proof",
                index + 1
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinalizeSingleTapeDataResult {
    Passed {
        recovery_point_id: Uuid,
        volume_record_id: Uuid,
    },
    Rejected(String),
    FencedOut,
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
             WHERE state IN ('queued','preparing','waiting_for_media','writing','written','verifying','restoring','comparing','blocked','interrupted')
               AND finished_at IS NULL) AS active_jobs,
            ((SELECT count(*) FROM job_runs
              WHERE state IN ('waiting_for_media','blocked','interrupted','failed')
                AND (finished_at IS NULL OR finished_at >= now() - interval '7 days')) +
             (SELECT count(*) FROM movement_tasks
              WHERE state IN ('ready','departed','partially_confirmed','overdue','blocked')) +
             (SELECT count(*) FROM cartridges cartridge
              WHERE cartridge.identity_status <> 'reconciled'
                 OR cartridge.health_status IN (
                    'qualification_interrupted', 'write_unverified', 'quarantined', 'failed'
                 )
                 OR (cartridge.confirmed_location_id IS NULL AND EXISTS (
                    SELECT 1 FROM volume_records volume
                    WHERE volume.cartridge_id = cartridge.id AND volume.finalized
                 )))) AS required_actions",
    )
    .fetch_one(pool)
    .await
}

pub async fn list_required_actions(
    pool: &PgPool,
) -> Result<Vec<RequiredActionRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT action.id, action.title, action.detail, action.state,
                action.due_at, action.route
         FROM (
             SELECT movement.id::text AS id,
                    CASE movement.state
                        WHEN 'overdue' THEN 'Overdue tape movement'
                        WHEN 'blocked' THEN 'Blocked tape movement'
                        WHEN 'departed' THEN 'Confirm tape arrival'
                        WHEN 'partially_confirmed' THEN 'Complete partial tape delivery'
                        ELSE 'Tape movement ready'
                    END AS title,
                    concat_ws(' · ',
                        COALESCE(source.name, 'Unknown custody') || ' → ' || destination.name,
                        movement.external_id,
                        cardinality(movement.required_media_ids)::text || ' tape(s)') AS detail,
                    movement.state,
                    movement.due_at,
                    'movements/' || movement.id::text AS route,
                    movement.updated_at AS sort_at
             FROM movement_tasks movement
             LEFT JOIN locations source ON source.id = movement.source_location_id
             JOIN locations destination ON destination.id = movement.destination_location_id
             WHERE movement.state IN (
                 'ready', 'departed', 'partially_confirmed', 'overdue', 'blocked'
             )
             UNION ALL
             SELECT run.id::text,
                    CASE run.state
                        WHEN 'waiting_for_media' THEN 'Job is waiting for media'
                        WHEN 'interrupted' THEN 'Job was interrupted'
                        WHEN 'failed' THEN 'Job failed'
                        ELSE 'Job is blocked'
                    END,
                    concat_ws(' · ', job.external_id, run.failure_class),
                    run.state,
                    NULL::timestamptz,
                    'jobs'::text,
                    COALESCE(run.finished_at, run.started_at, run.created_at)
             FROM job_runs run
             JOIN jobs job ON job.id = run.job_id
             WHERE run.state IN ('waiting_for_media', 'blocked', 'interrupted', 'failed')
               AND (run.finished_at IS NULL OR run.finished_at >= now() - interval '7 days')
             UNION ALL
             SELECT 'media:' || cartridge.id::text,
                    CASE
                        WHEN cartridge.identity_status = 'mismatch'
                            THEN 'Resolve cartridge identity mismatch'
                        WHEN cartridge.identity_status = 'foreign'
                            THEN 'Review foreign cartridge'
                        WHEN cartridge.health_status IN (
                            'qualification_interrupted', 'write_unverified',
                            'quarantined', 'failed'
                        ) THEN 'Review quarantined cartridge'
                        WHEN cartridge.identity_status = 'unknown'
                            THEN 'Reconcile cartridge identity'
                        ELSE 'Confirm protected cartridge location'
                    END,
                    concat_ws(' · ',
                        cartridge.internal_label,
                        CASE
                            WHEN cartridge.identity_status <> 'reconciled'
                                THEN 'identity ' || cartridge.identity_status
                            WHEN cartridge.health_status IN (
                                'qualification_interrupted', 'write_unverified',
                                'quarantined', 'failed'
                            ) THEN 'health ' || cartridge.health_status
                            ELSE 'finalized content has no confirmed location'
                        END),
                    CASE
                        WHEN cartridge.identity_status IN ('mismatch', 'foreign')
                          OR cartridge.health_status IN (
                            'qualification_interrupted', 'write_unverified',
                            'quarantined', 'failed'
                          ) THEN 'blocked'
                        ELSE 'attention'
                    END,
                    NULL::timestamptz,
                    'tapes/' || cartridge.id::text,
                    cartridge.updated_at
             FROM cartridges cartridge
             WHERE cartridge.identity_status <> 'reconciled'
                OR cartridge.health_status IN (
                    'qualification_interrupted', 'write_unverified', 'quarantined', 'failed'
                )
                OR (cartridge.confirmed_location_id IS NULL AND EXISTS (
                    SELECT 1 FROM volume_records volume
                    WHERE volume.cartridge_id = cartridge.id AND volume.finalized
                ))
         ) action
         ORDER BY (action.state = 'overdue') DESC,
                  action.due_at NULLS LAST, action.sort_at DESC
         LIMIT 100",
    )
    .fetch_all(pool)
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
            j.created_at AS job_created_at,
            COALESCE(p.name, data_profile.name,
                     CASE WHEN rehearsal.id IS NOT NULL THEN 'Restore run' END) AS policy_name,
            CASE WHEN data_profile.id IS NOT NULL OR rehearsal.id IS NOT NULL
                 THEN 'file' ELSE p.representation END AS representation,
            COALESCE(s.dataset_name, rehearsal_source.dataset_name) AS dataset_name,
            latest.id AS run_id,
            latest.state, latest.attempt, latest.correlation_id,
            latest.started_at, latest.finished_at, latest.failure_class
     FROM jobs j
     LEFT JOIN protection_policies p ON p.id = j.policy_id
     LEFT JOIN single_tape_data_profiles data_profile ON data_profile.job_id = j.id
     LEFT JOIN sources s ON s.id = COALESCE(p.source_id, data_profile.source_id)
     LEFT JOIN restore_rehearsals rehearsal ON rehearsal.job_id = j.id
     LEFT JOIN recovery_points rehearsal_point
       ON rehearsal_point.id = rehearsal.recovery_point_id
     LEFT JOIN sources rehearsal_source ON rehearsal_source.id = rehearsal_point.source_id
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
             UNION ALL
             SELECT expected_path FROM single_tape_data_runs WHERE state = 'running'
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
             SELECT 1 FROM single_tape_data_runs
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
           AND NOT EXISTS (
               SELECT 1 FROM single_tape_data_runs data_run
               WHERE data_run.drive_id = drive_operations.drive_id
                 AND data_run.state IN ('queued', 'running'))
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
            c.media_generation, c.media_format, c.identity_status, c.health_status,
            c.worm, c.pool_id, c.reserved_by,
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
    queue_qualification_run_inner(
        pool,
        drive_id,
        expected_stable_id,
        cartridge_id,
        typed_cartridge_label,
        scratch_media_confirmed,
        destructive_overwrite_confirmed,
        None,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn queue_qualification_run_idempotent(
    pool: &PgPool,
    drive_id: Uuid,
    expected_stable_id: &str,
    cartridge_id: Uuid,
    typed_cartridge_label: &str,
    scratch_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
    idempotency_key: &str,
    request_fingerprint: &str,
    creation_enabled: bool,
) -> Result<QueueQualificationRunResult, sqlx::Error> {
    queue_qualification_run_inner(
        pool,
        drive_id,
        expected_stable_id,
        cartridge_id,
        typed_cartridge_label,
        scratch_media_confirmed,
        destructive_overwrite_confirmed,
        Some((idempotency_key, request_fingerprint)),
        creation_enabled,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn queue_qualification_run_inner(
    pool: &PgPool,
    drive_id: Uuid,
    expected_stable_id: &str,
    cartridge_id: Uuid,
    typed_cartridge_label: &str,
    scratch_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
    idempotency: Option<(&str, &str)>,
    creation_enabled: bool,
) -> Result<QueueQualificationRunResult, sqlx::Error> {
    if let Some((key, fingerprint)) = idempotency {
        if key.is_empty()
            || key.len() > 256
            || key.chars().any(char::is_control)
            || fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Ok(QueueQualificationRunResult::Blocked(
                "the qualification idempotency key or request fingerprint is invalid".to_owned(),
            ));
        }
    }
    let mut transaction = pool.begin().await?;
    if let Some((key, fingerprint)) = idempotency {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(format!("tapescale:qualification:{key}"))
            .execute(&mut *transaction)
            .await?;
        let existing = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, request_fingerprint
             FROM qualification_runs
             WHERE idempotency_key = $1
             FOR UPDATE",
        )
        .bind(key)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some((id, existing_fingerprint)) = existing {
            if existing_fingerprint != fingerprint {
                transaction.rollback().await?;
                return Ok(QueueQualificationRunResult::IdempotencyConflict);
            }
            let run = sqlx::query_as(&format!("{QUALIFICATION_RUN_SELECT} WHERE id = $1"))
                .bind(id)
                .fetch_one(&mut *transaction)
                .await?;
            transaction.commit().await?;
            return Ok(QueueQualificationRunResult::Existing(Box::new(run)));
        }
        if !creation_enabled {
            transaction.rollback().await?;
            return Ok(QueueQualificationRunResult::Blocked(
                "destructive lab tape qualification is disabled; existing requests remain available for idempotent replay"
                    .to_owned(),
            ));
        }
    }
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
             SELECT 1 FROM single_tape_data_runs
             WHERE (drive_id = $1 OR cartridge_id = $2) AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM resource_leases
             WHERE ((resource_kind = 'drive' AND resource_id = $1::text)
                 OR (resource_kind = 'media' AND resource_id = $2::text))
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
             idempotency_key, request_fingerprint, requested_by_label, correlation_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 $11, $12, $13, $14, $15, $16, $17, 'local-api', $18)",
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
    .bind(idempotency.map(|(key, _)| key))
    .bind(idempotency.map(|(_, fingerprint)| fingerprint))
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
        sqlx::query(
            "UPDATE cartridges
             SET on_tape_identity = NULL, identity_status = 'unknown',
                 health_status = 'qualification_interrupted', updated_at = now()
             WHERE id = $1",
        )
        .bind(run.cartridge_id)
        .execute(&mut *transaction)
        .await?;
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
               WHERE ((l.resource_kind = 'drive'
                        AND l.resource_id = qualification_runs.drive_id::text)
                   OR (l.resource_kind = 'media'
                        AND l.resource_id = qualification_runs.cartridge_id::text))
                 AND l.released_at IS NULL AND l.expires_at > now())
           AND NOT EXISTS (
               SELECT 1 FROM single_tape_data_runs data_run
               WHERE (data_run.drive_id = qualification_runs.drive_id
                   OR data_run.cartridge_id = qualification_runs.cartridge_id)
                 AND data_run.state IN ('queued', 'running'))
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
        "SELECT EXISTS(
             SELECT 1 FROM drive_operations
             WHERE drive_id = $1 AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM single_tape_data_runs
             WHERE (drive_id = $1 OR cartridge_id = $2)
               AND state IN ('queued', 'running'))",
    )
    .bind(run.drive_id)
    .bind(run.cartridge_id)
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
    let cartridge_health = if state == QualificationTerminalState::Passed {
        "qualified_scratch"
    } else {
        "qualification_failed"
    };
    sqlx::query(
        "UPDATE cartridges
         SET on_tape_identity = NULL, identity_status = 'unknown',
             health_status = $1, updated_at = now()
         WHERE id = $2",
    )
    .bind(cartridge_health)
    .bind(claim.run.cartridge_id)
    .execute(&mut *transaction)
    .await?;
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
        return Some("WORM media cannot be used for a destructive tape workflow".to_owned());
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

const SINGLE_TAPE_DATA_PROFILE_SELECT: &str = "SELECT id, job_id, source_id,
            source_snapshot_id, name, source_subpath, restore_subpath,
            source_mount_path, restore_root_path, logical_root,
            engine_name, engine_version, adapter_version, format_version,
            immutable_snapshot_confirmed, enabled, production_eligible,
            workflow_kind, policy_id, media_pool_id, schedule_enabled, retention_days,
            created_at, updated_at
     FROM single_tape_data_profiles";

const SINGLE_TAPE_DATA_RUN_SELECT: &str = "SELECT id, profile_id, job_run_id,
            source_snapshot_id, drive_id, cartridge_id,
            expected_stable_id, expected_path, expected_vendor, expected_model,
            expected_serial, expected_wwn, expected_firmware,
            expected_cartridge_label, expected_media_generation, expected_media_format,
            source_mount_path, restore_root_path, logical_root,
            engine_name, engine_version, adapter_version, format_version,
            scratch_media_confirmed, destructive_overwrite_confirmed,
            encryption_deviation_confirmed,
            media_write_started, media_write_started_at,
            workflow_kind, trigger_kind, requested_level, scheduled_occurrence,
            retention_days, compression_mode, resolved_compression, spanning_enabled,
            cancel_requested_at, cancel_requested_by_label,
            state, requested_by_label, correlation_id,
            drive_lease_id, drive_fencing_token, media_lease_id, media_fencing_token,
            attempt, result, created_at, started_at, finished_at
     FROM single_tape_data_runs";

const DATA_RUN_VOLUME_SELECT: &str = "SELECT id, single_tape_data_run_id,
            volume_order, cartridge_id, expected_cartridge_label,
            expected_media_generation, expected_media_format,
            engine_volume_name, engine_media_id, state, media_lease_id,
            media_fencing_token, media_write_started_at, written_at, verified_at,
            encoded_bytes, first_record, last_record, created_at, updated_at
     FROM single_tape_data_run_volumes";

const DATA_RUN_MEDIA_REQUEST_SELECT: &str = "SELECT id, single_tape_data_run_id,
            request_sequence, phase, volume_order, expected_cartridge_id,
            expected_engine_volume_name, state, authorized_cartridge_id,
            requested_at, authorized_at, loaded_at, completed_at,
            confirmation_evidence
     FROM single_tape_data_media_requests";

#[derive(Clone, Debug, FromRow)]
struct SingleTapeSnapshotBinding {
    id: Uuid,
    source_id: Uuid,
    snapshot_guid: String,
    snapshot_name: String,
    logical_root: String,
    physical_path: Option<String>,
    released_at: Option<DateTime<Utc>>,
}

pub async fn create_single_tape_data_profile(
    pool: &PgPool,
    profile: &NewSingleTapeDataProfile,
) -> Result<CreateSingleTapeDataProfileResult, sqlx::Error> {
    let name = profile.name.trim();
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Ok(CreateSingleTapeDataProfileResult::Blocked(
            "profile name must contain 1 to 128 printable characters".to_owned(),
        ));
    }
    if !profile.immutable_snapshot_confirmed {
        return Ok(CreateSingleTapeDataProfileResult::Blocked(
            "the selected source must be verified as an immutable mounted snapshot or expendable read-only fixture"
                .to_owned(),
        ));
    }
    let source_subpath = match normalize_single_tape_subpath(&profile.source_subpath) {
        Ok(path) => path,
        Err(reason) => return Ok(CreateSingleTapeDataProfileResult::Blocked(reason)),
    };
    let restore_subpath = match normalize_single_tape_subpath(&profile.restore_subpath) {
        Ok(path) => path,
        Err(reason) => return Ok(CreateSingleTapeDataProfileResult::Blocked(reason)),
    };
    for (label, path) in [
        ("source mount", profile.source_mount_path.as_str()),
        ("restore root", profile.restore_root_path.as_str()),
    ] {
        if let Some(reason) = single_tape_path_blocker(path, label) {
            return Ok(CreateSingleTapeDataProfileResult::Blocked(reason));
        }
    }
    let source_path = Path::new(&profile.source_mount_path);
    let restore_path = Path::new(&profile.restore_root_path);
    if !source_path.ends_with(Path::new(&source_subpath))
        || !restore_path.ends_with(Path::new(&restore_subpath))
    {
        return Ok(CreateSingleTapeDataProfileResult::Blocked(
            "resolved source and restore paths must end in their normalized contained subpaths"
                .to_owned(),
        ));
    }
    if source_path.starts_with(restore_path) || restore_path.starts_with(source_path) {
        return Ok(CreateSingleTapeDataProfileResult::Blocked(
            "the source mount and restore root must be separate, non-nested paths".to_owned(),
        ));
    }
    let logical_root = format!("/{source_subpath}");

    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('tapescale:g0.2-local-source-binding'))")
        .execute(&mut *transaction)
        .await?;
    let name_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM single_tape_data_profiles WHERE name = $1)",
    )
    .bind(name)
    .fetch_one(&mut *transaction)
    .await?;
    if name_exists {
        transaction.rollback().await?;
        return Ok(CreateSingleTapeDataProfileResult::Blocked(
            "a single-tape source profile already uses that name".to_owned(),
        ));
    }

    let profile_id = Uuid::now_v7();
    let installation_id = match sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM installations
         WHERE name = 'G0.2 local mounted-source laboratory' AND mode = 'simulation'
         ORDER BY created_at LIMIT 1 FOR UPDATE",
    )
    .fetch_optional(&mut *transaction)
    .await?
    {
        Some(id) => id,
        None => {
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO installations (name, site_timezone, mode)
                 VALUES ('G0.2 local mounted-source laboratory', 'UTC', 'simulation')
                 RETURNING id",
            )
            .fetch_one(&mut *transaction)
            .await?
        }
    };
    let truenas_id = match sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM truenas_instances
         WHERE credential_secret_ref = 'g0.2-local-mounted-source:no-api'
         ORDER BY created_at LIMIT 1 FOR UPDATE",
    )
    .fetch_optional(&mut *transaction)
    .await?
    {
        Some(id) => id,
        None => {
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO truenas_instances
                    (name, websocket_url, certificate_fingerprint,
                     api_version, software_version, credential_secret_ref)
                 VALUES ('G0.2 local read-only mount (not API connected)',
                         'wss://local-mounted-source.invalid',
                         'not-applicable-local-mounted-source',
                         'not-connected', 'verified-read-only-mounted-fixture',
                         'g0.2-local-mounted-source:no-api')
                 RETURNING id",
            )
            .fetch_one(&mut *transaction)
            .await?
        }
    };
    let source_scope_hash =
        sqlx::query_scalar::<_, String>("SELECT encode(digest($1::text, 'sha256'), 'hex')")
            .bind(format!(
                "g0.2-mounted-source-v1\n{source_subpath}\n{}\n{logical_root}",
                profile.source_mount_path
            ))
            .fetch_one(&mut *transaction)
            .await?;
    let dataset_guid = format!("g0.2-mounted:{source_scope_hash}");
    let source_id = match sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM sources
         WHERE truenas_instance_id = $1 AND dataset_guid = $2
         FOR UPDATE",
    )
    .bind(truenas_id)
    .bind(&dataset_guid)
    .fetch_optional(&mut *transaction)
    .await?
    {
        Some(id) => id,
        None => {
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO sources
                    (truenas_instance_id, dataset_guid, dataset_name, source_type,
                     logical_root, selection, scope_hash, enabled)
                 VALUES ($1, $2, $3, 'filesystem', $4, $5, $6, true)
                 RETURNING id",
            )
            .bind(truenas_id)
            .bind(&dataset_guid)
            .bind(&source_subpath)
            .bind(&logical_root)
            .bind(json!({
                "binding": "verified_read_only_mounted_source",
                "source_subpath": source_subpath,
                "physical_path": profile.source_mount_path,
                "immutable_snapshot_confirmed": true,
                "production_eligible": false
            }))
            .bind(&source_scope_hash)
            .fetch_one(&mut *transaction)
            .await?
        }
    };
    let base_snapshot_guid = format!("g0.2-attested:{source_scope_hash}");
    let existing_snapshot = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM snapshots
         WHERE source_id = $1 AND snapshot_guid = $2 AND released_at IS NULL
         FOR UPDATE",
    )
    .bind(source_id)
    .bind(&base_snapshot_guid)
    .fetch_optional(&mut *transaction)
    .await?;
    let source_snapshot_id = if let Some(id) = existing_snapshot {
        id
    } else {
        let snapshot_guid = if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM snapshots WHERE source_id = $1 AND snapshot_guid = $2)",
        )
        .bind(source_id)
        .bind(&base_snapshot_guid)
        .fetch_one(&mut *transaction)
        .await?
        {
            format!("{base_snapshot_guid}:{profile_id}")
        } else {
            base_snapshot_guid
        };
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO snapshots
                (source_id, snapshot_guid, snapshot_name, physical_path, logical_root,
                 consistency, application_owned, created_at)
             VALUES ($1, $2, $3, $4, $5, 'crash_consistent', false, now())
             RETURNING id",
        )
        .bind(source_id)
        .bind(snapshot_guid)
        .bind(format!("verified-read-only-mounted:{source_subpath}"))
        .bind(&profile.source_mount_path)
        .bind(&logical_root)
        .fetch_one(&mut *transaction)
        .await?
    };
    let job_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO jobs (id, external_id, kind, priority)
         VALUES ($1, $2, 'backup', 50)",
    )
    .bind(job_id)
    .bind(format!("g0.2-file-roundtrip-{profile_id}"))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO single_tape_data_profiles
            (id, job_id, source_id, source_snapshot_id, name,
             source_subpath, restore_subpath, source_mount_path, restore_root_path,
             logical_root, immutable_snapshot_confirmed)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(profile_id)
    .bind(job_id)
    .bind(source_id)
    .bind(source_snapshot_id)
    .bind(name)
    .bind(&source_subpath)
    .bind(&restore_subpath)
    .bind(&profile.source_mount_path)
    .bind(&profile.restore_root_path)
    .bind(&logical_root)
    .bind(profile.immutable_snapshot_confirmed)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         VALUES ($1, 'local-api', 'g0_2.profile.create', 'single_tape_data_profile', $2,
                 $3, 'created', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(profile_id.to_string())
    .bind(json!({
        "name": name,
        "installation_id": installation_id,
        "truenas_instance_id": truenas_id,
        "source_id": source_id,
        "source_snapshot_id": source_snapshot_id,
        "source_subpath": source_subpath,
        "restore_subpath": restore_subpath,
        "source_mount_path": profile.source_mount_path,
        "restore_root_path": profile.restore_root_path,
        "logical_root": logical_root,
        "immutable_snapshot_confirmed": true,
        "scope": "laboratory single-tape full-file physical restore exercise",
        "production_eligible": false
    }))
    .bind(Uuid::now_v7())
    .execute(&mut *transaction)
    .await?;
    let record = sqlx::query_as(&format!("{SINGLE_TAPE_DATA_PROFILE_SELECT} WHERE id = $1"))
        .bind(profile_id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(CreateSingleTapeDataProfileResult::Created(Box::new(record)))
}

pub async fn list_single_tape_data_profiles(
    pool: &PgPool,
) -> Result<Vec<SingleTapeDataProfileRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{SINGLE_TAPE_DATA_PROFILE_SELECT}
         WHERE workflow_kind = 'recovery_exercise' ORDER BY created_at DESC"
    ))
    .fetch_all(pool)
    .await
}

pub async fn list_single_tape_data_runs(
    pool: &PgPool,
) -> Result<Vec<SingleTapeDataRunRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{SINGLE_TAPE_DATA_RUN_SELECT}
         WHERE workflow_kind = 'recovery_exercise'
           AND (state IN ('queued', 'running')
            OR id IN (SELECT id FROM single_tape_data_runs
                      WHERE workflow_kind = 'recovery_exercise'
                      ORDER BY created_at DESC LIMIT 100))
         ORDER BY created_at DESC"
    ))
    .fetch_all(pool)
    .await
}

pub async fn get_single_tape_data_run(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<SingleTapeDataRunRecord>, sqlx::Error> {
    sqlx::query_as(&format!("{SINGLE_TAPE_DATA_RUN_SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn data_run_volumes(
    pool: &PgPool,
    id: Uuid,
) -> Result<Vec<DataRunVolumeRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{DATA_RUN_VOLUME_SELECT}
         WHERE single_tape_data_run_id = $1 ORDER BY volume_order"
    ))
    .bind(id)
    .fetch_all(pool)
    .await
}

pub async fn open_data_run_media_request(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<DataRunMediaRequestRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT}
         WHERE single_tape_data_run_id = $1
           AND state IN ('pending', 'authorized', 'loaded')
         ORDER BY request_sequence DESC LIMIT 1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn replayable_data_run_media_request(
    pool: &PgPool,
    id: Uuid,
    volume_order: i32,
    cartridge_id: Uuid,
    confirmation_evidence: &Value,
) -> Result<Option<DataRunMediaRequestRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT}
         WHERE single_tape_data_run_id = $1 AND volume_order = $2
           AND authorized_cartridge_id = $3
           AND state IN ('authorized', 'loaded', 'completed')
           AND confirmation_evidence =
               jsonb_build_object('phase', phase) || $4::jsonb
         ORDER BY request_sequence DESC LIMIT 1"
    ))
    .bind(id)
    .bind(volume_order)
    .bind(cartridge_id)
    .bind(confirmation_evidence)
    .fetch_optional(pool)
    .await
}

pub async fn single_tape_data_artifacts(
    pool: &PgPool,
    id: Uuid,
) -> Result<Vec<SingleTapeDataArtifactRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, artifact_kind, storage_ref, sha256, size_bytes, metadata, created_at
         FROM single_tape_data_artifacts
         WHERE single_tape_data_run_id = $1 ORDER BY created_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

pub async fn cancel_single_tape_data_run(
    pool: &PgPool,
    id: Uuid,
    actor_label: &str,
    reason: &str,
) -> Result<CancelSingleTapeDataRunResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let run = sqlx::query_as::<_, SingleTapeDataRunRecord>(&format!(
        "{SINGLE_TAPE_DATA_RUN_SELECT} WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(run) = run else {
        transaction.rollback().await?;
        return Ok(CancelSingleTapeDataRunResult::NotFound);
    };
    if !matches!(run.state.as_str(), "queued" | "running") {
        transaction.rollback().await?;
        return Ok(CancelSingleTapeDataRunResult::Terminal(Box::new(run)));
    }
    let actor_label = actor_label.trim();
    let actor_label = if actor_label.is_empty() {
        "local-api"
    } else {
        actor_label
    };
    if run.state == "queued" {
        sqlx::query(
            "UPDATE single_tape_data_runs
             SET state = 'cancelled', cancel_requested_at = COALESCE(cancel_requested_at, now()),
                 cancel_requested_by_label = COALESCE(cancel_requested_by_label, $1),
                 result = $2, started_at = COALESCE(started_at, now()), finished_at = now()
             WHERE id = $3 AND state = 'queued'",
        )
        .bind(actor_label)
        .bind(json!({
            "detail": "cancelled before the worker claimed the drive or touched media",
            "reason": reason,
            "media_write_started": false,
            "position_uncertain": false
        }))
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE job_runs
             SET state = 'cancelled', failure_class = 'operator_cancelled',
                 failure_detail = $1, finished_at = now()
             WHERE id = $2 AND state = 'queued' AND finished_at IS NULL",
        )
        .bind(json!({
            "detail": "operator cancelled before media access",
            "reason": reason
        }))
        .bind(run.job_run_id)
        .execute(&mut *transaction)
        .await?;
        append_job_transition(
            &mut transaction,
            run.job_run_id,
            Some("queued"),
            "cancelled",
            "operator cancelled before media access",
            json!({
                "reason": reason,
                "media_write_started": false,
                "position_uncertain": false
            }),
            run.correlation_id,
        )
        .await?;
        append_single_tape_data_event(
            &mut transaction,
            run.id,
            run.correlation_id,
            "cancelled",
            json!({
                "detail": "cancelled before the worker claimed the operation",
                "reason": reason,
                "media_write_started": false,
                "position_uncertain": false
            }),
            actor_label,
        )
        .await?;
        sqlx::query(
            "UPDATE backup_requests
             SET cancelled_at = COALESCE(cancelled_at, now()),
                 cancelled_by_label = COALESCE(cancelled_by_label, $2)
             WHERE assigned_data_run_id = $1",
        )
        .bind(run.id)
        .bind(actor_label)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO backup_request_events
                (backup_request_id, event_type, evidence, actor_label, correlation_id)
             SELECT request.id, 'cancelled', $2, $3, request.correlation_id
             FROM backup_requests request
             WHERE request.assigned_data_run_id = $1",
        )
        .bind(run.id)
        .bind(json!({
            "detail": "cancelled after media assignment and before worker claim",
            "reason": reason,
            "media_write_started": false,
            "tape_command_dispatched": false
        }))
        .bind(actor_label)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE cartridges SET reserved_by = NULL, updated_at = now()
             WHERE id = $1 AND reserved_by = $2",
        )
        .bind(run.cartridge_id)
        .bind(format!("single_tape_data:{}", run.id))
        .execute(&mut *transaction)
        .await?;
        let updated = sqlx::query_as(&format!("{SINGLE_TAPE_DATA_RUN_SELECT} WHERE id = $1"))
            .bind(run.id)
            .fetch_one(&mut *transaction)
            .await?;
        transaction.commit().await?;
        return Ok(CancelSingleTapeDataRunResult::Cancelled(Box::new(updated)));
    }

    if run.cancel_requested_at.is_none() {
        sqlx::query(
            "UPDATE single_tape_data_runs
             SET cancel_requested_at = now(), cancel_requested_by_label = $1
             WHERE id = $2 AND state = 'running' AND cancel_requested_at IS NULL",
        )
        .bind(actor_label)
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        append_single_tape_data_event(
            &mut transaction,
            run.id,
            run.correlation_id,
            "cancel_requested",
            json!({
                "detail": "operator requested cancellation; the worker will stop at the next safe observation boundary",
                "reason": reason,
                "media_write_started": run.media_write_started
            }),
            actor_label,
        )
        .await?;
    }
    let updated = sqlx::query_as(&format!("{SINGLE_TAPE_DATA_RUN_SELECT} WHERE id = $1"))
        .bind(run.id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(CancelSingleTapeDataRunResult::CancellationRequested(
        Box::new(updated),
    ))
}

pub async fn single_tape_data_cancel_requested(
    pool: &PgPool,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT cancel_requested_at IS NOT NULL
         FROM single_tape_data_runs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
}

pub async fn single_tape_data_events(
    pool: &PgPool,
    id: Uuid,
) -> Result<Vec<SingleTapeDataEventRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, event_type, evidence, correlation_id, occurred_at
         FROM single_tape_data_events
         WHERE single_tape_data_run_id = $1 ORDER BY occurred_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn queue_single_tape_data_run(
    pool: &PgPool,
    profile_id: Uuid,
    drive_id: Uuid,
    expected_stable_id: &str,
    cartridge_id: Uuid,
    typed_cartridge_label: &str,
    scratch_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
    encryption_deviation_confirmed: bool,
) -> Result<QueueSingleTapeDataRunResult, sqlx::Error> {
    queue_single_tape_data_run_inner(
        pool,
        profile_id,
        drive_id,
        expected_stable_id,
        cartridge_id,
        typed_cartridge_label,
        scratch_media_confirmed,
        destructive_overwrite_confirmed,
        encryption_deviation_confirmed,
        None,
    )
    .await
}

#[derive(Clone, Debug)]
struct ExistingBackupRequest {
    request_id: Uuid,
    job_run_id: Uuid,
    trigger_kind: String,
    scheduled_occurrence: Option<String>,
    retention_days: i32,
}

const BACKUP_READINESS_DRIVE_LIMIT: usize = 16;
const BACKUP_READINESS_CARTRIDGE_LIMIT: usize = 256;
const BACKUP_READINESS_PAIR_LIMIT: usize =
    BACKUP_READINESS_DRIVE_LIMIT * BACKUP_READINESS_CARTRIDGE_LIMIT;
const BACKUP_READINESS_CANDIDATE_LIMIT: usize = 256;
const BACKUP_READINESS_BLOCKER_LIMIT: usize = 256;

fn backup_media_readiness_limits() -> BackupMediaReadinessLimits {
    BackupMediaReadinessLimits {
        drives: BACKUP_READINESS_DRIVE_LIMIT,
        cartridges: BACKUP_READINESS_CARTRIDGE_LIMIT,
        pairs: BACKUP_READINESS_PAIR_LIMIT,
        candidates: BACKUP_READINESS_CANDIDATE_LIMIT,
        blockers: BACKUP_READINESS_BLOCKER_LIMIT,
    }
}

fn unevaluated_backup_media_readiness_counts(blocker_count: usize) -> BackupMediaReadinessCounts {
    BackupMediaReadinessCounts {
        drives_total: 0,
        drives_evaluated: 0,
        cartridges_total: 0,
        cartridges_evaluated: 0,
        pairs_evaluated: 0,
        candidates_matched: 0,
        candidates_returned: 0,
        blockers_matched: blocker_count,
        blockers_returned: blocker_count,
    }
}

#[must_use]
pub fn blocked_backup_media_readiness(
    profile_id: Uuid,
    backup_request_id: Option<Uuid>,
    blocker: BackupMediaReadinessBlocker,
) -> BackupMediaReadiness {
    BackupMediaReadiness {
        state: "blocked".to_owned(),
        profile_id,
        backup_request_id,
        candidates: Vec::new(),
        recommended_candidate: None,
        blockers: vec![blocker],
        counts: unevaluated_backup_media_readiness_counts(1),
        limits: backup_media_readiness_limits(),
        truncated: BackupMediaReadinessTruncation::default(),
    }
}

#[must_use]
pub fn assigned_backup_media_readiness(
    profile_id: Uuid,
    backup_request_id: Uuid,
) -> BackupMediaReadiness {
    BackupMediaReadiness {
        state: "assigned".to_owned(),
        profile_id,
        backup_request_id: Some(backup_request_id),
        candidates: Vec::new(),
        recommended_candidate: None,
        blockers: Vec::new(),
        counts: unevaluated_backup_media_readiness_counts(0),
        limits: backup_media_readiness_limits(),
        truncated: BackupMediaReadinessTruncation::default(),
    }
}

fn backup_media_blocker(
    code: &str,
    scope: &str,
    resource_id: Option<Uuid>,
    related_resource_id: Option<Uuid>,
    message: impl Into<String>,
    action: &str,
) -> BackupMediaReadinessBlocker {
    BackupMediaReadinessBlocker {
        code: code.to_owned(),
        scope: scope.to_owned(),
        resource_id,
        related_resource_id,
        message: message.into(),
        action: action.to_owned(),
    }
}

fn drive_readiness_code(reason: &str) -> (&'static str, &'static str) {
    if reason.contains("not been reconciled") {
        ("drive_identity_unconfirmed", "reconcile_drive")
    } else if reason.contains("trusted path") {
        ("drive_path_changed", "reconcile_drive")
    } else if reason.contains("observation is stale") {
        ("drive_observation_stale", "refresh")
    } else if reason.contains("not present") {
        ("drive_not_present", "connect_drive")
    } else if reason.contains("cannot access") {
        ("drive_inaccessible", "check_device_access")
    } else if reason.contains("verified stable") || reason.contains("more than one configured") {
        ("drive_identity_ambiguous", "reconcile_drive")
    } else if reason.contains("not ready") || reason.contains("no ready cartridge") {
        ("tape_not_loaded", "load_tape")
    } else if reason.contains("tape-motion") {
        ("drive_position_uncertain", "reconcile_drive")
    } else {
        ("drive_io_busy", "wait")
    }
}

fn cartridge_readiness_code(reason: &str) -> (&'static str, &'static str) {
    if reason.contains("WORM") {
        ("media_worm", "choose_other_tape")
    } else if reason.contains("protected content") || reason.contains("active hold") {
        ("media_protected", "open_media")
    } else if reason.contains("foreign") || reason.contains("mismatched") {
        ("media_identity_conflict", "reconcile_media")
    } else if reason.contains("reserved") {
        ("media_reserved", "wait")
    } else {
        ("media_ineligible", "open_media")
    }
}

fn missing_readback_qualification_blocker(
    drive_id: Uuid,
    cartridge_id: Uuid,
) -> BackupMediaReadinessBlocker {
    backup_media_blocker(
        "readback_setup_evidence_missing",
        "pair",
        Some(drive_id),
        Some(cartridge_id),
        "the retained setup evidence for this exact drive and archive tape is missing; do not run a destructive tape-preparation test on archive media",
        "open_backup",
    )
}

fn readback_identity_status_blocker(identity_status: &str) -> Option<&'static str> {
    matches!(identity_status, "foreign" | "mismatch")
        .then_some("the archive tape has a known foreign or mismatched identity")
}

/// Returns a live preview for the exact ordered archive volume requested by a
/// Bacula full-readback job. Already-written media is intentionally eligible,
/// but the retained hardware binding, drive motion gates, worker lease, and
/// exact passed drive/cartridge qualification must all still hold. The media
/// authorization transaction repeats these checks under locks.
pub async fn backup_readback_media_readiness(
    pool: &PgPool,
    profile_id: Uuid,
    backup_request_id: Uuid,
    data_run_id: Uuid,
    media_request_id: Uuid,
) -> Result<BackupMediaReadinessResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let run = sqlx::query_as::<_, SingleTapeDataRunRecord>(&format!(
        "{SINGLE_TAPE_DATA_RUN_SELECT} WHERE id = $1 AND profile_id = $2"
    ))
    .bind(data_run_id)
    .bind(profile_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(run) = run else {
        transaction.rollback().await?;
        return Ok(BackupMediaReadinessResult::NotFound);
    };
    let media_request = sqlx::query_as::<_, DataRunMediaRequestRecord>(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT}
         WHERE id = $1 AND single_tape_data_run_id = $2"
    ))
    .bind(media_request_id)
    .bind(run.id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(media_request) = media_request else {
        transaction.rollback().await?;
        return Ok(BackupMediaReadinessResult::NotFound);
    };
    let volume = sqlx::query_as::<_, DataRunVolumeRecord>(&format!(
        "{DATA_RUN_VOLUME_SELECT}
         WHERE single_tape_data_run_id = $1 AND volume_order = $2"
    ))
    .bind(run.id)
    .bind(media_request.volume_order)
    .fetch_optional(&mut *transaction)
    .await?;
    let drive = sqlx::query_as::<_, DriveRecord>(&format!("{DRIVE_SELECT} WHERE d.id = $1"))
        .bind(run.drive_id)
        .fetch_optional(&mut *transaction)
        .await?;
    let cartridge = if let Some(volume) = &volume {
        sqlx::query_as::<_, QualificationCartridge>(&format!(
            "{QUALIFICATION_CARTRIDGE_SELECT} WHERE c.id = $1"
        ))
        .bind(volume.cartridge_id)
        .fetch_optional(&mut *transaction)
        .await?
    } else {
        None
    };
    let parent_waiting = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM backup_requests request
             JOIN job_runs job ON job.id = request.job_run_id
             WHERE request.id = $1 AND request.profile_id = $2
               AND request.assigned_data_run_id = $3
               AND request.cancelled_at IS NULL
               AND job.id = $4 AND job.state = 'waiting_for_media'
               AND job.finished_at IS NULL)",
    )
    .bind(backup_request_id)
    .bind(profile_id)
    .bind(run.id)
    .bind(run.job_run_id)
    .fetch_one(&mut *transaction)
    .await?;
    let drive_lease_live = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM resource_leases lease
             WHERE lease.id = $1 AND lease.resource_kind = 'drive'
               AND lease.resource_id = $2::text
               AND lease.fencing_token = $3
               AND lease.released_at IS NULL AND lease.expires_at > now())",
    )
    .bind(run.drive_lease_id)
    .bind(run.drive_id)
    .bind(run.drive_fencing_token)
    .fetch_one(&mut *transaction)
    .await?;
    let volume_lease_live = if let Some(volume) = &volume {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1
                   FROM resource_leases media_lease
                   JOIN resource_leases drive_lease ON drive_lease.id = $1
                  WHERE media_lease.id = $2
                    AND media_lease.resource_kind = 'media'
                    AND media_lease.resource_id = $3::text
                    AND media_lease.fencing_token = $4
                    AND media_lease.owner_id = drive_lease.owner_id
                    AND media_lease.released_at IS NULL
                    AND media_lease.expires_at > now()
                    AND drive_lease.released_at IS NULL
                    AND drive_lease.expires_at > now())",
        )
        .bind(run.drive_lease_id)
        .bind(volume.media_lease_id)
        .bind(volume.cartridge_id)
        .bind(volume.media_fencing_token)
        .fetch_one(&mut *transaction)
        .await?
    } else {
        false
    };

    let mut blockers = Vec::new();
    if run.workflow_kind != "backup"
        || run.state != "running"
        || media_request.phase != "full_readback"
        || media_request.state != "pending"
        || !parent_waiting
    {
        blockers.push(backup_media_blocker(
            "request_not_waiting_for_media",
            "request",
            Some(backup_request_id),
            Some(media_request.id),
            "the backup is no longer waiting for this readback tape",
            "open_backup",
        ));
    }
    if !drive_lease_live {
        blockers.push(backup_media_blocker(
            "resource_lease_lost",
            "drive",
            Some(run.drive_id),
            None,
            "the backup worker no longer owns the tape drive",
            "refresh",
        ));
    }
    if !volume_lease_live {
        blockers.push(backup_media_blocker(
            "media_lease_lost",
            "media",
            volume.as_ref().map(|volume| volume.cartridge_id),
            volume.as_ref().map(|volume| volume.id),
            "the backup worker no longer owns the requested archive tape",
            "refresh",
        ));
    }
    if volume.as_ref().is_none_or(|volume| {
        media_request.expected_cartridge_id != Some(volume.cartridge_id)
            || media_request.expected_engine_volume_name.as_deref()
                != volume.engine_volume_name.as_deref()
            || !matches!(volume.state.as_str(), "written" | "verified")
    }) {
        blockers.push(backup_media_blocker(
            "archive_volume_mismatch",
            "media",
            media_request.expected_cartridge_id,
            Some(media_request.id),
            "the requested readback tape no longer matches the retained archive order",
            "open_backup",
        ));
    }

    let mut qualification_run_id = None;
    if let Some(drive) = &drive {
        if drive.stable_id != run.expected_stable_id
            || drive.observed_path.as_deref() != Some(run.expected_path.as_str())
            || drive.vendor != run.expected_vendor
            || drive.model != run.expected_model
            || drive.serial != run.expected_serial
            || drive.wwn != run.expected_wwn
            || drive.firmware != run.expected_firmware
        {
            blockers.push(backup_media_blocker(
                "drive_binding_changed",
                "drive",
                Some(drive.id),
                None,
                "the tape drive no longer matches the hardware binding retained by this backup",
                "reconcile_drive",
            ));
        } else if let Some(reason) = drive_motion_blocker(drive) {
            let (code, action) = drive_readiness_code(&reason);
            blockers.push(backup_media_blocker(
                code,
                "drive",
                Some(drive.id),
                None,
                reason,
                action,
            ));
        }
    } else {
        blockers.push(backup_media_blocker(
            "drive_not_present",
            "drive",
            Some(run.drive_id),
            None,
            "the retained tape drive is no longer registered",
            "connect_drive",
        ));
    }

    if let (Some(drive), Some(volume), Some(cartridge)) = (&drive, &volume, &cartridge) {
        if let Some(reason) = readback_identity_status_blocker(&cartridge.identity_status) {
            blockers.push(backup_media_blocker(
                "media_identity_conflict",
                "media",
                Some(cartridge.id),
                Some(volume.id),
                reason,
                "reconcile_media",
            ));
        } else if cartridge.internal_label != volume.expected_cartridge_label
            || cartridge.media_generation != volume.expected_media_generation
            || cartridge.media_format != volume.expected_media_format
        {
            blockers.push(backup_media_blocker(
                "media_identity_conflict",
                "media",
                Some(cartridge.id),
                Some(volume.id),
                "the registered tape metadata no longer matches the retained archive volume",
                "reconcile_media",
            ));
        } else {
            qualification_run_id =
                single_tape_qualification_passed_id(&mut transaction, drive, cartridge.id).await?;
            if qualification_run_id.is_none() {
                blockers.push(missing_readback_qualification_blocker(
                    drive.id,
                    cartridge.id,
                ));
            }
        }
    } else if volume.is_some() && cartridge.is_none() {
        blockers.push(backup_media_blocker(
            "media_not_registered",
            "media",
            volume.as_ref().map(|volume| volume.cartridge_id),
            volume.as_ref().map(|volume| volume.id),
            "the requested archive tape is no longer registered",
            "reconcile_media",
        ));
    }

    let candidate = if blockers.is_empty() {
        let drive = drive
            .as_ref()
            .expect("readback readiness checked the drive");
        let volume = volume
            .as_ref()
            .expect("readback readiness checked the volume");
        let cartridge = cartridge
            .as_ref()
            .expect("readback readiness checked the cartridge");
        Some(BackupMediaReadinessCandidate {
            drive_id: drive.id,
            stable_id: drive.stable_id.clone(),
            device_path: drive
                .observed_path
                .clone()
                .expect("motion-safe readiness requires an observed path"),
            cartridge_id: cartridge.id,
            cartridge_label: cartridge.internal_label.clone(),
            media_generation: cartridge.media_generation,
            media_format: cartridge.media_format.clone(),
            qualification_run_id: qualification_run_id
                .expect("readback readiness requires a passed setup test"),
            required_confirmation: Some(format!(
                "LOAD {} FOR BACKUP {} READBACK VOLUME {}",
                volume.expected_cartridge_label, backup_request_id, volume.volume_order
            )),
        })
    } else {
        None
    };
    let candidates = candidate.iter().cloned().collect::<Vec<_>>();
    let blockers_matched = blockers.len();
    let candidate_count = usize::from(candidate.is_some());
    let readiness = BackupMediaReadiness {
        state: if candidate.is_some() {
            "ready".to_owned()
        } else {
            "blocked".to_owned()
        },
        profile_id,
        backup_request_id: Some(backup_request_id),
        candidates,
        recommended_candidate: candidate,
        blockers,
        counts: BackupMediaReadinessCounts {
            drives_total: i64::from(drive.is_some()),
            drives_evaluated: usize::from(drive.is_some()),
            cartridges_total: i64::from(cartridge.is_some()),
            cartridges_evaluated: usize::from(cartridge.is_some()),
            pairs_evaluated: usize::from(drive.is_some() && cartridge.is_some()),
            candidates_matched: candidate_count,
            candidates_returned: candidate_count,
            blockers_matched,
            blockers_returned: blockers_matched,
        },
        limits: backup_media_readiness_limits(),
        truncated: BackupMediaReadinessTruncation::default(),
    };
    transaction.commit().await?;
    Ok(BackupMediaReadinessResult::Readiness(Box::new(readiness)))
}

#[derive(Debug, FromRow)]
struct BackupMediaReadinessPairRow {
    drive_id: Uuid,
    cartridge_id: Uuid,
    qualification_run_id: Option<Uuid>,
    busy: bool,
}

/// Returns a read-only preview of exact media pairs that the destructive
/// assignment transaction would currently accept. The mutation and worker
/// always repeat every gate under locks; callers must not treat this preview as
/// authority to write.
pub async fn backup_media_readiness(
    pool: &PgPool,
    profile_id: Uuid,
    backup_request: Option<(Uuid, Uuid)>,
) -> Result<BackupMediaReadinessResult, sqlx::Error> {
    backup_media_readiness_inner(pool, profile_id, backup_request, None).await
}

pub async fn backup_continuation_media_readiness(
    pool: &PgPool,
    profile_id: Uuid,
    data_run_id: Uuid,
) -> Result<BackupMediaReadinessResult, sqlx::Error> {
    backup_media_readiness_inner(pool, profile_id, None, Some(data_run_id)).await
}

async fn backup_media_readiness_inner(
    pool: &PgPool,
    profile_id: Uuid,
    backup_request: Option<(Uuid, Uuid)>,
    ignored_data_run_id: Option<Uuid>,
) -> Result<BackupMediaReadinessResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let profile = sqlx::query_as::<_, SingleTapeDataProfileRecord>(&format!(
        "{SINGLE_TAPE_DATA_PROFILE_SELECT} WHERE id = $1"
    ))
    .bind(profile_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(profile) = profile else {
        transaction.rollback().await?;
        return Ok(BackupMediaReadinessResult::NotFound);
    };
    let request_id = backup_request.map(|(request_id, _)| request_id);
    let mut blockers = Vec::new();
    if let Some((request_id, job_run_id)) = backup_request {
        let waiting = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1
                 FROM backup_requests request
                 JOIN job_runs run ON run.id = request.job_run_id
                 WHERE request.id = $1 AND request.profile_id = $2
                   AND request.job_run_id = $3
                   AND request.assigned_data_run_id IS NULL
                   AND request.cancelled_at IS NULL
                   AND run.state = 'waiting_for_media'
                   AND run.finished_at IS NULL)",
        )
        .bind(request_id)
        .bind(profile_id)
        .bind(job_run_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !waiting {
            blockers.push(backup_media_blocker(
                "request_not_waiting_for_media",
                "request",
                Some(request_id),
                None,
                "the backup request is no longer waiting for a tape",
                "open_backup",
            ));
        }
    }
    if profile.workflow_kind != "backup" {
        blockers.push(backup_media_blocker(
            "profile_wrong_workflow",
            "profile",
            Some(profile_id),
            None,
            "the selected profile is not an operational backup",
            "open_backup",
        ));
    }
    if !profile.enabled {
        blockers.push(backup_media_blocker(
            "profile_disabled",
            "profile",
            Some(profile_id),
            None,
            "the selected backup is disabled",
            "open_backup",
        ));
    }
    if profile.production_eligible || !profile.immutable_snapshot_confirmed {
        blockers.push(backup_media_blocker(
            "profile_safety_contract_invalid",
            "profile",
            Some(profile_id),
            None,
            "the backup profile no longer has the required read-only snapshot safety contract",
            "choose_snapshot",
        ));
    }
    let snapshot = sqlx::query_as::<_, SingleTapeSnapshotBinding>(
        "SELECT id, source_id, snapshot_guid, snapshot_name, logical_root,
                physical_path, released_at
         FROM snapshots WHERE id = $1",
    )
    .bind(profile.source_snapshot_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if snapshot.as_ref().is_none_or(|snapshot| {
        snapshot.source_id != profile.source_id
            || snapshot.released_at.is_some()
            || snapshot.logical_root != profile.logical_root
            || snapshot.physical_path.as_deref() != Some(profile.source_mount_path.as_str())
    }) {
        blockers.push(backup_media_blocker(
            "snapshot_unavailable",
            "profile",
            Some(profile_id),
            None,
            "the saved read-only snapshot is unavailable or no longer matches this backup",
            "choose_snapshot",
        ));
    }
    if !blockers.is_empty() {
        let counts = unevaluated_backup_media_readiness_counts(blockers.len());
        transaction.commit().await?;
        return Ok(BackupMediaReadinessResult::Readiness(Box::new(
            BackupMediaReadiness {
                state: "blocked".to_owned(),
                profile_id,
                backup_request_id: request_id,
                candidates: Vec::new(),
                recommended_candidate: None,
                blockers,
                counts,
                limits: backup_media_readiness_limits(),
                truncated: BackupMediaReadinessTruncation::default(),
            },
        )));
    }

    let drives_total = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM drives")
        .fetch_one(&mut *transaction)
        .await?;
    let drives = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT}
         ORDER BY d.reconciled DESC,
                  (d.observed_path IS NOT NULL AND d.observed_path = d.expected_path) DESC,
                  d.last_observed_at DESC NULLS LAST, d.stable_id, d.id
         LIMIT $1"
    ))
    .bind(BACKUP_READINESS_DRIVE_LIMIT as i64)
    .fetch_all(&mut *transaction)
    .await?;
    let cartridges_total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM cartridges c
         WHERE $1::uuid IS NULL OR c.pool_id = $1",
    )
    .bind(profile.media_pool_id)
    .fetch_one(&mut *transaction)
    .await?;
    let cartridges = sqlx::query_as::<_, QualificationCartridge>(&format!(
        "{QUALIFICATION_CARTRIDGE_SELECT}
         WHERE $1::uuid IS NULL OR c.pool_id = $1
         ORDER BY c.worm,
                  (c.health_status IN ('write_in_progress', 'write_unverified')),
                  (c.identity_status IN ('foreign', 'mismatch')),
                  (c.reserved_by IS NOT NULL), active_holds, has_content,
                  c.internal_label, c.id
         LIMIT $2"
    ))
    .bind(profile.media_pool_id)
    .bind(BACKUP_READINESS_CARTRIDGE_LIMIT as i64)
    .fetch_all(&mut *transaction)
    .await?;
    let drives_evaluated = drives.len();
    let cartridges_evaluated = cartridges.len();
    let mut truncated = BackupMediaReadinessTruncation {
        drives: drives_total > drives_evaluated as i64,
        cartridges: cartridges_total > cartridges_evaluated as i64,
        ..BackupMediaReadinessTruncation::default()
    };
    if drives_total == 0 {
        blockers.push(backup_media_blocker(
            "no_tape_drive",
            "system",
            None,
            None,
            "no tape drive has been discovered",
            "connect_drive",
        ));
    }
    if cartridges_total == 0 {
        blockers.push(backup_media_blocker(
            if profile.media_pool_id.is_some() {
                "no_media_in_pool"
            } else {
                "no_registered_media"
            },
            "system",
            None,
            profile.media_pool_id,
            if profile.media_pool_id.is_some() {
                "no tape is registered in this backup's media pool"
            } else {
                "no scratch tape has been registered"
            },
            if profile.media_pool_id.is_some() {
                "open_media"
            } else {
                "prepare_tape"
            },
        ));
    }
    let mut ready_drives = Vec::new();
    for drive in drives {
        if let Some(reason) = drive_motion_blocker(&drive) {
            let (code, action) = drive_readiness_code(&reason);
            blockers.push(backup_media_blocker(
                code,
                "drive",
                Some(drive.id),
                None,
                reason,
                action,
            ));
        } else {
            ready_drives.push(drive);
        }
    }
    let mut ready_cartridges = Vec::new();
    for cartridge in cartridges {
        let reason = if matches!(
            cartridge.health_status.as_str(),
            "write_in_progress" | "write_unverified"
        ) {
            Some("the tape has an unverified prior write".to_owned())
        } else {
            qualification_cartridge_blocker(&cartridge, None)
        };
        if let Some(reason) = reason {
            let (code, action) = if reason.contains("unverified prior write") {
                ("media_write_unverified", "prepare_tape")
            } else {
                cartridge_readiness_code(&reason)
            };
            blockers.push(backup_media_blocker(
                code,
                "cartridge",
                Some(cartridge.id),
                None,
                reason,
                action,
            ));
        } else {
            ready_cartridges.push(cartridge);
        }
    }

    let ready_drive_ids = ready_drives
        .iter()
        .map(|drive| drive.id)
        .collect::<Vec<_>>();
    let ready_cartridge_ids = ready_cartridges
        .iter()
        .map(|cartridge| cartridge.id)
        .collect::<Vec<_>>();
    let pair_count = ready_drive_ids
        .len()
        .saturating_mul(ready_cartridge_ids.len());
    let pair_rows = if ready_drive_ids.is_empty() || ready_cartridge_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, BackupMediaReadinessPairRow>(
            "WITH drive_ids AS (
                 SELECT drive_id, drive_order
                 FROM unnest($1::uuid[]) WITH ORDINALITY AS selected(drive_id, drive_order)
             ), cartridge_ids AS (
                 SELECT cartridge_id, cartridge_order
                 FROM unnest($2::uuid[]) WITH ORDINALITY AS selected(cartridge_id, cartridge_order)
             ), pairs AS (
                 SELECT drive_id, cartridge_id, drive_order, cartridge_order
                 FROM drive_ids CROSS JOIN cartridge_ids
             )
             SELECT pair.drive_id, pair.cartridge_id,
                    CASE WHEN latest.state = 'passed' THEN latest.id END
                        AS qualification_run_id,
                    EXISTS(
                        SELECT 1 FROM drive_operations operation
                        WHERE operation.drive_id = pair.drive_id
                          AND operation.state IN ('queued', 'running')
                        UNION ALL
                        SELECT 1 FROM qualification_runs active_qualification
                        WHERE (active_qualification.drive_id = pair.drive_id
                               OR active_qualification.cartridge_id = pair.cartridge_id)
                          AND active_qualification.state IN ('queued', 'running')
                        UNION ALL
                        SELECT 1 FROM single_tape_data_runs data_run
                        WHERE (data_run.profile_id = $3
                               OR data_run.drive_id = pair.drive_id
                               OR data_run.cartridge_id = pair.cartridge_id)
                          AND ($5::uuid IS NULL OR data_run.id <> $5)
                          AND data_run.state IN ('queued', 'running')
                        UNION ALL
                        SELECT 1 FROM resource_leases lease
                        WHERE ((lease.resource_kind = 'drive'
                                AND lease.resource_id = pair.drive_id::text)
                               OR (lease.resource_kind = 'media'
                                   AND lease.resource_id = pair.cartridge_id::text))
                          AND lease.released_at IS NULL AND lease.expires_at > now()
                          AND NOT ($5::uuid IS NOT NULL AND (
                              lease.id = (SELECT drive_lease_id
                                            FROM single_tape_data_runs
                                           WHERE id = $5)
                              OR lease.id IN (
                                  SELECT media_lease_id
                                    FROM single_tape_data_run_volumes
                                   WHERE single_tape_data_run_id = $5
                                     AND media_lease_id IS NOT NULL)))
                    ) AS busy
             FROM pairs pair
             JOIN drives drive ON drive.id = pair.drive_id
             JOIN cartridges cartridge ON cartridge.id = pair.cartridge_id
             LEFT JOIN LATERAL (
                 SELECT qualification.id, qualification.state
                 FROM qualification_runs qualification
                 WHERE qualification.drive_id = drive.id
                   AND qualification.cartridge_id = cartridge.id
                   AND qualification.expected_stable_id = drive.stable_id
                   AND qualification.expected_path = drive.observed_path
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
                   AND qualification.engine_name = 'Bacula Community'
                   AND qualification.engine_version = '15.0.3'
                   AND qualification.test_kind = 'btape_test'
                   AND qualification.attempt > 0
                 ORDER BY qualification.created_at DESC, qualification.id DESC
                 LIMIT 1
             ) latest ON true
             ORDER BY pair.drive_order, pair.cartridge_order
             LIMIT $4",
        )
        .bind(&ready_drive_ids)
        .bind(&ready_cartridge_ids)
        .bind(profile_id)
        .bind(BACKUP_READINESS_PAIR_LIMIT as i64)
        .bind(ignored_data_run_id)
        .fetch_all(&mut *transaction)
        .await?
    };
    truncated.pairs = pair_count > pair_rows.len();
    let mut candidates = Vec::new();
    for pair in pair_rows {
        if pair.busy {
            blockers.push(backup_media_blocker(
                "resource_busy",
                "pair",
                Some(pair.drive_id),
                Some(pair.cartridge_id),
                "the backup, drive, or tape already has active work",
                "wait",
            ));
            continue;
        }
        let Some(qualification_run_id) = pair.qualification_run_id else {
            blockers.push(backup_media_blocker(
                "setup_test_required",
                "pair",
                Some(pair.drive_id),
                Some(pair.cartridge_id),
                "this exact drive and tape need the setup test before backup use",
                "prepare_tape",
            ));
            continue;
        };
        let drive = ready_drives
            .iter()
            .find(|drive| drive.id == pair.drive_id)
            .expect("the bounded pair query only returns selected drives");
        let cartridge = ready_cartridges
            .iter()
            .find(|cartridge| cartridge.id == pair.cartridge_id)
            .expect("the bounded pair query only returns selected cartridges");
        let required_confirmation = request_id.map(|request_id| {
            format!(
                "OVERWRITE {} FOR BACKUP {request_id}",
                cartridge.internal_label
            )
        });
        candidates.push(BackupMediaReadinessCandidate {
            drive_id: drive.id,
            stable_id: drive.stable_id.clone(),
            device_path: drive
                .observed_path
                .clone()
                .expect("drive readiness requires an observed path"),
            cartridge_id: cartridge.id,
            cartridge_label: cartridge.internal_label.clone(),
            media_generation: cartridge.media_generation,
            media_format: cartridge.media_format.clone(),
            qualification_run_id,
            required_confirmation,
        });
    }
    if truncated.drives || truncated.cartridges || truncated.pairs {
        blockers.insert(
            0,
            backup_media_blocker(
                "readiness_scan_truncated",
                "system",
                None,
                None,
                "the bounded readiness preview did not inspect the complete drive and tape inventory",
                "open_media",
            ),
        );
    }
    let candidates_matched = candidates.len();
    truncated.candidates = candidates_matched > BACKUP_READINESS_CANDIDATE_LIMIT;
    candidates.truncate(BACKUP_READINESS_CANDIDATE_LIMIT);
    let recommended_candidate = (candidates_matched == 1).then(|| candidates[0].clone());
    let state = if candidates_matched == 0 {
        "blocked"
    } else if candidates_matched == 1 {
        "ready"
    } else {
        "selection_required"
    };
    let blockers_matched = blockers.len();
    truncated.blockers = blockers_matched > BACKUP_READINESS_BLOCKER_LIMIT;
    blockers.truncate(BACKUP_READINESS_BLOCKER_LIMIT);
    let counts = BackupMediaReadinessCounts {
        drives_total,
        drives_evaluated,
        cartridges_total,
        cartridges_evaluated,
        pairs_evaluated: pair_count.min(BACKUP_READINESS_PAIR_LIMIT),
        candidates_matched,
        candidates_returned: candidates.len(),
        blockers_matched,
        blockers_returned: blockers.len(),
    };
    transaction.commit().await?;
    Ok(BackupMediaReadinessResult::Readiness(Box::new(
        BackupMediaReadiness {
            state: state.to_owned(),
            profile_id,
            backup_request_id: request_id,
            candidates,
            recommended_candidate,
            blockers,
            counts,
            limits: backup_media_readiness_limits(),
            truncated,
        },
    )))
}

#[allow(clippy::too_many_arguments)]
pub async fn attach_single_tape_backup_run(
    pool: &PgPool,
    profile_id: Uuid,
    request_id: Uuid,
    job_run_id: Uuid,
    trigger_kind: &str,
    scheduled_occurrence: Option<&str>,
    retention_days: i32,
    drive_id: Uuid,
    expected_stable_id: &str,
    cartridge_id: Uuid,
    typed_cartridge_label: &str,
    loaded_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
    encryption_deviation_confirmed: bool,
) -> Result<QueueSingleTapeDataRunResult, sqlx::Error> {
    queue_single_tape_data_run_inner(
        pool,
        profile_id,
        drive_id,
        expected_stable_id,
        cartridge_id,
        typed_cartridge_label,
        loaded_media_confirmed,
        destructive_overwrite_confirmed,
        encryption_deviation_confirmed,
        Some(ExistingBackupRequest {
            request_id,
            job_run_id,
            trigger_kind: trigger_kind.to_owned(),
            scheduled_occurrence: scheduled_occurrence.map(ToOwned::to_owned),
            retention_days,
        }),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn queue_single_tape_data_run_inner(
    pool: &PgPool,
    profile_id: Uuid,
    drive_id: Uuid,
    expected_stable_id: &str,
    cartridge_id: Uuid,
    typed_cartridge_label: &str,
    scratch_media_confirmed: bool,
    destructive_overwrite_confirmed: bool,
    encryption_deviation_confirmed: bool,
    backup_request: Option<ExistingBackupRequest>,
) -> Result<QueueSingleTapeDataRunResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let profile = sqlx::query_as::<_, SingleTapeDataProfileRecord>(&format!(
        "{SINGLE_TAPE_DATA_PROFILE_SELECT} WHERE id = $1 FOR UPDATE"
    ))
    .bind(profile_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(profile) = profile else {
        transaction.rollback().await?;
        return Ok(QueueSingleTapeDataRunResult::NotFound);
    };
    let snapshot = sqlx::query_as::<_, SingleTapeSnapshotBinding>(
        "SELECT id, source_id, snapshot_guid, snapshot_name, logical_root,
                physical_path, released_at
         FROM snapshots WHERE id = $1 FOR UPDATE",
    )
    .bind(profile.source_snapshot_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let drive = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT} WHERE d.id = $1 FOR UPDATE OF d"
    ))
    .bind(drive_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let cartridge = sqlx::query_as::<_, QualificationCartridge>(&format!(
        "{QUALIFICATION_CARTRIDGE_SELECT} WHERE c.id = $1 FOR UPDATE OF c"
    ))
    .bind(cartridge_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let (Some(snapshot), Some(drive), Some(cartridge)) = (snapshot, drive, cartridge) else {
        transaction.rollback().await?;
        return Ok(QueueSingleTapeDataRunResult::NotFound);
    };

    let is_backup = backup_request.is_some();
    let (compression_mode, resolved_compression, spanning_enabled) = if is_backup {
        let Some(policy_id) = profile.policy_id else {
            transaction.rollback().await?;
            return Ok(QueueSingleTapeDataRunResult::Blocked(
                "the backup profile has no protection policy".to_owned(),
            ));
        };
        let settings = sqlx::query_as::<_, (String, bool)>(
            "SELECT compression_mode, spanning_enabled
               FROM protection_policies WHERE id = $1 FOR SHARE",
        )
        .bind(policy_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((requested, spanning)) = settings else {
            transaction.rollback().await?;
            return Ok(QueueSingleTapeDataRunResult::Blocked(
                "the backup protection policy is unavailable".to_owned(),
            ));
        };
        let resolved = match requested.as_str() {
            "auto" => "gzip",
            "none" => "none",
            "gzip" => "gzip",
            "lzo" => "lzo",
            _ => {
                transaction.rollback().await?;
                return Ok(QueueSingleTapeDataRunResult::Blocked(
                    "the configured compression mode is unsupported".to_owned(),
                ));
            }
        };
        (requested, resolved.to_owned(), spanning)
    } else {
        ("none".to_owned(), "none".to_owned(), false)
    };
    if let Some(request) = &backup_request {
        let valid_request = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1
                 FROM backup_requests request
                 JOIN job_runs run ON run.id = request.job_run_id
                 WHERE request.id = $1 AND request.profile_id = $2
                   AND request.job_run_id = $3
                   AND request.assigned_data_run_id IS NULL
                   AND request.cancelled_at IS NULL
                   AND run.state = 'waiting_for_media'
                   AND run.finished_at IS NULL)",
        )
        .bind(request.request_id)
        .bind(profile_id)
        .bind(request.job_run_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !valid_request {
            transaction.rollback().await?;
            return Ok(QueueSingleTapeDataRunResult::Blocked(
                "the backup request is no longer waiting for media".to_owned(),
            ));
        }
    }

    let qualification_passed =
        single_tape_qualification_passed(&mut transaction, &drive, cartridge_id).await?;
    let blocker = if profile.workflow_kind
        != if is_backup {
            "backup"
        } else {
            "recovery_exercise"
        } {
        Some("the selected profile belongs to a different physical workflow".to_owned())
    } else if !profile.enabled {
        Some("the selected source profile is disabled".to_owned())
    } else if profile.production_eligible {
        Some("this test-qualified single-volume workflow cannot be production eligible".to_owned())
    } else if !profile.immutable_snapshot_confirmed {
        Some(
            "the profile lacks persisted immutable-snapshot or read-only-source verification"
                .to_owned(),
        )
    } else if snapshot.source_id != profile.source_id
        || snapshot.released_at.is_some()
        || snapshot.logical_root != profile.logical_root
        || snapshot.physical_path.as_deref() != Some(profile.source_mount_path.as_str())
    {
        Some(
            "the selected active snapshot does not match the profile source and logical root"
                .to_owned(),
        )
    } else if drive.stable_id != expected_stable_id {
        Some("the requested stable identity does not match the current drive".to_owned())
    } else if !scratch_media_confirmed
        || !destructive_overwrite_confirmed
        || !encryption_deviation_confirmed
    {
        Some("loaded-media, destructive-overwrite, and unencrypted test-slice confirmations are all required".to_owned())
    } else if cartridge.internal_label != typed_cartridge_label {
        Some("the typed cartridge label does not exactly match the selected cartridge".to_owned())
    } else if profile
        .media_pool_id
        .is_some_and(|pool_id| cartridge.pool_id != Some(pool_id))
    {
        Some("the cartridge is not assigned to the backup profile's media pool".to_owned())
    } else if let Some(reason) = drive_motion_blocker(&drive) {
        Some(reason)
    } else if let Some(reason) = qualification_cartridge_blocker(&cartridge, None) {
        Some(reason)
    } else if matches!(
        cartridge.health_status.as_str(),
        "write_in_progress" | "write_unverified"
    ) {
        Some(
            "the cartridge has an unverified prior write; reconcile or requalify it before another write"
                .to_owned(),
        )
    } else if !qualification_passed {
        Some("the exact drive, firmware, path, and cartridge need a passed physical qualification before a data write".to_owned())
    } else {
        None
    };
    if let Some(reason) = blocker {
        transaction.rollback().await?;
        return Ok(QueueSingleTapeDataRunResult::Blocked(reason));
    }

    let busy = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM drive_operations
             WHERE drive_id = $1 AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM qualification_runs
             WHERE (drive_id = $1 OR cartridge_id = $2) AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM single_tape_data_runs
             WHERE (profile_id = $3 OR drive_id = $1 OR cartridge_id = $2)
               AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM resource_leases
             WHERE ((resource_kind = 'drive' AND resource_id = $1::text)
                 OR (resource_kind = 'media' AND resource_id = $2::text))
               AND released_at IS NULL AND expires_at > now()
         )",
    )
    .bind(drive_id)
    .bind(cartridge_id)
    .bind(profile_id)
    .fetch_one(&mut *transaction)
    .await?;
    if busy {
        transaction.rollback().await?;
        return Ok(QueueSingleTapeDataRunResult::Blocked(
            "the profile, drive, or cartridge already has queued work or an active controller lease"
                .to_owned(),
        ));
    }

    let expected_path = drive
        .observed_path
        .as_deref()
        .expect("the data gate requires an observed path");
    let id = Uuid::now_v7();
    let (
        job_run_id,
        correlation_id,
        workflow_kind,
        trigger_kind,
        scheduled_occurrence,
        retention_days,
        transition_from,
    ) = if let Some(request) = &backup_request {
        let correlation_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT correlation_id FROM job_runs WHERE id = $1 FOR UPDATE",
        )
        .bind(request.job_run_id)
        .fetch_one(&mut *transaction)
        .await?;
        let updated = sqlx::query(
            "UPDATE job_runs
             SET state = 'queued', source_snapshot_id = $1
             WHERE id = $2 AND job_id = $3 AND state = 'waiting_for_media'
               AND finished_at IS NULL",
        )
        .bind(snapshot.id)
        .bind(request.job_run_id)
        .bind(profile.job_id)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(QueueSingleTapeDataRunResult::Blocked(
                "the backup request changed while media was being assigned".to_owned(),
            ));
        }
        (
            request.job_run_id,
            correlation_id,
            "backup".to_owned(),
            request.trigger_kind.clone(),
            request.scheduled_occurrence.clone(),
            request.retention_days,
            Some("waiting_for_media"),
        )
    } else {
        let job_run_id = Uuid::now_v7();
        let correlation_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO job_runs
                (id, job_id, attempt, state, correlation_id, source_snapshot_id)
             VALUES ($1, $2, 1, 'queued', $3, $4)",
        )
        .bind(job_run_id)
        .bind(profile.job_id)
        .bind(correlation_id)
        .bind(snapshot.id)
        .execute(&mut *transaction)
        .await?;
        (
            job_run_id,
            correlation_id,
            "recovery_exercise".to_owned(),
            "attended".to_owned(),
            None,
            28,
            None,
        )
    };
    sqlx::query(
        "INSERT INTO single_tape_data_runs
            (id, profile_id, job_run_id, source_snapshot_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_vendor, expected_model,
             expected_serial, expected_wwn, expected_firmware,
             expected_cartridge_label, expected_media_generation, expected_media_format,
             source_mount_path, restore_root_path, logical_root,
             engine_name, engine_version, adapter_version, format_version,
             scratch_media_confirmed, destructive_overwrite_confirmed,
             encryption_deviation_confirmed, workflow_kind, trigger_kind,
             requested_level, scheduled_occurrence, retention_days,
             compression_mode, resolved_compression, spanning_enabled,
             requested_by_label, correlation_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 $11, $12, $13, $14, $15, $16, $17, $18, $19,
                 $20, $21, $22, $23, $24, $25, $26, $27, $28,
                 'full', $29, $30, $31, $32, $33, 'local-api', $34)",
    )
    .bind(id)
    .bind(profile_id)
    .bind(job_run_id)
    .bind(snapshot.id)
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
    .bind(&profile.source_mount_path)
    .bind(&profile.restore_root_path)
    .bind(&profile.logical_root)
    .bind(&profile.engine_name)
    .bind(&profile.engine_version)
    .bind(&profile.adapter_version)
    .bind(&profile.format_version)
    .bind(scratch_media_confirmed)
    .bind(destructive_overwrite_confirmed)
    .bind(encryption_deviation_confirmed)
    .bind(&workflow_kind)
    .bind(&trigger_kind)
    .bind(&scheduled_occurrence)
    .bind(retention_days)
    .bind(&compression_mode)
    .bind(&resolved_compression)
    .bind(spanning_enabled)
    .bind(correlation_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO single_tape_data_run_volumes
            (single_tape_data_run_id, volume_order, cartridge_id,
             expected_cartridge_label, expected_media_generation,
             expected_media_format, state)
         VALUES ($1, 1, $2, $3, $4, $5, 'authorized')",
    )
    .bind(id)
    .bind(cartridge_id)
    .bind(&cartridge.internal_label)
    .bind(cartridge.media_generation)
    .bind(&cartridge.media_format)
    .execute(&mut *transaction)
    .await?;
    if let Some(request) = &backup_request {
        let assigned = sqlx::query(
            "UPDATE backup_requests SET assigned_data_run_id = $1
             WHERE id = $2 AND assigned_data_run_id IS NULL AND cancelled_at IS NULL",
        )
        .bind(id)
        .bind(request.request_id)
        .execute(&mut *transaction)
        .await?;
        if assigned.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(QueueSingleTapeDataRunResult::Blocked(
                "the backup request could not retain its exact media assignment".to_owned(),
            ));
        }
        sqlx::query(
            "INSERT INTO backup_request_events
                (backup_request_id, event_type, evidence, actor_label, correlation_id)
             VALUES ($1, 'assignment_recorded', $2, 'local-api', $3)",
        )
        .bind(request.request_id)
        .bind(json!({
            "single_tape_data_run_id": id,
            "drive_id": drive_id,
            "stable_id": expected_stable_id,
            "cartridge_id": cartridge_id,
            "cartridge_label": typed_cartridge_label,
            "assignment_only": true,
            "safety_authority": false
        }))
        .bind(correlation_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO audit_events
                (id, actor_label, action, object_type, object_id,
                 requested, result, correlation_id)
             VALUES ($1, 'local-api', 'backup.request.assign', 'backup_request',
                     $2, $3, 'queued', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(request.request_id.to_string())
        .bind(json!({
            "single_tape_data_run_id": id,
            "drive_id": drive_id,
            "cartridge_id": cartridge_id
        }))
        .bind(correlation_id)
        .execute(&mut *transaction)
        .await?;
    }
    let reservation = format!("single_tape_data:{id}");
    sqlx::query("UPDATE cartridges SET reserved_by = $1, updated_at = now() WHERE id = $2")
        .bind(&reservation)
        .bind(cartridge_id)
        .execute(&mut *transaction)
        .await?;
    append_job_transition(
        &mut transaction,
        job_run_id,
        transition_from,
        "queued",
        if is_backup {
            "exact drive and cartridge assigned to attended full backup"
        } else {
            "G0.2 single-tape full-file recovery exercise queued"
        },
        json!({
            "single_tape_data_run_id": id,
            "workflow_kind": &workflow_kind,
            "trigger_kind": &trigger_kind,
            "scheduled_occurrence": &scheduled_occurrence,
            "production_eligible": false
        }),
        correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        id,
        correlation_id,
        "queued",
        json!({
            "job_run_id": job_run_id,
            "snapshot_id": snapshot.id,
            "snapshot_guid": snapshot.snapshot_guid,
            "snapshot_name": snapshot.snapshot_name,
            "immutable_snapshot_confirmed": profile.immutable_snapshot_confirmed,
            "drive_id": drive_id,
            "stable_id": expected_stable_id,
            "path": expected_path,
            "cartridge_id": cartridge_id,
            "cartridge_label": cartridge.internal_label,
            "engine": profile.engine_name,
            "engine_version": profile.engine_version,
            "encryption_deviation_confirmed": encryption_deviation_confirmed,
            "compression_mode": &compression_mode,
            "resolved_compression": &resolved_compression,
            "spanning_enabled": spanning_enabled,
            "workflow_kind": &workflow_kind,
            "trigger_kind": &trigger_kind,
            "scheduled_occurrence": &scheduled_occurrence,
            "scope": if is_backup {
                "attended full-file backup with complete physical-media readback"
            } else {
                "laboratory single-tape full-file backup and physical restore comparison"
            },
            "production_eligible": false
        }),
        "local-api",
    )
    .await?;
    let run = sqlx::query_as(&format!("{SINGLE_TAPE_DATA_RUN_SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(QueueSingleTapeDataRunResult::Queued(Box::new(run)))
}

pub async fn claim_next_single_tape_data_run(
    pool: &PgPool,
    worker_id: Uuid,
) -> Result<ClaimSingleTapeDataRunResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let run = sqlx::query_as::<_, SingleTapeDataRunRecord>(&format!(
        "{SINGLE_TAPE_DATA_RUN_SELECT}
         WHERE state = 'queued'
         ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1"
    ))
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(run) = run else {
        transaction.rollback().await?;
        return Ok(ClaimSingleTapeDataRunResult::Empty);
    };
    let profile = sqlx::query_as::<_, SingleTapeDataProfileRecord>(&format!(
        "{SINGLE_TAPE_DATA_PROFILE_SELECT} WHERE id = $1 FOR UPDATE"
    ))
    .bind(run.profile_id)
    .fetch_one(&mut *transaction)
    .await?;
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
    let snapshot = sqlx::query_as::<_, SingleTapeSnapshotBinding>(
        "SELECT id, source_id, snapshot_guid, snapshot_name, logical_root,
                physical_path, released_at
         FROM snapshots WHERE id = $1 FOR UPDATE",
    )
    .bind(run.source_snapshot_id)
    .fetch_one(&mut *transaction)
    .await?;
    let expected_reservation = format!("single_tape_data:{}", run.id);
    let conflicts = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM drive_operations
             WHERE drive_id = $1 AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM qualification_runs
             WHERE (drive_id = $1 OR cartridge_id = $2) AND state IN ('queued', 'running')
         )",
    )
    .bind(run.drive_id)
    .bind(run.cartridge_id)
    .fetch_one(&mut *transaction)
    .await?;
    let qualification_passed =
        single_tape_qualification_passed(&mut transaction, &drive, run.cartridge_id).await?;
    let blocker = if run.cancel_requested_at.is_some() {
        Some("cancellation was requested before the worker claimed the tape".to_owned())
    } else if run.created_at < Utc::now() - chrono::TimeDelta::minutes(15) {
        Some("the attended destructive confirmation expired before worker dispatch; inspect the source, drive, and loaded cartridge and assign media again".to_owned())
    } else if conflicts {
        Some("a qualification or tape-motion request conflicts with this data operation".to_owned())
    } else if !profile.enabled
        || profile.production_eligible
        || profile.workflow_kind != run.workflow_kind
        || profile.source_mount_path != run.source_mount_path
        || profile.restore_root_path != run.restore_root_path
        || profile.logical_root != run.logical_root
        || profile.engine_name != run.engine_name
        || profile.engine_version != run.engine_version
        || profile.adapter_version != run.adapter_version
        || profile.format_version != run.format_version
        || !profile.immutable_snapshot_confirmed
    {
        Some("the source profile or pinned engine binding changed after queuing".to_owned())
    } else if snapshot.id != profile.source_snapshot_id
        || snapshot.source_id != profile.source_id
        || snapshot.released_at.is_some()
        || snapshot.logical_root != run.logical_root
        || snapshot.physical_path.as_deref() != Some(run.source_mount_path.as_str())
    {
        Some("the bound source snapshot is released or no longer matches the profile".to_owned())
    } else if !run.scratch_media_confirmed
        || !run.destructive_overwrite_confirmed
        || !run.encryption_deviation_confirmed
    {
        Some("persisted scratch-media, destructive-overwrite, and unencrypted-test-data deviation confirmations are required".to_owned())
    } else if drive.stable_id != run.expected_stable_id
        || drive.observed_path.as_deref() != Some(run.expected_path.as_str())
        || drive.vendor != run.expected_vendor
        || drive.model != run.expected_model
        || drive.serial != run.expected_serial
        || drive.wwn != run.expected_wwn
        || drive.firmware != run.expected_firmware
    {
        Some(
            "the drive identity, path, model, serial, WWN, or firmware changed after queuing"
                .to_owned(),
        )
    } else if let Some(reason) = drive_motion_blocker(&drive) {
        Some(reason)
    } else if cartridge.id != run.cartridge_id
        || cartridge.internal_label != run.expected_cartridge_label
        || cartridge.media_generation != run.expected_media_generation
        || cartridge.media_format != run.expected_media_format
    {
        Some("the registered cartridge metadata changed after queuing".to_owned())
    } else if profile
        .media_pool_id
        .is_some_and(|pool_id| cartridge.pool_id != Some(pool_id))
    {
        Some("the cartridge no longer belongs to the backup profile's media pool".to_owned())
    } else if let Some(reason) =
        qualification_cartridge_blocker(&cartridge, Some(&expected_reservation))
    {
        Some(reason)
    } else if matches!(
        cartridge.health_status.as_str(),
        "write_in_progress" | "write_unverified"
    ) {
        Some(
            "the cartridge entered write-unverified quarantine after queuing; run a fresh destructive btape qualification"
                .to_owned(),
        )
    } else if !qualification_passed {
        Some("the exact passed physical qualification is no longer present for this drive and cartridge".to_owned())
    } else {
        None
    };
    if let Some(reason) = blocker {
        block_single_tape_data_run_in_transaction(&mut transaction, &run, &reason).await?;
        transaction.commit().await?;
        return Ok(ClaimSingleTapeDataRunResult::Blocked { id: run.id, reason });
    }

    sqlx::query(
        "UPDATE resource_leases SET released_at = now()
         WHERE ((resource_kind = 'drive' AND resource_id = $1::text)
             OR (resource_kind = 'media' AND resource_id = $2::text))
           AND released_at IS NULL AND expires_at <= now()",
    )
    .bind(run.drive_id)
    .bind(run.cartridge_id)
    .execute(&mut *transaction)
    .await?;

    let drive_lease_id = Uuid::now_v7();
    let drive_fencing_token = sqlx::query_scalar::<_, i64>(
        "INSERT INTO resource_leases
            (id, resource_kind, resource_id, owner_id, fencing_token, expires_at)
         VALUES ($1, 'drive', $2::text, $3, nextval('resource_fencing_token_seq'),
                 now() + interval '30 minutes')
         ON CONFLICT DO NOTHING RETURNING fencing_token",
    )
    .bind(drive_lease_id)
    .bind(run.drive_id)
    .bind(worker_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(drive_fencing_token) = drive_fencing_token else {
        transaction.rollback().await?;
        return Ok(ClaimSingleTapeDataRunResult::Empty);
    };
    let media_lease_id = Uuid::now_v7();
    let media_fencing_token = sqlx::query_scalar::<_, i64>(
        "INSERT INTO resource_leases
            (id, resource_kind, resource_id, owner_id, fencing_token, expires_at)
         VALUES ($1, 'media', $2::text, $3, nextval('resource_fencing_token_seq'),
                 now() + interval '30 minutes')
         ON CONFLICT DO NOTHING RETURNING fencing_token",
    )
    .bind(media_lease_id)
    .bind(run.cartridge_id)
    .bind(worker_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(media_fencing_token) = media_fencing_token else {
        transaction.rollback().await?;
        return Ok(ClaimSingleTapeDataRunResult::Empty);
    };

    sqlx::query(
        "UPDATE single_tape_data_runs
         SET state = 'running', drive_lease_id = $1, drive_fencing_token = $2,
             media_lease_id = $3, media_fencing_token = $4,
             attempt = attempt + 1, started_at = now()
         WHERE id = $5 AND state = 'queued'",
    )
    .bind(drive_lease_id)
    .bind(drive_fencing_token)
    .bind(media_lease_id)
    .bind(media_fencing_token)
    .bind(run.id)
    .execute(&mut *transaction)
    .await?;
    let volume_lease_updated = sqlx::query(
        "UPDATE single_tape_data_run_volumes
            SET media_lease_id = $1, media_fencing_token = $2, updated_at = now()
          WHERE single_tape_data_run_id = $3 AND volume_order = 1
            AND cartridge_id = $4 AND media_lease_id IS NULL",
    )
    .bind(media_lease_id)
    .bind(media_fencing_token)
    .bind(run.id)
    .bind(run.cartridge_id)
    .execute(&mut *transaction)
    .await?;
    if volume_lease_updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(ClaimSingleTapeDataRunResult::Blocked {
            id: run.id,
            reason: "the first ordered tape assignment could not be fenced".to_owned(),
        });
    }
    sqlx::query(
        "UPDATE job_runs SET state = 'preparing', started_at = now()
         WHERE id = $1 AND state = 'queued'",
    )
    .bind(run.job_run_id)
    .execute(&mut *transaction)
    .await?;
    append_job_transition(
        &mut transaction,
        run.job_run_id,
        Some("queued"),
        "preparing",
        if run.workflow_kind == "backup" {
            "worker claimed the fenced full backup"
        } else {
            "worker claimed the fenced G0.2 recovery exercise"
        },
        json!({
            "drive_lease_id": drive_lease_id,
            "drive_fencing_token": drive_fencing_token,
            "media_lease_id": media_lease_id,
            "media_fencing_token": media_fencing_token,
            "workflow_kind": &run.workflow_kind,
            "production_eligible": false
        }),
        run.correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        run.id,
        run.correlation_id,
        "started",
        json!({
            "drive_lease_id": drive_lease_id,
            "drive_fencing_token": drive_fencing_token,
            "media_lease_id": media_lease_id,
            "media_fencing_token": media_fencing_token,
            "phase": "inventorying",
            "workflow_kind": &run.workflow_kind,
            "detail": "worker claimed the fenced operation and is hashing the immutable source"
        }),
        "tapescale-worker",
    )
    .await?;
    let run = sqlx::query_as(&format!("{SINGLE_TAPE_DATA_RUN_SELECT} WHERE id = $1"))
        .bind(run.id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(ClaimSingleTapeDataRunResult::Claimed(Box::new(
        ClaimedSingleTapeDataRun {
            run,
            drive_lease_id,
            drive_fencing_token,
            media_lease_id,
            media_fencing_token,
            worker_id,
        },
    )))
}

/// Publish one durable manual-media request while the same fenced Bacula job
/// remains active.  Operator wait time is represented as state rather than as
/// a failed or restarted backup.
pub async fn ensure_data_run_media_request(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    phase: &str,
    volume_order: i32,
    expected_cartridge_id: Option<Uuid>,
    expected_engine_volume_name: Option<&str>,
) -> Result<DataRunMediaRequestResult, sqlx::Error> {
    let active_phase = match phase {
        "backup_write" => "writing",
        "full_readback" => "verifying",
        "restore" => "restoring",
        _ => {
            return Ok(DataRunMediaRequestResult::Blocked(
                "media request phase is unsupported".to_owned(),
            ))
        }
    };
    if volume_order <= 0
        || expected_engine_volume_name.is_some_and(|value| {
            value.is_empty()
                || value.len() > 127
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        return Ok(DataRunMediaRequestResult::Blocked(
            "media request identity is invalid".to_owned(),
        ));
    }
    if phase == "backup_write"
        && (expected_cartridge_id.is_some() || expected_engine_volume_name.is_some())
    {
        return Ok(DataRunMediaRequestResult::Blocked(
            "a write continuation must request a new eligible scratch tape".to_owned(),
        ));
    }
    if phase != "backup_write"
        && (expected_cartridge_id.is_none() || expected_engine_volume_name.is_none())
    {
        return Ok(DataRunMediaRequestResult::Blocked(
            "read requests must name the exact retained tape and engine volume".to_owned(),
        ));
    }

    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(DataRunMediaRequestResult::FencedOut);
    }
    let existing = sqlx::query_as::<_, DataRunMediaRequestRecord>(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT}
         WHERE single_tape_data_run_id = $1
           AND state IN ('pending', 'authorized', 'loaded')
         ORDER BY request_sequence DESC LIMIT 1 FOR UPDATE"
    ))
    .bind(claim.run.id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(existing) = existing {
        let exact = existing.phase == phase
            && existing.volume_order == volume_order
            && existing.expected_cartridge_id == expected_cartridge_id
            && existing.expected_engine_volume_name.as_deref() == expected_engine_volume_name;
        transaction.commit().await?;
        return if exact {
            Ok(DataRunMediaRequestResult::Ready(Box::new(existing)))
        } else {
            Ok(DataRunMediaRequestResult::Blocked(
                "a different media request is already active for this backup".to_owned(),
            ))
        };
    }
    if claim.run.workflow_kind != "backup" {
        transaction.rollback().await?;
        return Ok(DataRunMediaRequestResult::Blocked(
            "manual multi-volume continuation is enabled only for operational backups".to_owned(),
        ));
    }
    if phase == "backup_write" && (!claim.run.spanning_enabled || volume_order <= 1) {
        transaction.rollback().await?;
        return Ok(DataRunMediaRequestResult::Blocked(
            "this backup policy does not allow another tape".to_owned(),
        ));
    }
    let current =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(claim.run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    if current != active_phase {
        transaction.rollback().await?;
        return Ok(DataRunMediaRequestResult::Blocked(format!(
            "the backup is in {current}, not {active_phase}"
        )));
    }
    if phase == "backup_write" {
        let next_order = sqlx::query_scalar::<_, i32>(
            "SELECT COALESCE(max(volume_order), 0) + 1
               FROM single_tape_data_run_volumes
              WHERE single_tape_data_run_id = $1",
        )
        .bind(claim.run.id)
        .fetch_one(&mut *transaction)
        .await?;
        if volume_order != next_order {
            transaction.rollback().await?;
            return Ok(DataRunMediaRequestResult::Blocked(
                "the continuation tape order is not the next archive volume".to_owned(),
            ));
        }
    } else {
        let exact_retained = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1 FROM single_tape_data_run_volumes
                  WHERE single_tape_data_run_id = $1 AND volume_order = $2
                    AND cartridge_id = $3 AND engine_volume_name = $4)",
        )
        .bind(claim.run.id)
        .bind(volume_order)
        .bind(expected_cartridge_id)
        .bind(expected_engine_volume_name)
        .fetch_one(&mut *transaction)
        .await?;
        if !exact_retained {
            transaction.rollback().await?;
            return Ok(DataRunMediaRequestResult::Blocked(
                "the requested read tape is not part of this ordered archive".to_owned(),
            ));
        }
    }

    let request_sequence = sqlx::query_scalar::<_, i32>(
        "SELECT COALESCE(max(request_sequence), 0) + 1
           FROM single_tape_data_media_requests
          WHERE single_tape_data_run_id = $1",
    )
    .bind(claim.run.id)
    .fetch_one(&mut *transaction)
    .await?;
    let request_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO single_tape_data_media_requests
            (id, single_tape_data_run_id, request_sequence, phase, volume_order,
             expected_cartridge_id, expected_engine_volume_name)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(request_id)
    .bind(claim.run.id)
    .bind(request_sequence)
    .bind(phase)
    .bind(volume_order)
    .bind(expected_cartridge_id)
    .bind(expected_engine_volume_name)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE job_runs SET state = 'waiting_for_media' WHERE id = $1")
        .bind(claim.run.job_run_id)
        .execute(&mut *transaction)
        .await?;
    let evidence = json!({
        "phase": phase,
        "volume_order": volume_order,
        "expected_cartridge_id": expected_cartridge_id,
        "expected_engine_volume_name": expected_engine_volume_name,
        "detail": if phase == "backup_write" {
            format!("load and authorize an eligible scratch tape for archive volume {volume_order}")
        } else {
            format!("load the exact archive tape required for {phase}, volume {volume_order}")
        }
    });
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some(active_phase),
        "waiting_for_media",
        "Bacula requires a manual tape change",
        evidence.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        "media_required",
        evidence,
        "tapescale-worker",
    )
    .await?;
    let record = sqlx::query_as::<_, DataRunMediaRequestRecord>(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT} WHERE id = $1"
    ))
    .bind(request_id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(DataRunMediaRequestResult::Ready(Box::new(record)))
}

/// Authorize the inventory identity that the operator attests is currently
/// loaded for a pending manual-media request. The worker still revalidates the
/// drive before motion; standalone SAS drives generally cannot machine-read a
/// scratch cartridge barcode, so this is not a claim of automatic wrong-media
/// detection.
pub async fn authorize_data_run_media(
    pool: &PgPool,
    authorization: &NewDataRunMediaAuthorization<'_>,
) -> Result<AuthorizeDataRunMediaResult, sqlx::Error> {
    if !authorization.loaded_media_confirmed || !authorization.confirmation_evidence.is_object() {
        return Ok(AuthorizeDataRunMediaResult::Blocked(
            "loaded-media confirmation and structured confirmation evidence are required"
                .to_owned(),
        ));
    }
    let mut transaction = pool.begin().await?;
    let run = sqlx::query_as::<_, SingleTapeDataRunRecord>(&format!(
        "{SINGLE_TAPE_DATA_RUN_SELECT} WHERE id = $1 FOR UPDATE"
    ))
    .bind(authorization.data_run_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(run) = run else {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::NotFound);
    };
    let request = sqlx::query_as::<_, DataRunMediaRequestRecord>(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT}
         WHERE id = $1 AND single_tape_data_run_id = $2 FOR UPDATE"
    ))
    .bind(authorization.request_id)
    .bind(run.id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(request) = request else {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::NotFound);
    };
    if request.state != "pending" {
        if request.authorized_cartridge_id == Some(authorization.cartridge_id)
            && request.confirmation_evidence == authorization.confirmation_evidence
        {
            let volume = sqlx::query_as::<_, DataRunVolumeRecord>(&format!(
                "{DATA_RUN_VOLUME_SELECT}
                 WHERE single_tape_data_run_id = $1 AND volume_order = $2"
            ))
            .bind(run.id)
            .bind(request.volume_order)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some(volume) = volume {
                transaction.commit().await?;
                return Ok(AuthorizeDataRunMediaResult::Authorized {
                    request: Box::new(request),
                    volume: Box::new(volume),
                    idempotent_replay: true,
                });
            }
        }
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::Blocked(
            "the media request was already answered differently".to_owned(),
        ));
    }
    if run.state != "running" || run.workflow_kind != "backup" {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::Blocked(
            "the backup is not actively waiting for a tape".to_owned(),
        ));
    }
    if !matches!(request.phase.as_str(), "backup_write" | "full_readback") {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::Blocked(
            "the backup media request has an unsupported phase".to_owned(),
        ));
    }
    let job_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    if job_state != "waiting_for_media" {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::Blocked(
            "the parent backup is no longer waiting for media".to_owned(),
        ));
    }
    let drive = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT} WHERE d.id = $1 FOR UPDATE OF d"
    ))
    .bind(authorization.drive_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let cartridge = sqlx::query_as::<_, QualificationCartridge>(&format!(
        "{QUALIFICATION_CARTRIDGE_SELECT} WHERE c.id = $1 FOR UPDATE OF c"
    ))
    .bind(authorization.cartridge_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let (Some(drive), Some(cartridge)) = (drive, cartridge) else {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::NotFound);
    };
    if drive.id != run.drive_id
        || drive.stable_id != run.expected_stable_id
        || drive.stable_id != authorization.expected_stable_id
        || drive.observed_path.as_deref() != Some(run.expected_path.as_str())
        || cartridge.internal_label != authorization.typed_cartridge_label
    {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::Blocked(
            "the loaded drive or cartridge does not match the pending media request".to_owned(),
        ));
    }
    if let Some(reason) = drive_motion_blocker(&drive) {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::Blocked(reason));
    }
    if request.phase == "full_readback"
        && !single_tape_qualification_passed(&mut transaction, &drive, cartridge.id).await?
    {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::Blocked(
            "the retained setup evidence for this exact drive and archive tape is missing; do not run a destructive tape-preparation test on archive media"
                .to_owned(),
        ));
    }
    if request.phase == "full_readback" {
        if let Some(reason) = readback_identity_status_blocker(&cartridge.identity_status) {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(reason.to_owned()));
        }
    }
    let owner_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT owner_id FROM resource_leases
          WHERE id = $1 AND resource_kind = 'drive' AND resource_id = $2::text
            AND fencing_token = $3 AND released_at IS NULL AND expires_at > now()",
    )
    .bind(run.drive_lease_id)
    .bind(run.drive_id)
    .bind(run.drive_fencing_token)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(owner_id) = owner_id else {
        transaction.rollback().await?;
        return Ok(AuthorizeDataRunMediaResult::Blocked(
            "the backup worker no longer owns the drive".to_owned(),
        ));
    };

    let volume = if request.phase == "backup_write" {
        if !run.spanning_enabled || !authorization.destructive_overwrite_confirmed {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "another destructive tape requires a spanning-enabled policy and overwrite confirmation"
                    .to_owned(),
            ));
        }
        let profile_pool = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT media_pool_id FROM single_tape_data_profiles WHERE id = $1 FOR SHARE",
        )
        .bind(run.profile_id)
        .fetch_one(&mut *transaction)
        .await?;
        if profile_pool.is_some_and(|pool_id| cartridge.pool_id != Some(pool_id)) {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "the continuation tape is not in this backup's media pool".to_owned(),
            ));
        }
        let expected_reservation = format!("single_tape_data:{}", run.id);
        if let Some(reason) = qualification_cartridge_blocker(&cartridge, None) {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(reason));
        }
        if !single_tape_qualification_passed(&mut transaction, &drive, cartridge.id).await? {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "the exact drive and continuation cartridge need a current passed qualification"
                    .to_owned(),
            ));
        }
        let already_used = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1 FROM single_tape_data_run_volumes
                  WHERE single_tape_data_run_id = $1 AND cartridge_id = $2)",
        )
        .bind(run.id)
        .bind(cartridge.id)
        .fetch_one(&mut *transaction)
        .await?;
        if already_used {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "a continuation tape cannot be reused within the same archive".to_owned(),
            ));
        }
        let media_lease_id = Uuid::now_v7();
        let media_fencing_token = sqlx::query_scalar::<_, i64>(
            "INSERT INTO resource_leases
                (id, resource_kind, resource_id, owner_id, fencing_token, expires_at)
             VALUES ($1, 'media', $2::text, $3, nextval('resource_fencing_token_seq'),
                     now() + interval '30 minutes')
             ON CONFLICT DO NOTHING RETURNING fencing_token",
        )
        .bind(media_lease_id)
        .bind(cartridge.id)
        .bind(owner_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(media_fencing_token) = media_fencing_token else {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "the continuation tape is reserved by another operation".to_owned(),
            ));
        };
        sqlx::query("UPDATE cartridges SET reserved_by = $1, updated_at = now() WHERE id = $2")
            .bind(&expected_reservation)
            .bind(cartridge.id)
            .execute(&mut *transaction)
            .await?;
        let volume_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO single_tape_data_run_volumes
                (id, single_tape_data_run_id, volume_order, cartridge_id,
                 expected_cartridge_label, expected_media_generation,
                 expected_media_format, media_lease_id, media_fencing_token, state)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'authorized')",
        )
        .bind(volume_id)
        .bind(run.id)
        .bind(request.volume_order)
        .bind(cartridge.id)
        .bind(&cartridge.internal_label)
        .bind(cartridge.media_generation)
        .bind(&cartridge.media_format)
        .bind(media_lease_id)
        .bind(media_fencing_token)
        .execute(&mut *transaction)
        .await?;
        sqlx::query_as::<_, DataRunVolumeRecord>(&format!("{DATA_RUN_VOLUME_SELECT} WHERE id = $1"))
            .bind(volume_id)
            .fetch_one(&mut *transaction)
            .await?
    } else {
        if request.expected_cartridge_id != Some(cartridge.id) {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "the loaded tape is not the exact archive volume requested for readback".to_owned(),
            ));
        }
        let volume = sqlx::query_as::<_, DataRunVolumeRecord>(&format!(
            "{DATA_RUN_VOLUME_SELECT}
             WHERE single_tape_data_run_id = $1 AND volume_order = $2
               AND cartridge_id = $3 FOR UPDATE"
        ))
        .bind(run.id)
        .bind(request.volume_order)
        .bind(cartridge.id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(volume) = volume else {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "the requested readback volume is missing from the archive".to_owned(),
            ));
        };
        if volume.expected_cartridge_label != authorization.typed_cartridge_label
            || volume.engine_volume_name.as_deref()
                != request.expected_engine_volume_name.as_deref()
        {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "the loaded tape label does not match the required engine volume".to_owned(),
            ));
        }
        let media_lease_live = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1 FROM resource_leases
                  WHERE id = $1 AND resource_kind = 'media'
                    AND resource_id = $2::text AND owner_id = $3
                    AND fencing_token = $4 AND released_at IS NULL
                    AND expires_at > now())",
        )
        .bind(volume.media_lease_id)
        .bind(volume.cartridge_id)
        .bind(owner_id)
        .bind(volume.media_fencing_token)
        .fetch_one(&mut *transaction)
        .await?;
        if !media_lease_live {
            transaction.rollback().await?;
            return Ok(AuthorizeDataRunMediaResult::Blocked(
                "the backup worker no longer owns the requested archive tape".to_owned(),
            ));
        }
        volume
    };

    sqlx::query(
        "UPDATE single_tape_data_media_requests
            SET state = 'authorized', authorized_cartridge_id = $1,
                authorized_at = now(), confirmation_evidence = $2
          WHERE id = $3 AND state = 'pending'",
    )
    .bind(cartridge.id)
    .bind(&authorization.confirmation_evidence)
    .bind(request.id)
    .execute(&mut *transaction)
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        run.id,
        run.correlation_id,
        "media_authorized",
        json!({
            "media_request_id": request.id,
            "phase": request.phase,
            "volume_order": request.volume_order,
            "cartridge_id": cartridge.id,
            "cartridge_label": cartridge.internal_label,
            "destructive": request.phase == "backup_write",
            "loaded_media_identity_basis": "operator_attestation_and_inventory_binding",
            "machine_verified_loaded_cartridge": false
        }),
        "local-api",
    )
    .await?;
    let request = sqlx::query_as::<_, DataRunMediaRequestRecord>(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT} WHERE id = $1"
    ))
    .bind(request.id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(AuthorizeDataRunMediaResult::Authorized {
        request: Box::new(request),
        volume: Box::new(volume),
        idempotent_replay: false,
    })
}

pub async fn authorized_data_run_media_request(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
) -> Result<Option<(DataRunMediaRequestRecord, DataRunVolumeRecord)>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(None);
    }
    let request = sqlx::query_as::<_, DataRunMediaRequestRecord>(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT}
         WHERE single_tape_data_run_id = $1 AND state = 'authorized'
         ORDER BY request_sequence DESC LIMIT 1 FOR UPDATE"
    ))
    .bind(claim.run.id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(request) = request else {
        transaction.commit().await?;
        return Ok(None);
    };
    let volume = sqlx::query_as::<_, DataRunVolumeRecord>(&format!(
        "{DATA_RUN_VOLUME_SELECT}
         WHERE single_tape_data_run_id = $1 AND volume_order = $2"
    ))
    .bind(claim.run.id)
    .bind(request.volume_order)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some((request, volume)))
}

pub async fn mark_data_run_media_loaded(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    request_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let request = sqlx::query_as::<_, DataRunMediaRequestRecord>(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT} WHERE id = $1
           AND single_tape_data_run_id = $2 FOR UPDATE"
    ))
    .bind(request_id)
    .bind(claim.run.id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(request) = request else {
        transaction.rollback().await?;
        return Ok(false);
    };
    if request.state == "loaded" {
        transaction.commit().await?;
        return Ok(true);
    }
    if request.state != "authorized" {
        transaction.rollback().await?;
        return Ok(false);
    }
    let resume_state = match request.phase.as_str() {
        "backup_write" => "writing",
        "full_readback" => "verifying",
        "restore" => "restoring",
        _ => {
            transaction.rollback().await?;
            return Ok(false);
        }
    };
    sqlx::query(
        "UPDATE single_tape_data_media_requests
            SET state = 'loaded', loaded_at = now()
          WHERE id = $1 AND state = 'authorized'",
    )
    .bind(request.id)
    .execute(&mut *transaction)
    .await?;
    let parent = sqlx::query(
        "UPDATE job_runs SET state = $1
          WHERE id = $2 AND state = 'waiting_for_media'",
    )
    .bind(resume_state)
    .bind(claim.run.job_run_id)
    .execute(&mut *transaction)
    .await?;
    if parent.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    let evidence = json!({
        "media_request_id": request.id,
        "phase": request.phase,
        "volume_order": request.volume_order,
        "cartridge_id": request.authorized_cartridge_id,
        "engine_volume_name": request.expected_engine_volume_name
    });
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some("waiting_for_media"),
        resume_state,
        "authorized tape loaded; resuming the same Bacula job",
        evidence.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        "media_loaded",
        evidence,
        "tapescale-worker",
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

pub async fn complete_data_run_media_request(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    request_id: Uuid,
    media_exhausted: bool,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let request = sqlx::query_as::<_, DataRunMediaRequestRecord>(&format!(
        "{DATA_RUN_MEDIA_REQUEST_SELECT} WHERE id = $1
           AND single_tape_data_run_id = $2 FOR UPDATE"
    ))
    .bind(request_id)
    .bind(claim.run.id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(request) = request else {
        transaction.rollback().await?;
        return Ok(false);
    };
    if request.state == "completed" {
        transaction.commit().await?;
        return Ok(true);
    }
    if request.state != "loaded" {
        transaction.rollback().await?;
        return Ok(false);
    }
    sqlx::query(
        "UPDATE single_tape_data_media_requests
            SET state = 'completed', completed_at = now()
          WHERE id = $1 AND state = 'loaded'",
    )
    .bind(request.id)
    .execute(&mut *transaction)
    .await?;
    match request.phase.as_str() {
        "backup_write" => {
            let updated = sqlx::query(
                "UPDATE single_tape_data_run_volumes
                    SET state = 'written', written_at = COALESCE(written_at, now()),
                        updated_at = now()
                  WHERE single_tape_data_run_id = $1 AND volume_order = $2
                    AND state IN ('write_started', 'written')",
            )
            .bind(claim.run.id)
            .bind(request.volume_order)
            .execute(&mut *transaction)
            .await?;
            if updated.rows_affected() != 1 {
                transaction.rollback().await?;
                return Ok(false);
            }
        }
        "full_readback" => {
            let updated = sqlx::query(
                "UPDATE single_tape_data_run_volumes
                    SET state = 'written', written_at = COALESCE(written_at, now()),
                        verified_at = NULL, updated_at = now()
                  WHERE single_tape_data_run_id = $1 AND volume_order = $2
                    AND media_write_started_at IS NOT NULL
                    AND state IN ('write_started', 'written', 'verified')",
            )
            .bind(claim.run.id)
            .bind(request.volume_order)
            .execute(&mut *transaction)
            .await?;
            if updated.rows_affected() != 1 {
                transaction.rollback().await?;
                return Ok(false);
            }
        }
        "restore" => {}
        _ => {
            transaction.rollback().await?;
            return Ok(false);
        }
    }
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        match request.phase.as_str() {
            // This proves Bacula advanced past the presented tape. The entire
            // archive becomes verified only when the verify Job terminates
            // successfully and atomic finalization validates every volume.
            "full_readback" => "media_read",
            "backup_write" if media_exhausted => "media_full",
            _ => "written",
        },
        json!({
            "media_request_id": request.id,
            "phase": request.phase,
            "volume_order": request.volume_order,
            "cartridge_id": request.authorized_cartridge_id,
            "media_exhausted": media_exhausted
        }),
        "tapescale-worker",
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

pub async fn renew_single_tape_data_run_leases(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let valid = lock_valid_single_tape_claim(&mut transaction, claim).await?;
    if !valid {
        transaction.rollback().await?;
        return Ok(false);
    }
    let drive = sqlx::query(
        "UPDATE resource_leases SET expires_at = now() + interval '30 minutes'
         WHERE id = $1 AND owner_id = $2 AND fencing_token = $3
           AND released_at IS NULL AND expires_at > now()",
    )
    .bind(claim.drive_lease_id)
    .bind(claim.worker_id)
    .bind(claim.drive_fencing_token)
    .execute(&mut *transaction)
    .await?;
    let media = sqlx::query(
        "UPDATE resource_leases SET expires_at = now() + interval '30 minutes'
         WHERE id = $1 AND owner_id = $2 AND fencing_token = $3
           AND released_at IS NULL AND expires_at > now()",
    )
    .bind(claim.media_lease_id)
    .bind(claim.worker_id)
    .bind(claim.media_fencing_token)
    .execute(&mut *transaction)
    .await?;
    if drive.rows_affected() != 1 || media.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    let additional_expected = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM single_tape_data_run_volumes
          WHERE single_tape_data_run_id = $1 AND volume_order > 1
            AND media_lease_id IS NOT NULL",
    )
    .bind(claim.run.id)
    .fetch_one(&mut *transaction)
    .await?;
    let additional = sqlx::query(
        "UPDATE resource_leases lease
            SET expires_at = now() + interval '30 minutes'
           FROM single_tape_data_run_volumes volume
          WHERE volume.single_tape_data_run_id = $1 AND volume.volume_order > 1
            AND volume.media_lease_id = lease.id
            AND volume.media_fencing_token = lease.fencing_token
            AND lease.owner_id = $2 AND lease.released_at IS NULL
            AND lease.expires_at > now()",
    )
    .bind(claim.run.id)
    .bind(claim.worker_id)
    .execute(&mut *transaction)
    .await?;
    if i64::try_from(additional.rows_affected()).unwrap_or(i64::MAX) != additional_expected {
        transaction.rollback().await?;
        return Ok(false);
    }
    transaction.commit().await?;
    Ok(true)
}

pub async fn transition_single_tape_data_phase(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    phase: SingleTapeDataPhase,
    evidence: Value,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let current =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(claim.run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    if !phase.allowed_previous().contains(&current.as_str()) {
        transaction.rollback().await?;
        return Ok(false);
    }
    sqlx::query("UPDATE job_runs SET state = $1 WHERE id = $2")
        .bind(phase.as_str())
        .bind(claim.run.job_run_id)
        .execute(&mut *transaction)
        .await?;
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some(&current),
        phase.as_str(),
        if claim.run.workflow_kind == "backup" {
            "attended full backup advanced"
        } else {
            "G0.2 physical recovery exercise advanced"
        },
        evidence.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        phase.as_str(),
        evidence,
        "tapescale-worker",
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

/// Persist the irreversible media boundary before Bacula is allowed to label
/// or write the cartridge. The run marker and cartridge quarantine change are
/// committed in one fenced transaction.
pub async fn mark_single_tape_media_write_started(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    expected_engine_volume: &str,
) -> Result<bool, sqlx::Error> {
    mark_data_run_volume_write_started(pool, claim, 1, expected_engine_volume).await
}

pub async fn mark_data_run_volume_write_started(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    volume_order: i32,
    expected_engine_volume: &str,
) -> Result<bool, sqlx::Error> {
    if expected_engine_volume.is_empty()
        || expected_engine_volume.len() > 127
        || !expected_engine_volume
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Ok(false);
    }
    if volume_order <= 0 {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let job_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(claim.run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    if job_state != "writing" {
        transaction.rollback().await?;
        return Ok(false);
    }
    let volume = sqlx::query_as::<_, DataRunVolumeRecord>(&format!(
        "{DATA_RUN_VOLUME_SELECT}
         WHERE single_tape_data_run_id = $1 AND volume_order = $2 FOR UPDATE"
    ))
    .bind(claim.run.id)
    .bind(volume_order)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(volume) = volume else {
        transaction.rollback().await?;
        return Ok(false);
    };
    if volume.state != "authorized" || volume.engine_volume_name.is_some() {
        transaction.rollback().await?;
        return Ok(false);
    }
    if volume_order == 1 {
        let updated = sqlx::query(
            "UPDATE single_tape_data_runs
             SET media_write_started = true, media_write_started_at = now(),
                 result = result || jsonb_build_object(
                     'media_write_started', true,
                     'expected_engine_volume', $1::text)
             WHERE id = $2 AND state = 'running' AND NOT media_write_started",
        )
        .bind(expected_engine_volume)
        .bind(claim.run.id)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(false);
        }
    }
    let volume_updated = sqlx::query(
        "UPDATE single_tape_data_run_volumes
            SET state = 'write_started', media_write_started_at = now(),
                engine_volume_name = $1, updated_at = now()
          WHERE id = $2 AND state = 'authorized' AND engine_volume_name IS NULL",
    )
    .bind(expected_engine_volume)
    .bind(volume.id)
    .execute(&mut *transaction)
    .await?;
    if volume_updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    let cartridge_updated = sqlx::query(
        "UPDATE cartridges
         SET on_tape_identity = NULL, identity_status = 'unknown',
             health_status = 'write_in_progress', updated_at = now()
         WHERE id = $1 AND reserved_by = $2",
    )
    .bind(volume.cartridge_id)
    .bind(format!("single_tape_data:{}", claim.run.id))
    .execute(&mut *transaction)
    .await?;
    if cartridge_updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        "media_write_started",
        json!({
            "phase": "writing",
            "media_write_started": true,
            "volume_order": volume_order,
            "cartridge_id": volume.cartridge_id,
            "expected_engine_volume": expected_engine_volume,
            "detail": "durable media-write boundary committed before Bacula label dispatch"
        }),
        "tapescale-worker",
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

/// Promote every authorized archive volume to `written` only after the
/// completed Bacula backup has supplied an exact, ordered JobMedia/catalog
/// proof. This closes the initial-volume gap: volume one has no manual-media
/// request to complete, so it cannot be inferred written from request state.
pub async fn mark_data_run_backup_volumes_written(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    catalog_volume_names: &[String],
) -> Result<bool, sqlx::Error> {
    if catalog_volume_names.is_empty()
        || catalog_volume_names.iter().any(|name| {
            name.is_empty()
                || name.len() > 127
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        return Ok(false);
    }

    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let job_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(claim.run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    if claim.run.workflow_kind != "backup" || job_state != "writing" {
        transaction.rollback().await?;
        return Ok(false);
    }
    let volumes = sqlx::query_as::<_, DataRunVolumeRecord>(&format!(
        "{DATA_RUN_VOLUME_SELECT}
         WHERE single_tape_data_run_id = $1 ORDER BY volume_order FOR UPDATE"
    ))
    .bind(claim.run.id)
    .fetch_all(&mut *transaction)
    .await?;
    if validate_catalog_written_volume_chain(&volumes, catalog_volume_names).is_err() {
        transaction.rollback().await?;
        return Ok(false);
    }

    let updated = sqlx::query(
        "UPDATE single_tape_data_run_volumes
            SET state = 'written', written_at = COALESCE(written_at, now()),
                updated_at = now()
          WHERE single_tape_data_run_id = $1
            AND state IN ('write_started', 'written')",
    )
    .bind(claim.run.id)
    .execute(&mut *transaction)
    .await?;
    if usize::try_from(updated.rows_affected()).ok() != Some(volumes.len()) {
        transaction.rollback().await?;
        return Ok(false);
    }
    transaction.commit().await?;
    Ok(true)
}

/// Append fenced physical-media or engine-dispatch evidence without changing the job phase.
pub async fn append_single_tape_data_evidence_event(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    event_type: &str,
    evidence: Value,
) -> Result<bool, sqlx::Error> {
    if !matches!(event_type, "media_prepared" | "engine_dispatched") || !evidence.is_object() {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        event_type,
        evidence,
        "tapescale-worker",
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

/// A worker that has lost its fencing lease may still know that physical media
/// was touched. Quarantine every ordered cartridge whose durable write
/// boundary was crossed and revoke drive trust, but never release or rewrite
/// resources owned by a newer controller.
pub async fn quarantine_fenced_single_tape_write(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    detail: &str,
) -> Result<bool, sqlx::Error> {
    if detail.is_empty() || detail.len() > 4096 || detail.chars().any(char::is_control) {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    let record = sqlx::query_as::<_, (String, bool, Uuid, Uuid, Uuid)>(
        "SELECT state, media_write_started, cartridge_id, drive_id, correlation_id
         FROM single_tape_data_runs WHERE id = $1 FOR UPDATE",
    )
    .bind(claim.run.id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((state, media_write_started, cartridge_id, drive_id, correlation_id)) = record else {
        transaction.rollback().await?;
        return Ok(false);
    };
    if state == "passed"
        || !media_write_started
        || cartridge_id != claim.run.cartridge_id
        || drive_id != claim.run.drive_id
        || correlation_id != claim.run.correlation_id
    {
        transaction.rollback().await?;
        return Ok(false);
    }
    let quarantined_media_ids = sqlx::query_scalar::<_, Uuid>(
        "UPDATE cartridges cartridge
            SET on_tape_identity = NULL, identity_status = 'unknown',
                health_status = 'write_unverified', updated_at = now()
           FROM single_tape_data_run_volumes volume
          WHERE volume.single_tape_data_run_id = $1
            AND volume.cartridge_id = cartridge.id
            AND volume.media_write_started_at IS NOT NULL
          RETURNING cartridge.id",
    )
    .bind(claim.run.id)
    .fetch_all(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE single_tape_data_run_volumes
            SET state = 'write_unverified', updated_at = now()
          WHERE single_tape_data_run_id = $1
            AND media_write_started_at IS NOT NULL",
    )
    .bind(claim.run.id)
    .execute(&mut *transaction)
    .await?;
    mark_drive_motion_uncertain(&mut transaction, drive_id, detail).await?;
    let event_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM single_tape_data_events
             WHERE single_tape_data_run_id = $1 AND event_type = 'fenced_out')",
    )
    .bind(claim.run.id)
    .fetch_one(&mut *transaction)
    .await?;
    if !event_exists {
        append_single_tape_data_event(
            &mut transaction,
            claim.run.id,
            correlation_id,
            "fenced_out",
            json!({
                "detail": detail,
                "media_write_started": true,
                "quarantined_media_ids": quarantined_media_ids,
                "position_uncertain": true,
                "reconciliation_required": true
            }),
            "tapescale-worker",
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub async fn append_single_tape_data_artifact(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    artifact_kind: &str,
    storage_ref: &str,
    sha256: &str,
    size_bytes: u64,
    metadata: Value,
) -> Result<bool, sqlx::Error> {
    if !matches!(
        artifact_kind,
        "source_manifest" | "engine_write" | "restored_manifest" | "comparison" | "evidence_report"
    ) || storage_ref.is_empty()
        || storage_ref.chars().any(char::is_control)
        || sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || i64::try_from(size_bytes).is_err()
    {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let inserted = sqlx::query(
        "INSERT INTO single_tape_data_artifacts
            (single_tape_data_run_id, artifact_kind, storage_ref, sha256, size_bytes, metadata)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (single_tape_data_run_id, artifact_kind) DO NOTHING",
    )
    .bind(claim.run.id)
    .bind(artifact_kind)
    .bind(storage_ref)
    .bind(sha256)
    .bind(i64::try_from(size_bytes).expect("size was checked"))
    .bind(metadata)
    .execute(&mut *transaction)
    .await?;
    if inserted.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    transaction.commit().await?;
    Ok(true)
}

pub async fn finish_single_tape_data_run(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    state: SingleTapeDataTerminalState,
    mut result: Value,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let current =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(claim.run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    let (media_write_started, cancel_requested_by_label) =
        sqlx::query_as::<_, (bool, Option<String>)>(
            "SELECT media_write_started, cancel_requested_by_label
             FROM single_tape_data_runs WHERE id = $1",
        )
        .bind(claim.run.id)
        .fetch_one(&mut *transaction)
        .await?;
    if media_write_started {
        if let Some(object) = result.as_object_mut() {
            object.insert("media_write_started".to_owned(), Value::Bool(true));
        }
    }
    sqlx::query(
        "UPDATE single_tape_data_runs
         SET state = $1, result = $2, finished_at = now()
         WHERE id = $3 AND state = 'running'",
    )
    .bind(state.as_str())
    .bind(&result)
    .bind(claim.run.id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE job_runs
         SET state = $1, failure_class = $2, failure_detail = $3, finished_at = now()
         WHERE id = $4 AND finished_at IS NULL",
    )
    .bind(state.as_str())
    .bind(format!("{}_{}", claim.run.workflow_kind, state.as_str()))
    .bind(&result)
    .bind(claim.run.job_run_id)
    .execute(&mut *transaction)
    .await?;
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some(&current),
        state.as_str(),
        if claim.run.workflow_kind == "backup" {
            "full backup ended without a verified recovery point"
        } else {
            "G0.2 physical recovery exercise ended without a verified recovery point"
        },
        result.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        state.as_str(),
        result.clone(),
        "tapescale-worker",
    )
    .await?;
    let position_uncertain = result
        .get("position_uncertain")
        .and_then(Value::as_bool)
        .unwrap_or(matches!(
            state,
            SingleTapeDataTerminalState::Failed | SingleTapeDataTerminalState::Interrupted
        ));
    if position_uncertain {
        let detail = result
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("the physical data exercise did not leave a verified tape position");
        mark_drive_motion_uncertain(&mut transaction, claim.run.drive_id, detail).await?;
    }
    if media_write_started {
        let quarantined_media_ids = sqlx::query_scalar::<_, Uuid>(
            "UPDATE cartridges cartridge
                SET on_tape_identity = NULL, identity_status = 'unknown',
                    health_status = 'write_unverified', updated_at = now()
               FROM single_tape_data_run_volumes volume
              WHERE volume.single_tape_data_run_id = $1
                AND volume.cartridge_id = cartridge.id
                AND volume.media_write_started_at IS NOT NULL
              RETURNING cartridge.id",
        )
        .bind(claim.run.id)
        .fetch_all(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE single_tape_data_run_volumes
                SET state = 'write_unverified', updated_at = now()
              WHERE single_tape_data_run_id = $1
                AND media_write_started_at IS NOT NULL",
        )
        .bind(claim.run.id)
        .execute(&mut *transaction)
        .await?;
        if let Some(object) = result.as_object_mut() {
            object.insert(
                "quarantined_media_ids".to_owned(),
                serde_json::to_value(quarantined_media_ids)
                    .expect("UUID lists always serialize as JSON"),
            );
        }
        sqlx::query("UPDATE single_tape_data_runs SET result = $1 WHERE id = $2")
            .bind(&result)
            .bind(claim.run.id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE job_runs SET failure_detail = $1 WHERE id = $2")
            .bind(&result)
            .bind(claim.run.job_run_id)
            .execute(&mut *transaction)
            .await?;
    }
    if state == SingleTapeDataTerminalState::Cancelled {
        let cancel_actor = cancel_requested_by_label
            .as_deref()
            .unwrap_or("tapescale-worker");
        sqlx::query(
            "UPDATE backup_requests
             SET cancelled_at = COALESCE(cancelled_at, now()),
                 cancelled_by_label = COALESCE(cancelled_by_label, $2)
             WHERE assigned_data_run_id = $1",
        )
        .bind(claim.run.id)
        .bind(cancel_actor)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO backup_request_events
                (backup_request_id, event_type, evidence, actor_label, correlation_id)
             SELECT request.id, 'cancelled', $2, $3, request.correlation_id
             FROM backup_requests request
             WHERE request.assigned_data_run_id = $1",
        )
        .bind(claim.run.id)
        .bind(json!({
            "detail": "the assigned physical backup reached its safe cancelled terminal state",
            "media_write_started": media_write_started,
            "result": &result
        }))
        .bind(cancel_actor)
        .execute(&mut *transaction)
        .await?;
    }
    release_single_tape_data_resources(&mut transaction, &claim.run).await?;
    transaction.commit().await?;
    Ok(true)
}

pub async fn finalize_single_tape_data_pass(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    evidence: &SingleTapePassEvidence,
) -> Result<FinalizeSingleTapeDataResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::FencedOut);
    }
    let (current, media_write_started) = sqlx::query_as::<_, (String, bool)>(
        "SELECT job.state, data_run.media_write_started
         FROM job_runs job
         JOIN single_tape_data_runs data_run ON data_run.job_run_id = job.id
         WHERE job.id = $1 FOR UPDATE OF job",
    )
    .bind(claim.run.job_run_id)
    .fetch_one(&mut *transaction)
    .await?;
    if current != "comparing" {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "a pass can be finalized only after readback, isolated restore, and comparison phases"
                .to_owned(),
        ));
    }
    if !media_write_started {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "the durable pre-label media-write boundary is missing".to_owned(),
        ));
    }
    let artifacts = sqlx::query_as::<_, SingleTapeDataArtifactRecord>(
        "SELECT id, artifact_kind, storage_ref, sha256, size_bytes, metadata, created_at
         FROM single_tape_data_artifacts
         WHERE single_tape_data_run_id = $1 ORDER BY id FOR SHARE",
    )
    .bind(claim.run.id)
    .fetch_all(&mut *transaction)
    .await?;
    let source_manifest = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "source_manifest");
    let engine_write = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "engine_write");
    let restored_manifest = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "restored_manifest");
    let comparison = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "comparison");
    let (Some(source_manifest), Some(engine_write), Some(restored_manifest), Some(comparison)) =
        (source_manifest, engine_write, restored_manifest, comparison)
    else {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "source, finalized engine-write, restored-manifest, and comparison artifacts are all required"
                .to_owned(),
        ));
    };
    let evidence_strings_valid = [
        evidence.engine_operation_id.as_str(),
        evidence.engine_volume_name.as_str(),
        evidence.engine_record_id.as_str(),
        evidence.first_record.as_str(),
    ]
    .iter()
    .all(|value| !value.is_empty() && !value.chars().any(char::is_control));
    let manifests_match = source_manifest.sha256 == restored_manifest.sha256;
    let comparison_passed =
        comparison.metadata.get("matched").and_then(Value::as_bool) == Some(true);
    let engine_finalized = engine_write
        .metadata
        .get("finalized")
        .and_then(Value::as_bool)
        == Some(true);
    let physical_readback = evidence
        .full_readback_evidence
        .get("physical_media_readback")
        .and_then(Value::as_bool)
        == Some(true);
    let restore_matched = evidence
        .restore_test_evidence
        .get("matched")
        .and_then(Value::as_bool)
        == Some(true);
    let sizes_fit = i64::try_from(evidence.logical_bytes).is_ok()
        && i64::try_from(evidence.encoded_bytes).is_ok()
        && i64::try_from(evidence.file_count).is_ok();
    if !evidence_strings_valid
        || !manifests_match
        || !comparison_passed
        || !engine_finalized
        || !physical_readback
        || !restore_matched
        || !sizes_fit
    {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "pass evidence must prove a finalized engine write, matching normalized manifests, physical-media readback, and a matching isolated restore"
                .to_owned(),
        ));
    }

    let (source_id, source_scope_hash) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT p.source_id, s.scope_hash
         FROM single_tape_data_profiles p JOIN sources s ON s.id = p.source_id
         WHERE p.id = $1 FOR SHARE OF p, s",
    )
    .bind(claim.run.profile_id)
    .fetch_one(&mut *transaction)
    .await?;
    let recovery_point_id = Uuid::now_v7();
    let volume_record_id = Uuid::now_v7();
    let recovery_time = Utc::now();
    let coverage = json!({
        "scope": "G0.2 laboratory single-tape full-file physical restore exercise",
        "production_eligible": false,
        "logical_root": claim.run.logical_root,
        "source_snapshot_id": claim.run.source_snapshot_id,
        "file_count": evidence.file_count,
        "logical_bytes": evidence.logical_bytes,
        "encoded_bytes": evidence.encoded_bytes,
        "source_manifest_artifact_id": source_manifest.id,
        "restored_manifest_artifact_id": restored_manifest.id,
        "comparison_artifact_id": comparison.id,
        "declared_coverage": evidence.coverage
    });
    sqlx::query(
        "INSERT INTO recovery_points
            (id, source_id, run_id, representation, level, source_scope_hash,
             format_version, status, recovery_time, expires_at, indefinite,
             coverage, manifest_digest)
         VALUES ($1, $2, $3, 'file', 'full', $4, $5, 'verified', $6,
                 $6 + interval '28 days', false, $7, $8)",
    )
    .bind(recovery_point_id)
    .bind(source_id)
    .bind(claim.run.job_run_id)
    .bind(source_scope_hash)
    .bind(&claim.run.format_version)
    .bind(recovery_time)
    .bind(&coverage)
    .bind(&source_manifest.sha256)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO volume_records
            (id, recovery_point_id, cartridge_id, volume_order,
             first_record, last_record, encoded_bytes, finalized, engine_record_id)
         VALUES ($1, $2, $3, 1, $4, $5, $6, true, $7)",
    )
    .bind(volume_record_id)
    .bind(recovery_point_id)
    .bind(claim.run.cartridge_id)
    .bind(&evidence.first_record)
    .bind(&evidence.last_record)
    .bind(i64::try_from(evidence.encoded_bytes).expect("size was checked"))
    .bind(&evidence.engine_record_id)
    .execute(&mut *transaction)
    .await?;

    for (kind, verification_evidence) in [
        ("full_readback", &evidence.full_readback_evidence),
        ("restore_test", &evidence.restore_test_evidence),
    ] {
        sqlx::query(
            "INSERT INTO verification_records
                (id, recovery_point_id, kind, scope, outcome, drive_id, media_ids,
                 evidence, correlation_id, started_at, finished_at)
             VALUES ($1, $2, $3, $4, 'passed', $5, ARRAY[$6]::uuid[],
                     $7, $8, $9, now())",
        )
        .bind(Uuid::now_v7())
        .bind(recovery_point_id)
        .bind(kind)
        .bind(json!({
            "scope": "complete single-volume file recovery exercise",
            "file_count": evidence.file_count,
            "logical_bytes": evidence.logical_bytes,
            "manifest_sha256": source_manifest.sha256
        }))
        .bind(claim.run.drive_id)
        .bind(claim.run.cartridge_id)
        .bind(verification_evidence)
        .bind(claim.run.correlation_id)
        .bind(
            claim
                .run
                .started_at
                .expect("running runs have a start time"),
        )
        .execute(&mut *transaction)
        .await?;
    }

    let result = json!({
        "detail": "one exact full-file source snapshot was written through Bacula, read from physical tape, restored into an isolated destination, and compared successfully",
        "production_eligible": false,
        "g0_gate": "still_open",
        "engine_operation_id": evidence.engine_operation_id,
        "engine_volume_name": evidence.engine_volume_name,
        "recovery_point_id": recovery_point_id,
        "volume_record_id": volume_record_id,
        "source_manifest_sha256": source_manifest.sha256,
        "restored_manifest_sha256": restored_manifest.sha256,
        "logical_bytes": evidence.logical_bytes,
        "encoded_bytes": evidence.encoded_bytes,
        "file_count": evidence.file_count,
        "evidence": evidence.result
    });
    let data_run_updated = sqlx::query(
        "UPDATE single_tape_data_runs
         SET state = 'passed', result = $1, finished_at = now()
         WHERE id = $2 AND state = 'running'",
    )
    .bind(&result)
    .bind(claim.run.id)
    .execute(&mut *transaction)
    .await?;
    if data_run_updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "the physical recovery exercise was no longer running at finalization".to_owned(),
        ));
    }
    let job_run_updated = sqlx::query(
        "UPDATE job_runs
         SET state = 'verified', engine_operation_id = $1, finished_at = now()
         WHERE id = $2 AND state = 'comparing'",
    )
    .bind(&evidence.engine_operation_id)
    .bind(claim.run.job_run_id)
    .execute(&mut *transaction)
    .await?;
    if job_run_updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "the parent job was no longer in the comparison phase at finalization".to_owned(),
        ));
    }
    sqlx::query(
        "UPDATE cartridges
         SET on_tape_identity = $1, identity_status = 'reconciled',
             health_status = 'written_verified', updated_at = now()
         WHERE id = $2",
    )
    .bind(&evidence.engine_volume_name)
    .bind(claim.run.cartridge_id)
    .execute(&mut *transaction)
    .await?;
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some("comparing"),
        "verified",
        "full physical-media readback and isolated restore comparison passed",
        result.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        "passed",
        result,
        "tapescale-worker",
    )
    .await?;
    release_single_tape_data_resources(&mut transaction, &claim.run).await?;
    transaction.commit().await?;
    Ok(FinalizeSingleTapeDataResult::Passed {
        recovery_point_id,
        volume_record_id,
    })
}

/// Atomically promote one attended full-file backup into a verified recovery
/// point. Unlike the G0.2 roundtrip finalizer, this intentionally records no
/// restore-test claim: write completion, full media readback, and later restore
/// rehearsals remain independent evidence.
pub async fn finalize_single_tape_backup_pass(
    pool: &PgPool,
    claim: &ClaimedSingleTapeDataRun,
    evidence: &SingleTapePassEvidence,
) -> Result<FinalizeSingleTapeDataResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_single_tape_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::FencedOut);
    }
    let (current, media_write_started, cancel_requested, workflow_kind) =
        sqlx::query_as::<_, (String, bool, bool, String)>(
            "SELECT job.state, data_run.media_write_started,
                    data_run.cancel_requested_at IS NOT NULL,
                    data_run.workflow_kind
             FROM job_runs job
             JOIN single_tape_data_runs data_run ON data_run.job_run_id = job.id
             WHERE job.id = $1 FOR UPDATE OF job, data_run",
        )
        .bind(claim.run.job_run_id)
        .fetch_one(&mut *transaction)
        .await?;
    if workflow_kind != "backup" || current != "verifying" {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "a backup can be finalized only from its physical-readback phase".to_owned(),
        ));
    }
    if cancel_requested {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "operator cancellation was requested before atomic finalization".to_owned(),
        ));
    }
    if !media_write_started {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "the durable pre-label media-write boundary is missing".to_owned(),
        ));
    }

    let artifacts = sqlx::query_as::<_, SingleTapeDataArtifactRecord>(
        "SELECT id, artifact_kind, storage_ref, sha256, size_bytes, metadata, created_at
         FROM single_tape_data_artifacts
         WHERE single_tape_data_run_id = $1 ORDER BY id FOR SHARE",
    )
    .bind(claim.run.id)
    .fetch_all(&mut *transaction)
    .await?;
    let source_manifest = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "source_manifest");
    let engine_write = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "engine_write");
    let (Some(source_manifest), Some(engine_write)) = (source_manifest, engine_write) else {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "source-manifest and finalized engine-write artifacts are required".to_owned(),
        ));
    };
    let manifest_digest = source_manifest
        .metadata
        .get("manifest_digest")
        .and_then(Value::as_str)
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let engine_finalized = engine_write
        .metadata
        .get("finalized")
        .and_then(Value::as_bool)
        == Some(true);
    let physical_readback = evidence
        .full_readback_evidence
        .get("physical_media_readback")
        .and_then(Value::as_bool)
        == Some(true);
    let evidence_strings_valid = [
        evidence.engine_operation_id.as_str(),
        evidence.engine_volume_name.as_str(),
        evidence.engine_record_id.as_str(),
        evidence.first_record.as_str(),
    ]
    .iter()
    .all(|value| !value.is_empty() && !value.chars().any(char::is_control));
    let sizes_fit = i64::try_from(evidence.logical_bytes).is_ok()
        && i64::try_from(evidence.encoded_bytes).is_ok()
        && i64::try_from(evidence.file_count).is_ok();
    if manifest_digest.is_none()
        || !engine_finalized
        || !physical_readback
        || !evidence_strings_valid
        || !sizes_fit
    {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "backup evidence must prove a content-addressed manifest, finalized engine write, and complete physical-media readback"
                .to_owned(),
        ));
    }
    let run_volumes = sqlx::query_as::<_, DataRunVolumeRecord>(&format!(
        "{DATA_RUN_VOLUME_SELECT}
         WHERE single_tape_data_run_id = $1 ORDER BY volume_order FOR UPDATE"
    ))
    .bind(claim.run.id)
    .fetch_all(&mut *transaction)
    .await?;
    let finalized_volumes =
        match validated_backup_volume_evidence(&run_volumes, &engine_write.metadata, evidence) {
            Ok(volumes) => volumes,
            Err(reason) => {
                transaction.rollback().await?;
                return Ok(FinalizeSingleTapeDataResult::Rejected(reason));
            }
        };

    let (source_id, source_scope_hash) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT profile.source_id, source.scope_hash
         FROM single_tape_data_profiles profile
         JOIN sources source ON source.id = profile.source_id
         WHERE profile.id = $1 AND profile.workflow_kind = 'backup'
         FOR SHARE OF profile, source",
    )
    .bind(claim.run.profile_id)
    .fetch_one(&mut *transaction)
    .await?;
    let recovery_point_id = Uuid::now_v7();
    let volume_record_ids = finalized_volumes
        .iter()
        .map(|_| Uuid::now_v7())
        .collect::<Vec<_>>();
    let volume_record_id = volume_record_ids[0];
    let media_ids = finalized_volumes
        .iter()
        .map(|volume| volume.cartridge_id)
        .collect::<Vec<_>>();
    let recovery_time = Utc::now();
    let manifest_digest = manifest_digest.expect("manifest digest was validated");
    let limitations = if finalized_volumes.len() == 1 && !claim.run.spanning_enabled {
        json!([
            "single_volume",
            "full_only",
            "unencrypted",
            "server_verified_read_only_source"
        ])
    } else {
        json!([
            "full_only",
            "unencrypted",
            "server_verified_read_only_source"
        ])
    };
    let coverage = json!({
        "scope": if finalized_volumes.len() > 1 {
            "attended multi-volume full-file backup with complete physical-media readback"
        } else {
            "attended single-volume full-file backup with complete physical-media readback"
        },
        "qualification": "test_slice",
        "production_eligible": false,
        "limitations": limitations,
        "volume_count": finalized_volumes.len(),
        "spanning_enabled": claim.run.spanning_enabled,
        "compression": {
            "requested": claim.run.compression_mode,
            "resolved": claim.run.resolved_compression
        },
        "logical_root": claim.run.logical_root,
        "source_snapshot_id": claim.run.source_snapshot_id,
        "file_count": evidence.file_count,
        "logical_bytes": evidence.logical_bytes,
        "encoded_bytes": evidence.encoded_bytes,
        "source_manifest_artifact_id": source_manifest.id,
        "source_manifest_artifact_sha256": source_manifest.sha256,
        "declared_coverage": evidence.coverage
    });
    sqlx::query(
        "INSERT INTO recovery_points
            (id, source_id, run_id, representation, level, source_scope_hash,
             format_version, status, recovery_time, expires_at, indefinite,
             coverage, manifest_digest)
         VALUES ($1, $2, $3, 'file', 'full', $4, $5, 'verified', $6,
                 $6 + make_interval(days => $7), false, $8, $9)",
    )
    .bind(recovery_point_id)
    .bind(source_id)
    .bind(claim.run.job_run_id)
    .bind(source_scope_hash)
    .bind(&claim.run.format_version)
    .bind(recovery_time)
    .bind(claim.run.retention_days)
    .bind(&coverage)
    .bind(manifest_digest)
    .execute(&mut *transaction)
    .await?;
    for (volume, record_id) in finalized_volumes.iter().zip(volume_record_ids.iter()) {
        sqlx::query(
            "INSERT INTO volume_records
                (id, recovery_point_id, cartridge_id, volume_order,
                 first_record, last_record, encoded_bytes, finalized, engine_record_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, true, $8)",
        )
        .bind(record_id)
        .bind(recovery_point_id)
        .bind(volume.cartridge_id)
        .bind(volume.volume_order)
        .bind(&volume.first_record)
        .bind(&volume.last_record)
        .bind(volume.encoded_bytes)
        .bind(&volume.engine_record_id)
        .execute(&mut *transaction)
        .await?;
    }

    let verification_scope = json!({
        "scope": "complete ordered-volume full-file backup",
        "volume_count": finalized_volumes.len(),
        "file_count": evidence.file_count,
        "logical_bytes": evidence.logical_bytes,
        "manifest_digest": manifest_digest
    });
    let ordered_volume_evidence = finalized_volumes
        .iter()
        .map(|volume| {
            json!({
                "volume_order": volume.volume_order,
                "cartridge_id": volume.cartridge_id,
                "engine_volume_name": volume.engine_volume_name,
                "engine_media_id": volume.engine_media_id,
                "first_record": volume.first_record,
                "last_record": volume.last_record,
                "encoded_bytes": volume.encoded_bytes,
                "engine_record_id": volume.engine_record_id
            })
        })
        .collect::<Vec<_>>();
    for (kind, verification_evidence) in [
        (
            "write_complete",
            json!({
                "engine_operation_id": evidence.engine_operation_id,
                "engine_record_id": evidence.engine_record_id,
                "engine_volume_name": evidence.engine_volume_name,
                "artifact_id": engine_write.id,
                "artifact_sha256": engine_write.sha256,
                "volumes": ordered_volume_evidence,
                "finalized": true
            }),
        ),
        ("full_readback", evidence.full_readback_evidence.clone()),
    ] {
        sqlx::query(
            "INSERT INTO verification_records
                (id, recovery_point_id, kind, scope, outcome, drive_id, media_ids,
                 evidence, correlation_id, started_at, finished_at)
             VALUES ($1, $2, $3, $4, 'passed', $5, $6,
                     $7, $8, $9, now())",
        )
        .bind(Uuid::now_v7())
        .bind(recovery_point_id)
        .bind(kind)
        .bind(&verification_scope)
        .bind(claim.run.drive_id)
        .bind(&media_ids)
        .bind(verification_evidence)
        .bind(claim.run.correlation_id)
        .bind(
            claim
                .run
                .started_at
                .expect("running runs have a start time"),
        )
        .execute(&mut *transaction)
        .await?;
    }

    let result = json!({
        "detail": if finalized_volumes.len() > 1 {
            "the full-file source was written through Bacula across an ordered tape set and read back completely from physical media"
        } else {
            "the full-file source was written through Bacula and read back completely from physical tape"
        },
        "workflow_kind": "backup",
        "qualification": "test_slice",
        "production_eligible": false,
        "engine_operation_id": evidence.engine_operation_id,
        "engine_volume_name": evidence.engine_volume_name,
        "recovery_point_id": recovery_point_id,
        "volume_record_id": volume_record_id,
        "volume_record_ids": volume_record_ids,
        "volume_count": finalized_volumes.len(),
        "media_ids": media_ids,
        "volumes": ordered_volume_evidence,
        "spanning_enabled": claim.run.spanning_enabled,
        "compression": {
            "requested": claim.run.compression_mode,
            "resolved": claim.run.resolved_compression
        },
        "source_manifest_digest": manifest_digest,
        "source_manifest_artifact_sha256": source_manifest.sha256,
        "logical_bytes": evidence.logical_bytes,
        "encoded_bytes": evidence.encoded_bytes,
        "file_count": evidence.file_count,
        "retention_days": claim.run.retention_days,
        "evidence": evidence.result
    });
    let data_run_updated = sqlx::query(
        "UPDATE single_tape_data_runs
         SET state = 'passed', result = $1, finished_at = now()
         WHERE id = $2 AND state = 'running' AND cancel_requested_at IS NULL",
    )
    .bind(&result)
    .bind(claim.run.id)
    .execute(&mut *transaction)
    .await?;
    if data_run_updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "the full backup was no longer eligible for finalization".to_owned(),
        ));
    }
    let job_run_updated = sqlx::query(
        "UPDATE job_runs
         SET state = 'verified', engine_operation_id = $1, finished_at = now()
         WHERE id = $2 AND state = 'verifying' AND finished_at IS NULL",
    )
    .bind(&evidence.engine_operation_id)
    .bind(claim.run.job_run_id)
    .execute(&mut *transaction)
    .await?;
    if job_run_updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(FinalizeSingleTapeDataResult::Rejected(
            "the parent job was no longer in physical readback".to_owned(),
        ));
    }
    for volume in &finalized_volumes {
        let cartridge_updated = sqlx::query(
            "UPDATE cartridges
                SET on_tape_identity = $1, identity_status = 'reconciled',
                    health_status = 'written_verified', updated_at = now()
              WHERE id = $2",
        )
        .bind(&volume.engine_volume_name)
        .bind(volume.cartridge_id)
        .execute(&mut *transaction)
        .await?;
        if cartridge_updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(FinalizeSingleTapeDataResult::Rejected(format!(
                "archive volume {} cartridge disappeared during finalization",
                volume.volume_order
            )));
        }
        let run_volume_updated = sqlx::query(
            "UPDATE single_tape_data_run_volumes
                SET state = 'verified', engine_media_id = $1,
                    written_at = COALESCE(written_at, now()), verified_at = now(),
                    encoded_bytes = $2, first_record = $3, last_record = $4,
                    updated_at = now()
              WHERE id = $5 AND single_tape_data_run_id = $6
                AND cartridge_id = $7 AND engine_volume_name = $8
                AND media_write_started_at IS NOT NULL
                AND state IN ('write_started', 'written', 'verified')",
        )
        .bind(volume.engine_media_id)
        .bind(volume.encoded_bytes)
        .bind(&volume.first_record)
        .bind(&volume.last_record)
        .bind(volume.run_volume_id)
        .bind(claim.run.id)
        .bind(volume.cartridge_id)
        .bind(&volume.engine_volume_name)
        .execute(&mut *transaction)
        .await?;
        if run_volume_updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(FinalizeSingleTapeDataResult::Rejected(format!(
                "archive volume {} changed during finalization",
                volume.volume_order
            )));
        }
    }
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some("verifying"),
        "verified",
        "full physical-media readback passed and a recovery point was committed",
        result.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        "passed",
        result,
        "tapescale-worker",
    )
    .await?;
    release_single_tape_data_resources(&mut transaction, &claim.run).await?;
    transaction.commit().await?;
    Ok(FinalizeSingleTapeDataResult::Passed {
        recovery_point_id,
        volume_record_id,
    })
}

pub async fn block_queued_single_tape_data_runs(
    pool: &PgPool,
    reason: &str,
) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let queued = sqlx::query_as::<_, SingleTapeDataRunRecord>(&format!(
        "{SINGLE_TAPE_DATA_RUN_SELECT} WHERE state = 'queued' FOR UPDATE"
    ))
    .fetch_all(&mut *transaction)
    .await?;
    for run in &queued {
        block_single_tape_data_run_in_transaction(&mut transaction, run, reason).await?;
    }
    transaction.commit().await?;
    Ok(queued.len() as u64)
}

pub async fn recover_expired_single_tape_data_runs(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let expired = sqlx::query_as::<_, SingleTapeDataRunRecord>(&format!(
        "{SINGLE_TAPE_DATA_RUN_SELECT}
         WHERE id IN (
             SELECT data_run.id
             FROM single_tape_data_runs data_run
             LEFT JOIN resource_leases drive_lease ON drive_lease.id = data_run.drive_lease_id
             LEFT JOIN resource_leases media_lease ON media_lease.id = data_run.media_lease_id
             WHERE data_run.state = 'running'
               AND (drive_lease.id IS NULL OR drive_lease.released_at IS NOT NULL
                    OR drive_lease.expires_at <= now()
                    OR media_lease.id IS NULL OR media_lease.released_at IS NOT NULL
                    OR media_lease.expires_at <= now()
                    OR EXISTS (
                        SELECT 1
                          FROM single_tape_data_run_volumes volume
                          LEFT JOIN resource_leases extra_media_lease
                            ON extra_media_lease.id = volume.media_lease_id
                         WHERE volume.single_tape_data_run_id = data_run.id
                           AND volume.volume_order > 1
                           AND (extra_media_lease.id IS NULL
                                OR extra_media_lease.released_at IS NOT NULL
                                OR extra_media_lease.expires_at <= now())))
         ) FOR UPDATE"
    ))
    .fetch_all(&mut *transaction)
    .await?;
    for run in &expired {
        let detail = if run.workflow_kind == "backup" {
            "a drive or cartridge lease expired during the physical backup; no verified recovery point was created and tape position is uncertain"
        } else {
            "a drive or cartridge lease expired during the G0.2 physical recovery exercise; no verified recovery point was created and tape position is uncertain"
        };
        let result = json!({
            "detail": detail,
            "position_uncertain": true,
            "media_write_started": run.media_write_started
        });
        let current =
            sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
                .bind(run.job_run_id)
                .fetch_one(&mut *transaction)
                .await?;
        sqlx::query(
            "UPDATE single_tape_data_runs
             SET state = 'interrupted', result = $1, finished_at = now()
             WHERE id = $2 AND state = 'running'",
        )
        .bind(&result)
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE job_runs
             SET state = 'interrupted', failure_class = $1,
                 failure_detail = $2, finished_at = now()
             WHERE id = $3 AND finished_at IS NULL",
        )
        .bind(format!("{}_lease_expired", run.workflow_kind))
        .bind(&result)
        .bind(run.job_run_id)
        .execute(&mut *transaction)
        .await?;
        append_job_transition(
            &mut transaction,
            run.job_run_id,
            Some(&current),
            "interrupted",
            detail,
            result.clone(),
            run.correlation_id,
        )
        .await?;
        append_single_tape_data_event(
            &mut transaction,
            run.id,
            run.correlation_id,
            "interrupted",
            result,
            "tapescale-worker",
        )
        .await?;
        release_single_tape_data_resources(&mut transaction, run).await?;
        mark_drive_motion_uncertain(&mut transaction, run.drive_id, detail).await?;
        if run.media_write_started {
            sqlx::query(
                "UPDATE cartridges cartridge
                    SET on_tape_identity = NULL, identity_status = 'unknown',
                        health_status = 'write_unverified', updated_at = now()
                   FROM single_tape_data_run_volumes volume
                  WHERE volume.single_tape_data_run_id = $1
                    AND volume.cartridge_id = cartridge.id
                    AND volume.media_write_started_at IS NOT NULL",
            )
            .bind(run.id)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "UPDATE single_tape_data_run_volumes
                    SET state = 'write_unverified', updated_at = now()
                  WHERE single_tape_data_run_id = $1
                    AND media_write_started_at IS NOT NULL",
            )
            .bind(run.id)
            .execute(&mut *transaction)
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(expired.len() as u64)
}

fn normalize_single_tape_subpath(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.len() > 512
        || trimmed.chars().any(char::is_control)
        || Path::new(trimmed).is_absolute()
    {
        return Err(
            "source and restore selections must be printable relative paths of at most 512 bytes"
                .to_owned(),
        );
    }
    let components = trimmed.split('/').collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(
            "source and restore selections must be normalized relative paths without empty, dot, or traversal components"
                .to_owned(),
        );
    }
    Ok(components.join("/"))
}

fn single_tape_path_blocker(path: &str, label: &str) -> Option<String> {
    if path.len() > 4096 || path.chars().any(char::is_control) {
        return Some(format!(
            "the {label} must be a printable absolute path of at most 4096 bytes"
        ));
    }
    let candidate = Path::new(path);
    if !candidate.is_absolute() || path == "/" || path.ends_with('/') || path.contains("//") {
        return Some(format!(
            "the {label} must be a normalized non-root absolute path"
        ));
    }
    let mut saw_root = false;
    for component in candidate.components() {
        match component {
            Component::RootDir if !saw_root => saw_root = true,
            Component::Normal(_) if saw_root => {}
            _ => {
                return Some(format!(
                    "the {label} must not contain dot, traversal, prefix, or relative components"
                ));
            }
        }
    }
    None
}

async fn single_tape_qualification_passed(
    transaction: &mut Transaction<'_, Postgres>,
    drive: &DriveRecord,
    cartridge_id: Uuid,
) -> Result<bool, sqlx::Error> {
    Ok(
        single_tape_qualification_passed_id(transaction, drive, cartridge_id)
            .await?
            .is_some(),
    )
}

async fn single_tape_qualification_passed_id(
    transaction: &mut Transaction<'_, Postgres>,
    drive: &DriveRecord,
    cartridge_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let result = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT CASE WHEN qualification.state = 'passed' THEN qualification.id END
         FROM qualification_runs qualification
         JOIN cartridges cartridge ON cartridge.id = qualification.cartridge_id
         WHERE qualification.drive_id = $1
           AND qualification.cartridge_id = $2
           AND qualification.expected_stable_id = $3
           AND qualification.expected_path = $4
           AND qualification.expected_vendor IS NOT DISTINCT FROM $5
           AND qualification.expected_model IS NOT DISTINCT FROM $6
           AND qualification.expected_serial IS NOT DISTINCT FROM $7
           AND qualification.expected_wwn IS NOT DISTINCT FROM $8
           AND qualification.expected_firmware IS NOT DISTINCT FROM $9
           AND qualification.expected_cartridge_label = cartridge.internal_label
           AND qualification.expected_media_generation IS NOT DISTINCT FROM cartridge.media_generation
           AND qualification.expected_media_format IS NOT DISTINCT FROM cartridge.media_format
           AND qualification.engine_name = 'Bacula Community'
           AND qualification.engine_version = '15.0.3'
           AND qualification.test_kind = 'btape_test'
           AND qualification.attempt > 0
         ORDER BY qualification.created_at DESC, qualification.id DESC
         LIMIT 1",
    )
    .bind(drive.id)
    .bind(cartridge_id)
    .bind(&drive.stable_id)
    .bind(drive.observed_path.as_deref().unwrap_or(""))
    .bind(&drive.vendor)
    .bind(&drive.model)
    .bind(&drive.serial)
    .bind(&drive.wwn)
    .bind(&drive.firmware)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(result.flatten())
}

#[allow(clippy::too_many_arguments)]
async fn append_job_transition(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    from_state: Option<&str>,
    to_state: &str,
    reason: &str,
    evidence: Value,
    correlation_id: Uuid,
) -> Result<(), sqlx::Error> {
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

#[allow(clippy::too_many_arguments)]
async fn append_single_tape_data_event(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    correlation_id: Uuid,
    event_type: &str,
    evidence: Value,
    actor_label: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO single_tape_data_events
            (single_tape_data_run_id, event_type, evidence, correlation_id)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(run_id)
    .bind(event_type)
    .bind(&evidence)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    let workflow_kind = sqlx::query_scalar::<_, String>(
        "SELECT workflow_kind FROM single_tape_data_runs WHERE id = $1",
    )
    .bind(run_id)
    .fetch_one(&mut **transaction)
    .await?;
    let action_namespace = if workflow_kind == "backup" {
        "backup"
    } else {
        "g0_2"
    };
    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         VALUES ($1, $2, $3, 'single_tape_data_run', $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(actor_label)
    .bind(format!("{action_namespace}.data_run.{event_type}"))
    .bind(run_id.to_string())
    .bind(evidence)
    .bind(event_type)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn lock_valid_single_tape_claim(
    transaction: &mut Transaction<'_, Postgres>,
    claim: &ClaimedSingleTapeDataRun,
) -> Result<bool, sqlx::Error> {
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT data_run.id
         FROM single_tape_data_runs data_run
         JOIN resource_leases drive_lease ON drive_lease.id = data_run.drive_lease_id
         JOIN resource_leases media_lease ON media_lease.id = data_run.media_lease_id
         WHERE data_run.id = $1 AND data_run.state = 'running'
           AND data_run.drive_lease_id = $2 AND data_run.drive_fencing_token = $3
           AND data_run.media_lease_id = $4 AND data_run.media_fencing_token = $5
           AND drive_lease.owner_id = $6
           AND drive_lease.fencing_token = $3
           AND drive_lease.released_at IS NULL AND drive_lease.expires_at > now()
           AND media_lease.owner_id = $6
           AND media_lease.fencing_token = $5
           AND media_lease.released_at IS NULL AND media_lease.expires_at > now()
           AND NOT EXISTS (
               SELECT 1
                 FROM single_tape_data_run_volumes volume
                 LEFT JOIN resource_leases extra_media_lease
                   ON extra_media_lease.id = volume.media_lease_id
                WHERE volume.single_tape_data_run_id = data_run.id
                  AND volume.volume_order > 1
                  AND (extra_media_lease.id IS NULL
                       OR extra_media_lease.owner_id <> $6
                       OR extra_media_lease.fencing_token <> volume.media_fencing_token
                       OR extra_media_lease.released_at IS NOT NULL
                       OR extra_media_lease.expires_at <= now()))
         FOR UPDATE OF data_run, drive_lease, media_lease",
    )
    .bind(claim.run.id)
    .bind(claim.drive_lease_id)
    .bind(claim.drive_fencing_token)
    .bind(claim.media_lease_id)
    .bind(claim.media_fencing_token)
    .bind(claim.worker_id)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(locked.is_some())
}

async fn release_single_tape_data_resources(
    transaction: &mut Transaction<'_, Postgres>,
    run: &SingleTapeDataRunRecord,
) -> Result<(), sqlx::Error> {
    // No terminal run may retain an actionable operator prompt. This also
    // closes a pending request when cancellation arrives while Bacula is
    // paused for a manual tape change.
    sqlx::query(
        "UPDATE single_tape_data_media_requests
            SET state = 'cancelled'
          WHERE single_tape_data_run_id = $1
            AND state IN ('pending', 'authorized', 'loaded')",
    )
    .bind(run.id)
    .execute(&mut **transaction)
    .await?;
    if let Some(drive_lease_id) = run.drive_lease_id {
        sqlx::query(
            "UPDATE resource_leases SET released_at = now()
             WHERE id = $1 AND released_at IS NULL",
        )
        .bind(drive_lease_id)
        .execute(&mut **transaction)
        .await?;
    }
    // The scalar media lease remains the compatibility mirror for volume one.
    // Release every ordered-volume lease as the authoritative operation so a
    // completed or failed spanning backup cannot strand continuation media.
    sqlx::query(
        "UPDATE resource_leases lease SET released_at = now()
           FROM single_tape_data_run_volumes volume
          WHERE volume.single_tape_data_run_id = $1
            AND volume.media_lease_id = lease.id
            AND lease.released_at IS NULL",
    )
    .bind(run.id)
    .execute(&mut **transaction)
    .await?;
    if let Some(media_lease_id) = run.media_lease_id {
        sqlx::query(
            "UPDATE resource_leases SET released_at = now()
             WHERE id = $1 AND released_at IS NULL",
        )
        .bind(media_lease_id)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        "UPDATE cartridges cartridge
            SET reserved_by = NULL, updated_at = now()
           FROM single_tape_data_run_volumes volume
          WHERE volume.single_tape_data_run_id = $1
            AND volume.cartridge_id = cartridge.id
            AND cartridge.reserved_by = $2",
    )
    .bind(run.id)
    .bind(format!("single_tape_data:{}", run.id))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn block_single_tape_data_run_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    run: &SingleTapeDataRunRecord,
    reason: &str,
) -> Result<(), sqlx::Error> {
    let result = json!({"detail": reason, "verified_recovery_point_created": false});
    sqlx::query(
        "UPDATE single_tape_data_runs
         SET state = 'blocked', result = $1, started_at = now(), finished_at = now()
         WHERE id = $2 AND state = 'queued'",
    )
    .bind(&result)
    .bind(run.id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE job_runs
         SET state = 'blocked', failure_class = $1,
             failure_detail = $2, started_at = now(), finished_at = now()
         WHERE id = $3 AND state = 'queued'",
    )
    .bind(format!("{}_precondition", run.workflow_kind))
    .bind(&result)
    .bind(run.job_run_id)
    .execute(&mut **transaction)
    .await?;
    append_job_transition(
        transaction,
        run.job_run_id,
        Some("queued"),
        "blocked",
        if run.workflow_kind == "backup" {
            "full backup was blocked before any tape data operation"
        } else {
            "G0.2 physical recovery exercise was blocked before any tape data operation"
        },
        result.clone(),
        run.correlation_id,
    )
    .await?;
    append_single_tape_data_event(
        transaction,
        run.id,
        run.correlation_id,
        "blocked",
        result,
        "tapescale-worker",
    )
    .await?;
    release_single_tape_data_resources(transaction, run).await
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

/// Resolve the content-addressed source manifest retained by the physical run
/// that created a recovery point. Optional artifact fields distinguish an
/// existing point with no browsable manifest from a missing recovery point.
pub async fn recovery_point_manifest_binding(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<RecoveryPointManifestBinding>, sqlx::Error> {
    sqlx::query_as(
        "SELECT artifact.storage_ref,
                artifact.sha256 AS artifact_sha256,
                artifact.size_bytes AS artifact_size_bytes,
                artifact.metadata->>'manifest_digest' AS manifest_digest
         FROM recovery_points recovery_point
         LEFT JOIN single_tape_data_runs data_run
           ON data_run.job_run_id = recovery_point.run_id
         LEFT JOIN single_tape_data_artifacts artifact
           ON artifact.single_tape_data_run_id = data_run.id
          AND artifact.artifact_kind = 'source_manifest'
          AND recovery_point.coverage->>'source_manifest_artifact_id' = artifact.id::text
         WHERE recovery_point.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub const RECOVERY_DEPENDENCY_LIMIT: usize = 128;
pub const RECOVERY_VOLUME_LIMIT: usize = 256;
pub const RECOVERY_HOLD_LIMIT: usize = 256;
pub const RECOVERY_VERIFICATION_LIMIT: usize = 64;
pub const RECOVERY_EVENT_LIMIT: usize = 128;
pub const RECOVERY_ARTIFACT_LIMIT: usize = 32;
pub const RECOVERY_JSON_VALUE_LIMIT_BYTES: usize = 8 * 1024;

/// Loads the bounded, operator-facing recovery-point aggregate used for both
/// inspection and restore planning. Artifact payloads remain in their durable
/// storage locations; this aggregate exposes only their content-addressed
/// metadata.
pub async fn get_recovery_point_aggregate(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<RecoveryPointAggregate>, sqlx::Error> {
    let Some(mut recovery_point) = sqlx::query_as::<_, RecoveryPointDetailRecord>(
        "SELECT rp.id, rp.source_id, rp.run_id, rp.representation, rp.level,
                rp.source_scope_hash, rp.format_version, rp.key_lineage_id,
                rp.status, rp.recovery_time, rp.expires_at, rp.indefinite,
                rp.coverage, rp.manifest_digest, rp.created_at,
                s.dataset_name, s.source_type, s.logical_root
         FROM recovery_points rp
         JOIN sources s ON s.id = rp.source_id
         WHERE rp.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let mut dependencies = sqlx::query_as::<_, RecoveryDependencyRecord>(
        "WITH RECURSIVE dependency_walk(id, dependency_kind, depth, path) AS (
             SELECT rd.depends_on_id, rd.dependency_kind, 1::integer,
                    ARRAY[$1, rd.depends_on_id]::uuid[]
             FROM recovery_dependencies rd
             WHERE rd.recovery_point_id = $1
             UNION ALL
             SELECT rd.depends_on_id, rd.dependency_kind, walk.depth + 1,
                    walk.path || rd.depends_on_id
             FROM dependency_walk walk
             JOIN recovery_dependencies rd ON rd.recovery_point_id = walk.id
             WHERE walk.depth < 128 AND NOT rd.depends_on_id = ANY(walk.path)
         ), unique_walk AS (
             SELECT DISTINCT ON (id) id, dependency_kind, depth
             FROM dependency_walk
             ORDER BY id, depth DESC
         )
         SELECT rp.id, rp.representation, rp.level, rp.status,
                rp.recovery_time, rp.expires_at, rp.indefinite,
                rp.format_version, rp.key_lineage_id, rp.manifest_digest,
                walk.dependency_kind, walk.depth
         FROM unique_walk walk
         JOIN recovery_points rp ON rp.id = walk.id
         ORDER BY walk.depth DESC, rp.recovery_time, rp.id
         LIMIT $2",
    )
    .bind(id)
    .bind(i64::try_from(RECOVERY_DEPENDENCY_LIMIT + 1).expect("small fixed limit"))
    .fetch_all(pool)
    .await?;
    let dependencies_truncated = truncate_rows(&mut dependencies, RECOVERY_DEPENDENCY_LIMIT);

    let mut recovery_ids = dependencies
        .iter()
        .map(|dependency| dependency.id)
        .collect::<Vec<_>>();
    recovery_ids.push(id);
    recovery_ids.sort_unstable();
    recovery_ids.dedup();

    let mut volume_rows = sqlx::query_as::<_, RecoveryVolumeRow>(
        "SELECT vr.id, vr.recovery_point_id, vr.volume_order,
                vr.first_record, vr.last_record, vr.encoded_bytes,
                vr.finalized, vr.engine_record_id,
                c.id AS cartridge_id, c.internal_label, c.barcode,
                c.media_generation, c.media_format, c.on_tape_identity,
                c.identity_status, c.health_status, c.worm, c.reserved_by,
                l.id AS location_id, l.name AS location_name, l.kind AS location_kind
         FROM volume_records vr
         JOIN cartridges c ON c.id = vr.cartridge_id
         LEFT JOIN locations l ON l.id = c.confirmed_location_id
         WHERE vr.recovery_point_id = ANY($1::uuid[])
         ORDER BY vr.recovery_point_id, vr.volume_order, vr.id
         LIMIT $2",
    )
    .bind(&recovery_ids)
    .bind(i64::try_from(RECOVERY_VOLUME_LIMIT + 1).expect("small fixed limit"))
    .fetch_all(pool)
    .await?;
    let volumes_truncated = truncate_rows(&mut volume_rows, RECOVERY_VOLUME_LIMIT);

    let drive_rows = sqlx::query_as::<_, RecoveryDriveRow>(
        "SELECT d.id, d.stable_id, d.generation, d.vendor, d.model,
                d.serial, d.wwn, d.firmware, d.observed_path, d.last_observed_at,
                COALESCE(
                    jsonb_agg(DISTINCT jsonb_build_object(
                        'cartridge_id', q.cartridge_id,
                        'generation', q.expected_media_generation,
                        'format', q.expected_media_format
                    )) FILTER (WHERE q.id IS NOT NULL),
                    '[]'::jsonb
                ) AS qualified_media
         FROM drives d
         LEFT JOIN LATERAL (
             SELECT latest.*
             FROM (
                 SELECT DISTINCT ON (
                            candidate.cartridge_id,
                            candidate.expected_media_generation,
                            candidate.expected_media_format
                        )
                        candidate.*
                 FROM qualification_runs candidate
                 JOIN cartridges qualified_cartridge
                   ON qualified_cartridge.id = candidate.cartridge_id
                 WHERE candidate.drive_id = d.id
                   -- A queued request that was rejected before btape ran does
                   -- not invalidate the last physical result.  Only terminal
                   -- attempts that could have changed our knowledge of the
                   -- drive/media pair participate in latest-result selection.
                   AND candidate.state IN ('passed', 'failed', 'interrupted')
                   AND candidate.expected_cartridge_label = qualified_cartridge.internal_label
                   AND candidate.expected_media_generation
                       IS NOT DISTINCT FROM qualified_cartridge.media_generation
                   AND candidate.expected_media_format
                       IS NOT DISTINCT FROM qualified_cartridge.media_format
                   AND candidate.engine_name = 'Bacula Community'
                   AND candidate.engine_version = '15.0.3'
                   AND candidate.test_kind = 'btape_test'
                   AND candidate.attempt > 0
                 ORDER BY candidate.cartridge_id,
                          candidate.expected_media_generation,
                          candidate.expected_media_format,
                          candidate.created_at DESC, candidate.id DESC
             ) latest
             WHERE latest.state = 'passed'
               AND latest.expected_stable_id = d.stable_id
               AND latest.expected_path IS NOT DISTINCT FROM d.observed_path
               AND latest.expected_vendor IS NOT DISTINCT FROM d.vendor
               AND latest.expected_model IS NOT DISTINCT FROM d.model
               AND latest.expected_serial IS NOT DISTINCT FROM d.serial
               AND latest.expected_wwn IS NOT DISTINCT FROM d.wwn
               AND latest.expected_firmware IS NOT DISTINCT FROM d.firmware
         ) q ON true
         WHERE d.reconciled
           AND d.health->>'present' = 'true'
           AND d.health->>'accessible' = 'true'
           AND d.health->>'identity_verified' = 'true'
           AND COALESCE(d.health->>'identity_conflict', 'false') <> 'true'
           AND COALESCE(d.health->>'motion_position_uncertain', 'false') <> 'true'
           AND d.health->>'immediate_mode' = 'false'
           AND d.health->>'in_flight' = '0'
           AND d.last_observed_at >= now() - interval '2 minutes'
         GROUP BY d.id
         ORDER BY d.stable_id",
    )
    .fetch_all(pool)
    .await?;

    let volumes = volume_rows
        .into_iter()
        .map(|volume| recovery_volume_with_drives(volume, &drive_rows))
        .collect::<Vec<_>>();
    let media_ids = volumes
        .iter()
        .map(|volume| volume.cartridge_id)
        .collect::<Vec<_>>();

    let mut holds = sqlx::query_as::<_, RecoveryHoldRecord>(
        "SELECT h.id, h.cartridge_id, h.recovery_point_id, h.kind, h.reason,
                h.expires_at, h.released_at, h.created_at,
                (h.released_at IS NULL AND
                 (h.expires_at IS NULL OR h.expires_at > now())) AS active
         FROM holds h
         WHERE h.recovery_point_id = ANY($1::uuid[])
            OR h.cartridge_id = ANY($2::uuid[])
         ORDER BY active DESC, h.created_at DESC, h.id
         LIMIT $3",
    )
    .bind(&recovery_ids)
    .bind(&media_ids)
    .bind(i64::try_from(RECOVERY_HOLD_LIMIT + 1).expect("small fixed limit"))
    .fetch_all(pool)
    .await?;
    let holds_truncated = truncate_rows(&mut holds, RECOVERY_HOLD_LIMIT);

    let mut verifications = sqlx::query_as::<_, RecoveryVerificationRecord>(
        "SELECT vr.id, vr.recovery_point_id, vr.kind, vr.scope, vr.outcome,
                vr.evidence, vr.correlation_id, vr.started_at, vr.finished_at,
                vr.drive_id, d.stable_id AS drive_stable_id, vr.media_ids,
                COALESCE(ARRAY(
                    SELECT cartridge.internal_label
                    FROM unnest(vr.media_ids) AS media(cartridge_id)
                    JOIN cartridges cartridge ON cartridge.id = media.cartridge_id
                    ORDER BY cartridge.internal_label
                ), '{}'::text[]) AS media_labels
         FROM verification_records vr
         LEFT JOIN drives d ON d.id = vr.drive_id
         WHERE vr.recovery_point_id = ANY($1::uuid[])
         ORDER BY vr.recovery_point_id, vr.started_at, vr.id
         LIMIT $2",
    )
    .bind(&recovery_ids)
    .bind(i64::try_from(RECOVERY_VERIFICATION_LIMIT + 1).expect("small fixed limit"))
    .fetch_all(pool)
    .await?;
    let verifications_truncated = truncate_rows(&mut verifications, RECOVERY_VERIFICATION_LIMIT);

    let mut origin = sqlx::query_as::<_, RecoveryOriginRecord>(
        "SELECT j.id AS job_id, j.external_id AS job_external_id,
                j.kind AS job_kind, jr.id AS run_id, jr.state AS run_state,
                jr.attempt, jr.correlation_id, jr.engine_operation_id,
                jr.created_at AS run_created_at, jr.started_at, jr.finished_at,
                data_run.id AS data_run_id, data_run.workflow_kind,
                profile.name AS profile_name,
                data_run.engine_name, data_run.engine_version,
                data_run.adapter_version, data_run.result AS data_run_result
         FROM job_runs jr
         JOIN jobs j ON j.id = jr.job_id
         LEFT JOIN single_tape_data_runs data_run ON data_run.job_run_id = jr.id
         LEFT JOIN single_tape_data_profiles profile ON profile.id = data_run.profile_id
         WHERE jr.id = $1",
    )
    .bind(recovery_point.run_id)
    .fetch_one(pool)
    .await?;

    let mut events = sqlx::query_as::<_, RecoveryEventRecord>(
        "SELECT event.source, event.event_id, event.event_type, event.detail,
                event.evidence, event.correlation_id, event.occurred_at
         FROM (
             SELECT 'job_transition'::text AS source, jt.id::text AS event_id,
                    jt.to_state AS event_type, jt.reason AS detail,
                    jt.evidence, jt.correlation_id, jt.occurred_at
             FROM job_transitions jt
             WHERE jt.run_id = $1
             UNION ALL
             SELECT 'recovery_exercise'::text AS source, event.id::text AS event_id,
                    event.event_type, ''::text AS detail,
                    event.evidence, event.correlation_id, event.occurred_at
             FROM single_tape_data_events event
             JOIN single_tape_data_runs data_run
               ON data_run.id = event.single_tape_data_run_id
             WHERE data_run.job_run_id = $1
         ) event
         ORDER BY event.occurred_at, event.source, event.event_id
         LIMIT $2",
    )
    .bind(recovery_point.run_id)
    .bind(i64::try_from(RECOVERY_EVENT_LIMIT + 1).expect("small fixed limit"))
    .fetch_all(pool)
    .await?;
    let events_truncated = truncate_rows(&mut events, RECOVERY_EVENT_LIMIT);

    let mut artifacts = sqlx::query_as::<_, RecoveryArtifactRecord>(
        "SELECT artifact.id, artifact.artifact_kind,
                artifact.sha256, artifact.size_bytes, artifact.metadata,
                artifact.created_at
         FROM single_tape_data_artifacts artifact
         JOIN single_tape_data_runs data_run
           ON data_run.id = artifact.single_tape_data_run_id
         WHERE data_run.job_run_id = $1
         ORDER BY artifact.created_at, artifact.id
         LIMIT $2",
    )
    .bind(recovery_point.run_id)
    .bind(i64::try_from(RECOVERY_ARTIFACT_LIMIT + 1).expect("small fixed limit"))
    .fetch_all(pool)
    .await?;
    let artifacts_truncated = truncate_rows(&mut artifacts, RECOVERY_ARTIFACT_LIMIT);

    let mut values_truncated = false;
    bound_value_in_place(&mut recovery_point.coverage, &mut values_truncated);
    for verification in &mut verifications {
        bound_value_in_place(&mut verification.scope, &mut values_truncated);
        bound_value_in_place(&mut verification.evidence, &mut values_truncated);
    }
    if let Some(result) = &mut origin.data_run_result {
        bound_value_in_place(result, &mut values_truncated);
    }
    for event in &mut events {
        bound_value_in_place(&mut event.evidence, &mut values_truncated);
    }
    for artifact in &mut artifacts {
        bound_value_in_place(&mut artifact.metadata, &mut values_truncated);
    }

    Ok(Some(RecoveryPointAggregate {
        recovery_point,
        volumes,
        dependencies,
        holds,
        verifications,
        origin,
        events,
        artifacts,
        caps: RecoveryAggregateCaps {
            dependencies_truncated,
            volumes_truncated,
            holds_truncated,
            verifications_truncated,
            events_truncated,
            artifacts_truncated,
            values_truncated,
        },
    }))
}

pub async fn create_restore_plan(
    pool: &PgPool,
    plan: &NewRestorePlan,
) -> Result<RestorePlanRecord, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let id = Uuid::now_v7();
    let record = sqlx::query_as::<_, RestorePlanRecord>(
        "INSERT INTO restore_plans
            (id, selection, dependency_chain, required_media_ids,
             key_lineage_ids, destination, conflict_policy, blockers,
             estimates, preview_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 now() + interval '30 minutes')
         RETURNING id, selection, dependency_chain, required_media_ids,
                   key_lineage_ids, destination, conflict_policy, blockers,
                   estimates, preview_hash, created_at, expires_at",
    )
    .bind(id)
    .bind(&plan.selection)
    .bind(&plan.dependency_chain)
    .bind(&plan.required_media_ids)
    .bind(&plan.key_lineage_ids)
    .bind(&plan.destination)
    .bind(&plan.conflict_policy)
    .bind(&plan.blockers)
    .bind(&plan.estimates)
    .bind(&plan.preview_hash)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events
            (id, actor_label, action, object_type, object_id,
             requested, result, correlation_id)
         VALUES ($1, 'local-api', 'restore.plan', 'restore_plan', $2,
                 $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(id.to_string())
    .bind(json!({
        "selection": plan.selection,
        "destination": plan.destination,
        "conflict_policy": plan.conflict_policy,
        "preview_hash": plan.preview_hash
    }))
    .bind(if plan.blockers.as_array().is_some_and(Vec::is_empty) {
        "ready"
    } else {
        "blocked"
    })
    .bind(Uuid::now_v7())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(record)
}

const RESTORE_REHEARSAL_RUN_SELECT: &str = "SELECT id, restore_plan_id, job_id, job_run_id,
            recovery_point_id, original_data_run_id, drive_id, cartridge_id,
            expected_stable_id, expected_path, expected_vendor, expected_model,
            expected_serial, expected_wwn, expected_firmware,
            expected_volume_label, expected_media_generation, expected_media_format,
            expected_on_tape_identity, backup_job_id, expected_engine_media_id,
            expected_engine_record_id, source_manifest_artifact_id,
            source_manifest_storage_ref, source_manifest_sha256, source_manifest_digest,
            source_manifest_size_bytes,
            source_mount_path, logical_root, destination_subpath,
            estimated_required_bytes, available_bytes, required_free_reserve_bytes,
            engine_name, engine_version, adapter_version, format_version,
            preview_hash, production_eligible, state, phase, requested_by_label,
            correlation_id, cancel_requested_at, cancel_requested_by_label,
            drive_lease_id, drive_fencing_token, media_lease_id, media_fencing_token,
            attempt, result, created_at, started_at, finished_at
     FROM restore_rehearsals";

#[derive(Clone, Debug, FromRow)]
struct RestoreRehearsalOriginBinding {
    recovery_point_id: Uuid,
    origin_job_run_id: Uuid,
    original_data_run_id: Uuid,
    workflow_kind: String,
    origin_drive_id: Uuid,
    source_snapshot_id: Uuid,
    representation: String,
    level: String,
    recovery_status: String,
    recovery_expires_at: Option<DateTime<Utc>>,
    recovery_indefinite: bool,
    key_lineage_id: Option<String>,
    recovery_format_version: String,
    manifest_digest: Option<String>,
    coverage: Value,
    origin_job_state: String,
    origin_engine_operation_id: Option<String>,
    origin_data_state: String,
    origin_result: Value,
    engine_name: String,
    engine_version: String,
    adapter_version: String,
    format_version: String,
    production_eligible: bool,
    encryption_deviation_confirmed: bool,
    source_mount_path: String,
    logical_root: String,
    source_manifest_artifact_id: i64,
    source_manifest_storage_ref: String,
    source_manifest_sha256: String,
    source_manifest_digest: Option<String>,
    source_manifest_size_bytes: i64,
}

#[derive(Clone, Debug, FromRow)]
struct RestoreRehearsalVolumeBinding {
    cartridge_id: Uuid,
    engine_record_id: String,
    finalized: bool,
    internal_label: String,
    media_generation: Option<i16>,
    media_format: Option<String>,
    on_tape_identity: Option<String>,
    identity_status: String,
    health_status: String,
    reserved_by: Option<String>,
    confirmed_location_id: Option<Uuid>,
}

pub async fn list_restore_rehearsals(
    pool: &PgPool,
) -> Result<Vec<RestoreRehearsalRunRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{RESTORE_REHEARSAL_RUN_SELECT}
         WHERE state IN ('queued', 'running')
            OR id IN (SELECT id FROM restore_rehearsals ORDER BY created_at DESC LIMIT 100)
         ORDER BY created_at DESC"
    ))
    .fetch_all(pool)
    .await
}

pub async fn get_restore_rehearsal(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<RestoreRehearsalRunRecord>, sqlx::Error> {
    sqlx::query_as(&format!("{RESTORE_REHEARSAL_RUN_SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn restore_rehearsal_events(
    pool: &PgPool,
    id: Uuid,
) -> Result<Vec<RestoreRehearsalEventRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, event_type, evidence, correlation_id, occurred_at
         FROM restore_rehearsal_events
         WHERE restore_rehearsal_id = $1 ORDER BY occurred_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

pub async fn restore_rehearsal_artifacts(
    pool: &PgPool,
    id: Uuid,
) -> Result<Vec<RestoreRehearsalArtifactRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, artifact_kind, storage_ref, sha256, size_bytes, metadata, created_at
         FROM restore_rehearsal_artifacts
         WHERE restore_rehearsal_id = $1 ORDER BY created_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

pub async fn running_restore_rehearsal_paths(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT expected_path FROM restore_rehearsals
         WHERE state = 'running' ORDER BY expected_path",
    )
    .fetch_all(pool)
    .await
}

async fn load_restore_rehearsal_origin(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_point_id: Uuid,
) -> Result<Option<RestoreRehearsalOriginBinding>, sqlx::Error> {
    sqlx::query_as(
        "SELECT rp.id AS recovery_point_id, rp.run_id AS origin_job_run_id,
                data_run.id AS original_data_run_id, data_run.workflow_kind,
                data_run.drive_id AS origin_drive_id,
                data_run.source_snapshot_id, rp.representation, rp.level,
                rp.status AS recovery_status, rp.expires_at AS recovery_expires_at,
                rp.indefinite AS recovery_indefinite, rp.key_lineage_id,
                rp.format_version AS recovery_format_version, rp.manifest_digest,
                rp.coverage, job_run.state AS origin_job_state,
                job_run.engine_operation_id AS origin_engine_operation_id,
                data_run.state AS origin_data_state, data_run.result AS origin_result,
                data_run.engine_name, data_run.engine_version,
                data_run.adapter_version, data_run.format_version,
                profile.production_eligible, data_run.encryption_deviation_confirmed,
                data_run.source_mount_path, data_run.logical_root,
                artifact.id AS source_manifest_artifact_id,
                artifact.storage_ref AS source_manifest_storage_ref,
                artifact.sha256 AS source_manifest_sha256,
                artifact.metadata->>'manifest_digest' AS source_manifest_digest,
                artifact.size_bytes AS source_manifest_size_bytes
         FROM recovery_points rp
         JOIN job_runs job_run ON job_run.id = rp.run_id
         JOIN single_tape_data_runs data_run ON data_run.job_run_id = job_run.id
         JOIN single_tape_data_profiles profile ON profile.id = data_run.profile_id
         JOIN single_tape_data_artifacts artifact
           ON artifact.single_tape_data_run_id = data_run.id
          AND artifact.artifact_kind = 'source_manifest'
         WHERE rp.id = $1
         FOR UPDATE OF rp, job_run, data_run, profile, artifact",
    )
    .bind(recovery_point_id)
    .fetch_optional(&mut **transaction)
    .await
}

async fn load_restore_rehearsal_volume(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_point_id: Uuid,
) -> Result<Option<RestoreRehearsalVolumeBinding>, sqlx::Error> {
    sqlx::query_as(
        "SELECT cartridge.id AS cartridge_id, volume.engine_record_id,
                volume.finalized, cartridge.internal_label,
                cartridge.media_generation, cartridge.media_format,
                cartridge.on_tape_identity, cartridge.identity_status,
                cartridge.health_status, cartridge.reserved_by,
                cartridge.confirmed_location_id
         FROM volume_records volume
         JOIN cartridges cartridge ON cartridge.id = volume.cartridge_id
         WHERE volume.recovery_point_id = $1 AND volume.volume_order = 1
         FOR UPDATE OF volume, cartridge",
    )
    .bind(recovery_point_id)
    .fetch_optional(&mut **transaction)
    .await
}

async fn restore_rehearsal_full_readback_passed(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_point_id: Uuid,
    drive_id: Uuid,
    cartridge_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM verification_records
             WHERE recovery_point_id = $1 AND kind = 'full_readback'
               AND outcome = 'passed' AND finished_at IS NOT NULL
               AND drive_id = $2 AND media_ids = ARRAY[$3]::uuid[]
         )",
    )
    .bind(recovery_point_id)
    .bind(drive_id)
    .bind(cartridge_id)
    .fetch_one(&mut **transaction)
    .await
}

fn restore_rehearsal_manifest_binding_valid(origin: &RestoreRehearsalOriginBinding) -> bool {
    let Some(source_manifest_digest) = origin.source_manifest_digest.as_deref() else {
        return false;
    };
    if !valid_sha256(&origin.source_manifest_sha256) || !valid_sha256(source_manifest_digest) {
        return false;
    }
    match origin.workflow_kind.as_str() {
        // The legacy recovery exercise used the serialized artifact identity as
        // the recovery-point manifest identity.
        "recovery_exercise" => {
            origin.manifest_digest.as_deref() == Some(origin.source_manifest_sha256.as_str())
        }
        // Operational backups retain the normalized file-manifest digest as
        // the recovery-point identity and the artifact digest independently.
        "backup" => origin.manifest_digest.as_deref() == Some(source_manifest_digest),
        _ => false,
    }
}

fn restore_plan_recovery_point_id(plan: &RestorePlanRecord) -> Option<Uuid> {
    plan.selection
        .get("request")
        .and_then(|request| request.get("recovery_point_id"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn restore_rehearsal_destination(plan: &RestorePlanRecord) -> Result<String, String> {
    let path = plan
        .destination
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "the restore plan has no destination path".to_owned())?;
    let normalized = normalize_single_tape_subpath(path)?;
    let destination_ready = plan.destination.get("approved").and_then(Value::as_bool) == Some(true)
        && plan
            .destination
            .get("is_new_destination")
            .and_then(Value::as_bool)
            == Some(true)
        && plan
            .destination
            .get("conflicts_found")
            .and_then(Value::as_u64)
            == Some(0)
        && plan.conflict_policy == "skip"
        && plan
            .destination
            .get("automatic_publication_requested")
            .and_then(Value::as_bool)
            != Some(true);
    if !destination_ready {
        return Err(
            "recovery rehearsal requires an approved, new, conflict-free, unpublished destination"
                .to_owned(),
        );
    }
    Ok(normalized)
}

fn restore_rehearsal_capacity(plan: &RestorePlanRecord) -> Result<(i64, i64, i64), String> {
    let estimated = plan
        .estimates
        .get("destination_required_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            "the restore plan lacks an asserted destination-capacity estimate".to_owned()
        })?;
    let available = plan
        .estimates
        .get("available_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            "the restore plan lacks an asserted available destination capacity".to_owned()
        })?;
    let reserve = plan
        .estimates
        .get("required_free_reserve_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| "the restore plan lacks a required free-space reserve".to_owned())?;
    let needed = estimated
        .checked_add(reserve)
        .ok_or_else(|| "the restore capacity assertion overflowed".to_owned())?;
    if available < needed {
        return Err(
            "the asserted available destination capacity does not cover the estimate and reserve"
                .to_owned(),
        );
    }
    let estimated = i64::try_from(estimated)
        .map_err(|_| "the destination estimate exceeds the supported range".to_owned())?;
    let available = i64::try_from(available)
        .map_err(|_| "the available capacity exceeds the supported range".to_owned())?;
    let reserve = i64::try_from(reserve)
        .map_err(|_| "the free-space reserve exceeds the supported range".to_owned())?;
    Ok((estimated, available, reserve))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_bacula_job_id(value: Option<&str>) -> Option<i64> {
    value?
        .strip_prefix("bacula-job:")?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}

fn parse_bacula_engine_record(value: &str) -> Option<(i64, i64)> {
    let remainder = value.strip_prefix("bacula-media:")?;
    let (media, job) = remainder.split_once(":job:")?;
    let media = media.parse::<i64>().ok().filter(|id| *id > 0)?;
    let job = job.parse::<i64>().ok().filter(|id| *id > 0)?;
    Some((media, job))
}

#[allow(clippy::too_many_lines)]
pub async fn queue_restore_rehearsal(
    pool: &PgPool,
    restore_plan_id: Uuid,
    preview_hash: &str,
    drive_id: Uuid,
    requested_by_label: &str,
    attestations: &RestoreRehearsalAttestations,
) -> Result<QueueRestoreRehearsalResult, sqlx::Error> {
    let requested_by_label = requested_by_label.trim();
    if requested_by_label.is_empty()
        || requested_by_label.len() > 256
        || requested_by_label.chars().any(char::is_control)
        || !valid_sha256(preview_hash)
    {
        return Ok(QueueRestoreRehearsalResult::Blocked(
            "the operator label and exact lowercase preview hash are required".to_owned(),
        ));
    }
    let expected_confirmation = format!("RESTORE PLAN {restore_plan_id} TO ISOLATED DESTINATION");
    if attestations.confirmation != expected_confirmation
        || !attestations.tape_loaded_confirmed
        || !attestations.isolated_destination_confirmed
        || !attestations.attended_confirmed
    {
        return Ok(QueueRestoreRehearsalResult::Blocked(
            "the exact attended restore-plan confirmation and all operator attestations are required"
                .to_owned(),
        ));
    }

    let mut transaction = pool.begin().await?;
    let plan = sqlx::query_as::<_, RestorePlanRecord>(
        "SELECT id, selection, dependency_chain, required_media_ids,
                key_lineage_ids, destination, conflict_policy, blockers,
                estimates, preview_hash, created_at, expires_at
         FROM restore_plans WHERE id = $1 FOR UPDATE",
    )
    .bind(restore_plan_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(plan) = plan else {
        transaction.rollback().await?;
        return Ok(QueueRestoreRehearsalResult::NotFound);
    };
    let existing = sqlx::query_as::<_, RestoreRehearsalRunRecord>(&format!(
        "{RESTORE_REHEARSAL_RUN_SELECT}
         WHERE restore_plan_id = $1 AND state IN ('queued', 'running')
         FOR UPDATE"
    ))
    .bind(restore_plan_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(existing) = existing {
        if existing.preview_hash == preview_hash && existing.drive_id == drive_id {
            transaction.commit().await?;
            return Ok(QueueRestoreRehearsalResult::Queued(Box::new(existing)));
        }
        transaction.rollback().await?;
        return Ok(QueueRestoreRehearsalResult::Blocked(
            "the restore plan already has active work with a different preview or drive".to_owned(),
        ));
    }
    let Some(recovery_point_id) = restore_plan_recovery_point_id(&plan) else {
        transaction.rollback().await?;
        return Ok(QueueRestoreRehearsalResult::Blocked(
            "the restore plan does not contain one valid recovery-point identity".to_owned(),
        ));
    };
    let destination_subpath = match restore_rehearsal_destination(&plan) {
        Ok(path) => path,
        Err(reason) => {
            transaction.rollback().await?;
            return Ok(QueueRestoreRehearsalResult::Blocked(reason));
        }
    };
    let (estimated_required_bytes, available_bytes, required_free_reserve_bytes) =
        match restore_rehearsal_capacity(&plan) {
            Ok(capacity) => capacity,
            Err(reason) => {
                transaction.rollback().await?;
                return Ok(QueueRestoreRehearsalResult::Blocked(reason));
            }
        };
    let plan_blocker = if plan.preview_hash != preview_hash {
        Some("the restore preview changed or the supplied preview hash is stale".to_owned())
    } else if plan.expires_at <= Utc::now() {
        Some("the restore readiness preview expired; generate and review a new plan".to_owned())
    } else if plan.selection.get("ready").and_then(Value::as_bool) != Some(true)
        || plan
            .selection
            .get("execution_supported")
            .and_then(Value::as_bool)
            != Some(true)
        || !plan.blockers.as_array().is_some_and(Vec::is_empty)
    {
        Some("the restore plan is not ready or still contains blockers".to_owned())
    } else if plan.dependency_chain != vec![recovery_point_id] {
        Some(
            "attended replay supports only one full recovery point without dependencies".to_owned(),
        )
    } else if !plan.key_lineage_ids.is_empty() {
        Some(
            "encrypted or key-dependent recovery points are outside the attended replay gate"
                .to_owned(),
        )
    } else if plan.required_media_ids.len() != 1 {
        Some("attended replay requires exactly one finalized cartridge".to_owned())
    } else {
        None
    };
    if let Some(reason) = plan_blocker {
        transaction.rollback().await?;
        return Ok(QueueRestoreRehearsalResult::Blocked(reason));
    }

    let origin = load_restore_rehearsal_origin(&mut transaction, recovery_point_id).await?;
    let volume = load_restore_rehearsal_volume(&mut transaction, recovery_point_id).await?;
    let drive = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT} WHERE d.id = $1 FOR UPDATE OF d"
    ))
    .bind(drive_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let (Some(origin), Some(volume), Some(drive)) = (origin, volume, drive) else {
        transaction.rollback().await?;
        return Ok(QueueRestoreRehearsalResult::NotFound);
    };
    let dependency_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM recovery_dependencies WHERE recovery_point_id = $1",
    )
    .bind(recovery_point_id)
    .fetch_one(&mut *transaction)
    .await?;
    let volume_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM volume_records WHERE recovery_point_id = $1",
    )
    .bind(recovery_point_id)
    .fetch_one(&mut *transaction)
    .await?;
    let backup_job_id = parse_bacula_job_id(origin.origin_engine_operation_id.as_deref());
    let engine_record = parse_bacula_engine_record(&volume.engine_record_id);
    let engine_volume_name = origin
        .origin_result
        .get("engine_volume_name")
        .and_then(Value::as_str);
    let coverage_artifact_id = origin
        .coverage
        .get("source_manifest_artifact_id")
        .and_then(Value::as_i64);
    let qualification_passed =
        single_tape_qualification_passed(&mut transaction, &drive, volume.cartridge_id).await?;
    let full_readback_passed = restore_rehearsal_full_readback_passed(
        &mut transaction,
        recovery_point_id,
        drive.id,
        volume.cartridge_id,
    )
    .await?;
    let readable = drive
        .generation
        .and_then(drive_generation)
        .zip(media_format(
            volume.media_generation,
            volume.media_format.as_deref(),
        ))
        .is_some_and(|(drive, media)| can_read(drive, media));

    let eligibility_blocker = if origin.recovery_point_id != recovery_point_id
        || origin.representation != "file"
        || origin.level != "full"
        || origin.recovery_status != "verified"
        || origin.key_lineage_id.is_some()
    {
        Some("only a verified, unencrypted full file recovery point can be replayed".to_owned())
    } else if !origin.recovery_indefinite
        && origin
            .recovery_expires_at
            .is_none_or(|expires_at| expires_at <= Utc::now())
    {
        Some("the selected recovery point has expired and is not retained indefinitely".to_owned())
    } else if !matches!(
        origin.workflow_kind.as_str(),
        "recovery_exercise" | "backup"
    ) {
        Some(
            "the recovery point did not originate from a supported physical backup workflow"
                .to_owned(),
        )
    } else if origin.recovery_format_version != "bacula-community-15.0.3-lab-v1"
        || origin.format_version != "bacula-community-15.0.3-lab-v1"
        || origin.engine_name != "Bacula Community"
        || origin.engine_version != "15.0.3"
        || origin.adapter_version != "g0.2-lab-v1"
    {
        Some("the recovery point is not bound to the supported pinned Bacula format".to_owned())
    } else if origin.origin_job_state != "verified"
        || origin.origin_data_state != "passed"
        || origin.production_eligible
        || !origin.encryption_deviation_confirmed
    {
        Some("the originating physical backup is not a passed, verified run".to_owned())
    } else if dependency_count != 0 || volume_count != 1 {
        Some("the recovery point is not a dependency-free single-volume capture".to_owned())
    } else if plan.required_media_ids.as_slice() != [volume.cartridge_id] {
        Some("the plan's exact cartridge no longer matches the recovery point".to_owned())
    } else if !volume.finalized {
        Some("the recovery point's only volume record is not finalized".to_owned())
    } else if backup_job_id.is_none()
        || engine_record.is_none()
        || engine_record.map(|(_, job)| job) != backup_job_id
    {
        Some("the retained Bacula backup JobId or media record binding is invalid".to_owned())
    } else if coverage_artifact_id != Some(origin.source_manifest_artifact_id)
        || !restore_rehearsal_manifest_binding_valid(&origin)
        || origin.source_manifest_size_bytes <= 0
    {
        Some("the retained source-manifest identity is incomplete or inconsistent".to_owned())
    } else if !full_readback_passed {
        Some(
            "the exact single-volume recovery point lacks a completed physical full-readback verification"
                .to_owned(),
        )
    } else if drive.id != origin.origin_drive_id {
        Some(
            "this attended replay requires the exact drive used by the verified physical backup"
                .to_owned(),
        )
    } else if let Some(reason) = drive_motion_blocker(&drive) {
        Some(reason)
    } else if !readable || !qualification_passed {
        Some("the exact drive/cartridge pair lacks current read eligibility and passed qualification".to_owned())
    } else if volume.identity_status != "reconciled"
        || !matches!(
            volume.health_status.as_str(),
            "written_verified" | "healthy" | "verified"
        )
        || volume.confirmed_location_id.is_none()
    {
        Some(
            "the cartridge identity, verified health, or confirmed custody is not ready".to_owned(),
        )
    } else if volume.reserved_by.is_some() {
        Some("the required cartridge is already reserved".to_owned())
    } else if engine_volume_name.is_none()
        || volume.on_tape_identity.as_deref() != engine_volume_name
    {
        Some("the current on-tape identity disagrees with the verified backup volume".to_owned())
    } else {
        None
    };
    if let Some(reason) = eligibility_blocker {
        transaction.rollback().await?;
        return Ok(QueueRestoreRehearsalResult::Blocked(reason));
    }

    let busy = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM drive_operations
             WHERE drive_id = $1 AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM qualification_runs
             WHERE (drive_id = $1 OR cartridge_id = $2) AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM single_tape_data_runs
             WHERE (drive_id = $1 OR cartridge_id = $2) AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM restore_rehearsals
             WHERE (restore_plan_id = $3 OR drive_id = $1 OR cartridge_id = $2)
               AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM resource_leases
             WHERE ((resource_kind = 'drive' AND resource_id = $1::text)
                 OR (resource_kind = 'media' AND resource_id = $2::text))
               AND released_at IS NULL AND expires_at > now()
         )",
    )
    .bind(drive_id)
    .bind(volume.cartridge_id)
    .bind(restore_plan_id)
    .fetch_one(&mut *transaction)
    .await?;
    if busy {
        transaction.rollback().await?;
        return Ok(QueueRestoreRehearsalResult::Blocked(
            "the restore plan, exact drive, or cartridge already has queued work or an active controller lease"
                .to_owned(),
        ));
    }

    let id = Uuid::now_v7();
    let job_id = Uuid::now_v7();
    let job_run_id = Uuid::now_v7();
    let correlation_id = Uuid::now_v7();
    let expected_path = drive
        .observed_path
        .as_deref()
        .expect("the drive gate requires an observed path");
    let expected_volume_label = engine_volume_name.expect("the identity gate checked this");
    let backup_job_id = backup_job_id.expect("the backup JobId gate checked this");
    let expected_engine_media_id = engine_record
        .expect("the engine record gate checked this")
        .0;
    let source_manifest_digest = origin
        .source_manifest_digest
        .as_deref()
        .expect("the manifest digest gate checked this");
    sqlx::query(
        "INSERT INTO jobs (id, external_id, kind, priority)
         VALUES ($1, $2, 'restore', 10)",
    )
    .bind(job_id)
    .bind(format!("restore-rehearsal-{id}"))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO job_runs
            (id, job_id, attempt, state, correlation_id, source_snapshot_id)
         VALUES ($1, $2, 1, 'queued', $3, $4)",
    )
    .bind(job_run_id)
    .bind(job_id)
    .bind(correlation_id)
    .bind(origin.source_snapshot_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO restore_rehearsals
            (id, restore_plan_id, job_id, job_run_id, recovery_point_id,
             original_data_run_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_vendor, expected_model,
             expected_serial, expected_wwn, expected_firmware,
             expected_volume_label, expected_media_generation, expected_media_format,
             expected_on_tape_identity, backup_job_id, expected_engine_media_id,
             expected_engine_record_id, source_manifest_artifact_id,
             source_manifest_storage_ref, source_manifest_sha256, source_manifest_digest,
             source_manifest_size_bytes, source_mount_path, logical_root,
             destination_subpath, estimated_required_bytes, available_bytes,
             required_free_reserve_bytes, engine_name, engine_version,
             adapter_version, format_version, preview_hash,
             requested_by_label, correlation_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
                 $21, $22, $23, $24, $25, $26, $27, $28, $29, $30,
                 $31, $32, $33, $34, $35, $36, $37, $38, $39, $40)",
    )
    .bind(id)
    .bind(restore_plan_id)
    .bind(job_id)
    .bind(job_run_id)
    .bind(recovery_point_id)
    .bind(origin.original_data_run_id)
    .bind(drive_id)
    .bind(volume.cartridge_id)
    .bind(&drive.stable_id)
    .bind(expected_path)
    .bind(&drive.vendor)
    .bind(&drive.model)
    .bind(&drive.serial)
    .bind(&drive.wwn)
    .bind(&drive.firmware)
    .bind(expected_volume_label)
    .bind(volume.media_generation)
    .bind(&volume.media_format)
    .bind(expected_volume_label)
    .bind(backup_job_id)
    .bind(expected_engine_media_id)
    .bind(&volume.engine_record_id)
    .bind(origin.source_manifest_artifact_id)
    .bind(&origin.source_manifest_storage_ref)
    .bind(&origin.source_manifest_sha256)
    .bind(source_manifest_digest)
    .bind(origin.source_manifest_size_bytes)
    .bind(&origin.source_mount_path)
    .bind(&origin.logical_root)
    .bind(&destination_subpath)
    .bind(estimated_required_bytes)
    .bind(available_bytes)
    .bind(required_free_reserve_bytes)
    .bind(&origin.engine_name)
    .bind(&origin.engine_version)
    .bind(&origin.adapter_version)
    .bind(&origin.format_version)
    .bind(preview_hash)
    .bind(requested_by_label)
    .bind(correlation_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE cartridges SET reserved_by = $1, updated_at = now() WHERE id = $2")
        .bind(format!("restore_rehearsal:{id}"))
        .bind(volume.cartridge_id)
        .execute(&mut *transaction)
        .await?;
    append_job_transition(
        &mut transaction,
        job_run_id,
        None,
        "queued",
        "attended read-only recovery rehearsal queued",
        json!({"restore_rehearsal_id": id, "restore_plan_id": restore_plan_id,
               "recovery_point_id": recovery_point_id, "production_eligible": false}),
        correlation_id,
    )
    .await?;
    append_restore_rehearsal_event_in_transaction(
        &mut transaction,
        id,
        correlation_id,
        "queued",
        json!({
            "job_run_id": job_run_id,
            "origin_job_run_id": origin.origin_job_run_id,
            "restore_plan_id": restore_plan_id,
            "preview_hash": preview_hash,
            "recovery_point_id": recovery_point_id,
            "drive_id": drive_id,
            "stable_id": drive.stable_id,
            "cartridge_id": volume.cartridge_id,
            "cartridge_label": volume.internal_label,
            "bacula_backup_job_id": backup_job_id,
            "bacula_media_id": expected_engine_media_id,
            "destination_subpath": destination_subpath,
            "estimated_required_bytes": estimated_required_bytes,
            "available_bytes": available_bytes,
            "required_free_reserve_bytes": required_free_reserve_bytes,
            "operator_attestations": {
                "confirmation": attestations.confirmation,
                "tape_loaded_confirmed": attestations.tape_loaded_confirmed,
                "isolated_destination_confirmed": attestations.isolated_destination_confirmed,
                "attended_confirmed": attestations.attended_confirmed,
                "assertion_only": true,
                "safety_authority": false
            },
            "origin_workflow_kind": origin.workflow_kind,
            "scope": "attended single-volume full-file recovery replay",
            "production_eligible": false
        }),
        requested_by_label,
    )
    .await?;
    let run = sqlx::query_as(&format!("{RESTORE_REHEARSAL_RUN_SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(QueueRestoreRehearsalResult::Queued(Box::new(run)))
}

#[allow(clippy::too_many_arguments)]
async fn append_restore_rehearsal_event_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    correlation_id: Uuid,
    event_type: &str,
    evidence: Value,
    actor_label: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO restore_rehearsal_events
            (restore_rehearsal_id, event_type, evidence, correlation_id)
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
         VALUES ($1, $2, $3, 'restore_rehearsal', $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(actor_label)
    .bind(format!("restore_rehearsal.{event_type}"))
    .bind(run_id.to_string())
    .bind(evidence)
    .bind(event_type)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn lock_valid_restore_rehearsal_claim(
    transaction: &mut Transaction<'_, Postgres>,
    claim: &ClaimedRestoreRehearsalRun,
) -> Result<bool, sqlx::Error> {
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT rehearsal.id
         FROM restore_rehearsals rehearsal
         JOIN resource_leases drive_lease ON drive_lease.id = rehearsal.drive_lease_id
         JOIN resource_leases media_lease ON media_lease.id = rehearsal.media_lease_id
         WHERE rehearsal.id = $1 AND rehearsal.state = 'running'
           AND rehearsal.drive_lease_id = $2 AND rehearsal.drive_fencing_token = $3
           AND rehearsal.media_lease_id = $4 AND rehearsal.media_fencing_token = $5
           AND drive_lease.owner_id = $6 AND drive_lease.fencing_token = $3
           AND drive_lease.released_at IS NULL AND drive_lease.expires_at > now()
           AND media_lease.owner_id = $6 AND media_lease.fencing_token = $5
           AND media_lease.released_at IS NULL AND media_lease.expires_at > now()
         FOR UPDATE OF rehearsal, drive_lease, media_lease",
    )
    .bind(claim.run.id)
    .bind(claim.drive_lease_id)
    .bind(claim.drive_fencing_token)
    .bind(claim.media_lease_id)
    .bind(claim.media_fencing_token)
    .bind(claim.worker_id)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(locked.is_some())
}

async fn release_restore_rehearsal_resources(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RestoreRehearsalRunRecord,
) -> Result<(), sqlx::Error> {
    if let Some(lease_id) = run.drive_lease_id {
        sqlx::query(
            "UPDATE resource_leases SET released_at = now()
             WHERE id = $1 AND released_at IS NULL",
        )
        .bind(lease_id)
        .execute(&mut **transaction)
        .await?;
    }
    if let Some(lease_id) = run.media_lease_id {
        sqlx::query(
            "UPDATE resource_leases SET released_at = now()
             WHERE id = $1 AND released_at IS NULL",
        )
        .bind(lease_id)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        "UPDATE cartridges SET reserved_by = NULL, updated_at = now()
         WHERE id = $1 AND reserved_by = $2",
    )
    .bind(run.cartridge_id)
    .bind(format!("restore_rehearsal:{}", run.id))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn block_restore_rehearsal_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    run: &RestoreRehearsalRunRecord,
    reason: &str,
) -> Result<(), sqlx::Error> {
    let result = json!({
        "detail": reason,
        "tape_command_dispatched": false,
        "restore_test_passed": false,
        "production_eligible": false
    });
    let updated = sqlx::query(
        "UPDATE restore_rehearsals
         SET state = 'blocked', phase = 'blocked', result = $1,
             started_at = now(), finished_at = now()
         WHERE id = $2 AND state = 'queued'",
    )
    .bind(&result)
    .bind(run.id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Ok(());
    }
    sqlx::query(
        "UPDATE job_runs
         SET state = 'blocked', failure_class = 'restore_rehearsal_precondition',
             failure_detail = $1, started_at = now(), finished_at = now()
         WHERE id = $2 AND state = 'queued'",
    )
    .bind(&result)
    .bind(run.job_run_id)
    .execute(&mut **transaction)
    .await?;
    append_job_transition(
        transaction,
        run.job_run_id,
        Some("queued"),
        "blocked",
        "recovery rehearsal was blocked before any tape read",
        result.clone(),
        run.correlation_id,
    )
    .await?;
    append_restore_rehearsal_event_in_transaction(
        transaction,
        run.id,
        run.correlation_id,
        "blocked",
        result,
        "tapescale-worker",
    )
    .await?;
    release_restore_rehearsal_resources(transaction, run).await
}

#[allow(clippy::too_many_lines)]
pub async fn claim_next_restore_rehearsal_run(
    pool: &PgPool,
    worker_id: Uuid,
) -> Result<ClaimRestoreRehearsalRunResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let run = sqlx::query_as::<_, RestoreRehearsalRunRecord>(&format!(
        "{RESTORE_REHEARSAL_RUN_SELECT}
         WHERE state = 'queued' ORDER BY created_at
         FOR UPDATE SKIP LOCKED LIMIT 1"
    ))
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(run) = run else {
        transaction.rollback().await?;
        return Ok(ClaimRestoreRehearsalRunResult::Empty);
    };
    let plan = sqlx::query_as::<_, RestorePlanRecord>(
        "SELECT id, selection, dependency_chain, required_media_ids,
                key_lineage_ids, destination, conflict_policy, blockers,
                estimates, preview_hash, created_at, expires_at
         FROM restore_plans WHERE id = $1 FOR UPDATE",
    )
    .bind(run.restore_plan_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let origin = load_restore_rehearsal_origin(&mut transaction, run.recovery_point_id).await?;
    let volume = load_restore_rehearsal_volume(&mut transaction, run.recovery_point_id).await?;
    let drive = sqlx::query_as::<_, DriveRecord>(&format!(
        "{DRIVE_SELECT} WHERE d.id = $1 FOR UPDATE OF d"
    ))
    .bind(run.drive_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let (Some(plan), Some(origin), Some(volume), Some(drive)) = (plan, origin, volume, drive)
    else {
        let reason =
            "a bound restore plan, recovery point, drive, or cartridge no longer exists".to_owned();
        block_restore_rehearsal_in_transaction(&mut transaction, &run, &reason).await?;
        transaction.commit().await?;
        return Ok(ClaimRestoreRehearsalRunResult::Blocked { id: run.id, reason });
    };

    let destination = restore_rehearsal_destination(&plan);
    let capacity = restore_rehearsal_capacity(&plan);
    let backup_job_id = parse_bacula_job_id(origin.origin_engine_operation_id.as_deref());
    let engine_record = parse_bacula_engine_record(&volume.engine_record_id);
    let expected_reservation = format!("restore_rehearsal:{}", run.id);
    let qualification_passed =
        single_tape_qualification_passed(&mut transaction, &drive, run.cartridge_id).await?;
    let full_readback_passed = restore_rehearsal_full_readback_passed(
        &mut transaction,
        run.recovery_point_id,
        run.drive_id,
        run.cartridge_id,
    )
    .await?;
    let (dependency_count, volume_count) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT
             (SELECT count(*) FROM recovery_dependencies WHERE recovery_point_id = $1),
             (SELECT count(*) FROM volume_records WHERE recovery_point_id = $1)",
    )
    .bind(run.recovery_point_id)
    .fetch_one(&mut *transaction)
    .await?;
    let readable = drive
        .generation
        .and_then(drive_generation)
        .zip(media_format(
            volume.media_generation,
            volume.media_format.as_deref(),
        ))
        .is_some_and(|(drive, media)| can_read(drive, media));
    let conflicts = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM drive_operations
             WHERE drive_id = $1 AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM qualification_runs
             WHERE (drive_id = $1 OR cartridge_id = $2) AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM single_tape_data_runs
             WHERE (drive_id = $1 OR cartridge_id = $2) AND state IN ('queued', 'running')
             UNION ALL
             SELECT 1 FROM restore_rehearsals
             WHERE id <> $3 AND (drive_id = $1 OR cartridge_id = $2)
               AND state IN ('queued', 'running')
         )",
    )
    .bind(run.drive_id)
    .bind(run.cartridge_id)
    .bind(run.id)
    .fetch_one(&mut *transaction)
    .await?;
    let coverage_artifact_id = origin
        .coverage
        .get("source_manifest_artifact_id")
        .and_then(Value::as_i64);
    let blocker = if run.cancel_requested_at.is_some() {
        Some("the recovery rehearsal was cancelled before worker dispatch".to_owned())
    } else if plan.preview_hash != run.preview_hash
        || plan.expires_at <= Utc::now()
        || plan.selection.get("ready").and_then(Value::as_bool) != Some(true)
        || plan
            .selection
            .get("execution_supported")
            .and_then(Value::as_bool)
            != Some(true)
        || !plan.blockers.as_array().is_some_and(Vec::is_empty)
    {
        Some("the restore plan expired, changed, or is no longer ready".to_owned())
    } else if plan.dependency_chain != vec![run.recovery_point_id]
        || plan.required_media_ids.as_slice() != [run.cartridge_id]
        || !plan.key_lineage_ids.is_empty()
    {
        Some("the plan no longer describes the exact dependency-free single cartridge".to_owned())
    } else if destination.as_deref() != Ok(run.destination_subpath.as_str())
        || capacity
            != Ok((
                run.estimated_required_bytes,
                run.available_bytes,
                run.required_free_reserve_bytes,
            ))
    {
        Some("the approved destination or capacity assertions changed after queuing".to_owned())
    } else if origin.original_data_run_id != run.original_data_run_id
        || origin.origin_drive_id != run.drive_id
        || !matches!(
            origin.workflow_kind.as_str(),
            "recovery_exercise" | "backup"
        )
        || origin.representation != "file"
        || origin.level != "full"
        || origin.recovery_status != "verified"
        || origin.key_lineage_id.is_some()
        || origin.origin_job_state != "verified"
        || origin.origin_data_state != "passed"
        || origin.production_eligible
        || !origin.encryption_deviation_confirmed
    {
        Some(
            "the originating full-file physical-backup evidence changed or is no longer valid"
                .to_owned(),
        )
    } else if !origin.recovery_indefinite
        && origin
            .recovery_expires_at
            .is_none_or(|expires_at| expires_at <= Utc::now())
    {
        Some("the selected recovery point expired before worker dispatch".to_owned())
    } else if origin.engine_name != run.engine_name
        || origin.engine_version != run.engine_version
        || origin.adapter_version != run.adapter_version
        || origin.format_version != run.format_version
        || origin.recovery_format_version != run.format_version
        || origin.source_mount_path != run.source_mount_path
        || origin.logical_root != run.logical_root
    {
        Some("the pinned engine, format, or source identity changed after queuing".to_owned())
    } else if coverage_artifact_id != Some(run.source_manifest_artifact_id)
        || origin.source_manifest_artifact_id != run.source_manifest_artifact_id
        || origin.source_manifest_storage_ref != run.source_manifest_storage_ref
        || origin.source_manifest_sha256 != run.source_manifest_sha256
        || origin.source_manifest_digest.as_deref() != Some(run.source_manifest_digest.as_str())
        || origin.source_manifest_size_bytes != run.source_manifest_size_bytes
        || !restore_rehearsal_manifest_binding_valid(&origin)
        || !valid_sha256(&run.source_manifest_sha256)
        || !valid_sha256(&run.source_manifest_digest)
    {
        Some("the retained source-manifest binding changed or is incomplete".to_owned())
    } else if dependency_count != 0 || volume_count != 1 || !full_readback_passed {
        Some(
            "the recovery point no longer has one dependency-free, fully read-back physical volume"
                .to_owned(),
        )
    } else if backup_job_id != Some(run.backup_job_id)
        || engine_record != Some((run.expected_engine_media_id, run.backup_job_id))
        || volume.engine_record_id != run.expected_engine_record_id
    {
        Some("the retained Bacula job, media, or record binding changed".to_owned())
    } else if volume.cartridge_id != run.cartridge_id
        || !volume.finalized
        || volume.on_tape_identity.as_deref() != Some(run.expected_volume_label.as_str())
        || volume.identity_status != "reconciled"
        || !matches!(
            volume.health_status.as_str(),
            "written_verified" | "healthy" | "verified"
        )
        || volume.confirmed_location_id.is_none()
        || volume.reserved_by.as_deref() != Some(expected_reservation.as_str())
    {
        Some("the exact cartridge identity, health, custody, or reservation changed".to_owned())
    } else if drive.stable_id != run.expected_stable_id
        || drive.observed_path.as_deref() != Some(run.expected_path.as_str())
        || drive.vendor != run.expected_vendor
        || drive.model != run.expected_model
        || drive.serial != run.expected_serial
        || drive.wwn != run.expected_wwn
        || drive.firmware != run.expected_firmware
    {
        Some("the exact drive identity, path, or firmware changed after queuing".to_owned())
    } else if let Some(reason) = drive_motion_blocker(&drive) {
        Some(reason)
    } else if !readable || !qualification_passed {
        Some("the exact drive/cartridge pair is no longer read-eligible and qualified".to_owned())
    } else if conflicts {
        Some("another tape operation conflicts with the queued recovery rehearsal".to_owned())
    } else {
        None
    };
    if let Some(reason) = blocker {
        block_restore_rehearsal_in_transaction(&mut transaction, &run, &reason).await?;
        transaction.commit().await?;
        return Ok(ClaimRestoreRehearsalRunResult::Blocked { id: run.id, reason });
    }

    sqlx::query(
        "UPDATE resource_leases SET released_at = now()
         WHERE ((resource_kind = 'drive' AND resource_id = $1::text)
             OR (resource_kind = 'media' AND resource_id = $2::text))
           AND released_at IS NULL AND expires_at <= now()",
    )
    .bind(run.drive_id)
    .bind(run.cartridge_id)
    .execute(&mut *transaction)
    .await?;
    let drive_lease_id = Uuid::now_v7();
    let drive_fencing_token = sqlx::query_scalar::<_, i64>(
        "INSERT INTO resource_leases
            (id, resource_kind, resource_id, owner_id, fencing_token, expires_at)
         VALUES ($1, 'drive', $2::text, $3, nextval('resource_fencing_token_seq'),
                 now() + interval '30 minutes')
         ON CONFLICT DO NOTHING RETURNING fencing_token",
    )
    .bind(drive_lease_id)
    .bind(run.drive_id)
    .bind(worker_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(drive_fencing_token) = drive_fencing_token else {
        transaction.rollback().await?;
        return Ok(ClaimRestoreRehearsalRunResult::Empty);
    };
    let media_lease_id = Uuid::now_v7();
    let media_fencing_token = sqlx::query_scalar::<_, i64>(
        "INSERT INTO resource_leases
            (id, resource_kind, resource_id, owner_id, fencing_token, expires_at)
         VALUES ($1, 'media', $2::text, $3, nextval('resource_fencing_token_seq'),
                 now() + interval '30 minutes')
         ON CONFLICT DO NOTHING RETURNING fencing_token",
    )
    .bind(media_lease_id)
    .bind(run.cartridge_id)
    .bind(worker_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(media_fencing_token) = media_fencing_token else {
        transaction.rollback().await?;
        return Ok(ClaimRestoreRehearsalRunResult::Empty);
    };
    sqlx::query(
        "UPDATE restore_rehearsals
         SET state = 'running', phase = 'preparing',
             drive_lease_id = $1, drive_fencing_token = $2,
             media_lease_id = $3, media_fencing_token = $4,
             attempt = attempt + 1, started_at = now()
         WHERE id = $5 AND state = 'queued'",
    )
    .bind(drive_lease_id)
    .bind(drive_fencing_token)
    .bind(media_lease_id)
    .bind(media_fencing_token)
    .bind(run.id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE job_runs SET state = 'preparing', started_at = now()
         WHERE id = $1 AND state = 'queued'",
    )
    .bind(run.job_run_id)
    .execute(&mut *transaction)
    .await?;
    let lease_evidence = json!({
        "drive_lease_id": drive_lease_id,
        "drive_fencing_token": drive_fencing_token,
        "media_lease_id": media_lease_id,
        "media_fencing_token": media_fencing_token,
        "read_only_rehearsal": true,
        "production_eligible": false
    });
    append_job_transition(
        &mut transaction,
        run.job_run_id,
        Some("queued"),
        "preparing",
        "worker claimed the fenced read-only recovery rehearsal",
        lease_evidence.clone(),
        run.correlation_id,
    )
    .await?;
    append_restore_rehearsal_event_in_transaction(
        &mut transaction,
        run.id,
        run.correlation_id,
        "started",
        lease_evidence,
        "tapescale-worker",
    )
    .await?;
    let run = sqlx::query_as(&format!("{RESTORE_REHEARSAL_RUN_SELECT} WHERE id = $1"))
        .bind(run.id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(ClaimRestoreRehearsalRunResult::Claimed(Box::new(
        ClaimedRestoreRehearsalRun {
            run,
            drive_lease_id,
            drive_fencing_token,
            media_lease_id,
            media_fencing_token,
            worker_id,
        },
    )))
}

pub async fn renew_restore_rehearsal_run_leases(
    pool: &PgPool,
    claim: &ClaimedRestoreRehearsalRun,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_restore_rehearsal_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let drive = sqlx::query(
        "UPDATE resource_leases SET expires_at = now() + interval '30 minutes'
         WHERE id = $1 AND owner_id = $2 AND fencing_token = $3
           AND released_at IS NULL AND expires_at > now()",
    )
    .bind(claim.drive_lease_id)
    .bind(claim.worker_id)
    .bind(claim.drive_fencing_token)
    .execute(&mut *transaction)
    .await?;
    let media = sqlx::query(
        "UPDATE resource_leases SET expires_at = now() + interval '30 minutes'
         WHERE id = $1 AND owner_id = $2 AND fencing_token = $3
           AND released_at IS NULL AND expires_at > now()",
    )
    .bind(claim.media_lease_id)
    .bind(claim.worker_id)
    .bind(claim.media_fencing_token)
    .execute(&mut *transaction)
    .await?;
    if drive.rows_affected() != 1 || media.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    transaction.commit().await?;
    Ok(true)
}

pub async fn restore_rehearsal_cancellation_requested(
    pool: &PgPool,
    claim: &ClaimedRestoreRehearsalRun,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if !lock_valid_restore_rehearsal_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(true);
    }
    let requested = sqlx::query_scalar::<_, bool>(
        "SELECT cancel_requested_at IS NOT NULL
         FROM restore_rehearsals WHERE id = $1",
    )
    .bind(claim.run.id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(requested)
}

pub async fn transition_restore_rehearsal_phase(
    pool: &PgPool,
    claim: &ClaimedRestoreRehearsalRun,
    phase: RestoreRehearsalPhase,
    evidence: Value,
) -> Result<bool, sqlx::Error> {
    if !evidence.is_object() {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    if !lock_valid_restore_rehearsal_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let current =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(claim.run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    if !phase.allowed_previous().contains(&current.as_str()) {
        transaction.rollback().await?;
        return Ok(false);
    }
    sqlx::query(
        "UPDATE restore_rehearsals SET phase = $1
         WHERE id = $2 AND state = 'running'",
    )
    .bind(phase.as_str())
    .bind(claim.run.id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE job_runs SET state = $1 WHERE id = $2")
        .bind(phase.as_str())
        .bind(claim.run.job_run_id)
        .execute(&mut *transaction)
        .await?;
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some(&current),
        phase.as_str(),
        "read-only recovery rehearsal advanced",
        evidence.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_restore_rehearsal_event_in_transaction(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        phase.as_str(),
        evidence,
        "tapescale-worker",
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

pub async fn append_restore_rehearsal_event(
    pool: &PgPool,
    claim: &ClaimedRestoreRehearsalRun,
    event_type: &str,
    evidence: Value,
) -> Result<bool, sqlx::Error> {
    if !matches!(
        event_type,
        "media_observed" | "engine_dispatched" | "engine_completed"
    ) || !evidence.is_object()
    {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    if !lock_valid_restore_rehearsal_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    append_restore_rehearsal_event_in_transaction(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        event_type,
        evidence,
        "tapescale-worker",
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub async fn append_restore_rehearsal_artifact(
    pool: &PgPool,
    claim: &ClaimedRestoreRehearsalRun,
    artifact_kind: &str,
    storage_ref: &str,
    sha256: &str,
    size_bytes: u64,
    metadata: Value,
) -> Result<bool, sqlx::Error> {
    if !matches!(
        artifact_kind,
        "engine_restore" | "restored_manifest" | "comparison" | "evidence_report"
    ) || storage_ref.is_empty()
        || storage_ref.chars().any(char::is_control)
        || !valid_sha256(sha256)
        || i64::try_from(size_bytes).is_err()
        || !metadata.is_object()
    {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    if !lock_valid_restore_rehearsal_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let inserted = sqlx::query(
        "INSERT INTO restore_rehearsal_artifacts
            (restore_rehearsal_id, artifact_kind, storage_ref, sha256, size_bytes, metadata)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (restore_rehearsal_id, artifact_kind) DO NOTHING",
    )
    .bind(claim.run.id)
    .bind(artifact_kind)
    .bind(storage_ref)
    .bind(sha256)
    .bind(i64::try_from(size_bytes).expect("size was checked"))
    .bind(metadata)
    .execute(&mut *transaction)
    .await?;
    if inserted.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    transaction.commit().await?;
    Ok(true)
}

pub async fn request_restore_rehearsal_cancel(
    pool: &PgPool,
    id: Uuid,
    actor_label: &str,
) -> Result<CancelRestoreRehearsalResult, sqlx::Error> {
    let actor_label = actor_label.trim();
    if actor_label.is_empty()
        || actor_label.len() > 256
        || actor_label.chars().any(char::is_control)
    {
        return Ok(CancelRestoreRehearsalResult::Blocked(
            "a printable operator label of at most 256 bytes is required".to_owned(),
        ));
    }
    let mut transaction = pool.begin().await?;
    let run = sqlx::query_as::<_, RestoreRehearsalRunRecord>(&format!(
        "{RESTORE_REHEARSAL_RUN_SELECT} WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(run) = run else {
        transaction.rollback().await?;
        return Ok(CancelRestoreRehearsalResult::NotFound);
    };
    if !matches!(run.state.as_str(), "queued" | "running") {
        transaction.commit().await?;
        return Ok(CancelRestoreRehearsalResult::AlreadyTerminal(Box::new(run)));
    }
    if run.state == "queued" {
        let result = json!({
            "detail": "operator cancelled the recovery rehearsal before worker dispatch",
            "tape_command_dispatched": false,
            "position_uncertain": false,
            "restore_test_passed": false
        });
        sqlx::query(
            "UPDATE restore_rehearsals
             SET state = 'cancelled', phase = 'cancelled', result = $1,
                 cancel_requested_at = now(), cancel_requested_by_label = $2,
                 started_at = now(), finished_at = now()
             WHERE id = $3 AND state = 'queued'",
        )
        .bind(&result)
        .bind(actor_label)
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE job_runs
             SET state = 'cancelled', failure_class = 'operator_cancelled',
                 failure_detail = $1, started_at = now(), finished_at = now()
             WHERE id = $2 AND state = 'queued'",
        )
        .bind(&result)
        .bind(run.job_run_id)
        .execute(&mut *transaction)
        .await?;
        append_job_transition(
            &mut transaction,
            run.job_run_id,
            Some("queued"),
            "cancelled",
            "operator cancelled the rehearsal before tape dispatch",
            result.clone(),
            run.correlation_id,
        )
        .await?;
        append_restore_rehearsal_event_in_transaction(
            &mut transaction,
            run.id,
            run.correlation_id,
            "cancelled",
            result,
            actor_label,
        )
        .await?;
        release_restore_rehearsal_resources(&mut transaction, &run).await?;
        let run = sqlx::query_as(&format!("{RESTORE_REHEARSAL_RUN_SELECT} WHERE id = $1"))
            .bind(id)
            .fetch_one(&mut *transaction)
            .await?;
        transaction.commit().await?;
        return Ok(CancelRestoreRehearsalResult::Cancelled(Box::new(run)));
    }
    if run.cancel_requested_at.is_none() {
        sqlx::query(
            "UPDATE restore_rehearsals
             SET cancel_requested_at = now(), cancel_requested_by_label = $1
             WHERE id = $2 AND state = 'running' AND cancel_requested_at IS NULL",
        )
        .bind(actor_label)
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        append_restore_rehearsal_event_in_transaction(
            &mut transaction,
            run.id,
            run.correlation_id,
            "cancel_requested",
            json!({
                "detail": "operator requested cancellation; the worker must stop at a safe boundary",
                "phase": run.phase,
                "position_may_require_reconciliation": true
            }),
            actor_label,
        )
        .await?;
    }
    let run = sqlx::query_as(&format!("{RESTORE_REHEARSAL_RUN_SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(CancelRestoreRehearsalResult::Requested(Box::new(run)))
}

pub async fn request_cancel_restore_rehearsal(
    pool: &PgPool,
    id: Uuid,
    actor_label: &str,
) -> Result<CancelRestoreRehearsalResult, sqlx::Error> {
    request_restore_rehearsal_cancel(pool, id, actor_label).await
}

pub async fn finish_restore_rehearsal_run(
    pool: &PgPool,
    claim: &ClaimedRestoreRehearsalRun,
    state: RestoreRehearsalTerminalState,
    result: Value,
) -> Result<bool, sqlx::Error> {
    if !result.is_object() {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    if !lock_valid_restore_rehearsal_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let current =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(claim.run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    let updated = sqlx::query(
        "UPDATE restore_rehearsals
         SET state = $1, phase = $1, result = $2, finished_at = now()
         WHERE id = $3 AND state = 'running'",
    )
    .bind(state.as_str())
    .bind(&result)
    .bind(claim.run.id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(false);
    }
    sqlx::query(
        "UPDATE job_runs
         SET state = $1, failure_class = $2, failure_detail = $3,
             finished_at = now()
         WHERE id = $4 AND finished_at IS NULL",
    )
    .bind(state.as_str())
    .bind(format!("restore_rehearsal_{}", state.as_str()))
    .bind(&result)
    .bind(claim.run.job_run_id)
    .execute(&mut *transaction)
    .await?;
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some(&current),
        state.as_str(),
        "read-only recovery rehearsal ended without a passed restore test",
        result.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_restore_rehearsal_event_in_transaction(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        state.as_str(),
        result.clone(),
        "tapescale-worker",
    )
    .await?;
    let position_uncertain = result
        .get("position_uncertain")
        .and_then(Value::as_bool)
        .unwrap_or(matches!(
            state,
            RestoreRehearsalTerminalState::Failed | RestoreRehearsalTerminalState::Interrupted
        ));
    if position_uncertain {
        let detail = result
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("the interrupted tape read left drive position uncertain");
        mark_drive_motion_uncertain(&mut transaction, claim.run.drive_id, detail).await?;
    }
    release_restore_rehearsal_resources(&mut transaction, &claim.run).await?;
    transaction.commit().await?;
    Ok(true)
}

pub async fn finalize_restore_rehearsal_pass(
    pool: &PgPool,
    claim: &ClaimedRestoreRehearsalRun,
    result: Value,
) -> Result<FinalizeRestoreRehearsalResult, sqlx::Error> {
    let source_backup_job_id = result.get("source_backup_job_id").and_then(Value::as_u64);
    let reported_restore_job_id = result.get("restore_job_id").and_then(Value::as_u64);
    let restore_job_id = result
        .get("restore_job")
        .and_then(|job| job.get("job_id"))
        .and_then(Value::as_u64);
    let catalog_source_job_id = result
        .get("restore_catalog")
        .and_then(|catalog| catalog.get("source_backup_job_id"))
        .and_then(Value::as_u64);
    let catalog_restore_job_id = result
        .get("restore_catalog")
        .and_then(|catalog| catalog.get("job"))
        .and_then(|job| job.get("job_id"))
        .and_then(Value::as_u64);
    let restored_manifest_sha256 = result
        .get("restored_manifest_sha256")
        .and_then(Value::as_str)
        .filter(|value| valid_sha256(value));
    let source_manifest_digest = result
        .get("source_manifest_digest")
        .and_then(Value::as_str)
        .filter(|value| valid_sha256(value));
    let restored_manifest_digest = result
        .get("restored_manifest_digest")
        .and_then(Value::as_str)
        .filter(|value| valid_sha256(value));
    let report_comparison = result.get("comparison");
    let (
        Some(source_backup_job_id),
        Some(reported_restore_job_id),
        Some(restore_job_id),
        Some(catalog_source_job_id),
        Some(catalog_restore_job_id),
        Some(restored_manifest_sha256),
        Some(source_manifest_digest),
        Some(restored_manifest_digest),
        Some(report_comparison),
    ) = (
        source_backup_job_id,
        reported_restore_job_id,
        restore_job_id,
        catalog_source_job_id,
        catalog_restore_job_id,
        restored_manifest_sha256,
        source_manifest_digest,
        restored_manifest_digest,
        report_comparison,
    )
    else {
        return Ok(FinalizeRestoreRehearsalResult::Rejected(
            "pass report requires correlated source, restore, catalog, manifest, and comparison evidence"
                .to_owned(),
        ));
    };
    let Ok(restore_job_id_i64) = i64::try_from(restore_job_id) else {
        return Ok(FinalizeRestoreRehearsalResult::Rejected(
            "restore JobId exceeds the supported catalog range".to_owned(),
        ));
    };
    if restore_job_id_i64 <= 0 {
        return Ok(FinalizeRestoreRehearsalResult::Rejected(
            "restore JobId must be positive".to_owned(),
        ));
    }
    let expected_engine_operation_id = format!("bacula-job:{restore_job_id_i64}");
    let expected_destination = Path::new("/restore")
        .join(&claim.run.destination_subpath)
        .join(claim.run.id.to_string());
    if source_backup_job_id != u64::try_from(claim.run.backup_job_id).unwrap_or_default()
        || reported_restore_job_id != restore_job_id
        || catalog_source_job_id != source_backup_job_id
        || catalog_restore_job_id != restore_job_id
        || result.get("engine_operation_id").and_then(Value::as_str)
            != Some(expected_engine_operation_id.as_str())
        || result.get("production_eligible").and_then(Value::as_bool) != Some(false)
        || result.get("media_operation").and_then(Value::as_str) != Some("read_only")
        || result
            .get("media_mutation_commands_dispatched")
            .and_then(Value::as_bool)
            != Some(false)
        || result
            .get("restore_destination")
            .and_then(Value::as_str)
            .is_none_or(|destination| Path::new(destination) != expected_destination)
        || source_manifest_digest != claim.run.source_manifest_digest
        || restored_manifest_digest != claim.run.source_manifest_digest
        || report_comparison.get("matched").and_then(Value::as_bool) != Some(true)
        || report_comparison
            .get("expected_digest")
            .and_then(Value::as_str)
            != Some(claim.run.source_manifest_digest.as_str())
        || report_comparison
            .get("actual_digest")
            .and_then(Value::as_str)
            != Some(claim.run.source_manifest_digest.as_str())
    {
        return Ok(FinalizeRestoreRehearsalResult::Rejected(
            "pass report does not match the retained backup job and source manifest".to_owned(),
        ));
    }

    let mut transaction = pool.begin().await?;
    if !lock_valid_restore_rehearsal_claim(&mut transaction, claim).await? {
        transaction.rollback().await?;
        return Ok(FinalizeRestoreRehearsalResult::FencedOut);
    }
    let cancellation_requested = sqlx::query_scalar::<_, bool>(
        "SELECT cancel_requested_at IS NOT NULL
         FROM restore_rehearsals WHERE id = $1",
    )
    .bind(claim.run.id)
    .fetch_one(&mut *transaction)
    .await?;
    if cancellation_requested {
        transaction.rollback().await?;
        return Ok(FinalizeRestoreRehearsalResult::Rejected(
            "the operator requested cancellation before pass finalization".to_owned(),
        ));
    }
    let current =
        sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
            .bind(claim.run.job_run_id)
            .fetch_one(&mut *transaction)
            .await?;
    if current != "comparing" {
        transaction.rollback().await?;
        return Ok(FinalizeRestoreRehearsalResult::Rejected(
            "a rehearsal can pass only after physical restore and comparison".to_owned(),
        ));
    }
    let artifacts = sqlx::query_as::<_, RestoreRehearsalArtifactRecord>(
        "SELECT id, artifact_kind, storage_ref, sha256, size_bytes, metadata, created_at
         FROM restore_rehearsal_artifacts
         WHERE restore_rehearsal_id = $1 ORDER BY id FOR SHARE",
    )
    .bind(claim.run.id)
    .fetch_all(&mut *transaction)
    .await?;
    let engine = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "engine_restore");
    let restored = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "restored_manifest");
    let comparison = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "comparison");
    let report = artifacts
        .iter()
        .find(|artifact| artifact.artifact_kind == "evidence_report");
    let (Some(engine), Some(restored), Some(comparison), Some(report)) =
        (engine, restored, comparison, report)
    else {
        transaction.rollback().await?;
        return Ok(FinalizeRestoreRehearsalResult::Rejected(
            "engine restore, restored manifest, comparison, and evidence-report artifacts are required"
                .to_owned(),
        ));
    };
    let engine_valid = engine.metadata.get("completed").and_then(Value::as_bool) == Some(true)
        && engine
            .metadata
            .get("read_only_media_operation")
            .and_then(Value::as_bool)
            == Some(true)
        && engine.metadata.get("backup_job_id").and_then(Value::as_i64)
            == Some(claim.run.backup_job_id)
        && engine
            .metadata
            .get("engine_media_id")
            .and_then(Value::as_i64)
            == Some(claim.run.expected_engine_media_id)
        && engine
            .metadata
            .get("restore_job_id")
            .and_then(Value::as_i64)
            == Some(restore_job_id_i64)
        && engine.metadata.get("volume").and_then(Value::as_str)
            == Some(claim.run.expected_volume_label.as_str())
        && engine
            .metadata
            .get("destination")
            .and_then(Value::as_str)
            .is_some_and(|destination| Path::new(destination) == expected_destination);
    let restored_valid = restored
        .metadata
        .get("manifest_digest")
        .and_then(Value::as_str)
        == Some(claim.run.source_manifest_digest.as_str())
        && restored.sha256 == restored_manifest_sha256;
    let comparison_valid = comparison.metadata.get("matched").and_then(Value::as_bool)
        == Some(true)
        && comparison
            .metadata
            .get("expected_digest")
            .and_then(Value::as_str)
            == Some(claim.run.source_manifest_digest.as_str())
        && comparison
            .metadata
            .get("actual_digest")
            .and_then(Value::as_str)
            == Some(claim.run.source_manifest_digest.as_str());
    let report_valid = report.metadata.get("complete").and_then(Value::as_bool) == Some(true)
        && report.metadata.get("matched").and_then(Value::as_bool) == Some(true)
        && report
            .metadata
            .get("read_only_media_operation")
            .and_then(Value::as_bool)
            == Some(true)
        && report
            .metadata
            .get("source_manifest_digest")
            .and_then(Value::as_str)
            == Some(claim.run.source_manifest_digest.as_str())
        && report
            .metadata
            .get("restored_manifest_digest")
            .and_then(Value::as_str)
            == Some(claim.run.source_manifest_digest.as_str());
    if !engine_valid || !restored_valid || !comparison_valid || !report_valid {
        transaction.rollback().await?;
        return Ok(FinalizeRestoreRehearsalResult::Rejected(
            "pass evidence does not prove the exact physical restore and retained-manifest match"
                .to_owned(),
        ));
    }

    let verification_record_id = Uuid::now_v7();
    let started_at = claim
        .run
        .started_at
        .expect("a claimed rehearsal has a start time");
    let verification_evidence = json!({
        "restore_rehearsal_id": claim.run.id,
        "restore_plan_id": claim.run.restore_plan_id,
        "preview_hash": claim.run.preview_hash,
        "source_manifest_artifact_id": claim.run.source_manifest_artifact_id,
        "source_manifest_sha256": claim.run.source_manifest_sha256,
        "source_manifest_digest": claim.run.source_manifest_digest,
        "restored_manifest_artifact_id": restored.id,
        "restored_manifest_sha256": restored.sha256,
        "comparison_artifact_id": comparison.id,
        "engine_restore_artifact_id": engine.id,
        "bacula_backup_job_id": claim.run.backup_job_id,
        "bacula_restore_job_id": restore_job_id_i64,
        "bacula_media_id": claim.run.expected_engine_media_id,
        "engine_operation_id": expected_engine_operation_id,
        "physical_media_readback": true,
        "matched": true,
        "production_eligible": false,
        "result": result
    });
    sqlx::query(
        "INSERT INTO verification_records
            (id, recovery_point_id, kind, scope, outcome, drive_id, media_ids,
             evidence, correlation_id, started_at, finished_at)
         VALUES ($1, $2, 'restore_test', $3, 'passed', $4, ARRAY[$5]::uuid[],
                 $6, $7, $8, now())",
    )
    .bind(verification_record_id)
    .bind(claim.run.recovery_point_id)
    .bind(json!({
        "scope": "complete attended single-volume file recovery replay",
        "source_manifest_digest": claim.run.source_manifest_digest,
        "destination_subpath": claim.run.destination_subpath
    }))
    .bind(claim.run.drive_id)
    .bind(claim.run.cartridge_id)
    .bind(&verification_evidence)
    .bind(claim.run.correlation_id)
    .bind(started_at)
    .execute(&mut *transaction)
    .await?;
    let pass_result = json!({
        "detail": "physical tape was restored again into isolation and matched the retained source manifest",
        "verification_record_id": verification_record_id,
        "restore_test_passed": true,
        "engine_operation_id": expected_engine_operation_id,
        "restore_job_id": restore_job_id_i64,
        "source_manifest_digest": claim.run.source_manifest_digest,
        "production_eligible": false,
        "execution_scope": "attended_single_volume_file_restore",
        "evidence": result
    });
    let updated = sqlx::query(
        "UPDATE restore_rehearsals
         SET state = 'passed', phase = 'passed', result = $1, finished_at = now()
         WHERE id = $2 AND state = 'running' AND phase = 'comparing'",
    )
    .bind(&pass_result)
    .bind(claim.run.id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        transaction.rollback().await?;
        return Ok(FinalizeRestoreRehearsalResult::FencedOut);
    }
    sqlx::query(
        "UPDATE job_runs
         SET state = 'completed', engine_operation_id = $1, finished_at = now()
         WHERE id = $2 AND state = 'comparing'",
    )
    .bind(&expected_engine_operation_id)
    .bind(claim.run.job_run_id)
    .execute(&mut *transaction)
    .await?;
    append_job_transition(
        &mut transaction,
        claim.run.job_run_id,
        Some("comparing"),
        "completed",
        "physical recovery replay and retained-manifest comparison passed",
        pass_result.clone(),
        claim.run.correlation_id,
    )
    .await?;
    append_restore_rehearsal_event_in_transaction(
        &mut transaction,
        claim.run.id,
        claim.run.correlation_id,
        "passed",
        pass_result,
        "tapescale-worker",
    )
    .await?;
    release_restore_rehearsal_resources(&mut transaction, &claim.run).await?;
    transaction.commit().await?;
    Ok(FinalizeRestoreRehearsalResult::Passed {
        verification_record_id,
    })
}

pub async fn block_queued_restore_rehearsal_runs(
    pool: &PgPool,
    reason: &str,
) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let queued = sqlx::query_as::<_, RestoreRehearsalRunRecord>(&format!(
        "{RESTORE_REHEARSAL_RUN_SELECT} WHERE state = 'queued' FOR UPDATE"
    ))
    .fetch_all(&mut *transaction)
    .await?;
    for run in &queued {
        block_restore_rehearsal_in_transaction(&mut transaction, run, reason).await?;
    }
    transaction.commit().await?;
    Ok(queued.len() as u64)
}

pub async fn recover_expired_restore_rehearsal_runs(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let expired = sqlx::query_as::<_, RestoreRehearsalRunRecord>(&format!(
        "{RESTORE_REHEARSAL_RUN_SELECT}
         WHERE id IN (
             SELECT rehearsal.id
             FROM restore_rehearsals rehearsal
             LEFT JOIN resource_leases drive_lease
               ON drive_lease.id = rehearsal.drive_lease_id
             LEFT JOIN resource_leases media_lease
               ON media_lease.id = rehearsal.media_lease_id
             WHERE rehearsal.state = 'running'
               AND (drive_lease.id IS NULL OR drive_lease.released_at IS NOT NULL
                    OR drive_lease.expires_at <= now()
                    OR media_lease.id IS NULL OR media_lease.released_at IS NOT NULL
                    OR media_lease.expires_at <= now())
         ) FOR UPDATE"
    ))
    .fetch_all(&mut *transaction)
    .await?;
    for run in &expired {
        let detail = "a drive or cartridge lease expired during the physical tape read; the rehearsal is interrupted and drive position requires reconciliation";
        let result = json!({
            "detail": detail,
            "position_uncertain": true,
            "restore_test_passed": false,
            "reconciliation_required": true
        });
        let current =
            sqlx::query_scalar::<_, String>("SELECT state FROM job_runs WHERE id = $1 FOR UPDATE")
                .bind(run.job_run_id)
                .fetch_one(&mut *transaction)
                .await?;
        sqlx::query(
            "UPDATE restore_rehearsals
             SET state = 'interrupted', phase = 'interrupted', result = $1,
                 finished_at = now()
             WHERE id = $2 AND state = 'running'",
        )
        .bind(&result)
        .bind(run.id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE job_runs
             SET state = 'interrupted', failure_class = 'restore_rehearsal_lease_expired',
                 failure_detail = $1, finished_at = now()
             WHERE id = $2 AND finished_at IS NULL",
        )
        .bind(&result)
        .bind(run.job_run_id)
        .execute(&mut *transaction)
        .await?;
        append_job_transition(
            &mut transaction,
            run.job_run_id,
            Some(&current),
            "interrupted",
            detail,
            result.clone(),
            run.correlation_id,
        )
        .await?;
        append_restore_rehearsal_event_in_transaction(
            &mut transaction,
            run.id,
            run.correlation_id,
            "interrupted",
            result,
            "tapescale-worker",
        )
        .await?;
        release_restore_rehearsal_resources(&mut transaction, run).await?;
        mark_drive_motion_uncertain(&mut transaction, run.drive_id, detail).await?;
    }
    transaction.commit().await?;
    Ok(expired.len() as u64)
}

pub async fn quarantine_fenced_restore_rehearsal_run(
    pool: &PgPool,
    claim: &ClaimedRestoreRehearsalRun,
    detail: &str,
) -> Result<bool, sqlx::Error> {
    if detail.is_empty() || detail.len() > 4096 || detail.chars().any(char::is_control) {
        return Ok(false);
    }
    let mut transaction = pool.begin().await?;
    let record = sqlx::query_as::<_, (String, Uuid, Uuid, Uuid)>(
        "SELECT state, drive_id, cartridge_id, correlation_id
         FROM restore_rehearsals WHERE id = $1 FOR UPDATE",
    )
    .bind(claim.run.id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((state, drive_id, cartridge_id, correlation_id)) = record else {
        transaction.rollback().await?;
        return Ok(false);
    };
    if state == "passed"
        || drive_id != claim.run.drive_id
        || cartridge_id != claim.run.cartridge_id
        || correlation_id != claim.run.correlation_id
    {
        transaction.rollback().await?;
        return Ok(false);
    }
    mark_drive_motion_uncertain(&mut transaction, drive_id, detail).await?;
    let event_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM restore_rehearsal_events
             WHERE restore_rehearsal_id = $1 AND event_type = 'fenced_out')",
    )
    .bind(claim.run.id)
    .fetch_one(&mut *transaction)
    .await?;
    if !event_exists {
        append_restore_rehearsal_event_in_transaction(
            &mut transaction,
            claim.run.id,
            correlation_id,
            "fenced_out",
            json!({
                "detail": detail,
                "position_uncertain": true,
                "reconciliation_required": true,
                "read_only_media_operation": true
            }),
            "tapescale-worker",
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(true)
}

fn truncate_rows<T>(rows: &mut Vec<T>, limit: usize) -> bool {
    let truncated = rows.len() > limit;
    rows.truncate(limit);
    truncated
}

fn recovery_volume_with_drives(
    volume: RecoveryVolumeRow,
    drives: &[RecoveryDriveRow],
) -> RecoveryVolumeRecord {
    let compatible_qualified_drives = drives
        .iter()
        .filter_map(|drive| compatible_recovery_drive(drive, &volume))
        .collect();
    let available = volume.finalized
        && volume.identity_status == "reconciled"
        && matches!(
            volume.health_status.as_str(),
            "written_verified" | "healthy" | "verified"
        )
        && volume.reserved_by.is_none();
    RecoveryVolumeRecord {
        id: volume.id,
        recovery_point_id: volume.recovery_point_id,
        volume_order: volume.volume_order,
        first_record: volume.first_record,
        last_record: volume.last_record,
        encoded_bytes: volume.encoded_bytes,
        finalized: volume.finalized,
        engine_record_id: volume.engine_record_id,
        cartridge_id: volume.cartridge_id,
        internal_label: volume.internal_label,
        barcode: volume.barcode,
        media_generation: volume.media_generation,
        media_format: volume.media_format,
        on_tape_identity: volume.on_tape_identity,
        identity_status: volume.identity_status,
        health_status: volume.health_status,
        worm: volume.worm,
        reserved_by: volume.reserved_by,
        location_id: volume.location_id,
        location_name: volume.location_name,
        location_kind: volume.location_kind,
        available,
        compatible_qualified_drives,
    }
}

fn compatible_recovery_drive(
    drive: &RecoveryDriveRow,
    volume: &RecoveryVolumeRow,
) -> Option<RecoveryDrivePrerequisite> {
    let matrix_compatible = drive
        .generation
        .and_then(drive_generation)
        .zip(media_format(
            volume.media_generation,
            volume.media_format.as_deref(),
        ))
        .is_some_and(|(drive, media)| can_read(drive, media));
    let physically_qualified_for_media =
        drive.qualified_media.as_array().is_some_and(|qualified| {
            qualified.iter().any(|entry| {
                let cartridge_id = entry
                    .get("cartridge_id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok());
                let generation = entry.get("generation").and_then(Value::as_i64);
                let format = entry.get("format").and_then(Value::as_str);
                cartridge_id == Some(volume.cartridge_id)
                    && generation == volume.media_generation.map(i64::from)
                    && formats_equivalent(format, volume.media_format.as_deref())
            })
        });
    if !physically_qualified_for_media {
        return None;
    }
    let mut compatibility_basis = Vec::new();
    if matrix_compatible {
        compatibility_basis.push("explicit_lto_read_matrix".to_owned());
    }
    if physically_qualified_for_media {
        compatibility_basis.push("exact_g0_1_pair".to_owned());
    }
    Some(RecoveryDrivePrerequisite {
        id: drive.id,
        stable_id: drive.stable_id.clone(),
        generation: drive.generation,
        vendor: drive.vendor.clone(),
        model: drive.model.clone(),
        serial: drive.serial.clone(),
        wwn: drive.wwn.clone(),
        firmware: drive.firmware.clone(),
        observed_path: drive.observed_path.clone(),
        last_observed_at: drive.last_observed_at,
        compatibility_basis,
    })
}

fn formats_equivalent(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => normalize_media_format(left) == normalize_media_format(right),
        (None, None) => true,
        _ => false,
    }
}

fn normalize_media_format(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
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

fn media_format(generation: Option<i16>, format: Option<&str>) -> Option<MediaFormat> {
    if format.is_some_and(|value| {
        matches!(
            normalize_media_format(value).as_str(),
            "LTO7TYPEM" | "LTO7M" | "M8"
        )
    }) {
        return Some(MediaFormat::Lto7TypeM);
    }
    match generation {
        Some(1) => Some(MediaFormat::Lto1),
        Some(2) => Some(MediaFormat::Lto2),
        Some(3) => Some(MediaFormat::Lto3),
        Some(4) => Some(MediaFormat::Lto4),
        Some(5) => Some(MediaFormat::Lto5),
        Some(6) => Some(MediaFormat::Lto6),
        Some(7) => Some(MediaFormat::Lto7),
        Some(8) => Some(MediaFormat::Lto8),
        Some(9) => Some(MediaFormat::Lto9),
        Some(10) => Some(MediaFormat::Lto10),
        _ => None,
    }
}

fn bound_value_in_place(value: &mut Value, truncated: &mut bool) {
    let original = std::mem::take(value);
    let original_bytes = serde_json::to_vec(&original).expect("a JSON value always serializes");
    let original_sha256 = format!("{:x}", Sha256::digest(&original_bytes));
    let bounded = bounded_json_value(original, 0, truncated);
    let bounded_size = serde_json::to_vec(&bounded)
        .expect("a JSON value always serializes")
        .len();
    if bounded_size > RECOVERY_JSON_VALUE_LIMIT_BYTES {
        *truncated = true;
        *value = json!({
            "_tapescale_truncated": "value exceeded the evidence export bound",
            "original_size_bytes": original_bytes.len(),
            "original_sha256": original_sha256
        });
    } else {
        *value = bounded;
    }
}

fn bounded_json_value(value: Value, depth: usize, truncated: &mut bool) -> Value {
    const MAX_DEPTH: usize = 8;
    const MAX_MEMBERS: usize = 64;
    const MAX_STRING_CHARS: usize = 4 * 1024;

    if depth >= MAX_DEPTH {
        *truncated = true;
        return json!({"_tapescale_truncated": "maximum JSON depth reached"});
    }
    match value {
        Value::String(value) if value.chars().count() > MAX_STRING_CHARS => {
            *truncated = true;
            let mut bounded = value.chars().take(MAX_STRING_CHARS).collect::<String>();
            bounded.push_str("…[truncated]");
            Value::String(bounded)
        }
        Value::Array(values) => {
            let original_len = values.len();
            let mut bounded = values
                .into_iter()
                .take(MAX_MEMBERS)
                .map(|value| bounded_json_value(value, depth + 1, truncated))
                .collect::<Vec<_>>();
            if original_len > MAX_MEMBERS {
                *truncated = true;
                bounded.push(json!({
                    "_tapescale_truncated_items": original_len - MAX_MEMBERS
                }));
            }
            Value::Array(bounded)
        }
        Value::Object(values) => {
            let original_len = values.len();
            let mut bounded = serde_json::Map::new();
            for (key, value) in values.into_iter().take(MAX_MEMBERS) {
                bounded.insert(key, bounded_json_value(value, depth + 1, truncated));
            }
            if original_len > MAX_MEMBERS {
                *truncated = true;
                bounded.insert(
                    "_tapescale_truncated_members".to_owned(),
                    Value::from(original_len - MAX_MEMBERS),
                );
            }
            Value::Object(bounded)
        }
        value => value,
    }
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

#[cfg(test)]
mod recovery_vault_tests {
    use super::*;

    fn run_volume(run_id: Uuid, order: i32, name: &str) -> DataRunVolumeRecord {
        let now = Utc::now();
        DataRunVolumeRecord {
            id: Uuid::now_v7(),
            single_tape_data_run_id: run_id,
            volume_order: order,
            cartridge_id: Uuid::now_v7(),
            expected_cartridge_label: format!("TAPE-{order}"),
            expected_media_generation: Some(9),
            expected_media_format: Some("LTO-9".to_owned()),
            engine_volume_name: Some(name.to_owned()),
            engine_media_id: None,
            state: "written".to_owned(),
            media_lease_id: Some(Uuid::now_v7()),
            media_fencing_token: Some(i64::from(order)),
            media_write_started_at: Some(now),
            written_at: Some(now),
            verified_at: None,
            encoded_bytes: None,
            first_record: None,
            last_record: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn volume(cartridge_id: Uuid) -> RecoveryVolumeRow {
        RecoveryVolumeRow {
            id: Uuid::now_v7(),
            recovery_point_id: Uuid::now_v7(),
            volume_order: 1,
            first_record: "file=0:block=0".to_owned(),
            last_record: Some("file=1:block=1".to_owned()),
            encoded_bytes: Some(1024),
            finalized: true,
            engine_record_id: "media-1".to_owned(),
            cartridge_id,
            internal_label: "TS-L9-001".to_owned(),
            barcode: None,
            media_generation: Some(9),
            media_format: Some("LTO-9".to_owned()),
            on_tape_identity: Some("TS-L9-001".to_owned()),
            identity_status: "reconciled".to_owned(),
            health_status: "written_verified".to_owned(),
            worm: false,
            reserved_by: None,
            location_id: None,
            location_name: None,
            location_kind: None,
        }
    }

    fn drive(qualified_media: Value) -> RecoveryDriveRow {
        RecoveryDriveRow {
            id: Uuid::now_v7(),
            stable_id: "scsi:naa.5001".to_owned(),
            generation: Some(9),
            vendor: Some("IBM".to_owned()),
            model: Some("ULTRIUM-HH9".to_owned()),
            serial: Some("DRIVE-001".to_owned()),
            wwn: Some("naa.5001".to_owned()),
            firmware: Some("A1".to_owned()),
            observed_path: Some("/dev/nst0".to_owned()),
            last_observed_at: Some(Utc::now()),
            qualified_media,
        }
    }

    #[test]
    fn missing_readback_setup_evidence_never_offers_destructive_preparation() {
        let blocker = missing_readback_qualification_blocker(Uuid::now_v7(), Uuid::now_v7());
        assert_eq!(blocker.code, "readback_setup_evidence_missing");
        assert_eq!(blocker.action, "open_backup");
        assert_ne!(blocker.action, "prepare_tape");
        assert!(blocker.message.contains("do not run a destructive"));
    }

    #[test]
    fn readback_rejects_known_identity_conflicts_but_allows_post_write_unknown() {
        assert!(readback_identity_status_blocker("foreign").is_some());
        assert!(readback_identity_status_blocker("mismatch").is_some());
        assert!(readback_identity_status_blocker("unknown").is_none());
        assert!(readback_identity_status_blocker("reconciled").is_none());
    }

    #[test]
    fn lto_matrix_never_substitutes_for_exact_physical_pair() {
        let volume = volume(Uuid::now_v7());
        assert!(compatible_recovery_drive(&drive(json!([])), &volume).is_none());
    }

    #[test]
    fn exact_physical_pair_is_exposed_with_an_explicit_basis() {
        let cartridge_id = Uuid::now_v7();
        let volume = volume(cartridge_id);
        let compatible = compatible_recovery_drive(
            &drive(json!([{
                "cartridge_id": cartridge_id,
                "generation": 9,
                "format": "LTO-9"
            }])),
            &volume,
        )
        .expect("the exact passed G0.1 pair should be eligible");
        assert!(compatible
            .compatibility_basis
            .contains(&"exact_g0_1_pair".to_owned()));
    }

    #[test]
    fn evidence_json_values_are_bounded_and_marked() {
        let mut truncated = false;
        let bounded = bounded_json_value(Value::String("x".repeat(20_000)), 0, &mut truncated);
        assert!(truncated);
        assert!(bounded
            .as_str()
            .is_some_and(|value| value.ends_with("…[truncated]")));
    }

    #[test]
    fn ordered_bacula_evidence_maps_every_authorized_archive_volume() {
        let run_id = Uuid::now_v7();
        let run_volumes = vec![
            run_volume(run_id, 1, "ARCHIVE-V0001"),
            run_volume(run_id, 2, "ARCHIVE-V0002"),
        ];
        let metadata = json!({
            "volumes": [
                {"media_id": 11, "volume_name": "ARCHIVE-V0001", "volume_bytes": 100},
                {"media_id": 12, "volume_name": "ARCHIVE-V0002", "volume_bytes": 200}
            ],
            "segments": [
                {"volume_name":"ARCHIVE-V0001", "start_file":0, "start_block":0,
                 "end_file":1, "end_block":10},
                {"volume_name":"ARCHIVE-V0002", "start_file":0, "start_block":0,
                 "end_file":2, "end_block":20}
            ]
        });
        let evidence = SingleTapePassEvidence {
            engine_operation_id: "bacula-job:44".to_owned(),
            engine_volume_name: "ARCHIVE-V0001".to_owned(),
            engine_record_id: "bacula-media:11:job:44".to_owned(),
            first_record: "file=0:block=0".to_owned(),
            last_record: Some("file=2:block=20".to_owned()),
            logical_bytes: 250,
            encoded_bytes: 300,
            file_count: 2,
            coverage: json!({"complete":true}),
            full_readback_evidence: json!({"physical_media_readback":true}),
            restore_test_evidence: json!({"outcome":"not_run"}),
            result: json!({}),
        };

        let finalized = validated_backup_volume_evidence(&run_volumes, &metadata, &evidence)
            .expect("ordered multi-volume catalog proof should validate");
        assert_eq!(finalized.len(), 2);
        assert_eq!(finalized[1].engine_media_id, 12);
        assert_eq!(finalized[1].engine_record_id, "bacula-media:12:job:44");
        assert_eq!(finalized[1].last_record.as_deref(), Some("file=2:block=20"));
    }

    #[test]
    fn ordered_bacula_evidence_rejects_a_swapped_cartridge_chain() {
        let run_id = Uuid::now_v7();
        let run_volumes = vec![
            run_volume(run_id, 1, "ARCHIVE-V0001"),
            run_volume(run_id, 2, "ARCHIVE-V0002"),
        ];
        let metadata = json!({
            "volumes": [
                {"media_id": 12, "volume_name": "ARCHIVE-V0002", "volume_bytes": 200},
                {"media_id": 11, "volume_name": "ARCHIVE-V0001", "volume_bytes": 100}
            ],
            "segments": [
                {"volume_name":"ARCHIVE-V0002", "start_file":0, "start_block":0,
                 "end_file":1, "end_block":1},
                {"volume_name":"ARCHIVE-V0001", "start_file":0, "start_block":0,
                 "end_file":1, "end_block":1}
            ]
        });
        let evidence = SingleTapePassEvidence {
            engine_operation_id: "bacula-job:44".to_owned(),
            engine_volume_name: "ARCHIVE-V0001".to_owned(),
            engine_record_id: "bacula-media:11:job:44".to_owned(),
            first_record: "file=0:block=0".to_owned(),
            last_record: Some("file=1:block=1".to_owned()),
            logical_bytes: 250,
            encoded_bytes: 300,
            file_count: 2,
            coverage: json!({"complete":true}),
            full_readback_evidence: json!({"physical_media_readback":true}),
            restore_test_evidence: json!({"outcome":"not_run"}),
            result: json!({}),
        };

        assert!(validated_backup_volume_evidence(&run_volumes, &metadata, &evidence).is_err());
    }

    #[test]
    fn completed_catalog_proof_promotes_the_initial_and_continuation_volumes() {
        let run_id = Uuid::now_v7();
        let mut first = run_volume(run_id, 1, "ARCHIVE-V0001");
        first.state = "write_started".to_owned();
        first.written_at = None;
        let volumes = vec![first, run_volume(run_id, 2, "ARCHIVE-V0002")];
        let catalog = vec!["ARCHIVE-V0001".to_owned(), "ARCHIVE-V0002".to_owned()];

        validate_catalog_written_volume_chain(&volumes, &catalog)
            .expect("strict catalog order should prove both written tapes");
    }

    #[test]
    fn completed_catalog_proof_rejects_missing_or_swapped_tapes() {
        let run_id = Uuid::now_v7();
        let volumes = vec![
            run_volume(run_id, 1, "ARCHIVE-V0001"),
            run_volume(run_id, 2, "ARCHIVE-V0002"),
        ];
        assert!(validate_catalog_written_volume_chain(
            &volumes,
            &["ARCHIVE-V0002".to_owned(), "ARCHIVE-V0001".to_owned()]
        )
        .is_err());
        assert!(
            validate_catalog_written_volume_chain(&volumes, &["ARCHIVE-V0001".to_owned()]).is_err()
        );
    }
}
