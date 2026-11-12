use std::{collections::BTreeSet, path::Path};

use anyhow::Result;
use clap::Parser;
use serde_json::{json, Value};
use tapescale::{
    config::Config,
    device::{
        discover_sas_drives, execute_sas_motion, SasDiscoveryConfig, SasMotionError,
        SasMotionOperation, SasProbeResult,
    },
    engine::{simulator::SimulatorEngine, QualificationStatus, TapeEngine},
    store,
    tool_runner::ToolError,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "tapescale-worker",
    version,
    about = "TapeScale durable orchestration worker"
)]
struct Arguments {
    /// Perform one reconciliation pass and exit.
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let arguments = Arguments::parse();
    let config = Config::from_env()?;
    let pool = match &config.database_url {
        Some(url) => Some(store::connect(url, config.database_connect_timeout).await?),
        None if config.demo_mode => None,
        None => anyhow::bail!("DATABASE_URL is required outside simulation mode"),
    };
    let sas_config = SasDiscoveryConfig::from_env()?;
    let mut worker_singleton_lock = match pool.as_ref() {
        Some(pool) => Some(store::acquire_worker_singleton(pool).await?),
        None => None,
    };
    if sas_config.is_enabled() {
        info!(
            configured_devices = sas_config.devices.len(),
            "SAS tape discovery enabled"
        );
    }
    if config.lab_tape_motion_enabled {
        warn!("unqualified lab tape motion is enabled; only rewind and unload are allowlisted");
    }
    let engine = SimulatorEngine::new();
    let worker_id = Uuid::now_v7();
    let shutdown = CancellationToken::new();
    let shutdown_listener = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_listener.cancel();
    });

    loop {
        if shutdown.is_cancelled() {
            break;
        }
        reconcile_once(
            pool.as_ref(),
            &engine,
            &sas_config,
            config.lab_tape_motion_enabled,
            worker_id,
            &shutdown,
        )
        .await?;
        if arguments.once {
            break;
        }
        tokio::select! {
            () = tokio::time::sleep(config.worker_poll_interval) => {},
            () = shutdown.cancelled() => {
                info!("worker drain requested; no new operation will be claimed");
                break;
            }
        }
    }
    if let Some(connection) = worker_singleton_lock.as_mut() {
        store::release_worker_singleton(connection).await?;
    }
    Ok(())
}

async fn reconcile_once(
    pool: Option<&sqlx::PgPool>,
    engine: &impl TapeEngine,
    sas_config: &SasDiscoveryConfig,
    lab_tape_motion_enabled: bool,
    worker_id: Uuid,
    cancellation: &CancellationToken,
) -> Result<()> {
    let identity = engine.identity().await?;
    if identity.qualification != QualificationStatus::PhysicallyQualified {
        warn!(
            qualification = ?identity.qualification,
            "engine is not physically qualified; tape data operations remain blocked"
        );
    }

    reconcile_sas_devices(pool, sas_config).await?;

    if let Some(pool) = pool {
        process_drive_operations(
            pool,
            sas_config,
            lab_tape_motion_enabled,
            worker_id,
            cancellation,
        )
        .await?;
        let queued = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM job_runs WHERE state IN ('queued', 'blocked', 'interrupted') AND finished_at IS NULL",
        )
        .fetch_one(pool)
        .await?;
        info!(queued_runs = queued, "durable queue reconciled");
    } else {
        info!("simulation reconciliation complete; no durable queue configured");
    }
    Ok(())
}

async fn reconcile_sas_devices(
    pool: Option<&sqlx::PgPool>,
    config: &SasDiscoveryConfig,
) -> Result<()> {
    if !config.is_enabled() {
        return Ok(());
    }
    let blocked_paths = match pool {
        Some(pool) => store::running_drive_operation_paths(pool)
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>(),
        None => BTreeSet::new(),
    };
    let mut discovery_config = config.clone();
    discovery_config
        .devices
        .retain(|path| !blocked_paths.contains(&path.display().to_string()));
    for path in blocked_paths {
        warn!(device = %path, "status discovery deferred while a tape-motion operation is running");
    }
    let results = discover_sas_drives(&discovery_config).await;
    persist_sas_results(pool, &results).await
}

async fn persist_sas_results(
    pool: Option<&sqlx::PgPool>,
    results: &[SasProbeResult],
) -> Result<()> {
    for result in results {
        match result {
            SasProbeResult::Observed(observation) => {
                info!(
                    stable_id = %observation.stable_id,
                    device = %observation.observed_path,
                    status = %observation.status,
                    identity_verified = observation.identity_verified,
                    identity_conflict = observation.identity_conflict,
                    "SAS tape drive observed"
                );
                if let Some(pool) = pool {
                    store::record_drive_observation(pool, observation).await?;
                }
            }
            SasProbeResult::Missing(missing) => {
                warn!(
                    device = %missing.observed_path,
                    reason = %missing.reason,
                    "configured SAS tape drive is unavailable"
                );
                if let Some(pool) = pool {
                    store::mark_drive_missing(pool, missing).await?;
                }
            }
        }
    }
    Ok(())
}

