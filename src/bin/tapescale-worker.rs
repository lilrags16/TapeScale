use std::{collections::BTreeSet, path::Path, time::Duration};

use anyhow::Result;
use clap::Parser;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tapescale::{
    config::Config,
    device::{
        discover_sas_drives, execute_sas_motion, SasDiscoveryConfig, SasMotionError,
        SasMotionOperation, SasProbeResult,
    },
    engine::{simulator::SimulatorEngine, QualificationStatus, TapeEngine},
    qualification::{BtapeOutcomeKind, BtapeQualificationConfig},
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
    if config.lab_tape_qualification_enabled {
        warn!(
            btape = %config.btape_path.display(),
            "destructive Bacula btape qualification is enabled for attended scratch-media tests"
        );
    }
    let btape_config = BtapeQualificationConfig {
        enabled: config.lab_tape_qualification_enabled,
        program: config.btape_path.clone(),
        state_dir: config.bacula_state_dir.clone(),
        timeout: config.qualification_timeout,
    };
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
            &btape_config,
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
    btape_config: &BtapeQualificationConfig,
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
        process_qualification_runs(pool, sas_config, btape_config, worker_id, cancellation).await?;
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

async fn process_qualification_runs(
    pool: &sqlx::PgPool,
    config: &SasDiscoveryConfig,
    btape_config: &BtapeQualificationConfig,
    worker_id: Uuid,
    cancellation: &CancellationToken,
) -> Result<()> {
    let expired = store::recover_expired_qualification_runs(pool).await?;
    if expired > 0 {
        warn!(
            expired_runs = expired,
            "expired destructive qualification runs require drive reconciliation"
        );
    }
    if !btape_config.enabled {
        let blocked = store::block_queued_qualification_runs(
            pool,
            "destructive lab tape qualification was disabled before worker execution",
        )
        .await?;
        if blocked > 0 {
            warn!(
                blocked_runs = blocked,
                "queued qualification runs were blocked"
            );
        }
        return Ok(());
    }
    if cancellation.is_cancelled() {
        return Ok(());
    }
    match store::claim_next_qualification_run(pool, worker_id).await? {
        store::ClaimQualificationRunResult::Claimed(claim) => {
            execute_claimed_qualification_run(pool, config, btape_config, &claim, cancellation)
                .await?;
        }
        store::ClaimQualificationRunResult::Blocked { id, reason } => {
            warn!(qualification_run_id = %id, %reason, "tape qualification was blocked during worker revalidation");
        }
        store::ClaimQualificationRunResult::Empty => {}
    }
    Ok(())
}

async fn execute_claimed_qualification_run(
    pool: &sqlx::PgPool,
    config: &SasDiscoveryConfig,
    btape_config: &BtapeQualificationConfig,
    claim: &store::ClaimedQualificationRun,
    cancellation: &CancellationToken,
) -> Result<()> {
    let before = discover_sas_drives(config).await;
    persist_sas_results(Some(pool), &before).await?;
    let before_observation = matching_observation(
        &before,
        &claim.run.expected_path,
        &claim.run.expected_stable_id,
    );
    let Some(before_observation) = before_observation else {
        finish_qualification(
            pool,
            claim,
            store::QualificationTerminalState::Blocked,
            json!({
                "detail": "the exact configured drive identity was not observed immediately before destructive qualification",
                "position_uncertain": false,
                "command_dispatched": false
            }),
        )
        .await?;
        return Ok(());
    };
    if let Some(reason) = qualification_observation_blocker(before_observation, &claim.run) {
        finish_qualification(
            pool,
            claim,
            store::QualificationTerminalState::Blocked,
            json!({
                "detail": reason,
                "position_uncertain": false,
                "command_dispatched": false,
                "before": before_observation.health()
            }),
        )
        .await?;
        return Ok(());
    }
    if !store::renew_qualification_run_lease(pool, claim).await? {
        store::recover_expired_qualification_runs(pool).await?;
        warn!(
            qualification_run_id = %claim.run.id,
            "qualification lease expired before btape dispatch; no command was executed"
        );
        return Ok(());
    }
    if cancellation.is_cancelled() {
        finish_qualification(
            pool,
            claim,
            store::QualificationTerminalState::Blocked,
            json!({
                "detail": "worker shutdown was requested before btape dispatch",
                "position_uncertain": false,
                "command_dispatched": false
            }),
        )
        .await?;
        return Ok(());
    }

    info!(
        qualification_run_id = %claim.run.id,
        drive_id = %claim.run.drive_id,
        cartridge_id = %claim.run.cartridge_id,
        cartridge_label = %claim.run.expected_cartridge_label,
        stable_id = %claim.run.expected_stable_id,
        device = %claim.run.expected_path,
        fencing_token = claim.fencing_token,
        "starting destructive Bacula btape test on operator-designated scratch media"
    );
    let (outcome, lease_heartbeat_failure) =
        run_btape_with_lease_heartbeat(pool, btape_config, claim, cancellation).await;
    let mut terminal = match outcome.kind {
        BtapeOutcomeKind::Passed => store::QualificationTerminalState::Passed,
        BtapeOutcomeKind::Failed => store::QualificationTerminalState::Failed,
        BtapeOutcomeKind::Blocked => store::QualificationTerminalState::Blocked,
        BtapeOutcomeKind::Interrupted => store::QualificationTerminalState::Interrupted,
    };
    let mut result = serde_json::to_value(&outcome)?;
    insert_result_field(&mut result, "before", before_observation.health());
    let lease_heartbeat_failed = lease_heartbeat_failure.is_some();
    if let Some(detail) = lease_heartbeat_failure {
        terminal = store::QualificationTerminalState::Interrupted;
        insert_result_field(&mut result, "kind", json!("interrupted"));
        insert_result_field(&mut result, "position_uncertain", json!(true));
        insert_result_field(&mut result, "lease_heartbeat_failure", json!(detail));
    }

    if !store::renew_qualification_run_lease(pool, claim).await? {
        store::recover_expired_qualification_runs(pool).await?;
        warn!(
            qualification_run_id = %claim.run.id,
            "qualification lease expired before btape evidence could be committed"
        );
        return Ok(());
    }

    if outcome.kind == BtapeOutcomeKind::Passed && !lease_heartbeat_failed {
        if cancellation.is_cancelled() {
            terminal = store::QualificationTerminalState::Interrupted;
            insert_result_field(&mut result, "kind", json!("interrupted"));
            insert_result_field(&mut result, "position_uncertain", json!(true));
            insert_result_field(
                &mut result,
                "detail",
                json!("worker shutdown was requested before the required post-test rewind"),
            );
        } else {
            let cleanup = execute_sas_motion(
                config,
                Path::new(&claim.run.expected_path),
                SasMotionOperation::Rewind,
                cancellation,
            )
            .await;
            let after = discover_sas_drives(config).await;
            persist_sas_results(Some(pool), &after).await?;
            let after_observation = matching_observation(
                &after,
                &claim.run.expected_path,
                &claim.run.expected_stable_id,
            );
            match cleanup {
                Ok(output) => {
                    match verified_qualification_postcondition(after_observation, &claim.run) {
                        Ok(after_health) => {
                            insert_result_field(
                                &mut result,
                                "cleanup",
                                json!({
                                    "action": "rewind",
                                    "postcondition": "ready_at_bot",
                                    "tool": output,
                                    "after": after_health
                                }),
                            );
                        }
                        Err(detail) => {
                            terminal = store::QualificationTerminalState::Failed;
                            insert_result_field(&mut result, "kind", json!("failed"));
                            insert_result_field(&mut result, "position_uncertain", json!(true));
                            insert_result_field(&mut result, "detail", json!(detail));
                            insert_result_field(
                                &mut result,
                                "cleanup",
                                json!({"action":"rewind","tool":output}),
                            );
                        }
                    }
                }
                Err(error) => {
                    terminal = if matches!(error, SasMotionError::Tool(ToolError::Cancelled { .. }))
                    {
                        store::QualificationTerminalState::Interrupted
                    } else {
                        store::QualificationTerminalState::Failed
                    };
                    insert_result_field(
                        &mut result,
                        "kind",
                        json!(qualification_terminal_label(terminal)),
                    );
                    insert_result_field(&mut result, "position_uncertain", json!(true));
                    insert_result_field(
                        &mut result,
                        "detail",
                        json!(format!("post-test rewind failed: {error}")),
                    );
                }
            }
        }
    } else if outcome.position_uncertain {
        let after = discover_sas_drives(config).await;
        persist_sas_results(Some(pool), &after).await?;
        insert_result_field(
            &mut result,
            "after",
            matching_observation(
                &after,
                &claim.run.expected_path,
                &claim.run.expected_stable_id,
            )
            .map(|observation| observation.health())
            .unwrap_or(Value::Null),
        );
    }

    let (result, evidence_overflow) = bound_qualification_result(result);
    if evidence_overflow && terminal == store::QualificationTerminalState::Passed {
        terminal = store::QualificationTerminalState::Failed;
    }
    finish_qualification(pool, claim, terminal, result).await
}

async fn run_btape_with_lease_heartbeat(
    pool: &sqlx::PgPool,
    btape_config: &BtapeQualificationConfig,
    claim: &store::ClaimedQualificationRun,
    cancellation: &CancellationToken,
) -> (tapescale::qualification::BtapeOutcome, Option<String>) {
    let operation_cancellation = cancellation.child_token();
    let execution = btape_config.run(
        claim.run.id,
        Path::new(&claim.run.expected_path),
        &operation_cancellation,
    );
    tokio::pin!(execution);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(60));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            outcome = &mut execution => return (outcome, None),
            _ = heartbeat.tick() => {
                match store::renew_qualification_run_lease(pool, claim).await {
                    Ok(true) => {}
                    Ok(false) => {
                        operation_cancellation.cancel();
                        let outcome = execution.await;
                        return (
                            outcome,
                            Some("the fenced drive lease could not be renewed during btape; execution was cancelled".to_owned()),
                        );
                    }
                    Err(error) => {
                        operation_cancellation.cancel();
                        let outcome = execution.await;
                        return (
                            outcome,
                            Some(format!("the drive-lease heartbeat failed during btape: {error}")),
                        );
                    }
                }
            }
        }
    }
}

