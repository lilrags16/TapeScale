use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod bacula;
pub mod simulator;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualificationStatus {
    Simulated,
    DetectedUnqualified,
    PhysicallyQualified,
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineIdentity {
    pub adapter: String,
    pub adapter_version: String,
    pub engine_name: String,
    pub engine_version: String,
    pub qualification: QualificationStatus,
    pub format_selected: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineCapabilities {
    pub file_backup: bool,
    pub native_stream: bool,
    pub multi_volume: bool,
    pub full_readback: bool,
    pub changer_control: bool,
    pub catalog_rebuild: bool,
    pub evidence: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BackupRequest {
    pub job_id: Uuid,
    pub source_id: Uuid,
    pub snapshot_id: String,
    pub logical_root: String,
    pub representation: Representation,
    pub media_ids: Vec<Uuid>,
    pub correlation_id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
    File,
    NativeZfs,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Preparing,
    WaitingForMedia,
    Writing,
    Written,
    Verifying,
    Verified,
    ReconciliationRequired,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineOperation {
    pub id: Uuid,
    pub request: BackupRequest,
    pub state: OperationState,
    pub logical_bytes: u64,
    pub encoded_bytes: u64,
    pub active_media_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub detail: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine operation was not found")]
    NotFound,
    #[error("engine operation is blocked: {0}")]
    Blocked(String),
    #[error("engine integration failed: {0}")]
    Integration(String),
}

#[async_trait]
pub trait TapeEngine: Send + Sync {
    async fn identity(&self) -> Result<EngineIdentity, EngineError>;
    async fn capabilities(&self) -> Result<EngineCapabilities, EngineError>;
    async fn start_backup(&self, request: BackupRequest) -> Result<EngineOperation, EngineError>;
    async fn poll(&self, operation_id: Uuid) -> Result<EngineOperation, EngineError>;
    async fn request_safe_cancel(&self, operation_id: Uuid)
        -> Result<EngineOperation, EngineError>;
    async fn readback_verify(&self, operation_id: Uuid) -> Result<EngineOperation, EngineError>;
}