async fn process_drive_operations(
    pool: &sqlx::PgPool,
    config: &SasDiscoveryConfig,
    enabled: bool,
    worker_id: Uuid,
    cancellation: &CancellationToken,
) -> Result<()> {
    let expired = store::recover_expired_drive_operations(pool).await?;
    if expired > 0 {
        warn!(
            expired_operations = expired,
            "expired tape-motion leases were blocked for reconciliation"
        );
    }
    if !enabled {
        let blocked = store::block_queued_drive_operations(
            pool,
            "unqualified lab tape motion was disabled before worker execution",
        )
        .await?;
        if blocked > 0 {
            warn!(
                blocked_operations = blocked,
                "queued tape motion was blocked"
            );
        }
        return Ok(());
    }

    for _ in 0..config.devices.len().max(1) {
        if cancellation.is_cancelled() {
            break;
        }
        match store::claim_next_drive_operation(pool, worker_id).await? {
            store::ClaimDriveOperationResult::Claimed(claim) => {
                execute_claimed_drive_operation(pool, config, &claim, cancellation).await?;
            }
            store::ClaimDriveOperationResult::Blocked { id, reason } => {
                warn!(operation_id = %id, %reason, "tape motion was blocked during worker revalidation");
            }
            store::ClaimDriveOperationResult::Empty => break,
        }
    }
    Ok(())
}

async fn execute_claimed_drive_operation(
    pool: &sqlx::PgPool,
    config: &SasDiscoveryConfig,
    claim: &store::ClaimedDriveOperation,
    cancellation: &CancellationToken,
) -> Result<()> {
    if !claim.operation.scratch_media_confirmed {
        finish_motion(
            pool,
            claim,
            store::DriveOperationTerminalState::Blocked,
            json!({
                "detail": "the persisted request does not confirm designated scratch media; execution is denied",
                "position_uncertain": false
            }),
        )
        .await?;
        return Ok(());
    }

    let operation = match claim.operation.action.as_str() {
        "rewind" => SasMotionOperation::Rewind,
        "unload" => SasMotionOperation::Unload,
        other => {
            finish_motion(
                pool,
                claim,
                store::DriveOperationTerminalState::Blocked,
                json!({
                    "detail": format!("unsupported persisted tape-motion action: {other}"),
                    "position_uncertain": false
                }),
            )
            .await?;
            return Ok(());
        }
    };

    let before = discover_sas_drives(config).await;
    persist_sas_results(Some(pool), &before).await?;
    let before_observation = matching_observation(
        &before,
        &claim.operation.expected_path,
        &claim.operation.expected_stable_id,
    );
    let Some(before_observation) = before_observation else {
        finish_motion(
            pool,
            claim,
            store::DriveOperationTerminalState::Blocked,
            json!({
                "detail": "the exact configured drive identity was not observed immediately before motion",
                "position_uncertain": false
            }),
        )
        .await?;
        return Ok(());
    };
    if let Some(reason) = observation_motion_blocker(before_observation, &claim.operation) {
        finish_motion(
            pool,
            claim,
            store::DriveOperationTerminalState::Blocked,
            json!({
                "detail": reason,
                "position_uncertain": false,
                "before": before_observation.health()
            }),
        )
        .await?;
        return Ok(());
    }

    if !store::renew_drive_operation_lease(pool, claim).await? {
        store::recover_expired_drive_operations(pool).await?;
        warn!(
            operation_id = %claim.operation.id,
            "tape-motion lease expired before command dispatch; no command was executed"
        );
        return Ok(());
    }
    if cancellation.is_cancelled() {
        finish_motion(
            pool,
            claim,
            store::DriveOperationTerminalState::Blocked,
            json!({
                "detail": "worker shutdown was requested before command dispatch",
                "position_uncertain": false
            }),
        )
        .await?;
        return Ok(());
    }

    info!(
        operation_id = %claim.operation.id,
        drive_id = %claim.operation.drive_id,
        stable_id = %claim.operation.expected_stable_id,
        action = operation.as_str(),
        fencing_token = claim.fencing_token,
        "executing allowlisted lab tape motion"
    );
    let execution = execute_sas_motion(
        config,
        Path::new(&claim.operation.expected_path),
        operation,
        cancellation,
    )
    .await;

    if matches!(
        &execution,
        Err(SasMotionError::Tool(ToolError::Cancelled { .. }))
    ) {
        finish_motion(
            pool,
            claim,
            store::DriveOperationTerminalState::Failed,
            json!({
                "detail": "worker shutdown interrupted tape motion; physical position requires reconciliation",
                "position_uncertain": true,
                "before": before_observation.health()
            }),
        )
        .await?;
        return Ok(());
    }

    let lease_renewed_after_execution = store::renew_drive_operation_lease(pool, claim).await?;

    let after = discover_sas_drives(config).await;
    persist_sas_results(Some(pool), &after).await?;
    let after_observation = matching_observation(
        &after,
        &claim.operation.expected_path,
        &claim.operation.expected_stable_id,
    );

    let (state, result) = match execution {
        Err(error) => (
            store::DriveOperationTerminalState::Failed,
            json!({
                "detail": error.to_string(),
                "position_uncertain": true,
                "before": before_observation.health(),
                "after": after_observation.map(|observation| observation.health())
            }),
        ),
        Ok(output) if !lease_renewed_after_execution => (
            store::DriveOperationTerminalState::Failed,
            json!({
                "detail": "the drive lease expired before the motion result could be committed",
                "position_uncertain": true,
                "before": before_observation.health(),
                "after": after_observation.map(|observation| observation.health()),
                "tool": output
            }),
        ),
        Ok(output) => {
            match verified_motion_postcondition(operation, after_observation, &claim.operation) {
                Ok(after_health) => (
                    store::DriveOperationTerminalState::Succeeded,
                    json!({
                        "detail": format!("{} completed with a verified postcondition", operation.as_str()),
                        "position_uncertain": false,
                        "before": before_observation.health(),
                        "after": after_health,
                        "tool": output
                    }),
                ),
                Err(reason) => (
                    store::DriveOperationTerminalState::Failed,
                    json!({
                        "detail": reason,
                        "position_uncertain": true,
                        "before": before_observation.health(),
                        "after": after_observation.map(|observation| observation.health()),
                        "tool": output
                    }),
                ),
            }
        }
    };
    finish_motion(pool, claim, state, result).await?;
    Ok(())
}