fn qualification_terminal_label(state: store::QualificationTerminalState) -> &'static str {
    match state {
        store::QualificationTerminalState::Passed => "passed",
        store::QualificationTerminalState::Failed => "failed",
        store::QualificationTerminalState::Blocked => "blocked",
        store::QualificationTerminalState::Interrupted => "interrupted",
    }
}

fn insert_result_field(result: &mut Value, key: &str, value: Value) {
    if let Some(object) = result.as_object_mut() {
        object.insert(key.to_owned(), value);
    }
}

fn bound_qualification_result(result: Value) -> (Value, bool) {
    const MAX_RESULT_BYTES: usize = 900 * 1024;
    let serialized = serde_json::to_vec(&result).unwrap_or_default();
    if serialized.len() <= MAX_RESULT_BYTES {
        return (result, false);
    }
    let position_uncertain = result
        .get("position_uncertain")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let prior_kind = result
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    let digest = Sha256::digest(&serialized)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    (
        json!({
            "kind": if prior_kind == "passed" {"failed"} else {prior_kind},
            "position_uncertain": position_uncertain,
            "detail": "qualification evidence exceeded its durable size budget; the run cannot pass",
            "evidence_reduced": true,
            "original_json_bytes": serialized.len(),
            "original_json_sha256": digest
        }),
        true,
    )
}

