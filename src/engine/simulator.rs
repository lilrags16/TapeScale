use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{
    BackupRequest, EngineCapabilities, EngineError, EngineIdentity, EngineOperation,
    OperationState, QualificationStatus, TapeEngine,
};

/// Deterministic development adapter. Its evidence can never satisfy physical qualification.
#[derive(Clone, Default)]
pub struct SimulatorEngine {
    operations: Arc<RwLock<HashMap<Uuid, EngineOperation>>>,
}

impl SimulatorEngine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TapeEngine for SimulatorEngine {
    async fn identity(&self) -> Result<EngineIdentity, EngineError> {
        Ok(EngineIdentity {
            adapter: "tapescale-simulator".to_owned(),
            adapter_version: env!("CARGO_PKG_VERSION").to_owned(),
            engine_name: "Deterministic safety simulator".to_owned(),
            engine_version: "1".to_owned(),
            qualification: QualificationStatus::Simulated,
            format_selected: false,
        })
    }

    async fn capabilities(&self) -> Result<EngineCapabilities, EngineError> {
        Ok(EngineCapabilities {
            file_backup: true,
            native_stream: true,
            multi_volume: true,
            full_readback: true,
            changer_control: true,
            catalog_rebuild: true,
            evidence: [
                (
                    "scope".to_owned(),
                    "control-plane simulation only".to_owned(),
                ),
                ("physical_hardware".to_owned(), "not tested".to_owned()),
                ("format".to_owned(), "not selected".to_owned()),
            ]
            .into(),
        })
    }

    async fn start_backup(&self, request: BackupRequest) -> Result<EngineOperation, EngineError> {
        if request.media_ids.is_empty() {
            return Err(EngineError::Blocked(
                "at least one reconciled compatible tape is required".to_owned(),
            ));
        }
        if request.logical_root.is_empty() || !request.logical_root.starts_with('/') {
            return Err(EngineError::Blocked(
                "the logical source root must be an absolute catalog path".to_owned(),
            ));
        }
        let now = Utc::now();
        let operation = EngineOperation {
            id: Uuid::now_v7(),
            active_media_id: request.media_ids.first().copied(),
            request,
            state: OperationState::Preparing,
            logical_bytes: 0,
            encoded_bytes: 0,
            started_at: now,
            updated_at: now,
            detail: "Simulator accepted the operation; no physical write occurred.".to_owned(),
        };
        self.operations
            .write()
            .await
            .insert(operation.id, operation.clone());
        Ok(operation)
    }

    async fn poll(&self, operation_id: Uuid) -> Result<EngineOperation, EngineError> {
        let mut operations = self.operations.write().await;
        let operation = operations
            .get_mut(&operation_id)
            .ok_or(EngineError::NotFound)?;
        operation.updated_at = Utc::now();
        match operation.state {
            OperationState::Preparing => {
                operation.state = OperationState::Writing;
                operation.detail = "Simulating bounded record writes.".to_owned();
            }
            OperationState::Writing => {
                operation.logical_bytes = 2_684_354_560;
                operation.encoded_bytes = 1_912_602_624;
                operation.state = OperationState::Written;
                operation.detail =
                    "Simulated records finalized. Physical evidence: not run.".to_owned();
            }
            _ => {}
        }
        Ok(operation.clone())
    }

    async fn request_safe_cancel(
        &self,
        operation_id: Uuid,
    ) -> Result<EngineOperation, EngineError> {
        let mut operations = self.operations.write().await;
        let operation = operations
            .get_mut(&operation_id)
            .ok_or(EngineError::NotFound)?;
        operation.state = OperationState::Cancelled;
        operation.updated_at = Utc::now();
        operation.detail =
            "Cancelled at a simulator record boundary; prior recovery points preserved.".to_owned();
        Ok(operation.clone())
    }

    async fn readback_verify(&self, operation_id: Uuid) -> Result<EngineOperation, EngineError> {
        let mut operations = self.operations.write().await;
        let operation = operations
            .get_mut(&operation_id)
            .ok_or(EngineError::NotFound)?;
        if operation.state != OperationState::Written {
            return Err(EngineError::Blocked(
                "read-back requires a finalized written operation".to_owned(),
            ));
        }
        operation.state = OperationState::Verified;
        operation.updated_at = Utc::now();
        operation.detail =
            "Simulator read-back passed; this is not physical verification evidence.".to_owned();
        Ok(operation.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Representation, TapeEngine};

    fn request() -> BackupRequest {
        BackupRequest {
            job_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            snapshot_id: "snap-guid-01".to_owned(),
            logical_root: "/tank/production".to_owned(),
            representation: Representation::File,
            media_ids: vec![Uuid::new_v4()],
            correlation_id: Uuid::new_v4(),
        }
    }

    #[tokio::test]
    async fn simulator_can_never_report_physical_qualification() {
        let engine = SimulatorEngine::new();
        assert_eq!(
            engine.identity().await.expect("identity").qualification,
            QualificationStatus::Simulated
        );
    }

    #[tokio::test]
    async fn verification_requires_written_state() {
        let engine = SimulatorEngine::new();
        let operation = engine.start_backup(request()).await.expect("operation");
        assert!(matches!(
            engine.readback_verify(operation.id).await,
            Err(EngineError::Blocked(_))
        ));
    }
}