fn matching_observation<'a>(
    results: &'a [SasProbeResult],
    expected_path: &str,
    expected_stable_id: &str,
) -> Option<&'a tapescale::device::SasDriveObservation> {
    results.iter().find_map(|result| match result {
        SasProbeResult::Observed(observation)
            if observation.observed_path == expected_path
                && observation.stable_id == expected_stable_id =>
        {
            Some(observation.as_ref())
        }
        _ => None,
    })
}

fn observation_motion_blocker(
    observation: &tapescale::device::SasDriveObservation,
    operation: &store::DriveOperationRecord,
) -> Option<&'static str> {
    if !observation.identity_verified || observation.identity_conflict {
        return Some("the current drive identity is unverified or conflicted");
    }
    if !observation.accessible {
        return Some("the current tape device is inaccessible");
    }
    if observation.position_lost != Some(false) {
        return Some("the kernel has not confirmed a known tape position");
    }
    if observation.immediate_mode != Some(false) {
        return Some("kernel immediate tape motion is enabled or unknown");
    }
    if observation.in_flight != Some(0) {
        return Some("the kernel reports outstanding or unknown tape I/O");
    }
    if observation.vendor != operation.expected_vendor
        || observation.model != operation.expected_model
        || observation.serial != operation.expected_serial
        || observation.wwn != operation.expected_wwn
        || observation.firmware != operation.expected_firmware
    {
        return Some("drive model, serial, WWN, or firmware changed before execution");
    }
    if observation.medium_loaded != Some(true) || observation.ready != Some(true) {
        return Some("no ready cartridge is loaded");
    }
    None
}

fn verified_motion_postcondition(
    operation: SasMotionOperation,
    observation: Option<&tapescale::device::SasDriveObservation>,
    expected: &store::DriveOperationRecord,
) -> Result<Value, String> {
    let observation = observation.ok_or_else(|| {
        "the exact drive identity was not observed after the tape command".to_owned()
    })?;
    if !observation.identity_verified
        || observation.identity_conflict
        || !observation.accessible
        || observation.position_lost != Some(false)
        || observation.immediate_mode != Some(false)
        || observation.in_flight != Some(0)
    {
        return Err("drive identity, access, or position was not safe after motion".to_owned());
    }
    if observation.vendor != expected.expected_vendor
        || observation.model != expected.expected_model
        || observation.serial != expected.expected_serial
        || observation.wwn != expected.expected_wwn
        || observation.firmware != expected.expected_firmware
    {
        return Err("drive model, serial, WWN, or firmware changed during motion".to_owned());
    }
    match operation {
        SasMotionOperation::Rewind
            if observation.medium_loaded == Some(true)
                && observation.ready == Some(true)
                && observation.at_bot == Some(true) => {}
        SasMotionOperation::Unload if observation.medium_loaded == Some(false) => {}
        SasMotionOperation::Rewind => {
            return Err("rewind returned without an observed ready cartridge at BOT".to_owned());
        }
        SasMotionOperation::Unload => {
            return Err("unload returned without an observed empty drive".to_owned());
        }
    }
    Ok(observation.health())
}