fn qualification_observation_blocker(
    observation: &tapescale::device::SasDriveObservation,
    run: &store::QualificationRunRecord,
) -> Option<&'static str> {
    if !observation.identity_verified || observation.identity_conflict {
        return Some("the current drive identity is unverified or conflicted");
    }
    if !observation.accessible {
        return Some("the current tape device is inaccessible");
    }
    if observation.immediate_mode != Some(false) {
        return Some("kernel immediate tape motion is enabled or unknown");
    }
    if observation.in_flight != Some(0) {
        return Some("the kernel reports outstanding or unknown tape I/O");
    }
    if observation.vendor != run.expected_vendor
        || observation.model != run.expected_model
        || observation.serial != run.expected_serial
        || observation.wwn != run.expected_wwn
        || observation.firmware != run.expected_firmware
    {
        return Some("drive model, serial, WWN, or firmware changed before qualification");
    }
    if observation.medium_loaded != Some(true) || observation.ready != Some(true) {
        return Some("no ready cartridge is loaded");
    }
    None
}

fn verified_qualification_postcondition(
    observation: Option<&tapescale::device::SasDriveObservation>,
    expected: &store::QualificationRunRecord,
) -> Result<Value, String> {
    let observation = observation.ok_or_else(|| {
        "the exact drive identity was not observed after the post-test rewind".to_owned()
    })?;
    if let Some(reason) = qualification_observation_blocker(observation, expected) {
        return Err(reason.to_owned());
    }
    if observation.at_bot != Some(true) {
        return Err(
            "btape passed, but the required cleanup rewind did not establish BOT".to_owned(),
        );
    }
    Ok(observation.health())
}

