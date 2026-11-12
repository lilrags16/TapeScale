//! Durable off-site media custody operations.
//!
//! This module deliberately treats a movement plan, a device observation, and
//! physical custody as different facts.  Only an explicit physical confirmation
//! (or an append-only correction) updates `cartridges.confirmed_location_id`.
//! Every mutation is transactional and writes custody history, an audit record,
//! and a durable in-app outbox event before it commits.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

const MAX_JSON_BYTES: usize = 1_048_576;
const MAX_LIST_LIMIT: u32 = 500;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocationKind {
    Onsite,
    Offsite,
    InTransit,
    Unknown,
    Device,
}

impl LocationKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Onsite => "onsite",
            Self::Offsite => "offsite",
            Self::InTransit => "in_transit",
            Self::Unknown => "unknown",
            Self::Device => "device",
        }
    }

    fn parse(value: &str) -> Result<Self, CustodyOpsError> {
        match value {
            "onsite" => Ok(Self::Onsite),
            "offsite" => Ok(Self::Offsite),
            "in_transit" => Ok(Self::InTransit),
            "unknown" => Ok(Self::Unknown),
            "device" => Ok(Self::Device),
            other => Err(CustodyOpsError::Invariant(format!(
                "database contains unknown location kind {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MovementKind {
    TakeOffsite,
    Return,
    Swap,
    Recall,
}

impl MovementKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::TakeOffsite => "take_offsite",
            Self::Return => "return",
            Self::Swap => "swap",
            Self::Recall => "recall",
        }
    }

    fn parse(value: &str) -> Result<Self, CustodyOpsError> {
        match value {
            "take_offsite" => Ok(Self::TakeOffsite),
            "return" => Ok(Self::Return),
            "swap" => Ok(Self::Swap),
            "recall" => Ok(Self::Recall),
            other => Err(CustodyOpsError::Invariant(format!(
                "database contains unknown movement kind {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MovementState {
    Planned,
    Ready,
    Departed,
    PartiallyConfirmed,
    Confirmed,
    Overdue,
    Blocked,
    Cancelled,
}

impl MovementState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Ready => "ready",
            Self::Departed => "departed",
            Self::PartiallyConfirmed => "partially_confirmed",
            Self::Confirmed => "confirmed",
            Self::Overdue => "overdue",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self, CustodyOpsError> {
        match value {
            "planned" => Ok(Self::Planned),
            "ready" => Ok(Self::Ready),
            "departed" => Ok(Self::Departed),
            "partially_confirmed" => Ok(Self::PartiallyConfirmed),
            "confirmed" => Ok(Self::Confirmed),
            "overdue" => Ok(Self::Overdue),
            "blocked" => Ok(Self::Blocked),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(CustodyOpsError::Invariant(format!(
                "database contains unknown movement state {other:?}"
            ))),
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Confirmed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MovementMediaStage {
    Planned,
    Departed,
    Arrived,
    DirectConfirmed,
    Cancelled,
    Exception,
}

impl MovementMediaStage {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Departed => "departed",
            Self::Arrived => "arrived",
            Self::DirectConfirmed => "direct_confirmed",
            Self::Cancelled => "cancelled",
            Self::Exception => "exception",
        }
    }

    fn parse(value: &str) -> Result<Self, CustodyOpsError> {
        match value {
            "planned" => Ok(Self::Planned),
            "departed" => Ok(Self::Departed),
            "arrived" => Ok(Self::Arrived),
            "direct_confirmed" => Ok(Self::DirectConfirmed),
            "cancelled" => Ok(Self::Cancelled),
            "exception" => Ok(Self::Exception),
            other => Err(CustodyOpsError::Invariant(format!(
                "database contains unknown movement media stage {other:?}"
            ))),
        }
    }

    fn is_confirmed_destination(&self) -> bool {
        matches!(self, Self::Arrived | Self::DirectConfirmed)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CustodyActor {
    pub user_id: Option<Uuid>,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CreateLocationRequest {
    pub name: String,
    pub kind: LocationKind,
    #[serde(default = "empty_object")]
    pub details: Value,
    pub actor: CustodyActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct LocationRecord {
    pub id: Uuid,
    pub name: String,
    pub kind: LocationKind,
    pub details: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CreateMovementRequest {
    pub external_id: String,
    pub idempotency_key: String,
    pub kind: MovementKind,
    /// `None` is the explicit Unknown bootstrap source.  It is accepted only
    /// when every selected cartridge is currently unlocated, and only a direct
    /// physical confirmation may advance it.
    pub source_location_id: Option<Uuid>,
    pub destination_location_id: Uuid,
    /// Required for the two-step departure/arrival workflow.  Direct physical
    /// confirmation does not need a transit location.
    pub transit_location_id: Option<Uuid>,
    pub cartridge_ids: Vec<Uuid>,
    #[serde(default = "empty_object")]
    pub readiness: Value,
    pub assigned_user_id: Option<Uuid>,
    pub due_at: DateTime<Utc>,
    pub actor: CustodyActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MovementListQuery {
    #[serde(default)]
    pub states: Vec<MovementState>,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
}

impl Default for MovementListQuery {
    fn default() -> Self {
        Self {
            states: Vec::new(),
            limit: default_list_limit(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MovementSummary {
    pub id: Uuid,
    pub external_id: String,
    pub kind: MovementKind,
    pub state: MovementState,
    pub confirmation_version: i64,
    pub source_location_id: Option<Uuid>,
    pub source_name: Option<String>,
    pub destination_location_id: Uuid,
    pub destination_name: String,
    pub transit_location_id: Option<Uuid>,
    pub transit_name: Option<String>,
    pub due_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub media_total: i64,
    pub media_planned: i64,
    pub media_departed: i64,
    pub media_confirmed: i64,
    pub media_cancelled: i64,
    pub media_exceptions: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MovementMediaRecord {
    pub cartridge_id: Uuid,
    pub internal_label: String,
    pub barcode: Option<String>,
    pub stage: MovementMediaStage,
    pub expected_source_location_id: Option<Uuid>,
    pub expected_source_name: Option<String>,
    pub destination_location_id: Uuid,
    pub destination_name: String,
    pub confirmed_location_id: Option<Uuid>,
    pub confirmed_location_name: Option<String>,
    pub reservation_held: bool,
    pub last_confirmation_version: i64,
    pub departed_at: Option<DateTime<Utc>>,
    pub arrived_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub latest_custody_event_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CustodyEventRecord {
    pub id: Uuid,
    pub cartridge_id: Option<Uuid>,
    pub internal_label: Option<String>,
    pub event_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_label: String,
    pub source_location_id: Option<Uuid>,
    pub source_name: Option<String>,
    pub destination_location_id: Option<Uuid>,
    pub destination_name: Option<String>,
    pub note: String,
    pub corrects_event_id: Option<Uuid>,
    pub evidence: Value,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MovementDetail {
    pub summary: MovementSummary,
    pub readiness: Value,
    pub assigned_user_id: Option<Uuid>,
    pub media: Vec<MovementMediaRecord>,
    pub custody_events: Vec<CustodyEventRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CreateMovementResult {
    pub movement_task_id: Uuid,
    pub state: MovementState,
    pub confirmation_version: i64,
    pub reserved_cartridge_ids: Vec<Uuid>,
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PhysicalConfirmationRequest {
    pub cartridge_ids: Vec<Uuid>,
    pub idempotency_key: String,
    pub expected_confirmation_version: i64,
    pub physically_confirmed: bool,
    #[serde(default)]
    pub note: String,
    pub actor: CustodyActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CancelMovementRequest {
    pub idempotency_key: String,
    pub expected_confirmation_version: i64,
    pub reason: String,
    pub actor: CustodyActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppendCorrectionRequest {
    pub corrects_event_id: Uuid,
    pub cartridge_id: Uuid,
    pub expected_current_location_id: Option<Uuid>,
    pub confirmed_location_id: Uuid,
    pub idempotency_key: String,
    pub expected_confirmation_version: i64,
    pub physically_confirmed: bool,
    pub note: String,
    pub actor: CustodyActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MovementMutationResult {
    pub movement_task_id: Uuid,
    pub operation_id: Uuid,
    pub state: MovementState,
    pub confirmation_version: i64,
    pub affected_cartridge_ids: Vec<Uuid>,
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReconcileOverdueRequest {
    pub as_of: DateTime<Utc>,
    pub actor: CustodyActor,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct OverdueReconciliationResult {
    pub as_of: DateTime<Utc>,
    pub newly_overdue_task_ids: Vec<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum CustodyOpsError {
    #[error("{resource} {id} was not found")]
    NotFound { resource: &'static str, id: String },
    #[error("invalid {field}: {message}")]
    Invalid {
        field: &'static str,
        message: String,
    },
    #[error("{code}: {message}")]
    Conflict { code: &'static str, message: String },
    #[error("movement confirmation version is stale: expected {expected}, current {actual}")]
    StaleVersion { expected: i64, actual: i64 },
    #[error(
        "cartridge {cartridge_id} is at {actual_location_id:?}, not expected source {expected_location_id:?}"
    )]
    WrongSource {
        cartridge_id: Uuid,
        expected_location_id: Option<Uuid>,
        actual_location_id: Option<Uuid>,
    },
    #[error("cartridge {cartridge_id} is reserved by {reserved_by}")]
    ReservationConflict {
        cartridge_id: Uuid,
        reserved_by: String,
    },
    #[error("idempotency key {key:?} was already used for a different request")]
    IdempotencyConflict { key: String },
    #[error("custody database invariant failed: {0}")]
    Invariant(String),
    #[error("custody database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, FromRow)]
struct LocationRow {
    id: Uuid,
    name: String,
    kind: String,
    details: Value,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct MovementSummaryRow {
    id: Uuid,
    external_id: String,
    kind: String,
    state: String,
    confirmation_version: i64,
    source_location_id: Option<Uuid>,
    source_name: Option<String>,
    destination_location_id: Uuid,
    destination_name: String,
    transit_location_id: Option<Uuid>,
    transit_name: Option<String>,
    due_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    media_total: i64,
    media_planned: i64,
    media_departed: i64,
    media_confirmed: i64,
    media_cancelled: i64,
    media_exceptions: i64,
}

#[derive(Debug, FromRow)]
struct MovementHeaderRow {
    external_id: String,
    kind: String,
    source_location_id: Option<Uuid>,
    destination_location_id: Uuid,
    transit_location_id: Option<Uuid>,
    state: String,
    confirmation_version: i64,
}

#[derive(Debug, FromRow)]
struct MovementMediaRow {
    cartridge_id: Uuid,
    internal_label: String,
    barcode: Option<String>,
    stage: String,
    expected_source_location_id: Option<Uuid>,
    expected_source_name: Option<String>,
    destination_location_id: Uuid,
    destination_name: String,
    confirmed_location_id: Option<Uuid>,
    confirmed_location_name: Option<String>,
    reservation_owner: String,
    reserved_by: Option<String>,
    last_confirmation_version: i64,
    departed_at: Option<DateTime<Utc>>,
    arrived_at: Option<DateTime<Utc>>,
    cancelled_at: Option<DateTime<Utc>>,
    latest_custody_event_id: Option<Uuid>,
}

#[derive(Debug, FromRow)]
struct LockedMediaRow {
    cartridge_id: Uuid,
    internal_label: String,
    barcode: Option<String>,
    stage: String,
    expected_source_location_id: Option<Uuid>,
    destination_location_id: Uuid,
    reservation_owner: String,
    reserved_by: Option<String>,
    confirmed_location_id: Option<Uuid>,
}

#[derive(Debug, FromRow)]
struct CustodyEventRow {
    id: Uuid,
    cartridge_id: Option<Uuid>,
    internal_label: Option<String>,
    event_type: String,
    actor_id: Option<Uuid>,
    actor_label: String,
    source_location_id: Option<Uuid>,
    source_name: Option<String>,
    destination_location_id: Option<Uuid>,
    destination_name: Option<String>,
    note: String,
    corrects_event_id: Option<Uuid>,
    evidence: Value,
    occurred_at: DateTime<Utc>,
    correlation_id: Uuid,
}

#[derive(Debug, FromRow)]
struct OperationReceiptRow {
    id: Uuid,
    request_fingerprint: String,
    resulting_confirmation_version: i64,
    resulting_task_state: String,
    affected_media_ids: Vec<Uuid>,
}

#[derive(Debug, FromRow)]
struct LockedCartridgeRow {
    id: Uuid,
    internal_label: String,
    barcode: Option<String>,
    confirmed_location_id: Option<Uuid>,
    reserved_by: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfirmationOperation {
    Departure,
    Arrival,
    Direct,
}

impl ConfirmationOperation {
    fn operation_kind(self) -> &'static str {
        match self {
            Self::Departure => "departure_confirmed",
            Self::Arrival => "arrival_confirmed",
            Self::Direct => "direct_move_confirmed",
        }
    }

    fn event_type(self) -> &'static str {
        self.operation_kind()
    }

    fn resulting_stage(self) -> MovementMediaStage {
        match self {
            Self::Departure => MovementMediaStage::Departed,
            Self::Arrival => MovementMediaStage::Arrived,
            Self::Direct => MovementMediaStage::DirectConfirmed,
        }
    }

    fn expected_stage(self) -> MovementMediaStage {
        match self {
            Self::Departure | Self::Direct => MovementMediaStage::Planned,
            Self::Arrival => MovementMediaStage::Departed,
        }
    }
}

fn empty_object() -> Value {
    json!({})
}

const fn default_list_limit() -> u32 {
    100
}

fn location_from_row(row: LocationRow) -> Result<LocationRecord, CustodyOpsError> {
    Ok(LocationRecord {
        id: row.id,
        name: row.name,
        kind: LocationKind::parse(&row.kind)?,
        details: row.details,
        created_at: row.created_at,
    })
}

fn summary_from_row(row: MovementSummaryRow) -> Result<MovementSummary, CustodyOpsError> {
    Ok(MovementSummary {
        id: row.id,
        external_id: row.external_id,
        kind: MovementKind::parse(&row.kind)?,
        state: MovementState::parse(&row.state)?,
        confirmation_version: row.confirmation_version,
        source_location_id: row.source_location_id,
        source_name: row.source_name,
        destination_location_id: row.destination_location_id,
        destination_name: row.destination_name,
        transit_location_id: row.transit_location_id,
        transit_name: row.transit_name,
        due_at: row.due_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        media_total: row.media_total,
        media_planned: row.media_planned,
        media_departed: row.media_departed,
        media_confirmed: row.media_confirmed,
        media_cancelled: row.media_cancelled,
        media_exceptions: row.media_exceptions,
    })
}

fn media_from_row(row: MovementMediaRow) -> Result<MovementMediaRecord, CustodyOpsError> {
    Ok(MovementMediaRecord {
        cartridge_id: row.cartridge_id,
        internal_label: row.internal_label,
        barcode: row.barcode,
        stage: MovementMediaStage::parse(&row.stage)?,
        expected_source_location_id: row.expected_source_location_id,
        expected_source_name: row.expected_source_name,
        destination_location_id: row.destination_location_id,
        destination_name: row.destination_name,
        confirmed_location_id: row.confirmed_location_id,
        confirmed_location_name: row.confirmed_location_name,
        reservation_held: row.reserved_by.as_deref() == Some(row.reservation_owner.as_str()),
        last_confirmation_version: row.last_confirmation_version,
        departed_at: row.departed_at,
        arrived_at: row.arrived_at,
        cancelled_at: row.cancelled_at,
        latest_custody_event_id: row.latest_custody_event_id,
    })
}

fn event_from_row(row: CustodyEventRow) -> CustodyEventRecord {
    CustodyEventRecord {
        id: row.id,
        cartridge_id: row.cartridge_id,
        internal_label: row.internal_label,
        event_type: row.event_type,
        actor_id: row.actor_id,
        actor_label: row.actor_label,
        source_location_id: row.source_location_id,
        source_name: row.source_name,
        destination_location_id: row.destination_location_id,
        destination_name: row.destination_name,
        note: row.note,
        corrects_event_id: row.corrects_event_id,
        evidence: row.evidence,
        occurred_at: row.occurred_at,
        correlation_id: row.correlation_id,
    }
}

fn validate_actor(actor: &CustodyActor) -> Result<(), CustodyOpsError> {
    validate_nonempty_text("actor.label", &actor.label, 256)
}

fn validate_nonempty_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), CustodyOpsError> {
    if value.trim().is_empty() {
        return Err(CustodyOpsError::Invalid {
            field,
            message: "must not be empty".to_owned(),
        });
    }
    if value.len() > max_bytes {
        return Err(CustodyOpsError::Invalid {
            field,
            message: format!("must be at most {max_bytes} bytes"),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(CustodyOpsError::Invalid {
            field,
            message: "must not contain control characters".to_owned(),
        });
    }
    Ok(())
}

fn validate_json(field: &'static str, value: &Value) -> Result<(), CustodyOpsError> {
    let size = serde_json::to_vec(value).map_err(|error| CustodyOpsError::Invalid {
        field,
        message: format!("could not be encoded: {error}"),
    })?;
    if size.len() > MAX_JSON_BYTES {
        return Err(CustodyOpsError::Invalid {
            field,
            message: format!("must be at most {MAX_JSON_BYTES} encoded bytes"),
        });
    }
    Ok(())
}

fn normalize_cartridge_ids(
    cartridge_ids: &[Uuid],
    field: &'static str,
) -> Result<Vec<Uuid>, CustodyOpsError> {
    if cartridge_ids.is_empty() {
        return Err(CustodyOpsError::Invalid {
            field,
            message: "must contain at least one cartridge".to_owned(),
        });
    }
    let mut normalized = cartridge_ids.to_vec();
    normalized.sort_unstable();
    let original_len = normalized.len();
    normalized.dedup();
    if normalized.len() != original_len {
        return Err(CustodyOpsError::Invalid {
            field,
            message: "must not contain duplicate cartridge IDs".to_owned(),
        });
    }
    Ok(normalized)
}

fn fingerprint(value: &Value) -> Result<String, CustodyOpsError> {
    let encoded = serde_json::to_vec(value).map_err(|error| CustodyOpsError::Invalid {
        field: "request",
        message: format!("could not be fingerprinted: {error}"),
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn validate_movement_location_shape(
    source_location_id: Option<Uuid>,
    destination_location_id: Uuid,
    transit_location_id: Option<Uuid>,
) -> Result<(), CustodyOpsError> {
    if source_location_id == Some(destination_location_id) {
        return Err(CustodyOpsError::Invalid {
            field: "destination_location_id",
            message: "must differ from the source location".to_owned(),
        });
    }
    if let Some(transit_location_id) = transit_location_id {
        if Some(transit_location_id) == source_location_id
            || transit_location_id == destination_location_id
        {
            return Err(CustodyOpsError::Invalid {
                field: "transit_location_id",
                message: "must differ from both movement endpoints".to_owned(),
            });
        }
    }
    Ok(())
}

/// List all configured physical, transit, device, and unknown locations.
pub async fn list_locations(pool: &PgPool) -> Result<Vec<LocationRecord>, CustodyOpsError> {
    let rows = sqlx::query_as::<_, LocationRow>(
        "SELECT id, name, kind, details, created_at
         FROM locations
         ORDER BY lower(name), id",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(location_from_row).collect()
}

/// Create a location and append audit/outbox evidence in the same transaction.
pub async fn create_location(
    pool: &PgPool,
    request: CreateLocationRequest,
) -> Result<LocationRecord, CustodyOpsError> {
    validate_nonempty_text("name", &request.name, 256)?;
    validate_actor(&request.actor)?;
    validate_json("details", &request.details)?;
    let name = request.name.trim().to_owned();
    let location_id = Uuid::now_v7();
    let mut transaction = pool.begin().await?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('location:' || $1, 0))")
        .bind(&name)
        .execute(&mut *transaction)
        .await?;

    let existing = sqlx::query_scalar::<_, Uuid>("SELECT id FROM locations WHERE name = $1")
        .bind(&name)
        .fetch_optional(&mut *transaction)
        .await?;
    if existing.is_some() {
        return Err(CustodyOpsError::Conflict {
            code: "location_name_exists",
            message: format!("a location named {name:?} already exists"),
        });
    }

    let row = sqlx::query_as::<_, LocationRow>(
        "INSERT INTO locations (id, name, kind, details)
         VALUES ($1, $2, $3, $4)
         RETURNING id, name, kind, details, created_at",
    )
    .bind(location_id)
    .bind(&name)
    .bind(request.kind.as_str())
    .bind(&request.details)
    .fetch_one(&mut *transaction)
    .await?;

    append_audit(
        &mut transaction,
        &request.actor,
        "location.created",
        "location",
        &location_id.to_string(),
        json!({"name": name, "kind": request.kind}),
        "created",
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("location:{location_id}:created"),
        json!({
            "event_type": "location.created",
            "location_id": location_id,
            "name": row.name,
            "kind": row.kind,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;
    transaction.commit().await?;
    location_from_row(row)
}

/// Create and atomically reserve an exact set of cartridges for a movement.
///
/// `source_location_id = None` is the explicit bootstrap-from-Unknown path.  It
/// succeeds only when every selected cartridge has no confirmed location.  Such
/// a task can be completed with [`confirm_direct_move`], but cannot depart into
/// a two-step transit workflow.
pub async fn create_movement(
    pool: &PgPool,
    request: CreateMovementRequest,
) -> Result<CreateMovementResult, CustodyOpsError> {
    validate_nonempty_text("external_id", &request.external_id, 256)?;
    validate_nonempty_text("idempotency_key", &request.idempotency_key, 256)?;
    validate_actor(&request.actor)?;
    validate_json("readiness", &request.readiness)?;
    let cartridge_ids = normalize_cartridge_ids(&request.cartridge_ids, "cartridge_ids")?;
    validate_movement_location_shape(
        request.source_location_id,
        request.destination_location_id,
        request.transit_location_id,
    )?;

    let external_id = request.external_id.trim().to_owned();
    let idempotency_key = request.idempotency_key.trim().to_owned();
    let request_fingerprint = fingerprint(&json!({
        "external_id": external_id,
        "kind": request.kind,
        "source_location_id": request.source_location_id,
        "destination_location_id": request.destination_location_id,
        "transit_location_id": request.transit_location_id,
        "cartridge_ids": cartridge_ids,
        "readiness": request.readiness,
        "assigned_user_id": request.assigned_user_id,
        "due_at": request.due_at,
        "actor": request.actor,
    }))?;
    let mut transaction = pool.begin().await?;

    // Serializes concurrent first attempts using the same key, so a retry never
    // loses a race to its own cartridge reservation.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&idempotency_key)
        .execute(&mut *transaction)
        .await?;

    if let Some((task_id, state, version, stored_fingerprint, media_ids)) =
        sqlx::query_as::<_, (Uuid, String, i64, Option<String>, Vec<Uuid>)>(
            "SELECT id, state, confirmation_version, request_fingerprint, required_media_ids
             FROM movement_tasks WHERE idempotency_key = $1",
        )
        .bind(&idempotency_key)
        .fetch_optional(&mut *transaction)
        .await?
    {
        if stored_fingerprint.as_deref() != Some(request_fingerprint.as_str()) {
            return Err(CustodyOpsError::IdempotencyConflict {
                key: idempotency_key,
            });
        }
        transaction.commit().await?;
        let mut media_ids = media_ids;
        media_ids.sort_unstable();
        return Ok(CreateMovementResult {
            movement_task_id: task_id,
            state: MovementState::parse(&state)?,
            confirmation_version: version,
            reserved_cartridge_ids: media_ids,
            idempotent_replay: true,
        });
    }

    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM movement_tasks WHERE external_id = $1)",
    )
    .bind(&external_id)
    .fetch_one(&mut *transaction)
    .await?
    {
        return Err(CustodyOpsError::Conflict {
            code: "movement_external_id_exists",
            message: format!("movement external ID {external_id:?} already exists"),
        });
    }

    if let Some(source_location_id) = request.source_location_id {
        let source_kind = require_location(&mut transaction, source_location_id).await?;
        if matches!(source_kind, LocationKind::Unknown | LocationKind::InTransit) {
            return Err(CustodyOpsError::Invalid {
                field: "source_location_id",
                message: "must identify a confirmed endpoint; use null for Unknown".to_owned(),
            });
        }
    }
    let destination_kind =
        require_location(&mut transaction, request.destination_location_id).await?;
    if matches!(
        destination_kind,
        LocationKind::Unknown | LocationKind::InTransit
    ) {
        return Err(CustodyOpsError::Invalid {
            field: "destination_location_id",
            message: "must identify a physical endpoint".to_owned(),
        });
    }
    if let Some(transit_location_id) = request.transit_location_id {
        let transit_kind = require_location(&mut transaction, transit_location_id).await?;
        if transit_kind != LocationKind::InTransit {
            return Err(CustodyOpsError::Invalid {
                field: "transit_location_id",
                message: "must identify a location whose kind is in_transit".to_owned(),
            });
        }
    }

    let cartridges = sqlx::query_as::<_, LockedCartridgeRow>(
        "SELECT id, internal_label, barcode, confirmed_location_id, reserved_by
         FROM cartridges
         WHERE id = ANY($1::uuid[])
         ORDER BY id
         FOR UPDATE",
    )
    .bind(&cartridge_ids)
    .fetch_all(&mut *transaction)
    .await?;
    if cartridges.len() != cartridge_ids.len() {
        let found: std::collections::BTreeSet<_> =
            cartridges.iter().map(|cartridge| cartridge.id).collect();
        let missing = cartridge_ids
            .iter()
            .find(|id| !found.contains(id))
            .copied()
            .expect("length mismatch implies a missing cartridge");
        return Err(CustodyOpsError::NotFound {
            resource: "cartridge",
            id: missing.to_string(),
        });
    }
    for cartridge in &cartridges {
        if cartridge.confirmed_location_id != request.source_location_id {
            return Err(CustodyOpsError::WrongSource {
                cartridge_id: cartridge.id,
                expected_location_id: request.source_location_id,
                actual_location_id: cartridge.confirmed_location_id,
            });
        }
        if let Some(reserved_by) = &cartridge.reserved_by {
            return Err(CustodyOpsError::ReservationConflict {
                cartridge_id: cartridge.id,
                reserved_by: reserved_by.clone(),
            });
        }
    }

    let movement_task_id = Uuid::now_v7();
    let reservation_owner = format!("movement:{movement_task_id}");
    let state = MovementState::Ready;
    sqlx::query(
        "INSERT INTO movement_tasks (
            id, external_id, kind, source_location_id, destination_location_id,
            transit_location_id, required_media_ids, state, readiness,
            assigned_user_id, due_at, idempotency_key, request_fingerprint
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(movement_task_id)
    .bind(&external_id)
    .bind(request.kind.as_str())
    .bind(request.source_location_id)
    .bind(request.destination_location_id)
    .bind(request.transit_location_id)
    .bind(&cartridge_ids)
    .bind(state.as_str())
    .bind(&request.readiness)
    .bind(request.assigned_user_id)
    .bind(request.due_at)
    .bind(&idempotency_key)
    .bind(&request_fingerprint)
    .execute(&mut *transaction)
    .await?;

    let now = Utc::now();
    for cartridge in &cartridges {
        sqlx::query(
            "INSERT INTO movement_task_media (
                movement_task_id, cartridge_id, expected_source_location_id,
                destination_location_id, reservation_owner, stage
             ) VALUES ($1, $2, $3, $4, $5, 'planned')",
        )
        .bind(movement_task_id)
        .bind(cartridge.id)
        .bind(request.source_location_id)
        .bind(request.destination_location_id)
        .bind(&reservation_owner)
        .execute(&mut *transaction)
        .await?;

        let reserved = sqlx::query(
            "UPDATE cartridges
             SET reserved_by = $1, updated_at = $2
             WHERE id = $3 AND reserved_by IS NULL",
        )
        .bind(&reservation_owner)
        .bind(now)
        .bind(cartridge.id)
        .execute(&mut *transaction)
        .await?;
        if reserved.rows_affected() != 1 {
            return Err(CustodyOpsError::ReservationConflict {
                cartridge_id: cartridge.id,
                reserved_by: "another operation".to_owned(),
            });
        }

        append_custody_event(
            &mut transaction,
            Uuid::now_v7(),
            movement_task_id,
            &external_id,
            cartridge.id,
            &cartridge.internal_label,
            &request.actor,
            "movement_planned",
            request.source_location_id,
            Some(request.destination_location_id),
            None,
            "",
            None,
            json!({
                "movement_kind": request.kind,
                "stage": "planned",
                "confirmation_version": 0,
                "physical_custody_changed": false,
                "barcode": cartridge.barcode,
            }),
            now,
            request.correlation_id,
        )
        .await?;
    }

    append_audit(
        &mut transaction,
        &request.actor,
        "movement.created",
        "movement_task",
        &movement_task_id.to_string(),
        json!({
            "external_id": external_id,
            "kind": request.kind,
            "source_location_id": request.source_location_id,
            "destination_location_id": request.destination_location_id,
            "transit_location_id": request.transit_location_id,
            "cartridge_ids": cartridge_ids,
            "idempotency_key": idempotency_key,
        }),
        "reserved",
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("movement:{movement_task_id}:created"),
        json!({
            "event_type": "movement.created",
            "movement_task_id": movement_task_id,
            "external_id": external_id,
            "state": state,
            "cartridge_ids": cartridge_ids,
            "due_at": request.due_at,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(CreateMovementResult {
        movement_task_id,
        state,
        confirmation_version: 0,
        reserved_cartridge_ids: cartridge_ids,
        idempotent_replay: false,
    })
}

/// List movement tasks with normalized per-stage counts.
pub async fn list_movements(
    pool: &PgPool,
    query: MovementListQuery,
) -> Result<Vec<MovementSummary>, CustodyOpsError> {
    if query.limit == 0 || query.limit > MAX_LIST_LIMIT {
        return Err(CustodyOpsError::Invalid {
            field: "limit",
            message: format!("must be between 1 and {MAX_LIST_LIMIT}"),
        });
    }
    let states: Vec<String> = query
        .states
        .iter()
        .map(|state| state.as_str().to_owned())
        .collect();
    let rows = sqlx::query_as::<_, MovementSummaryRow>(
        "SELECT task.id, task.external_id, task.kind, task.state,
                task.confirmation_version, task.source_location_id,
                source.name AS source_name,
                task.destination_location_id, destination.name AS destination_name,
                task.transit_location_id, transit.name AS transit_name,
                task.due_at, task.created_at, task.updated_at,
                count(media.cartridge_id) AS media_total,
                count(*) FILTER (WHERE media.stage = 'planned') AS media_planned,
                count(*) FILTER (WHERE media.stage = 'departed') AS media_departed,
                count(*) FILTER (WHERE media.stage IN ('arrived', 'direct_confirmed')) AS media_confirmed,
                count(*) FILTER (WHERE media.stage = 'cancelled') AS media_cancelled,
                count(*) FILTER (WHERE media.stage = 'exception') AS media_exceptions
         FROM movement_tasks AS task
         LEFT JOIN locations AS source ON source.id = task.source_location_id
         JOIN locations AS destination ON destination.id = task.destination_location_id
         LEFT JOIN locations AS transit ON transit.id = task.transit_location_id
         LEFT JOIN movement_task_media AS media ON media.movement_task_id = task.id
         WHERE (cardinality($1::text[]) = 0 OR task.state = ANY($1::text[]))
         GROUP BY task.id, source.name, destination.name, transit.name
         ORDER BY task.due_at, task.created_at, task.id
         LIMIT $2",
    )
    .bind(&states)
    .bind(i64::from(query.limit))
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(summary_from_row).collect()
}

/// Load a task, its exact cartridges, and its append-only custody history.
pub async fn get_movement(
    pool: &PgPool,
    movement_task_id: Uuid,
) -> Result<MovementDetail, CustodyOpsError> {
    get_movement_by_selector(pool, Some(movement_task_id), None).await
}

/// Load a movement task by its caller-stable external identifier.
pub async fn get_movement_by_external_id(
    pool: &PgPool,
    external_id: &str,
) -> Result<MovementDetail, CustodyOpsError> {
    validate_nonempty_text("external_id", external_id, 256)?;
    get_movement_by_selector(pool, None, Some(external_id)).await
}

async fn get_movement_by_selector(
    pool: &PgPool,
    movement_task_id: Option<Uuid>,
    external_id: Option<&str>,
) -> Result<MovementDetail, CustodyOpsError> {
    let summary_row = sqlx::query_as::<_, MovementSummaryRow>(
        "SELECT task.id, task.external_id, task.kind, task.state,
                task.confirmation_version, task.source_location_id,
                source.name AS source_name,
                task.destination_location_id, destination.name AS destination_name,
                task.transit_location_id, transit.name AS transit_name,
                task.due_at, task.created_at, task.updated_at,
                count(media.cartridge_id) AS media_total,
                count(*) FILTER (WHERE media.stage = 'planned') AS media_planned,
                count(*) FILTER (WHERE media.stage = 'departed') AS media_departed,
                count(*) FILTER (WHERE media.stage IN ('arrived', 'direct_confirmed')) AS media_confirmed,
                count(*) FILTER (WHERE media.stage = 'cancelled') AS media_cancelled,
                count(*) FILTER (WHERE media.stage = 'exception') AS media_exceptions
         FROM movement_tasks AS task
         LEFT JOIN locations AS source ON source.id = task.source_location_id
         JOIN locations AS destination ON destination.id = task.destination_location_id
         LEFT JOIN locations AS transit ON transit.id = task.transit_location_id
         LEFT JOIN movement_task_media AS media ON media.movement_task_id = task.id
         WHERE ($1::uuid IS NOT NULL AND task.id = $1)
            OR ($2::text IS NOT NULL AND task.external_id = $2)
         GROUP BY task.id, source.name, destination.name, transit.name",
    )
        .bind(movement_task_id)
        .bind(external_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| CustodyOpsError::NotFound {
            resource: "movement task",
            id: external_id
                .map(ToOwned::to_owned)
                .or_else(|| movement_task_id.map(|id| id.to_string()))
                .unwrap_or_else(|| "<unspecified>".to_owned()),
        })?;
    let task_id = summary_row.id;
    let summary = summary_from_row(summary_row)?;

    let (readiness, assigned_user_id) = sqlx::query_as::<_, (Value, Option<Uuid>)>(
        "SELECT readiness, assigned_user_id FROM movement_tasks WHERE id = $1",
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;

    let media_rows = sqlx::query_as::<_, MovementMediaRow>(
        "SELECT media.cartridge_id, cartridge.internal_label, cartridge.barcode,
                media.stage, media.expected_source_location_id,
                expected_source.name AS expected_source_name,
                media.destination_location_id,
                destination.name AS destination_name,
                cartridge.confirmed_location_id,
                confirmed.name AS confirmed_location_name,
                media.reservation_owner, cartridge.reserved_by,
                media.last_confirmation_version, media.departed_at,
                media.arrived_at, media.cancelled_at,
                media.latest_custody_event_id
         FROM movement_task_media AS media
         JOIN cartridges AS cartridge ON cartridge.id = media.cartridge_id
         LEFT JOIN locations AS expected_source ON expected_source.id = media.expected_source_location_id
         JOIN locations AS destination ON destination.id = media.destination_location_id
         LEFT JOIN locations AS confirmed ON confirmed.id = cartridge.confirmed_location_id
         WHERE media.movement_task_id = $1
         ORDER BY cartridge.internal_label, cartridge.id",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await?;
    let media = media_rows
        .into_iter()
        .map(media_from_row)
        .collect::<Result<Vec<_>, _>>()?;

    let event_rows = sqlx::query_as::<_, CustodyEventRow>(
        "SELECT event.id, event.cartridge_id, cartridge.internal_label,
                event.event_type, event.actor_id, event.actor_label,
                event.source_location_id, source.name AS source_name,
                event.destination_location_id,
                COALESCE(destination.name, event.destination_label) AS destination_name,
                event.note, event.corrects_event_id, event.evidence,
                event.occurred_at, event.correlation_id
         FROM custody_events AS event
         LEFT JOIN cartridges AS cartridge ON cartridge.id = event.cartridge_id
         LEFT JOIN locations AS source ON source.id = event.source_location_id
         LEFT JOIN locations AS destination ON destination.id = event.destination_location_id
         WHERE event.movement_task_id = $1
         ORDER BY event.occurred_at, event.id",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await?;
    let custody_events = event_rows.into_iter().map(event_from_row).collect();

    Ok(MovementDetail {
        summary,
        readiness,
        assigned_user_id,
        media,
        custody_events,
    })
}

/// Confirm that selected tapes physically departed their exact confirmed source.
/// A configured `in_transit` location is required; Unknown cannot depart.
pub async fn confirm_departure(
    pool: &PgPool,
    movement_task_id: Uuid,
    request: PhysicalConfirmationRequest,
) -> Result<MovementMutationResult, CustodyOpsError> {
    confirm_physical_movement(
        pool,
        movement_task_id,
        request,
        ConfirmationOperation::Departure,
    )
    .await
}

/// Confirm arrival of any subset of departed tapes.  Remaining tapes preserve
/// their own stages, so partial delivery never becomes a fully confirmed task.
pub async fn confirm_arrival(
    pool: &PgPool,
    movement_task_id: Uuid,
    request: PhysicalConfirmationRequest,
) -> Result<MovementMutationResult, CustodyOpsError> {
    confirm_physical_movement(
        pool,
        movement_task_id,
        request,
        ConfirmationOperation::Arrival,
    )
    .await
}

/// Directly confirm physical transfer from source to destination.  This is also
/// the safe bootstrap path from Unknown (`source_location_id = None`).
pub async fn confirm_direct_move(
    pool: &PgPool,
    movement_task_id: Uuid,
    request: PhysicalConfirmationRequest,
) -> Result<MovementMutationResult, CustodyOpsError> {
    confirm_physical_movement(
        pool,
        movement_task_id,
        request,
        ConfirmationOperation::Direct,
    )
    .await
}

async fn confirm_physical_movement(
    pool: &PgPool,
    movement_task_id: Uuid,
    request: PhysicalConfirmationRequest,
    operation: ConfirmationOperation,
) -> Result<MovementMutationResult, CustodyOpsError> {
    validate_confirmation_request(&request)?;
    let cartridge_ids = normalize_cartridge_ids(&request.cartridge_ids, "cartridge_ids")?;
    let idempotency_key = request.idempotency_key.trim().to_owned();
    let request_fingerprint = fingerprint(&json!({
        "operation": operation.operation_kind(),
        "cartridge_ids": cartridge_ids,
        "expected_confirmation_version": request.expected_confirmation_version,
        "physically_confirmed": request.physically_confirmed,
        "note": request.note,
        "actor": request.actor,
    }))?;
    let mut transaction = pool.begin().await?;
    let task = lock_movement(&mut transaction, movement_task_id).await?;

    if let Some(receipt) =
        find_operation_receipt(&mut transaction, movement_task_id, &idempotency_key).await?
    {
        ensure_idempotent_match(&receipt, &request_fingerprint, &idempotency_key)?;
        transaction.commit().await?;
        return mutation_result_from_receipt(movement_task_id, receipt, true);
    }
    require_confirmation_version(&task, request.expected_confirmation_version)?;
    let task_state = MovementState::parse(&task.state)?;
    if task_state.is_terminal() {
        return Err(CustodyOpsError::Conflict {
            code: "movement_terminal",
            message: format!("movement is already {}", task_state.as_str()),
        });
    }
    if operation == ConfirmationOperation::Departure && task.source_location_id.is_none() {
        return Err(CustodyOpsError::Conflict {
            code: "unknown_source_cannot_depart",
            message: "use direct confirmation to establish custody for unlocated media".to_owned(),
        });
    }

    let (expected_location_id, resulting_location_id) = match operation {
        ConfirmationOperation::Departure => {
            let transit = task
                .transit_location_id
                .ok_or_else(|| CustodyOpsError::Conflict {
                    code: "transit_location_required",
                    message: "this movement has no configured in-transit location".to_owned(),
                })?;
            (task.source_location_id, transit)
        }
        ConfirmationOperation::Arrival => {
            let transit = task
                .transit_location_id
                .ok_or_else(|| CustodyOpsError::Conflict {
                    code: "transit_location_required",
                    message: "this movement has no configured in-transit location".to_owned(),
                })?;
            (Some(transit), task.destination_location_id)
        }
        ConfirmationOperation::Direct => (task.source_location_id, task.destination_location_id),
    };

    let media = lock_selected_media(&mut transaction, movement_task_id, &cartridge_ids).await?;
    if media.len() != cartridge_ids.len() {
        return Err(CustodyOpsError::Conflict {
            code: "media_not_in_movement",
            message: "one or more selected cartridges do not belong to this movement".to_owned(),
        });
    }
    let expected_stage = operation.expected_stage();
    for item in &media {
        let actual_stage = MovementMediaStage::parse(&item.stage)?;
        if actual_stage != expected_stage {
            return Err(CustodyOpsError::Conflict {
                code: "invalid_media_stage",
                message: format!(
                    "cartridge {} is {}, expected {}",
                    item.internal_label,
                    actual_stage.as_str(),
                    expected_stage.as_str()
                ),
            });
        }
        if item.expected_source_location_id != task.source_location_id
            || item.destination_location_id != task.destination_location_id
        {
            return Err(CustodyOpsError::Invariant(format!(
                "movement media {} does not match task endpoints",
                item.cartridge_id
            )));
        }
        if item.confirmed_location_id != expected_location_id {
            return Err(CustodyOpsError::WrongSource {
                cartridge_id: item.cartridge_id,
                expected_location_id,
                actual_location_id: item.confirmed_location_id,
            });
        }
        if item.reserved_by.as_deref() != Some(item.reservation_owner.as_str()) {
            return Err(CustodyOpsError::ReservationConflict {
                cartridge_id: item.cartridge_id,
                reserved_by: item
                    .reserved_by
                    .clone()
                    .unwrap_or_else(|| "<not reserved>".to_owned()),
            });
        }
    }

    let new_version = task.confirmation_version + 1;
    let now = Utc::now();
    let resulting_stage = operation.resulting_stage();
    for item in &media {
        let event_id = Uuid::now_v7();
        let terminal = operation != ConfirmationOperation::Departure;
        let cartridge_update = if terminal {
            sqlx::query(
                "UPDATE cartridges
                 SET confirmed_location_id = $1, reserved_by = NULL, updated_at = $2
                 WHERE id = $3 AND reserved_by = $4",
            )
            .bind(resulting_location_id)
            .bind(now)
            .bind(item.cartridge_id)
            .bind(&item.reservation_owner)
            .execute(&mut *transaction)
            .await?
        } else {
            sqlx::query(
                "UPDATE cartridges
                 SET confirmed_location_id = $1, updated_at = $2
                 WHERE id = $3 AND reserved_by = $4",
            )
            .bind(resulting_location_id)
            .bind(now)
            .bind(item.cartridge_id)
            .bind(&item.reservation_owner)
            .execute(&mut *transaction)
            .await?
        };
        if cartridge_update.rows_affected() != 1 {
            return Err(CustodyOpsError::ReservationConflict {
                cartridge_id: item.cartridge_id,
                reserved_by: "reservation changed during confirmation".to_owned(),
            });
        }

        append_custody_event(
            &mut transaction,
            event_id,
            movement_task_id,
            &task.external_id,
            item.cartridge_id,
            &item.internal_label,
            &request.actor,
            operation.event_type(),
            expected_location_id,
            Some(resulting_location_id),
            None,
            request.note.trim(),
            None,
            json!({
                "movement_kind": task.kind,
                "from_stage": expected_stage,
                "to_stage": resulting_stage,
                "confirmation_version": new_version,
                "physical_custody_changed": true,
                "idempotency_key": idempotency_key,
                "barcode": item.barcode,
            }),
            now,
            request.correlation_id,
        )
        .await?;

        sqlx::query(
            "UPDATE movement_task_media
             SET stage = $1, latest_custody_event_id = $2,
                 last_confirmation_version = $3,
                 departed_at = CASE WHEN $4 = 'departure_confirmed' THEN $5 ELSE departed_at END,
                 arrived_at = CASE WHEN $4 IN ('arrival_confirmed', 'direct_move_confirmed')
                                   THEN $5 ELSE arrived_at END,
                 updated_at = $5
             WHERE movement_task_id = $6 AND cartridge_id = $7",
        )
        .bind(resulting_stage.as_str())
        .bind(event_id)
        .bind(new_version)
        .bind(operation.operation_kind())
        .bind(now)
        .bind(movement_task_id)
        .bind(item.cartridge_id)
        .execute(&mut *transaction)
        .await?;
    }

    let derived_state =
        derive_task_state_in_transaction(&mut transaction, movement_task_id).await?;
    let resulting_state = preserve_overdue_state(&task_state, derived_state);
    sqlx::query(
        "UPDATE movement_tasks
         SET state = $1, confirmation_version = $2, updated_at = $3
         WHERE id = $4",
    )
    .bind(resulting_state.as_str())
    .bind(new_version)
    .bind(now)
    .bind(movement_task_id)
    .execute(&mut *transaction)
    .await?;

    let operation_id = Uuid::now_v7();
    insert_operation_receipt(
        &mut transaction,
        operation_id,
        movement_task_id,
        &idempotency_key,
        operation.operation_kind(),
        &request_fingerprint,
        request.expected_confirmation_version,
        new_version,
        &resulting_state,
        &cartridge_ids,
        &request.actor,
        request.correlation_id,
        json!({
            "physically_confirmed": true,
            "source_location_id": expected_location_id,
            "confirmed_location_id": resulting_location_id,
        }),
    )
    .await?;
    append_audit(
        &mut transaction,
        &request.actor,
        &format!("movement.{}", operation.operation_kind()),
        "movement_task",
        &movement_task_id.to_string(),
        json!({
            "cartridge_ids": cartridge_ids,
            "expected_confirmation_version": request.expected_confirmation_version,
            "idempotency_key": idempotency_key,
            "physically_confirmed": true,
        }),
        resulting_state.as_str(),
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("movement:{movement_task_id}:operation:{operation_id}"),
        json!({
            "event_type": format!("movement.{}", operation.operation_kind()),
            "movement_task_id": movement_task_id,
            "operation_id": operation_id,
            "state": resulting_state,
            "confirmation_version": new_version,
            "cartridge_ids": cartridge_ids,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;
    transaction.commit().await?;

    Ok(MovementMutationResult {
        movement_task_id,
        operation_id,
        state: resulting_state,
        confirmation_version: new_version,
        affected_cartridge_ids: cartridge_ids,
        idempotent_replay: false,
    })
}

/// Cancel the un-departed remainder of a movement and release only reservations
/// owned by that task.  A task with any tape physically in transit cannot be
/// cancelled; it must be arrived or corrected first.
pub async fn cancel_movement(
    pool: &PgPool,
    movement_task_id: Uuid,
    request: CancelMovementRequest,
) -> Result<MovementMutationResult, CustodyOpsError> {
    validate_nonempty_text("idempotency_key", &request.idempotency_key, 256)?;
    validate_nonempty_text("reason", &request.reason, 4096)?;
    validate_actor(&request.actor)?;
    if request.expected_confirmation_version < 0 {
        return Err(CustodyOpsError::Invalid {
            field: "expected_confirmation_version",
            message: "must not be negative".to_owned(),
        });
    }
    let idempotency_key = request.idempotency_key.trim().to_owned();
    let request_fingerprint = fingerprint(&json!({
        "operation": "cancelled",
        "expected_confirmation_version": request.expected_confirmation_version,
        "reason": request.reason,
        "actor": request.actor,
    }))?;
    let mut transaction = pool.begin().await?;
    let task = lock_movement(&mut transaction, movement_task_id).await?;
    if let Some(receipt) =
        find_operation_receipt(&mut transaction, movement_task_id, &idempotency_key).await?
    {
        ensure_idempotent_match(&receipt, &request_fingerprint, &idempotency_key)?;
        transaction.commit().await?;
        return mutation_result_from_receipt(movement_task_id, receipt, true);
    }
    require_confirmation_version(&task, request.expected_confirmation_version)?;
    let task_state = MovementState::parse(&task.state)?;
    if task_state.is_terminal() {
        return Err(CustodyOpsError::Conflict {
            code: "movement_terminal",
            message: format!("movement is already {}", task_state.as_str()),
        });
    }

    let media = lock_all_media(&mut transaction, movement_task_id).await?;
    if media.is_empty() {
        return Err(CustodyOpsError::Invariant(format!(
            "movement {movement_task_id} has no normalized media"
        )));
    }
    if media.iter().any(|item| item.stage == "departed") {
        return Err(CustodyOpsError::Conflict {
            code: "media_in_transit",
            message: "a movement with departed media cannot be cancelled".to_owned(),
        });
    }
    let pending: Vec<&LockedMediaRow> = media
        .iter()
        .filter(|item| item.stage == "planned")
        .collect();
    if pending.is_empty() {
        return Err(CustodyOpsError::Conflict {
            code: "nothing_to_cancel",
            message: "the movement has no un-departed media".to_owned(),
        });
    }
    for item in &pending {
        if item.reserved_by.as_deref() != Some(item.reservation_owner.as_str()) {
            return Err(CustodyOpsError::ReservationConflict {
                cartridge_id: item.cartridge_id,
                reserved_by: item
                    .reserved_by
                    .clone()
                    .unwrap_or_else(|| "<not reserved>".to_owned()),
            });
        }
    }

    let new_version = task.confirmation_version + 1;
    let now = Utc::now();
    let mut affected_cartridge_ids = Vec::with_capacity(pending.len());
    for item in pending {
        let event_id = Uuid::now_v7();
        let released = sqlx::query(
            "UPDATE cartridges
             SET reserved_by = NULL, updated_at = $1
             WHERE id = $2 AND reserved_by = $3",
        )
        .bind(now)
        .bind(item.cartridge_id)
        .bind(&item.reservation_owner)
        .execute(&mut *transaction)
        .await?;
        if released.rows_affected() != 1 {
            return Err(CustodyOpsError::ReservationConflict {
                cartridge_id: item.cartridge_id,
                reserved_by: "reservation changed during cancellation".to_owned(),
            });
        }
        append_custody_event(
            &mut transaction,
            event_id,
            movement_task_id,
            &task.external_id,
            item.cartridge_id,
            &item.internal_label,
            &request.actor,
            "movement_cancelled",
            item.confirmed_location_id,
            Some(task.destination_location_id),
            None,
            request.reason.trim(),
            None,
            json!({
                "confirmation_version": new_version,
                "physical_custody_changed": false,
                "idempotency_key": idempotency_key,
                "barcode": item.barcode,
            }),
            now,
            request.correlation_id,
        )
        .await?;
        sqlx::query(
            "UPDATE movement_task_media
             SET stage = 'cancelled', latest_custody_event_id = $1,
                 last_confirmation_version = $2, cancelled_at = $3, updated_at = $3
             WHERE movement_task_id = $4 AND cartridge_id = $5",
        )
        .bind(event_id)
        .bind(new_version)
        .bind(now)
        .bind(movement_task_id)
        .bind(item.cartridge_id)
        .execute(&mut *transaction)
        .await?;
        affected_cartridge_ids.push(item.cartridge_id);
    }
    affected_cartridge_ids.sort_unstable();

    let resulting_state = MovementState::Cancelled;
    sqlx::query(
        "UPDATE movement_tasks
         SET state = 'cancelled', confirmation_version = $1, updated_at = $2
         WHERE id = $3",
    )
    .bind(new_version)
    .bind(now)
    .bind(movement_task_id)
    .execute(&mut *transaction)
    .await?;
    let operation_id = Uuid::now_v7();
    insert_operation_receipt(
        &mut transaction,
        operation_id,
        movement_task_id,
        &idempotency_key,
        "cancelled",
        &request_fingerprint,
        request.expected_confirmation_version,
        new_version,
        &resulting_state,
        &affected_cartridge_ids,
        &request.actor,
        request.correlation_id,
        json!({"reason": request.reason.trim()}),
    )
    .await?;
    append_audit(
        &mut transaction,
        &request.actor,
        "movement.cancelled",
        "movement_task",
        &movement_task_id.to_string(),
        json!({
            "affected_cartridge_ids": affected_cartridge_ids,
            "expected_confirmation_version": request.expected_confirmation_version,
            "idempotency_key": idempotency_key,
            "reason": request.reason.trim(),
        }),
        "cancelled",
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("movement:{movement_task_id}:operation:{operation_id}"),
        json!({
            "event_type": "movement.cancelled",
            "movement_task_id": movement_task_id,
            "operation_id": operation_id,
            "state": resulting_state,
            "confirmation_version": new_version,
            "cartridge_ids": affected_cartridge_ids,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(MovementMutationResult {
        movement_task_id,
        operation_id,
        state: resulting_state,
        confirmation_version: new_version,
        affected_cartridge_ids,
        idempotent_replay: false,
    })
}

/// Append a correction to prior custody history and update the confirmed
/// location projection.  The original event is never edited or deleted.
pub async fn append_custody_correction(
    pool: &PgPool,
    movement_task_id: Uuid,
    request: AppendCorrectionRequest,
) -> Result<MovementMutationResult, CustodyOpsError> {
    validate_nonempty_text("idempotency_key", &request.idempotency_key, 256)?;
    validate_nonempty_text("note", &request.note, 4096)?;
    validate_actor(&request.actor)?;
    if !request.physically_confirmed {
        return Err(CustodyOpsError::Invalid {
            field: "physically_confirmed",
            message: "must be true before confirmed custody can change".to_owned(),
        });
    }
    if request.expected_confirmation_version < 0 {
        return Err(CustodyOpsError::Invalid {
            field: "expected_confirmation_version",
            message: "must not be negative".to_owned(),
        });
    }
    let idempotency_key = request.idempotency_key.trim().to_owned();
    let request_fingerprint = fingerprint(&json!({
        "operation": "custody_corrected",
        "corrects_event_id": request.corrects_event_id,
        "cartridge_id": request.cartridge_id,
        "expected_current_location_id": request.expected_current_location_id,
        "confirmed_location_id": request.confirmed_location_id,
        "expected_confirmation_version": request.expected_confirmation_version,
        "physically_confirmed": request.physically_confirmed,
        "note": request.note,
        "actor": request.actor,
    }))?;
    let mut transaction = pool.begin().await?;
    let task = lock_movement(&mut transaction, movement_task_id).await?;
    if let Some(receipt) =
        find_operation_receipt(&mut transaction, movement_task_id, &idempotency_key).await?
    {
        ensure_idempotent_match(&receipt, &request_fingerprint, &idempotency_key)?;
        transaction.commit().await?;
        return mutation_result_from_receipt(movement_task_id, receipt, true);
    }
    require_confirmation_version(&task, request.expected_confirmation_version)?;
    let prior_task_state = MovementState::parse(&task.state)?;

    let target_event_type = sqlx::query_scalar::<_, String>(
        "SELECT event_type
         FROM custody_events
         WHERE id = $1 AND movement_task_id = $2 AND cartridge_id = $3",
    )
    .bind(request.corrects_event_id)
    .bind(movement_task_id)
    .bind(request.cartridge_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| CustodyOpsError::NotFound {
        resource: "custody event",
        id: request.corrects_event_id.to_string(),
    })?;
    if !matches!(
        target_event_type.as_str(),
        "departure_confirmed" | "arrival_confirmed" | "direct_move_confirmed" | "custody_corrected"
    ) {
        return Err(CustodyOpsError::Conflict {
            code: "event_not_correctable",
            message: format!("event type {target_event_type:?} did not change physical custody"),
        });
    }

    let corrected_location_kind =
        require_location(&mut transaction, request.confirmed_location_id).await?;
    if corrected_location_kind == LocationKind::Unknown {
        return Err(CustodyOpsError::Invalid {
            field: "confirmed_location_id",
            message: "a physical confirmation cannot target an unknown location".to_owned(),
        });
    }
    let mut media =
        lock_selected_media(&mut transaction, movement_task_id, &[request.cartridge_id]).await?;
    let item = media.pop().ok_or_else(|| CustodyOpsError::Conflict {
        code: "media_not_in_movement",
        message: "the corrected cartridge does not belong to this movement".to_owned(),
    })?;
    if item.confirmed_location_id != request.expected_current_location_id {
        return Err(CustodyOpsError::WrongSource {
            cartridge_id: item.cartridge_id,
            expected_location_id: request.expected_current_location_id,
            actual_location_id: item.confirmed_location_id,
        });
    }

    let corrected_stage = if Some(request.confirmed_location_id) == task.source_location_id {
        MovementMediaStage::Planned
    } else if request.confirmed_location_id == task.destination_location_id {
        MovementMediaStage::Arrived
    } else if Some(request.confirmed_location_id) == task.transit_location_id {
        MovementMediaStage::Departed
    } else {
        MovementMediaStage::Exception
    };
    let needs_reservation = matches!(
        corrected_stage,
        MovementMediaStage::Planned | MovementMediaStage::Departed
    );
    match (&item.reserved_by, needs_reservation) {
        (Some(owner), _) if owner != &item.reservation_owner => {
            return Err(CustodyOpsError::ReservationConflict {
                cartridge_id: item.cartridge_id,
                reserved_by: owner.clone(),
            });
        }
        _ => {}
    }

    let new_version = task.confirmation_version + 1;
    let now = Utc::now();
    if needs_reservation {
        let updated = sqlx::query(
            "UPDATE cartridges
             SET confirmed_location_id = $1, reserved_by = $2, updated_at = $3
             WHERE id = $4 AND (reserved_by IS NULL OR reserved_by = $2)",
        )
        .bind(request.confirmed_location_id)
        .bind(&item.reservation_owner)
        .bind(now)
        .bind(item.cartridge_id)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(CustodyOpsError::ReservationConflict {
                cartridge_id: item.cartridge_id,
                reserved_by: "another operation".to_owned(),
            });
        }
    } else {
        sqlx::query(
            "UPDATE cartridges
             SET confirmed_location_id = $1,
                 reserved_by = CASE WHEN reserved_by = $2 THEN NULL ELSE reserved_by END,
                 updated_at = $3
             WHERE id = $4",
        )
        .bind(request.confirmed_location_id)
        .bind(&item.reservation_owner)
        .bind(now)
        .bind(item.cartridge_id)
        .execute(&mut *transaction)
        .await?;
    }

    let event_id = Uuid::now_v7();
    append_custody_event(
        &mut transaction,
        event_id,
        movement_task_id,
        &task.external_id,
        item.cartridge_id,
        &item.internal_label,
        &request.actor,
        "custody_corrected",
        request.expected_current_location_id,
        Some(request.confirmed_location_id),
        None,
        request.note.trim(),
        Some(request.corrects_event_id),
        json!({
            "corrected_event_type": target_event_type,
            "to_stage": corrected_stage,
            "confirmation_version": new_version,
            "physical_custody_changed": true,
            "idempotency_key": idempotency_key,
            "barcode": item.barcode,
        }),
        now,
        request.correlation_id,
    )
    .await?;
    sqlx::query(
        "UPDATE movement_task_media
         SET stage = $1, latest_custody_event_id = $2,
             last_confirmation_version = $3,
             departed_at = CASE
                 WHEN $1 = 'planned' THEN NULL
                 WHEN $1 = 'departed' THEN $4
                 ELSE departed_at END,
             arrived_at = CASE
                 WHEN $1 IN ('planned', 'departed') THEN NULL
                 WHEN $1 = 'arrived' THEN $4
                 ELSE arrived_at END,
             cancelled_at = NULL,
             updated_at = $4
         WHERE movement_task_id = $5 AND cartridge_id = $6",
    )
    .bind(corrected_stage.as_str())
    .bind(event_id)
    .bind(new_version)
    .bind(now)
    .bind(movement_task_id)
    .bind(item.cartridge_id)
    .execute(&mut *transaction)
    .await?;

    let derived_state =
        derive_task_state_in_transaction(&mut transaction, movement_task_id).await?;
    let resulting_state = preserve_overdue_state(&prior_task_state, derived_state);
    sqlx::query(
        "UPDATE movement_tasks
         SET state = $1, confirmation_version = $2, updated_at = $3
         WHERE id = $4",
    )
    .bind(resulting_state.as_str())
    .bind(new_version)
    .bind(now)
    .bind(movement_task_id)
    .execute(&mut *transaction)
    .await?;
    let operation_id = Uuid::now_v7();
    let affected_cartridge_ids = vec![item.cartridge_id];
    insert_operation_receipt(
        &mut transaction,
        operation_id,
        movement_task_id,
        &idempotency_key,
        "custody_corrected",
        &request_fingerprint,
        request.expected_confirmation_version,
        new_version,
        &resulting_state,
        &affected_cartridge_ids,
        &request.actor,
        request.correlation_id,
        json!({
            "corrects_event_id": request.corrects_event_id,
            "custody_event_id": event_id,
            "confirmed_location_id": request.confirmed_location_id,
        }),
    )
    .await?;
    append_audit(
        &mut transaction,
        &request.actor,
        "movement.custody_corrected",
        "movement_task",
        &movement_task_id.to_string(),
        json!({
            "cartridge_id": item.cartridge_id,
            "corrects_event_id": request.corrects_event_id,
            "expected_current_location_id": request.expected_current_location_id,
            "confirmed_location_id": request.confirmed_location_id,
            "expected_confirmation_version": request.expected_confirmation_version,
            "idempotency_key": idempotency_key,
            "physically_confirmed": true,
            "note": request.note.trim(),
        }),
        resulting_state.as_str(),
        request.correlation_id,
    )
    .await?;
    append_outbox(
        &mut transaction,
        format!("movement:{movement_task_id}:operation:{operation_id}"),
        json!({
            "event_type": "movement.custody_corrected",
            "movement_task_id": movement_task_id,
            "operation_id": operation_id,
            "custody_event_id": event_id,
            "state": resulting_state,
            "confirmation_version": new_version,
            "cartridge_ids": affected_cartridge_ids,
            "correlation_id": request.correlation_id,
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(MovementMutationResult {
        movement_task_id,
        operation_id,
        state: resulting_state,
        confirmation_version: new_version,
        affected_cartridge_ids,
        idempotent_replay: false,
    })
}

/// Reconcile due active movements into the overdue state.  This never changes
/// cartridge custody and therefore does not append a physical custody event.
pub async fn reconcile_overdue_movements(
    pool: &PgPool,
    request: ReconcileOverdueRequest,
) -> Result<OverdueReconciliationResult, CustodyOpsError> {
    validate_actor(&request.actor)?;
    let mut transaction = pool.begin().await?;
    let tasks = sqlx::query_as::<_, (Uuid, String, DateTime<Utc>, i64)>(
        "SELECT id, external_id, due_at, confirmation_version
         FROM movement_tasks
         WHERE due_at < $1
           AND state IN ('planned', 'ready', 'departed', 'partially_confirmed')
         ORDER BY due_at, id
         FOR UPDATE SKIP LOCKED",
    )
    .bind(request.as_of)
    .fetch_all(&mut *transaction)
    .await?;

    let mut newly_overdue_task_ids = Vec::with_capacity(tasks.len());
    for (task_id, external_id, due_at, confirmation_version) in tasks {
        sqlx::query("UPDATE movement_tasks SET state = 'overdue', updated_at = $1 WHERE id = $2")
            .bind(request.as_of)
            .bind(task_id)
            .execute(&mut *transaction)
            .await?;
        append_audit(
            &mut transaction,
            &request.actor,
            "movement.overdue",
            "movement_task",
            &task_id.to_string(),
            json!({
                "as_of": request.as_of,
                "due_at": due_at,
                "confirmation_version": confirmation_version,
            }),
            "overdue",
            request.correlation_id,
        )
        .await?;
        append_outbox(
            &mut transaction,
            format!("movement:{task_id}:overdue:v{confirmation_version}"),
            json!({
                "event_type": "movement.overdue",
                "movement_task_id": task_id,
                "external_id": external_id,
                "due_at": due_at,
                "as_of": request.as_of,
                "confirmation_version": confirmation_version,
                "correlation_id": request.correlation_id,
            }),
        )
        .await?;
        newly_overdue_task_ids.push(task_id);
    }
    transaction.commit().await?;
    Ok(OverdueReconciliationResult {
        as_of: request.as_of,
        newly_overdue_task_ids,
    })
}

fn validate_confirmation_request(
    request: &PhysicalConfirmationRequest,
) -> Result<(), CustodyOpsError> {
    validate_nonempty_text("idempotency_key", &request.idempotency_key, 256)?;
    validate_actor(&request.actor)?;
    if request.expected_confirmation_version < 0 {
        return Err(CustodyOpsError::Invalid {
            field: "expected_confirmation_version",
            message: "must not be negative".to_owned(),
        });
    }
    if !request.physically_confirmed {
        return Err(CustodyOpsError::Invalid {
            field: "physically_confirmed",
            message: "must be true before confirmed custody can change".to_owned(),
        });
    }
    if request.note.len() > 4096 {
        return Err(CustodyOpsError::Invalid {
            field: "note",
            message: "must be at most 4096 bytes".to_owned(),
        });
    }
    if request.note.contains('\0') {
        return Err(CustodyOpsError::Invalid {
            field: "note",
            message: "must not contain a NUL character".to_owned(),
        });
    }
    Ok(())
}

async fn require_location(
    transaction: &mut Transaction<'_, Postgres>,
    location_id: Uuid,
) -> Result<LocationKind, CustodyOpsError> {
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM locations WHERE id = $1")
        .bind(location_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| CustodyOpsError::NotFound {
            resource: "location",
            id: location_id.to_string(),
        })?;
    LocationKind::parse(&kind)
}

async fn lock_movement(
    transaction: &mut Transaction<'_, Postgres>,
    movement_task_id: Uuid,
) -> Result<MovementHeaderRow, CustodyOpsError> {
    sqlx::query_as::<_, MovementHeaderRow>(
        "SELECT external_id, kind, source_location_id, destination_location_id,
                transit_location_id, state, confirmation_version
         FROM movement_tasks
         WHERE id = $1
         FOR UPDATE",
    )
    .bind(movement_task_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or_else(|| CustodyOpsError::NotFound {
        resource: "movement task",
        id: movement_task_id.to_string(),
    })
}

async fn lock_selected_media(
    transaction: &mut Transaction<'_, Postgres>,
    movement_task_id: Uuid,
    cartridge_ids: &[Uuid],
) -> Result<Vec<LockedMediaRow>, CustodyOpsError> {
    sqlx::query_as::<_, LockedMediaRow>(
        "SELECT media.cartridge_id, cartridge.internal_label, cartridge.barcode,
                media.stage, media.expected_source_location_id,
                media.destination_location_id, media.reservation_owner,
                cartridge.reserved_by, cartridge.confirmed_location_id
         FROM movement_task_media AS media
         JOIN cartridges AS cartridge ON cartridge.id = media.cartridge_id
         WHERE media.movement_task_id = $1
           AND media.cartridge_id = ANY($2::uuid[])
         ORDER BY media.cartridge_id
         FOR UPDATE OF media, cartridge",
    )
    .bind(movement_task_id)
    .bind(cartridge_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(Into::into)
}

async fn lock_all_media(
    transaction: &mut Transaction<'_, Postgres>,
    movement_task_id: Uuid,
) -> Result<Vec<LockedMediaRow>, CustodyOpsError> {
    sqlx::query_as::<_, LockedMediaRow>(
        "SELECT media.cartridge_id, cartridge.internal_label, cartridge.barcode,
                media.stage, media.expected_source_location_id,
                media.destination_location_id, media.reservation_owner,
                cartridge.reserved_by, cartridge.confirmed_location_id
         FROM movement_task_media AS media
         JOIN cartridges AS cartridge ON cartridge.id = media.cartridge_id
         WHERE media.movement_task_id = $1
         ORDER BY media.cartridge_id
         FOR UPDATE OF media, cartridge",
    )
    .bind(movement_task_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(Into::into)
}

fn require_confirmation_version(
    task: &MovementHeaderRow,
    expected: i64,
) -> Result<(), CustodyOpsError> {
    if task.confirmation_version != expected {
        return Err(CustodyOpsError::StaleVersion {
            expected,
            actual: task.confirmation_version,
        });
    }
    Ok(())
}

async fn find_operation_receipt(
    transaction: &mut Transaction<'_, Postgres>,
    movement_task_id: Uuid,
    idempotency_key: &str,
) -> Result<Option<OperationReceiptRow>, CustodyOpsError> {
    sqlx::query_as::<_, OperationReceiptRow>(
        "SELECT id, request_fingerprint, resulting_confirmation_version,
                resulting_task_state, affected_media_ids
         FROM movement_task_operations
         WHERE movement_task_id = $1 AND idempotency_key = $2",
    )
    .bind(movement_task_id)
    .bind(idempotency_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(Into::into)
}

fn ensure_idempotent_match(
    receipt: &OperationReceiptRow,
    request_fingerprint: &str,
    idempotency_key: &str,
) -> Result<(), CustodyOpsError> {
    if receipt.request_fingerprint != request_fingerprint {
        return Err(CustodyOpsError::IdempotencyConflict {
            key: idempotency_key.to_owned(),
        });
    }
    Ok(())
}

fn mutation_result_from_receipt(
    movement_task_id: Uuid,
    receipt: OperationReceiptRow,
    idempotent_replay: bool,
) -> Result<MovementMutationResult, CustodyOpsError> {
    Ok(MovementMutationResult {
        movement_task_id,
        operation_id: receipt.id,
        state: MovementState::parse(&receipt.resulting_task_state)?,
        confirmation_version: receipt.resulting_confirmation_version,
        affected_cartridge_ids: receipt.affected_media_ids,
        idempotent_replay,
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_operation_receipt(
    transaction: &mut Transaction<'_, Postgres>,
    operation_id: Uuid,
    movement_task_id: Uuid,
    idempotency_key: &str,
    operation_kind: &str,
    request_fingerprint: &str,
    expected_confirmation_version: i64,
    resulting_confirmation_version: i64,
    resulting_state: &MovementState,
    affected_media_ids: &[Uuid],
    actor: &CustodyActor,
    correlation_id: Uuid,
    result: Value,
) -> Result<(), CustodyOpsError> {
    sqlx::query(
        "INSERT INTO movement_task_operations (
            id, movement_task_id, idempotency_key, operation_kind,
            request_fingerprint, expected_confirmation_version,
            resulting_confirmation_version, resulting_task_state,
            affected_media_ids, actor_id, actor_label, correlation_id, result
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(operation_id)
    .bind(movement_task_id)
    .bind(idempotency_key)
    .bind(operation_kind)
    .bind(request_fingerprint)
    .bind(expected_confirmation_version)
    .bind(resulting_confirmation_version)
    .bind(resulting_state.as_str())
    .bind(affected_media_ids)
    .bind(actor.user_id)
    .bind(actor.label.trim())
    .bind(correlation_id)
    .bind(result)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn derive_task_state_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    movement_task_id: Uuid,
) -> Result<MovementState, CustodyOpsError> {
    let stages = sqlx::query_scalar::<_, String>(
        "SELECT stage
         FROM movement_task_media
         WHERE movement_task_id = $1
         ORDER BY cartridge_id",
    )
    .bind(movement_task_id)
    .fetch_all(&mut **transaction)
    .await?;
    let parsed = stages
        .iter()
        .map(|stage| MovementMediaStage::parse(stage))
        .collect::<Result<Vec<_>, _>>()?;
    derive_task_state(&parsed)
}

fn derive_task_state(stages: &[MovementMediaStage]) -> Result<MovementState, CustodyOpsError> {
    if stages.is_empty() {
        return Err(CustodyOpsError::Invariant(
            "movement has no normalized media".to_owned(),
        ));
    }
    if stages
        .iter()
        .any(|stage| stage == &MovementMediaStage::Exception)
    {
        return Ok(MovementState::Blocked);
    }
    if stages
        .iter()
        .all(MovementMediaStage::is_confirmed_destination)
    {
        return Ok(MovementState::Confirmed);
    }
    if stages
        .iter()
        .all(|stage| stage == &MovementMediaStage::Cancelled)
    {
        return Ok(MovementState::Cancelled);
    }
    if stages
        .iter()
        .all(|stage| stage == &MovementMediaStage::Planned)
    {
        return Ok(MovementState::Ready);
    }
    if stages
        .iter()
        .all(|stage| stage == &MovementMediaStage::Departed)
    {
        return Ok(MovementState::Departed);
    }
    Ok(MovementState::PartiallyConfirmed)
}

fn preserve_overdue_state(
    prior_state: &MovementState,
    derived_state: MovementState,
) -> MovementState {
    if prior_state == &MovementState::Overdue
        && !matches!(
            derived_state,
            MovementState::Confirmed | MovementState::Cancelled
        )
    {
        MovementState::Overdue
    } else {
        derived_state
    }
}

#[allow(clippy::too_many_arguments)]
async fn append_custody_event(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    movement_task_id: Uuid,
    movement_task_external_id: &str,
    cartridge_id: Uuid,
    internal_label: &str,
    actor: &CustodyActor,
    event_type: &str,
    source_location_id: Option<Uuid>,
    destination_location_id: Option<Uuid>,
    destination_label: Option<&str>,
    note: &str,
    corrects_event_id: Option<Uuid>,
    evidence: Value,
    occurred_at: DateTime<Utc>,
    correlation_id: Uuid,
) -> Result<(), CustodyOpsError> {
    sqlx::query(
        "INSERT INTO custody_events (
            id, movement_task_id, movement_task_external_id, cartridge_id,
            actor_id, actor_label, event_type, source_location_id,
            destination_location_id, destination_label, tape_ids, note,
            corrects_event_id, evidence, occurred_at, correlation_id
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                   $11, $12, $13, $14, $15, $16)",
    )
    .bind(event_id)
    .bind(movement_task_id)
    .bind(movement_task_external_id)
    .bind(cartridge_id)
    .bind(actor.user_id)
    .bind(actor.label.trim())
    .bind(event_type)
    .bind(source_location_id)
    .bind(destination_location_id)
    .bind(destination_label)
    .bind(vec![internal_label.to_owned()])
    .bind(note)
    .bind(corrects_event_id)
    .bind(evidence)
    .bind(occurred_at)
    .bind(correlation_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn append_audit(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CustodyActor,
    action: &str,
    object_type: &str,
    object_id: &str,
    requested: Value,
    result: &str,
    correlation_id: Uuid,
) -> Result<(), CustodyOpsError> {
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
) -> Result<(), CustodyOpsError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> CustodyActor {
        CustodyActor {
            user_id: None,
            label: "tape operator".to_owned(),
        }
    }

    #[test]
    fn exact_media_set_rejects_empty_and_duplicate_ids() {
        let tape = Uuid::now_v7();
        assert!(matches!(
            normalize_cartridge_ids(&[], "cartridge_ids"),
            Err(CustodyOpsError::Invalid {
                field: "cartridge_ids",
                ..
            })
        ));
        assert!(matches!(
            normalize_cartridge_ids(&[tape, tape], "cartridge_ids"),
            Err(CustodyOpsError::Invalid {
                field: "cartridge_ids",
                ..
            })
        ));
    }

    #[test]
    fn exact_media_set_is_sorted_for_locks_and_fingerprints() {
        let low = Uuid::from_u128(1);
        let high = Uuid::from_u128(2);
        assert_eq!(
            normalize_cartridge_ids(&[high, low], "cartridge_ids").unwrap(),
            vec![low, high]
        );
    }

    #[test]
    fn unknown_source_without_transit_is_a_valid_onboarding_shape() {
        let destination = Uuid::from_u128(1);
        assert!(validate_movement_location_shape(None, destination, None).is_ok());
    }

    #[test]
    fn transit_must_be_distinct_from_present_endpoints() {
        let source = Uuid::from_u128(1);
        let destination = Uuid::from_u128(2);
        let transit = Uuid::from_u128(3);
        assert!(validate_movement_location_shape(Some(source), destination, None).is_ok());
        assert!(validate_movement_location_shape(Some(source), destination, Some(transit)).is_ok());
        assert!(matches!(
            validate_movement_location_shape(Some(source), source, None),
            Err(CustodyOpsError::Invalid {
                field: "destination_location_id",
                ..
            })
        ));
        assert!(matches!(
            validate_movement_location_shape(Some(source), destination, Some(source)),
            Err(CustodyOpsError::Invalid {
                field: "transit_location_id",
                ..
            })
        ));
        assert!(matches!(
            validate_movement_location_shape(Some(source), destination, Some(destination)),
            Err(CustodyOpsError::Invalid {
                field: "transit_location_id",
                ..
            })
        ));
    }

    #[test]
    fn partial_delivery_never_derives_confirmed_state() {
        let stages = [MovementMediaStage::Arrived, MovementMediaStage::Departed];
        assert_eq!(
            derive_task_state(&stages).unwrap(),
            MovementState::PartiallyConfirmed
        );
        let stages = [
            MovementMediaStage::DirectConfirmed,
            MovementMediaStage::Planned,
        ];
        assert_eq!(
            derive_task_state(&stages).unwrap(),
            MovementState::PartiallyConfirmed
        );
    }

    #[test]
    fn homogeneous_media_stages_derive_expected_task_states() {
        assert_eq!(
            derive_task_state(&[MovementMediaStage::Planned]).unwrap(),
            MovementState::Ready
        );
        assert_eq!(
            derive_task_state(&[MovementMediaStage::Departed]).unwrap(),
            MovementState::Departed
        );
        assert_eq!(
            derive_task_state(&[
                MovementMediaStage::Arrived,
                MovementMediaStage::DirectConfirmed,
            ])
            .unwrap(),
            MovementState::Confirmed
        );
        assert_eq!(
            derive_task_state(&[MovementMediaStage::Cancelled]).unwrap(),
            MovementState::Cancelled
        );
        assert_eq!(
            derive_task_state(&[MovementMediaStage::Arrived, MovementMediaStage::Exception,])
                .unwrap(),
            MovementState::Blocked
        );
    }

    #[test]
    fn confirmation_requires_an_explicit_physical_fact() {
        let request = PhysicalConfirmationRequest {
            cartridge_ids: vec![Uuid::now_v7()],
            idempotency_key: "scan-1".to_owned(),
            expected_confirmation_version: 0,
            physically_confirmed: false,
            note: String::new(),
            actor: actor(),
            correlation_id: Uuid::now_v7(),
        };
        assert!(matches!(
            validate_confirmation_request(&request),
            Err(CustodyOpsError::Invalid {
                field: "physically_confirmed",
                ..
            })
        ));
    }

    #[test]
    fn request_fingerprint_is_stable_and_sensitive() {
        let first = fingerprint(&json!({
            "operation": "arrival_confirmed",
            "cartridge_ids": [Uuid::from_u128(1), Uuid::from_u128(2)],
            "version": 2,
        }))
        .unwrap();
        let retry = fingerprint(&json!({
            "operation": "arrival_confirmed",
            "cartridge_ids": [Uuid::from_u128(1), Uuid::from_u128(2)],
            "version": 2,
        }))
        .unwrap();
        let changed = fingerprint(&json!({
            "operation": "arrival_confirmed",
            "cartridge_ids": [Uuid::from_u128(1), Uuid::from_u128(2)],
            "version": 3,
        }))
        .unwrap();
        assert_eq!(first, retry);
        assert_ne!(first, changed);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn movement_and_location_database_values_are_strictly_parsed() {
        assert_eq!(
            LocationKind::parse("in_transit").unwrap(),
            LocationKind::InTransit
        );
        assert_eq!(
            MovementState::parse("partially_confirmed").unwrap(),
            MovementState::PartiallyConfirmed
        );
        assert!(matches!(
            MovementMediaStage::parse("teleported"),
            Err(CustodyOpsError::Invariant(_))
        ));
    }

    #[test]
    fn overdue_remains_sticky_until_full_confirmation_or_cancellation() {
        assert_eq!(
            preserve_overdue_state(&MovementState::Overdue, MovementState::Departed),
            MovementState::Overdue
        );
        assert_eq!(
            preserve_overdue_state(&MovementState::Overdue, MovementState::PartiallyConfirmed),
            MovementState::Overdue
        );
        assert_eq!(
            preserve_overdue_state(&MovementState::Overdue, MovementState::Blocked),
            MovementState::Overdue
        );
        assert_eq!(
            preserve_overdue_state(&MovementState::Overdue, MovementState::Confirmed),
            MovementState::Confirmed
        );
        assert_eq!(
            preserve_overdue_state(&MovementState::Overdue, MovementState::Cancelled),
            MovementState::Cancelled
        );
    }
}