async fn finish_motion(
    pool: &sqlx::PgPool,
    claim: &store::ClaimedDriveOperation,
    state: store::DriveOperationTerminalState,
    result: Value,
) -> Result<()> {
    if !store::finish_drive_operation(pool, claim, state, result).await? {
        warn!(
            operation_id = %claim.operation.id,
            "tape-motion completion was rejected by its fencing token; reconciliation is required"
        );
    }
    Ok(())
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("tapescale=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}

async fn shutdown_signal() {
    let control_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = control_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapescale::device::SasDriveObservation;

    fn observation() -> SasDriveObservation {
        SasDriveObservation {
            stable_id: "scsi:naa.5001".to_owned(),
            identity_verified: true,
            identity_conflict: false,
            identity_basis: "vpd_83".to_owned(),
            vendor: Some("IBM".to_owned()),
            model: Some("ULT3580-TD9".to_owned()),
            serial: Some("DRIVE-1".to_owned()),
            wwn: Some("naa.5001".to_owned()),
            firmware: Some("A1".to_owned()),
            generation: None,
            observed_path: "/dev/nst0".to_owned(),
            device_major: 9,
            device_minor: 128,
            scsi_address: Some("1:0:0:0".to_owned()),
            generic_path: None,
            sysfs_state: Some("running".to_owned()),
            position_lost: Some(false),
            driver_options: Some(0),
            immediate_mode: Some(false),
            in_flight: Some(0),
            at_bot: Some(false),
            accessible: true,
            ready: Some(true),
            medium_loaded: Some(true),
            status: "ready".to_owned(),
            status_detail: None,
            raw_status: Some("ONLINE".to_owned()),
            observed_at: chrono::Utc::now(),
        }
    }

    fn operation() -> store::DriveOperationRecord {
        let now = chrono::Utc::now();
        store::DriveOperationRecord {
            id: Uuid::now_v7(),
            drive_id: Uuid::now_v7(),
            expected_stable_id: "scsi:naa.5001".to_owned(),
            expected_path: "/dev/nst0".to_owned(),
            expected_vendor: Some("IBM".to_owned()),
            expected_model: Some("ULT3580-TD9".to_owned()),
            expected_serial: Some("DRIVE-1".to_owned()),
            expected_wwn: Some("naa.5001".to_owned()),
            expected_firmware: Some("A1".to_owned()),
            action: "rewind".to_owned(),
            scratch_media_confirmed: true,
            state: "running".to_owned(),
            requested_by_label: "test".to_owned(),
            correlation_id: Uuid::now_v7(),
            lease_id: Some(Uuid::now_v7()),
            fencing_token: Some(1),
            attempt: 1,
            result: json!({}),
            created_at: now,
            started_at: Some(now),
            finished_at: None,
        }
    }

    #[test]
    fn rewind_requires_an_affirmative_bot_postcondition() {
        let mut observed = observation();
        assert!(verified_motion_postcondition(
            SasMotionOperation::Rewind,
            Some(&observed),
            &operation()
        )
        .is_err());
        observed.at_bot = Some(true);
        assert!(verified_motion_postcondition(
            SasMotionOperation::Rewind,
            Some(&observed),
            &operation()
        )
        .is_ok());
    }

    #[test]
    fn unload_requires_an_affirmative_empty_postcondition() {
        let mut observed = observation();
        observed.medium_loaded = None;
        observed.ready = None;
        assert!(verified_motion_postcondition(
            SasMotionOperation::Unload,
            Some(&observed),
            &operation()
        )
        .is_err());
        observed.medium_loaded = Some(false);
        assert!(verified_motion_postcondition(
            SasMotionOperation::Unload,
            Some(&observed),
            &operation()
        )
        .is_ok());
    }

    #[test]
    fn hardware_change_or_outstanding_io_blocks_motion() {
        let expected = operation();
        let mut observed = observation();
        observed.firmware = Some("A2".to_owned());
        assert!(observation_motion_blocker(&observed, &expected).is_some());

        observed.firmware = Some("A1".to_owned());
        observed.in_flight = Some(1);
        assert!(observation_motion_blocker(&observed, &expected).is_some());

        observed.in_flight = Some(0);
        observed.immediate_mode = Some(true);
        assert!(observation_motion_blocker(&observed, &expected).is_some());
    }
}