async fn finish_qualification(
    pool: &sqlx::PgPool,
    claim: &store::ClaimedQualificationRun,
    state: store::QualificationTerminalState,
    result: Value,
) -> Result<()> {
    if !store::finish_qualification_run(pool, claim, state, result).await? {
        warn!(
            qualification_run_id = %claim.run.id,
            "qualification completion was rejected by its fencing token; reconciliation is required"
        );
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
        || observation.immediate_mode != Some(false)
        || observation.in_flight != Some(0)
    {
        return Err(
            "drive identity, access, immediate mode, or I/O state was not safe after motion"
                .to_owned(),
        );
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

    fn qualification_run() -> store::QualificationRunRecord {
        let now = chrono::Utc::now();
        store::QualificationRunRecord {
            id: Uuid::now_v7(),
            drive_id: Uuid::now_v7(),
            cartridge_id: Uuid::now_v7(),
            expected_stable_id: "scsi:naa.5001".to_owned(),
            expected_path: "/dev/nst0".to_owned(),
            expected_vendor: Some("IBM".to_owned()),
            expected_model: Some("ULT3580-TD9".to_owned()),
            expected_serial: Some("DRIVE-1".to_owned()),
            expected_wwn: Some("naa.5001".to_owned()),
            expected_firmware: Some("A1".to_owned()),
            expected_cartridge_label: "TS-SCRATCH-001".to_owned(),
            expected_media_generation: Some(9),
            expected_media_format: Some("LTO-9".to_owned()),
            engine_name: "Bacula Community".to_owned(),
            engine_version: "15.0.3".to_owned(),
            test_kind: "btape_test".to_owned(),
            scratch_media_confirmed: true,
            destructive_overwrite_confirmed: true,
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
        observed.position_lost = Some(true);
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
        observed.position_lost = None;
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

    #[test]
    fn rewind_and_unload_can_recover_an_unknown_or_lost_position() {
        let expected = operation();
        let mut observed = observation();
        observed.position_lost = None;
        assert!(observation_motion_blocker(&observed, &expected).is_none());
        observed.position_lost = Some(true);
        assert!(observation_motion_blocker(&observed, &expected).is_none());
    }

    #[test]
    fn qualification_pass_requires_exact_identity_and_cleanup_at_bot() {
        let expected = qualification_run();
        let mut observed = observation();
        assert!(qualification_observation_blocker(&observed, &expected).is_none());
        assert!(verified_qualification_postcondition(Some(&observed), &expected).is_err());

        observed.at_bot = Some(true);
        assert!(verified_qualification_postcondition(Some(&observed), &expected).is_ok());

        observed.serial = Some("REPLACED".to_owned());
        assert!(qualification_observation_blocker(&observed, &expected).is_some());
        assert!(verified_qualification_postcondition(Some(&observed), &expected).is_err());
    }

    #[test]
    fn oversized_qualification_evidence_fails_closed() {
        let (bounded, overflow) = bound_qualification_result(json!({
            "kind":"passed",
            "position_uncertain":false,
            "evidence":{"tool":{"stdout":"x".repeat(950 * 1024)}}
        }));
        assert!(overflow);
        assert_eq!(bounded["kind"], "failed");
        assert_eq!(bounded["position_uncertain"], false);
        assert_eq!(bounded["evidence_reduced"], true);
        assert!(serde_json::to_vec(&bounded).expect("json").len() < 2_048);
    }
}
