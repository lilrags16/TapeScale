//! Attended, read-only recovery rehearsal for an already verified full-file point.
//!
//! This path accepts the exact full job retained by either a passed legacy G0.2 exercise or a
//! verified operational backup, recreates that run's fixed Bacula resources, and restores the
//! complete job into a fresh server-generated directory. Spanning jobs use an attended, ordered,
//! read-only media handoff for every retained volume. The result is independently compared with
//! the retained source manifest. This path never labels, erases, writes, or otherwise prepares
//! media.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use rustix::fs::{open, statvfs, Mode, OFlags};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    bacula_catalog::{
        validate_backup_job_on_volumes, validate_restore_job, BaculaBackupCatalogEvidence,
        BaculaRestoreCatalogEvidence,
    },
    bacula_runtime::{BaculaRuntime, BaculaRuntimeEvidence},
    config::Config,
    data_path::{
        build_and_persist_manifest_with_cancel, compare_manifests,
        create_isolated_restore_destination, persist_json_artifact, FileManifest,
        JsonArtifactSummary, ManifestArtifactSummary, ManifestComparison,
    },
    device::{
        discover_sas_drives, execute_sas_motion, SasDiscoveryConfig, SasMotionOperation,
        SasProbeResult,
    },
    domain::path::{resolve_link_target, validate_archive_path},
    engine::bacula::{
        BaculaAdapter, BaculaCommands, BaculaCompression, BaculaDataOptions, BaculaJobEvidence,
        BaculaJobId, BaculaJobResource, BaculaJobState, BaculaPaths, BaculaPoolName,
        BaculaRestoreSubpath, CorrelatedJobDispatch, DirectBconsoleExecutor, VolumeLabel,
    },
    store,
};

const LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureClass {
    Blocked,
    Failed,
    Interrupted,
    Cancelled,
}

#[derive(Debug)]
struct RehearsalFailure {
    class: FailureClass,
    detail: String,
}

impl RehearsalFailure {
    fn blocked(detail: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Blocked,
            detail: detail.into(),
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Failed,
            detail: detail.into(),
        }
    }

    fn interrupted(detail: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Interrupted,
            detail: detail.into(),
        }
    }

    fn cancelled(detail: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Cancelled,
            detail: detail.into(),
        }
    }
}

#[derive(Debug)]
struct CompletedRehearsal {
    source_manifest: FileManifest,
    restored_manifest: FileManifest,
    restored_manifest_artifact_sha256: String,
    comparison: ManifestComparison,
    restore: BaculaJobEvidence,
    restore_catalog: BaculaRestoreCatalogEvidence,
    restore_destination: PathBuf,
    volumes: Vec<RestoreVolumeEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RestoreVolumeEvidence {
    volume_order: i32,
    cartridge_label: String,
    volume_label: String,
    engine_media_id: u64,
}

#[derive(Clone, Debug)]
struct ManifestReference {
    original_data_run_id: Uuid,
    storage_ref: String,
    sha256: String,
    manifest_digest: String,
    size_bytes: i64,
}

pub async fn process_restore_rehearsal_runs(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    worker_id: Uuid,
    cancellation: &CancellationToken,
) -> Result<()> {
    let expired = store::recover_expired_restore_rehearsal_runs(pool).await?;
    if expired > 0 {
        warn!(
            expired_runs = expired,
            "expired restore rehearsals require drive reconciliation"
        );
    }
    if !config.lab_file_roundtrip_enabled {
        let blocked = store::block_queued_restore_rehearsal_runs(
            pool,
            "the attended Bacula restore-rehearsal gate was disabled before worker execution",
        )
        .await?;
        if blocked > 0 {
            warn!(
                blocked_runs = blocked,
                "queued restore rehearsals were blocked"
            );
        }
        return Ok(());
    }
    if cancellation.is_cancelled() {
        return Ok(());
    }

    match store::claim_next_restore_rehearsal_run(pool, worker_id).await? {
        store::ClaimRestoreRehearsalRunResult::Claimed(claim) => {
            execute_claimed(pool, sas_config, config, &claim, cancellation).await?;
        }
        store::ClaimRestoreRehearsalRunResult::Blocked { id, reason } => {
            warn!(restore_rehearsal_id = %id, %reason, "restore rehearsal was blocked during worker revalidation");
        }
        store::ClaimRestoreRehearsalRunResult::Empty => {}
    }
    Ok(())
}

async fn execute_claimed(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedRestoreRehearsalRun,
    cancellation: &CancellationToken,
) -> Result<()> {
    let before = discover_and_persist(pool, sas_config).await?;
    let Some(observation) = matching_observation(
        &before,
        &claim.run.expected_path,
        &claim.run.expected_stable_id,
    ) else {
        finish(
            pool,
            claim,
            store::RestoreRehearsalTerminalState::Blocked,
            json!({
                "detail": "the exact configured drive identity was not observed immediately before the restore rehearsal",
                "tape_read_dispatched": false,
                "position_uncertain": false
            }),
        )
        .await?;
        return Ok(());
    };
    if let Some(reason) = observation_blocker(observation, &claim.run) {
        finish(
            pool,
            claim,
            store::RestoreRehearsalTerminalState::Blocked,
            json!({
                "detail": reason,
                "tape_read_dispatched": false,
                "position_uncertain": false,
                "before": observation.health()
            }),
        )
        .await?;
        return Ok(());
    }

    let evidence_directory = config
        .evidence_root
        .join("restore-rehearsals")
        .join(claim.run.id.to_string());
    let (operation, runtime_evidence, tape_touched) = perform_rehearsal(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        &evidence_directory,
    )
    .await;
    let cleanup = if tape_touched {
        cleanup_drive(pool, sas_config, claim).await
    } else {
        (
            true,
            json!({"required": false, "position_uncertain": false}),
        )
    };
    complete_rehearsal(
        pool,
        claim,
        operation,
        runtime_evidence,
        tape_touched,
        cleanup,
        &evidence_directory,
        cancellation,
    )
    .await
}

async fn perform_rehearsal(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedRestoreRehearsalRun,
    cancellation: &CancellationToken,
    evidence_directory: &Path,
) -> (
    Result<CompletedRehearsal, RehearsalFailure>,
    Option<BaculaRuntimeEvidence>,
    bool,
) {
    let deadline = Instant::now() + config.data_run_timeout;
    let mut runtime: Option<BaculaRuntime> = None;
    let mut tape_touched = false;
    let operation = async {
        validate_contract(config, &claim.run)?;
        prepare_evidence_directory(&config.evidence_root, evidence_directory)?;

        let manifest_reference = ManifestReference {
            original_data_run_id: claim.run.original_data_run_id,
            storage_ref: claim.run.source_manifest_storage_ref.clone(),
            sha256: claim.run.source_manifest_sha256.clone(),
            manifest_digest: claim.run.source_manifest_digest.clone(),
            size_bytes: claim.run.source_manifest_size_bytes,
        };
        let evidence_root = config.evidence_root.clone();
        let source_manifest = tokio::task::spawn_blocking(move || {
            load_verified_source_manifest(&evidence_root, &manifest_reference)
        })
        .await
        .map_err(|error| {
            RehearsalFailure::blocked(format!("retained-manifest worker failed: {error}"))
        })??;
        let estimated_required_bytes = u64::try_from(claim.run.estimated_required_bytes)
            .map_err(|_| RehearsalFailure::blocked("restore-plan byte estimate is invalid"))?;
        let reserve_bytes = u64::try_from(claim.run.required_free_reserve_bytes)
            .map_err(|_| RehearsalFailure::blocked("restore-plan free-space reserve is invalid"))?;
        let asserted_available_bytes = u64::try_from(claim.run.available_bytes).map_err(|_| {
            RehearsalFailure::blocked("restore-plan available-byte assertion is invalid")
        })?;
        let capacity = destination_capacity_evidence(
            &config.restore_root,
            source_manifest
                .logical_bytes
                .max(estimated_required_bytes),
            reserve_bytes,
            source_manifest.logical_bytes,
            estimated_required_bytes,
            asserted_available_bytes,
        )?;

        if cancellation.is_cancelled() {
            return Err(RehearsalFailure::interrupted(
                "worker shutdown was requested before any tape read",
            ));
        }
        if cancellation_requested(pool, claim).await? {
            return Err(RehearsalFailure::cancelled(
                "restore rehearsal was cancelled before any tape read",
            ));
        }
        renew_claim(pool, claim, "before Bacula catalog preflight").await?;

        let backup_job_id = positive_bacula_job_id(claim.run.backup_job_id)?;
        let restore_volumes = store::restore_rehearsal_volumes(pool, claim.run.id)
            .await
            .map_err(|error| {
                RehearsalFailure::blocked(format!(
                    "could not load the retained ordered restore volumes: {error}"
                ))
            })?;
        let authorized_volumes = validate_restore_volume_ledger(claim, &restore_volumes)?;
        let volume = authorized_volumes
            .first()
            .expect("the restore-volume ledger validator requires a first volume")
            .clone();
        let bacula_pool = BaculaPoolName::for_run(claim.run.original_data_run_id);
        let backup_catalog = validate_backup_job_on_volumes(
            pool,
            backup_job_id,
            &authorized_volumes,
            &bacula_pool,
        )
        .await
        .map_err(|error| {
                RehearsalFailure::blocked(format!(
                    "the retained Bacula backup/catalog binding is no longer valid: {error}"
                ))
            })?;
        let ordered_volume_evidence =
            validate_restore_catalog_volumes(&restore_volumes, &backup_catalog, backup_job_id)?;

        let source_relative = Path::new(&claim.run.source_mount_path)
            .strip_prefix(&config.source_root)
            .map_err(|_| RehearsalFailure::blocked("retained source path escaped /source"))?;
        let paths = BaculaPaths::from_relative_source(
            source_relative,
            PathBuf::from(&claim.run.expected_path),
        )
        .map_err(|error| RehearsalFailure::blocked(error.to_string()))?;
        let database_url = config.database_url.as_deref().ok_or_else(|| {
            RehearsalFailure::blocked("DATABASE_URL is required for the Bacula catalog")
        })?;
        let data_options = if restore_volumes.len() > 1 {
            BaculaDataOptions::spanning(BaculaCompression::None)
        } else {
            BaculaDataOptions::single_volume(BaculaCompression::None)
        };
        runtime = Some(
            BaculaRuntime::start_with_data_options(
                pool,
                database_url,
                claim.run.original_data_run_id,
                paths,
                data_options,
                evidence_directory,
                cancellation,
            )
            .await
            .map_err(|error| RehearsalFailure::failed(error.to_string()))?,
        );

        let adapter = BaculaAdapter::new(DirectBconsoleExecutor::default());
        wait_for_console(pool, claim, &adapter, deadline, cancellation).await?;

        // Catalog checks and daemon startup must not turn an earlier observation into authority.
        // Re-probe the exact kernel device binding at the final boundary before Bacula may read it.
        let immediate = discover_and_persist(pool, sas_config)
            .await
            .map_err(|error| RehearsalFailure::blocked(error.to_string()))?;
        let immediate_observation = matching_observation(
            &immediate,
            &claim.run.expected_path,
            &claim.run.expected_stable_id,
        )
        .ok_or_else(|| {
            RehearsalFailure::blocked(
                "the exact drive identity disappeared immediately before restore dispatch",
            )
        })?;
        if let Some(reason) = observation_blocker(immediate_observation, &claim.run) {
            return Err(RehearsalFailure::blocked(reason));
        }
        renew_claim(pool, claim, "immediately before restore dispatch").await?;

        let restore_subpath = BaculaRestoreSubpath::parse(&claim.run.destination_subpath)
            .map_err(|error| RehearsalFailure::blocked(error.to_string()))?;
        let destination = restore_subpath.destination(claim.run.id);
        let secured_destination = create_isolated_restore_destination(
            &config.restore_root,
            &claim.run.destination_subpath,
            claim.run.id,
        )
        .map_err(|error| {
            RehearsalFailure::failed(format!(
                "could not securely create the fresh restore destination {}: {error}",
                destination.path().display()
            ))
        })?;
        if secured_destination != destination.path() {
            return Err(RehearsalFailure::blocked(
                "the secured destination did not match the pinned Bacula restore path",
            ));
        }

        transition(
            pool,
            claim,
            store::RestoreRehearsalPhase::Restoring,
            json!({
                "detail": "retained source manifest and Bacula catalog binding passed; starting read-only full restore",
                "backup_job_id": backup_job_id,
                "engine_media_id": backup_catalog.volume.media_id,
                "volume": volume.as_str(),
                "volume_count": ordered_volume_evidence.len(),
                "volumes": ordered_volume_evidence,
                "source_manifest_artifact_sha256": claim.run.source_manifest_sha256,
                "source_manifest_digest": source_manifest.digest_sha256,
                "restore_destination": destination.path(),
                "destination_capacity": capacity,
                "media_mutation_allowed": false
            }),
        )
        .await?;

        // From this point the command may have reached the Storage Daemon even if correlation or
        // evidence persistence fails. Cleanup and reconciliation are therefore mandatory.
        tape_touched = true;
        let restore_dispatch = adapter
            .dispatch_and_correlate(
                BaculaJobResource::Restore,
                &BaculaCommands::full_restore(backup_job_id, &destination),
            )
            .await
            .map_err(|error| {
                RehearsalFailure::failed(format!("Bacula restore dispatch failed: {error}"))
            })?;
        record_engine_dispatch(pool, claim, &restore_dispatch, &adapter).await?;
        let restore = wait_for_job(
            pool,
            sas_config,
            claim,
            &adapter,
            RestoreJobMediaContext {
                job_id: restore_dispatch.job_id,
                ordered_volumes: &authorized_volumes,
                pool: &bacula_pool,
            },
            deadline,
            cancellation,
        )
        .await?;
        let final_backup_catalog = validate_backup_job_on_volumes(
            pool,
            backup_job_id,
            &authorized_volumes,
            &bacula_pool,
        )
        .await
        .map_err(|error| {
            RehearsalFailure::failed(format!(
                "Bacula backup/catalog binding changed during restore: {error}"
            ))
        })?;
        let final_volume_evidence = validate_restore_catalog_volumes(
            &restore_volumes,
            &final_backup_catalog,
            backup_job_id,
        )?;
        if final_volume_evidence != ordered_volume_evidence {
            return Err(RehearsalFailure::failed(
                "ordered Bacula volume evidence changed during restore",
            ));
        }
        let restore_catalog = validate_restore_job(pool, restore.job_id, backup_job_id)
            .await
            .map_err(|error| {
                RehearsalFailure::failed(format!("Bacula restore catalog proof failed: {error}"))
            })?;
        let engine_restore = json!({
            "engine": "Bacula Community",
            "engine_version": "15.0.3",
            "read_only_media_operation": true,
            "source_backup_job_id": backup_job_id,
            "source_backup_catalog": final_backup_catalog,
            "restore_job": restore,
            "restore_catalog": restore_catalog,
            "volume": volume.as_str(),
            "volume_count": ordered_volume_evidence.len(),
            "volumes": ordered_volume_evidence,
            "restore_destination": destination.path()
        });
        let engine_artifact = persist_json_artifact(
            evidence_directory,
            "engine-restore",
            &engine_restore,
        )
        .map_err(|error| RehearsalFailure::failed(error.to_string()))?;
        append_json_artifact(
            pool,
            claim,
            "engine_restore",
            &engine_artifact,
            json!({
                "completed": true,
                "finalized": true,
                "physical_media_readback": true,
                "read_only_media_operation": true,
                "backup_job_id": backup_job_id,
                "restore_job_id": restore.job_id.get(),
                "engine_media_id": backup_catalog.volume.media_id,
                "media_id": backup_catalog.volume.media_id,
                "volume": volume.as_str(),
                "volume_count": ordered_volume_evidence.len(),
                "volumes": ordered_volume_evidence,
                "destination": destination.path()
            }),
        )
        .await?;

        transition(
            pool,
            claim,
            store::RestoreRehearsalPhase::Comparing,
            json!({
                "detail": "Bacula restore completed; independently hashing restored content and covered metadata",
                "restore_job": restore,
                "restore_destination": destination.path()
            }),
        )
        .await?;

        let restored_source_root = verified_restored_source_path(
            destination.path(),
            Path::new(&claim.run.source_mount_path),
        )?;
        let (restored_manifest, restored_artifact) = scan_manifest_with_lease(
            pool,
            claim,
            restored_source_root.clone(),
            evidence_directory.to_owned(),
            deadline,
            cancellation,
        )
        .await
        .map_err(|mut error| {
            error.detail = format!(
                "restored tree was not readable at {}: {}",
                restored_source_root.display(),
                error.detail
            );
            error
        })?;
        append_manifest_artifact(
            pool,
            claim,
            "restored_manifest",
            &restored_artifact,
            &restored_manifest,
        )
        .await?;

        let comparison = compare_manifests(&source_manifest, &restored_manifest);
        let comparison_artifact =
            persist_json_artifact(evidence_directory, "comparison", &comparison)
                .map_err(|error| RehearsalFailure::failed(error.to_string()))?;
        append_json_artifact(
            pool,
            claim,
            "comparison",
            &comparison_artifact,
            json!({
                "matched": comparison.matched,
                "expected_digest": comparison.expected_digest,
                "actual_digest": comparison.actual_digest,
                "differences": comparison.differences.len(),
                "differences_truncated": comparison.differences_truncated
            }),
        )
        .await?;
        if !comparison.matched {
            return Err(RehearsalFailure::failed(format!(
                "restored manifest did not match the retained source manifest ({} retained differences)",
                comparison.differences.len()
            )));
        }
        if cancellation.is_cancelled() {
            return Err(RehearsalFailure::interrupted(
                "worker shutdown was requested before restore-rehearsal finalization",
            ));
        }
        if cancellation_requested(pool, claim).await? {
            return Err(RehearsalFailure::cancelled(
                "restore rehearsal was cancelled before finalization",
            ));
        }
        renew_claim(pool, claim, "before restore-rehearsal finalization").await?;

        Ok(CompletedRehearsal {
            source_manifest,
            restored_manifest,
            restored_manifest_artifact_sha256: restored_artifact.artifact_sha256,
            comparison,
            restore,
            restore_catalog,
            restore_destination: destination.path().to_owned(),
            volumes: ordered_volume_evidence,
        })
    }
    .await;

    let runtime_evidence = if let Some(active_runtime) = runtime.take() {
        Some(active_runtime.stop().await)
    } else {
        None
    };
    (operation, runtime_evidence, tape_touched)
}

#[allow(clippy::too_many_arguments)]
async fn complete_rehearsal(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    operation: Result<CompletedRehearsal, RehearsalFailure>,
    runtime_evidence: Option<BaculaRuntimeEvidence>,
    tape_touched: bool,
    cleanup: (bool, Value),
    evidence_directory: &Path,
    cancellation: &CancellationToken,
) -> Result<()> {
    match operation {
        Ok(completed) if cleanup.0 => {
            let terminal = if cancellation.is_cancelled() {
                Some((
                    store::RestoreRehearsalTerminalState::Interrupted,
                    "worker shutdown was requested after cleanup and before finalization",
                ))
            } else if store::restore_rehearsal_cancellation_requested(pool, claim).await? {
                Some((
                    store::RestoreRehearsalTerminalState::Cancelled,
                    "operator cancelled the restore rehearsal before finalization",
                ))
            } else {
                None
            };
            if let Some((terminal, detail)) = terminal {
                finish_or_quarantine(
                    pool,
                    claim,
                    terminal,
                    json!({
                        "detail": detail,
                        "position_uncertain": false,
                        "tape_read_dispatched": true,
                        "cleanup": cleanup.1,
                        "runtime": runtime_evidence,
                        "media_mutation_commands_dispatched": false,
                        "restore_verification_created": false
                    }),
                )
                .await?;
                return Ok(());
            }
            let report = json!({
                "scope": if completed.volumes.len() > 1 {
                    "attended verified-point multi-volume full-file restore rehearsal"
                } else {
                    "attended verified-point single-volume full-file restore rehearsal"
                },
                "production_eligible": false,
                "media_operation": "read_only",
                "recovery_point_id": claim.run.recovery_point_id,
                "source_data_run_id": claim.run.original_data_run_id,
                "source_backup_job_id": claim.run.backup_job_id,
                "engine_operation_id": format!("bacula-job:{}", completed.restore.job_id),
                "restore_job_id": completed.restore.job_id.get(),
                "restore_job": completed.restore,
                "restore_catalog": completed.restore_catalog,
                "volume_count": completed.volumes.len(),
                "volumes": completed.volumes,
                "source_manifest_digest": completed.source_manifest.digest_sha256,
                "restored_manifest_digest": completed.restored_manifest.digest_sha256,
                "restored_manifest_sha256": completed.restored_manifest_artifact_sha256,
                "comparison": completed.comparison,
                "restore_destination": completed.restore_destination,
                "runtime": runtime_evidence,
                "cleanup": cleanup.1,
                "media_mutation_commands_dispatched": false
            });
            let finalize_attempt: Result<store::FinalizeRestoreRehearsalResult, String> = async {
                let report_artifact =
                    persist_json_artifact(evidence_directory, "evidence-report", &report)
                        .map_err(|error| error.to_string())?;
                append_json_artifact(
                    pool,
                    claim,
                    "evidence_report",
                    &report_artifact,
                    json!({
                        "complete": true,
                        "candidate_evidence": true,
                        "pending_atomic_finalization": true,
                        "matched": true,
                        "read_only_media_operation": true,
                        "volume_count": completed.volumes.len(),
                        "volumes": completed.volumes,
                        "source_manifest_digest": completed.source_manifest.digest_sha256,
                        "restored_manifest_digest": completed.restored_manifest.digest_sha256
                    }),
                )
                .await
                .map_err(|error| error.detail)?;
                store::finalize_restore_rehearsal_pass(pool, claim, report)
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;

            match finalize_attempt {
                Ok(store::FinalizeRestoreRehearsalResult::Passed {
                    verification_record_id,
                }) => info!(
                    restore_rehearsal_id = %claim.run.id,
                    %verification_record_id,
                    "verified-point restore rehearsal passed"
                ),
                Ok(store::FinalizeRestoreRehearsalResult::Rejected(reason)) => {
                    let terminal = if matches!(
                        store::restore_rehearsal_cancellation_requested(pool, claim).await,
                        Ok(true)
                    ) {
                        store::RestoreRehearsalTerminalState::Cancelled
                    } else {
                        store::RestoreRehearsalTerminalState::Failed
                    };
                    finish_or_quarantine(
                        pool,
                        claim,
                        terminal,
                        json!({
                            "detail": format!("final restore-rehearsal evidence was rejected: {reason}"),
                            "position_uncertain": false,
                            "tape_read_dispatched": true,
                            "cleanup": cleanup.1
                        }),
                    )
                    .await?;
                }
                Ok(store::FinalizeRestoreRehearsalResult::FencedOut) => {
                    let detail = "atomic restore-rehearsal finalization was fenced out after physical media work; drive and cartridge require reconciliation";
                    let _ =
                        store::quarantine_fenced_restore_rehearsal_run(pool, claim, detail).await;
                    return Err(anyhow::anyhow!(detail));
                }
                Err(error) => {
                    finish_or_quarantine(
                        pool,
                        claim,
                        store::RestoreRehearsalTerminalState::Failed,
                        json!({
                            "detail": format!("could not retain and atomically finalize restore-rehearsal pass evidence: {error}"),
                            "position_uncertain": false,
                            "tape_read_dispatched": true,
                            "cleanup": cleanup.1,
                            "runtime": runtime_evidence,
                            "media_mutation_commands_dispatched": false,
                            "restore_verification_created": false
                        }),
                    )
                    .await?;
                }
            }
        }
        Ok(_) => {
            finish_or_quarantine(
                pool,
                claim,
                store::RestoreRehearsalTerminalState::Failed,
                json!({
                    "detail": "restore and manifest comparison passed, but cleanup did not establish the exact drive at BOT",
                    "position_uncertain": true,
                    "tape_read_dispatched": true,
                    "cleanup": cleanup.1,
                    "runtime": runtime_evidence
                }),
            )
            .await?;
        }
        Err(failure) => {
            let terminal = match failure.class {
                FailureClass::Blocked if !tape_touched => {
                    store::RestoreRehearsalTerminalState::Blocked
                }
                FailureClass::Cancelled => store::RestoreRehearsalTerminalState::Cancelled,
                FailureClass::Interrupted => store::RestoreRehearsalTerminalState::Interrupted,
                FailureClass::Blocked | FailureClass::Failed => {
                    store::RestoreRehearsalTerminalState::Failed
                }
            };
            let result = json!({
                "detail": failure.detail,
                "position_uncertain": tape_touched && !cleanup.0,
                "tape_read_dispatched": tape_touched,
                "cleanup": cleanup.1,
                "runtime": runtime_evidence,
                "media_mutation_commands_dispatched": false,
                "restore_verification_created": false
            });
            if evidence_directory.is_dir() {
                if let Ok(artifact) =
                    persist_json_artifact(evidence_directory, "evidence-report", &result)
                {
                    let _ = append_json_artifact(
                        pool,
                        claim,
                        "evidence_report",
                        &artifact,
                        json!({"complete": false, "terminal": format!("{terminal:?}")}),
                    )
                    .await;
                }
            }
            finish_or_quarantine(pool, claim, terminal, result).await?;
        }
    }
    Ok(())
}

fn validate_restore_volume_ledger(
    claim: &store::ClaimedRestoreRehearsalRun,
    volumes: &[store::RestoreRehearsalVolumeRecord],
) -> Result<Vec<VolumeLabel>, RehearsalFailure> {
    let expected_count = usize::try_from(claim.run.volume_count)
        .map_err(|_| RehearsalFailure::blocked("restore volume count is invalid"))?;
    if volumes.is_empty() || volumes.len() != expected_count {
        return Err(RehearsalFailure::blocked(format!(
            "restore volume ledger contains {} rows but the fenced run requires {expected_count}",
            volumes.len()
        )));
    }

    let mut labels = Vec::with_capacity(volumes.len());
    for (index, volume) in volumes.iter().enumerate() {
        let expected_order = i32::try_from(index + 1)
            .map_err(|_| RehearsalFailure::blocked("restore volume order overflowed"))?;
        if volume.restore_rehearsal_id != claim.run.id || volume.volume_order != expected_order {
            return Err(RehearsalFailure::blocked(
                "restore volume ledger is not a contiguous ordered set for this run",
            ));
        }
        if !volume.active || volume.expected_volume_label != volume.expected_on_tape_identity {
            return Err(RehearsalFailure::blocked(format!(
                "restore volume {} is inactive or its Bacula and on-tape identities disagree",
                volume.volume_order
            )));
        }
        if volume.volume_order == 1 {
            if volume.cartridge_id != claim.run.cartridge_id
                || volume.expected_volume_label != claim.run.expected_volume_label
                || volume.expected_on_tape_identity != claim.run.expected_on_tape_identity
                || volume.expected_engine_media_id != claim.run.expected_engine_media_id
                || volume.expected_engine_record_id != claim.run.expected_engine_record_id
                || volume.state != "loaded"
                || volume.media_lease_id != Some(claim.media_lease_id)
                || volume.media_fencing_token != Some(claim.media_fencing_token)
            {
                return Err(RehearsalFailure::blocked(
                    "the first restore volume no longer matches the fenced loaded-media binding",
                ));
            }
        } else if volume.state != "reserved"
            || volume.media_lease_id.is_some()
            || volume.media_fencing_token.is_some()
        {
            return Err(RehearsalFailure::blocked(format!(
                "restore volume {} was not an untouched reserved continuation volume",
                volume.volume_order
            )));
        }
        labels.push(
            VolumeLabel::parse(volume.expected_volume_label.clone())
                .map_err(|error| RehearsalFailure::blocked(error.to_string()))?,
        );
    }
    Ok(labels)
}

fn validate_restore_catalog_volumes(
    expected: &[store::RestoreRehearsalVolumeRecord],
    catalog: &BaculaBackupCatalogEvidence,
    backup_job_id: BaculaJobId,
) -> Result<Vec<RestoreVolumeEvidence>, RehearsalFailure> {
    if catalog.volumes.len() != expected.len() {
        return Err(RehearsalFailure::blocked(format!(
            "Bacula backup uses {} volumes but the retained restore ledger requires {}",
            catalog.volumes.len(),
            expected.len()
        )));
    }
    let mut evidence = Vec::with_capacity(expected.len());
    for (expected, observed) in expected.iter().zip(&catalog.volumes) {
        let expected_media_id = u64::try_from(expected.expected_engine_media_id).map_err(|_| {
            RehearsalFailure::blocked(format!(
                "restore volume {} has an invalid retained Bacula MediaId",
                expected.volume_order
            ))
        })?;
        let expected_record = format!(
            "bacula-media:{expected_media_id}:job:{}",
            backup_job_id.get()
        );
        if observed.volume_name != expected.expected_volume_label
            || observed.media_id != expected_media_id
            || expected.expected_engine_record_id != expected_record
        {
            return Err(RehearsalFailure::blocked(format!(
                "Bacula catalog binding changed for retained restore volume {}",
                expected.volume_order
            )));
        }
        evidence.push(RestoreVolumeEvidence {
            volume_order: expected.volume_order,
            cartridge_label: expected.expected_cartridge_label.clone(),
            volume_label: expected.expected_volume_label.clone(),
            engine_media_id: expected_media_id,
        });
    }
    Ok(evidence)
}

fn validate_contract(
    config: &Config,
    run: &store::RestoreRehearsalRunRecord,
) -> Result<(), RehearsalFailure> {
    if config.source_root != Path::new("/source") || config.restore_root != Path::new("/restore") {
        return Err(RehearsalFailure::blocked(
            "the pinned Bacula restore adapter requires TAPESCALE_SOURCE_ROOT=/source and TAPESCALE_RESTORE_ROOT=/restore",
        ));
    }
    if run.engine_name != "Bacula Community"
        || run.engine_version != "15.0.3"
        || run.adapter_version != "g0.2-lab-v1"
        || run.format_version != "bacula-community-15.0.3-lab-v1"
    {
        return Err(RehearsalFailure::blocked(
            "the recovery point is not bound to the supported pinned Bacula adapter and format",
        ));
    }
    let source = Path::new(&run.source_mount_path);
    if source == config.source_root || !source.starts_with(&config.source_root) {
        return Err(RehearsalFailure::blocked(
            "the retained source selection is not a contained child of /source",
        ));
    }
    BaculaRestoreSubpath::parse(&run.destination_subpath)
        .map_err(|error| RehearsalFailure::blocked(error.to_string()))?;
    positive_bacula_job_id(run.backup_job_id)?;
    if run.expected_engine_media_id <= 0 {
        return Err(RehearsalFailure::blocked(
            "the retained engine media identity is missing",
        ));
    }
    Ok(())
}

fn prepare_evidence_directory(
    evidence_root: &Path,
    evidence_directory: &Path,
) -> Result<(), RehearsalFailure> {
    let parent = evidence_root.join("restore-rehearsals");
    fs::create_dir_all(&parent).map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not create restore-rehearsal evidence root {}: {error}",
            parent.display()
        ))
    })?;
    if evidence_directory.parent() != Some(parent.as_path()) {
        return Err(RehearsalFailure::blocked(
            "restore-rehearsal evidence path escaped its configured root",
        ));
    }
    fs::create_dir(evidence_directory).map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not create fresh restore-rehearsal evidence directory {}: {error}",
            evidence_directory.display()
        ))
    })
}

fn load_verified_source_manifest(
    evidence_root: &Path,
    reference: &ManifestReference,
) -> Result<FileManifest, RehearsalFailure> {
    let expected_run_directory = evidence_root.join(reference.original_data_run_id.to_string());
    let expected = expected_run_directory.join("source-manifest.json");
    if Path::new(&reference.storage_ref) != expected {
        return Err(RehearsalFailure::blocked(
            "retained source-manifest reference did not match the originating run's evidence path",
        ));
    }
    let expected_size = u64::try_from(reference.size_bytes)
        .map_err(|_| RehearsalFailure::blocked("retained source-manifest size is invalid"))?;
    if expected_size == 0 || expected_size > store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES {
        return Err(RehearsalFailure::blocked(format!(
            "retained source manifest exceeds the {} byte read bound",
            store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES
        )));
    }
    let run_directory_metadata =
        fs::symlink_metadata(&expected_run_directory).map_err(|error| {
            RehearsalFailure::blocked(format!(
                "originating run evidence directory {} is unavailable: {error}",
                expected_run_directory.display()
            ))
        })?;
    if !run_directory_metadata.is_dir() || run_directory_metadata.file_type().is_symlink() {
        return Err(RehearsalFailure::blocked(
            "originating run evidence directory is not a direct directory",
        ));
    }
    let metadata = fs::symlink_metadata(&expected).map_err(|error| {
        RehearsalFailure::blocked(format!(
            "retained source manifest {} is unavailable: {error}",
            expected.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != expected_size {
        return Err(RehearsalFailure::blocked(
            "retained source-manifest file type or size no longer matches its catalog record",
        ));
    }
    let canonical_root = evidence_root.canonicalize().map_err(|error| {
        RehearsalFailure::blocked(format!("could not resolve the evidence root: {error}"))
    })?;
    let canonical_run_directory = expected_run_directory.canonicalize().map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not resolve the originating run evidence directory: {error}"
        ))
    })?;
    if canonical_run_directory.parent() != Some(canonical_root.as_path()) {
        return Err(RehearsalFailure::blocked(
            "originating run evidence directory escaped the configured evidence root",
        ));
    }
    let canonical_manifest = expected.canonicalize().map_err(|error| {
        RehearsalFailure::blocked(format!("could not resolve the source manifest: {error}"))
    })?;
    if canonical_manifest.parent() != Some(canonical_run_directory.as_path())
        || canonical_manifest
            .file_name()
            .and_then(|name| name.to_str())
            != Some("source-manifest.json")
    {
        return Err(RehearsalFailure::blocked(
            "retained source-manifest path escaped the originating run evidence directory",
        ));
    }
    let manifest = load_authenticated_manifest(
        &canonical_manifest,
        expected_size,
        &reference.sha256,
        "retained source",
    )?;
    if manifest.digest_sha256 != reference.manifest_digest {
        return Err(RehearsalFailure::blocked(format!(
            "retained source-manifest digest mismatch: expected {}, observed {}",
            reference.manifest_digest, manifest.digest_sha256
        )));
    }
    validate_manifest_paths(&manifest)?;
    Ok(manifest)
}

fn load_verified_restored_manifest(
    evidence_directory: &Path,
    artifact: &ManifestArtifactSummary,
) -> Result<FileManifest, RehearsalFailure> {
    let expected = evidence_directory.join("restored-manifest.json");
    if artifact.storage_path != expected {
        return Err(RehearsalFailure::failed(
            "restored-manifest reference escaped its restore-rehearsal evidence directory",
        ));
    }
    let directory_metadata = fs::symlink_metadata(evidence_directory).map_err(|error| {
        RehearsalFailure::failed(format!(
            "restore-rehearsal evidence directory {} is unavailable: {error}",
            evidence_directory.display()
        ))
    })?;
    if !directory_metadata.is_dir() || directory_metadata.file_type().is_symlink() {
        return Err(RehearsalFailure::failed(
            "restore-rehearsal evidence directory is not a direct directory",
        ));
    }
    let metadata = fs::symlink_metadata(&expected).map_err(|error| {
        RehearsalFailure::failed(format!(
            "restored manifest {} is unavailable: {error}",
            expected.display()
        ))
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != artifact.size_bytes
    {
        return Err(RehearsalFailure::failed(
            "restored-manifest file type or size no longer matches its freshly written evidence",
        ));
    }
    let canonical_directory = evidence_directory.canonicalize().map_err(|error| {
        RehearsalFailure::failed(format!(
            "could not resolve the restore-rehearsal evidence directory: {error}"
        ))
    })?;
    let canonical_manifest = expected.canonicalize().map_err(|error| {
        RehearsalFailure::failed(format!("could not resolve the restored manifest: {error}"))
    })?;
    if canonical_manifest.parent() != Some(canonical_directory.as_path())
        || canonical_manifest
            .file_name()
            .and_then(|name| name.to_str())
            != Some("restored-manifest.json")
    {
        return Err(RehearsalFailure::failed(
            "restored-manifest path escaped its restore-rehearsal evidence directory",
        ));
    }
    let manifest = load_authenticated_manifest(
        &canonical_manifest,
        artifact.size_bytes,
        &artifact.artifact_sha256,
        "restored",
    )
    .map_err(|error| RehearsalFailure::failed(error.detail))?;
    if manifest.digest_sha256 != artifact.manifest_digest
        || manifest.entry_count != artifact.entry_count
        || manifest.file_count != artifact.file_count
        || manifest.logical_bytes != artifact.logical_bytes
        || manifest.coverage != artifact.coverage
    {
        return Err(RehearsalFailure::failed(
            "restored-manifest summary no longer matches its authenticated contents",
        ));
    }
    validate_manifest_paths(&manifest).map_err(|error| RehearsalFailure::failed(error.detail))?;
    Ok(manifest)
}

fn load_authenticated_manifest(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    description: &str,
) -> Result<FileManifest, RehearsalFailure> {
    if expected_size == 0 || expected_size > store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES {
        return Err(RehearsalFailure::blocked(format!(
            "{description} manifest exceeds the {} byte read bound",
            store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES
        )));
    }
    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not safely open the {description} manifest: {error}"
        ))
    })?;
    let mut file = fs::File::from(descriptor);
    let opened_metadata = file.metadata().map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not inspect the opened {description} manifest: {error}"
        ))
    })?;
    if !opened_metadata.is_file() || opened_metadata.len() != expected_size {
        return Err(RehearsalFailure::blocked(format!(
            "opened {description} manifest type or size no longer matches its evidence record"
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(expected_size).unwrap_or(0));
    file.by_ref()
        .take(expected_size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            RehearsalFailure::blocked(format!(
                "could not read the {description} manifest: {error}"
            ))
        })?;
    if u64::try_from(bytes.len()).ok() != Some(expected_size) {
        return Err(RehearsalFailure::blocked(format!(
            "{description} manifest length changed while it was read"
        )));
    }
    let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if actual_sha256 != expected_sha256 {
        return Err(RehearsalFailure::blocked(format!(
            "{description} manifest artifact SHA-256 mismatch: expected {expected_sha256}, observed {actual_sha256}"
        )));
    }
    parse_manifest_bytes(&bytes)
}

fn parse_manifest_bytes(bytes: &[u8]) -> Result<FileManifest, RehearsalFailure> {
    let manifest: FileManifest = serde_json::from_slice(bytes).map_err(|error| {
        RehearsalFailure::blocked(format!("manifest evidence is invalid JSON: {error}"))
    })?;
    if manifest.format != "tapescale-file-manifest-v2" {
        return Err(RehearsalFailure::blocked(
            "manifest evidence uses an unsupported format",
        ));
    }
    let entry_count = u64::try_from(manifest.entries.len())
        .map_err(|_| RehearsalFailure::blocked("manifest evidence entry count is out of range"))?;
    let file_count = u64::try_from(
        manifest
            .entries
            .iter()
            .filter(|entry| matches!(entry.kind, crate::data_path::EntryKind::File))
            .count(),
    )
    .map_err(|_| RehearsalFailure::blocked("manifest evidence file count is out of range"))?;
    let logical_bytes = manifest
        .entries
        .iter()
        .filter_map(|entry| entry.size)
        .try_fold(0_u64, u64::checked_add)
        .ok_or_else(|| {
            RehearsalFailure::blocked("manifest evidence logical byte count overflowed")
        })?;
    if manifest.entry_count != entry_count
        || manifest.file_count != file_count
        || manifest.logical_bytes != logical_bytes
        || manifest.coverage != crate::data_path::ManifestCoverage::default()
    {
        return Err(RehearsalFailure::blocked(
            "manifest evidence counters or coverage declaration are invalid",
        ));
    }
    let entry_bytes = serde_json::to_vec(&manifest.entries).map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not validate manifest evidence entries: {error}"
        ))
    })?;
    let digest = format!("{:x}", Sha256::digest(entry_bytes));
    if digest != manifest.digest_sha256 {
        return Err(RehearsalFailure::blocked(
            "manifest evidence entry digest is invalid",
        ));
    }
    Ok(manifest)
}

fn destination_capacity_evidence(
    restore_root: &Path,
    required_content_bytes: u64,
    reserve_bytes: u64,
    manifest_logical_bytes: u64,
    plan_estimated_bytes: u64,
    plan_asserted_available_bytes: u64,
) -> Result<Value, RehearsalFailure> {
    let status = statvfs(restore_root).map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not inspect destination filesystem capacity at {}: {error}",
            restore_root.display()
        ))
    })?;
    let available_bytes = status.f_bavail.saturating_mul(status.f_frsize);
    let required_bytes = required_content_bytes.saturating_add(reserve_bytes);
    if available_bytes < required_bytes {
        return Err(RehearsalFailure::blocked(format!(
            "restore destination has {available_bytes} available bytes but requires at least {required_bytes} bytes"
        )));
    }
    Ok(json!({
        "checked_by_worker": true,
        "available_bytes": available_bytes,
        "manifest_logical_bytes": manifest_logical_bytes,
        "plan_estimated_required_bytes": plan_estimated_bytes,
        "plan_asserted_available_bytes": plan_asserted_available_bytes,
        "required_content_bytes": required_content_bytes,
        "required_free_reserve_bytes": reserve_bytes,
        "required_with_reserve_bytes": required_bytes,
        "capacity_authority": "worker statvfs; plan available_bytes retained as operator assertion only"
    }))
}

fn validate_manifest_paths(manifest: &FileManifest) -> Result<(), RehearsalFailure> {
    let mut previous: Option<&str> = None;
    let mut root_seen = false;
    for entry in &manifest.entries {
        if previous.is_some_and(|value| value >= entry.path.as_str()) {
            return Err(RehearsalFailure::blocked(
                "retained source manifest paths are duplicated or not strictly ordered",
            ));
        }
        previous = Some(&entry.path);
        if entry.path == "." {
            if root_seen {
                return Err(RehearsalFailure::blocked(
                    "retained source manifest contains multiple root entries",
                ));
            }
            root_seen = true;
            if entry.symlink_target.is_some() {
                return Err(RehearsalFailure::blocked(
                    "retained source manifest root cannot be a symlink",
                ));
            }
            continue;
        }
        let safe_path = validate_archive_path(Path::new(&entry.path)).map_err(|_| {
            RehearsalFailure::blocked(format!(
                "retained source manifest contains unsafe archive path {}",
                entry.path
            ))
        })?;
        if let Some(target) = &entry.symlink_target {
            resolve_link_target(&safe_path, Path::new(target)).map_err(|_| {
                RehearsalFailure::blocked(format!(
                    "retained source manifest contains unsafe symlink target at {}",
                    entry.path
                ))
            })?;
        }
    }
    if !root_seen {
        return Err(RehearsalFailure::blocked(
            "retained source manifest is missing its root entry",
        ));
    }
    Ok(())
}

fn positive_bacula_job_id(value: i64) -> Result<BaculaJobId, RehearsalFailure> {
    let value = u64::try_from(value)
        .map_err(|_| RehearsalFailure::blocked("retained Bacula JobId is invalid"))?;
    BaculaJobId::new(value)
        .map_err(|error| RehearsalFailure::blocked(format!("invalid Bacula JobId: {error}")))
}

fn restored_source_path(
    destination: &Path,
    source_mount_path: &Path,
) -> Result<PathBuf, RehearsalFailure> {
    let relative = source_mount_path
        .strip_prefix("/")
        .map_err(|_| RehearsalFailure::blocked("retained source path is not absolute"))?;
    Ok(destination.join(relative))
}

fn verified_restored_source_path(
    destination: &Path,
    source_mount_path: &Path,
) -> Result<PathBuf, RehearsalFailure> {
    let restored = restored_source_path(destination, source_mount_path)?;
    let destination_canonical = destination.canonicalize().map_err(|error| {
        RehearsalFailure::failed(format!(
            "could not resolve the isolated restore destination {}: {error}",
            destination.display()
        ))
    })?;
    if destination_canonical != destination {
        return Err(RehearsalFailure::failed(
            "the isolated restore destination was redirected through a symlink",
        ));
    }
    let metadata = fs::symlink_metadata(&restored).map_err(|error| {
        RehearsalFailure::failed(format!(
            "restored source root {} is unavailable: {error}",
            restored.display()
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(RehearsalFailure::failed(
            "the restored source root is not a direct directory",
        ));
    }
    let canonical = restored.canonicalize().map_err(|error| {
        RehearsalFailure::failed(format!(
            "could not resolve restored source root {}: {error}",
            restored.display()
        ))
    })?;
    if canonical != restored || !canonical.starts_with(&destination_canonical) {
        return Err(RehearsalFailure::failed(
            "the restored source root escaped or was redirected outside its isolated destination",
        ));
    }
    Ok(restored)
}

async fn scan_manifest_with_lease(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    path: PathBuf,
    evidence_directory: PathBuf,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(FileManifest, ManifestArtifactSummary), RehearsalFailure> {
    let stop = Arc::new(AtomicBool::new(false));
    let scan_stop = Arc::clone(&stop);
    let mut scan = tokio::task::spawn_blocking(move || {
        let artifact = build_and_persist_manifest_with_cancel(
            &path,
            &evidence_directory,
            "restored-manifest",
            scan_stop.as_ref(),
        )
        .map_err(|error| RehearsalFailure::failed(error.to_string()))?;
        let manifest = load_verified_restored_manifest(&evidence_directory, &artifact)?;
        Ok::<_, RehearsalFailure>((manifest, artifact))
    });
    let mut heartbeat = tokio::time::interval(LEASE_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let failure = loop {
        tokio::select! {
            result = &mut scan => {
                return result
                    .map_err(|error| RehearsalFailure::failed(format!("streaming manifest worker failed: {error}")))?;
            }
            () = cancellation.cancelled() => {
                break RehearsalFailure::interrupted(
                    "worker shutdown was requested while hashing restored data",
                );
            }
            () = tokio::time::sleep_until(deadline.into()) => {
                break RehearsalFailure::interrupted(
                    "restored-data inventory exceeded the restore-rehearsal deadline",
                );
            }
            _ = heartbeat.tick() => {
                if cancellation_requested(pool, claim).await? {
                    break RehearsalFailure::cancelled(
                        "restore rehearsal was cancelled while hashing restored data",
                    );
                }
                if let Err(error) = renew_claim(pool, claim, "while hashing restored data").await {
                    break error;
                }
            }
        }
    };
    stop.store(true, Ordering::Release);
    let _ = scan.await;
    Err(failure)
}

async fn wait_for_console(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(), RehearsalFailure> {
    let mut last_error = None;
    for _ in 0..20 {
        if cancellation.is_cancelled() {
            return Err(RehearsalFailure::interrupted(
                "worker shutdown was requested while the Bacula Director was starting",
            ));
        }
        if cancellation_requested(pool, claim).await? {
            return Err(RehearsalFailure::cancelled(
                "restore rehearsal was cancelled before Bacula became ready",
            ));
        }
        if Instant::now() >= deadline {
            break;
        }
        match adapter.job_watermark(BaculaJobResource::Restore).await {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error.to_string()),
        }
        renew_claim(pool, claim, "during Bacula startup").await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(RehearsalFailure::failed(format!(
        "Bacula Director did not become ready: {}",
        last_error.unwrap_or_else(|| "deadline reached".to_owned())
    )))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveRestoreMedia {
    request_id: Option<Uuid>,
    volume_order: i32,
    automatic_remount_attempted: bool,
}

impl ActiveRestoreMedia {
    const fn initial() -> Self {
        Self {
            request_id: None,
            volume_order: 1,
            automatic_remount_attempted: false,
        }
    }

    fn next_volume_order(self) -> Result<i32, RehearsalFailure> {
        self.volume_order
            .checked_add(1)
            .ok_or_else(|| RehearsalFailure::failed("restore volume order overflowed"))
    }

    /// Claim the one automatic mount retry permitted for this exact active
    /// volume. Repeated mount dispatch can make a rejected or wrong cartridge
    /// cycle indefinitely, so another request must fail closed and return the
    /// drive to attended reconciliation.
    fn claim_automatic_remount(&mut self) -> bool {
        if self.automatic_remount_attempted {
            false
        } else {
            self.automatic_remount_attempted = true;
            true
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoreMediaWaitDecision {
    RemountCurrent,
    AdvanceTo(i32),
}

/// Decide a restore handoff only from Bacula's exact requested VolumeName.
/// JobStatus and counters are deliberately insufficient: a normal remount,
/// startup, or rejected wrong tape can all produce the same state transitions.
fn restore_media_wait_decision(
    active: ActiveRestoreMedia,
    ordered_volumes: &[VolumeLabel],
    requested_volume: &VolumeLabel,
) -> Result<RestoreMediaWaitDecision, RehearsalFailure> {
    let current_index = usize::try_from(active.volume_order.saturating_sub(1))
        .map_err(|_| RehearsalFailure::failed("active restore volume order is invalid"))?;
    let current = ordered_volumes.get(current_index).ok_or_else(|| {
        RehearsalFailure::failed(format!(
            "active restore volume {} is outside the authorized ordered set",
            active.volume_order
        ))
    })?;
    if requested_volume == current {
        return Ok(RestoreMediaWaitDecision::RemountCurrent);
    }

    let next_volume_order = active.next_volume_order()?;
    let next_index = current_index
        .checked_add(1)
        .ok_or_else(|| RehearsalFailure::failed("restore volume sequence index overflowed"))?;
    let Some(next) = ordered_volumes.get(next_index) else {
        return Err(RehearsalFailure::failed(format!(
            "Bacula requested unexpected volume {} after the final authorized restore volume",
            requested_volume.as_str()
        )));
    };
    if requested_volume != next {
        return Err(RehearsalFailure::failed(format!(
            "Bacula requested volume {}, but ordered volume {next_volume_order} is {}",
            requested_volume.as_str(),
            next.as_str()
        )));
    }
    Ok(RestoreMediaWaitDecision::AdvanceTo(next_volume_order))
}

const fn is_restore_media_wait(state: BaculaJobState) -> bool {
    matches!(
        state,
        BaculaJobState::WaitingForMount | BaculaJobState::WaitingForMedia
    )
}

#[derive(Clone, Copy)]
struct RestoreJobMediaContext<'a> {
    job_id: BaculaJobId,
    ordered_volumes: &'a [VolumeLabel],
    pool: &'a BaculaPoolName,
}

async fn wait_for_job(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedRestoreRehearsalRun,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    media: RestoreJobMediaContext<'_>,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<BaculaJobEvidence, RehearsalFailure> {
    let job_id = media.job_id;
    let mut deadline = deadline;
    let mut consecutive_poll_failures = 0_u8;
    let mut consecutive_media_proof_failures = 0_u8;
    // The claim transaction has already fenced and marked volume one loaded.
    // Seeding it here prevents Bacula's first pre-progress `M` from incorrectly
    // requesting volume one again; that state only remounts the current tape.
    let mut active_media = Some(ActiveRestoreMedia::initial());
    let mut operator_wait_started: Option<Instant> = None;
    let mut next_volume_order = 2_i32;
    loop {
        if cancellation.is_cancelled() {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(RehearsalFailure::interrupted(format!(
                "worker shutdown requested cancellation of Bacula restore JobId {job_id}; {cancellation_result}"
            )));
        }
        if cancellation_requested(pool, claim).await? {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(RehearsalFailure::cancelled(format!(
                "operator requested cancellation of Bacula restore JobId {job_id}; {cancellation_result}"
            )));
        }
        // A pending attended media request is operator time rather than tape
        // I/O time. Cancellation and lease heartbeats remain live, but the
        // restore deadline resumes only after the exact next tape is loaded.
        if active_media.is_some() && Instant::now() >= deadline {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(RehearsalFailure::interrupted(format!(
                "Bacula restore JobId {job_id} exceeded the restore-rehearsal deadline; {cancellation_result}"
            )));
        }
        if let Err(error) = renew_claim(pool, claim, "during Bacula restore").await {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(RehearsalFailure::interrupted(format!(
                "{}; {cancellation_result}",
                error.detail
            )));
        }
        let evidence = match adapter.job_evidence(job_id).await {
            Ok(evidence) => {
                consecutive_poll_failures = 0;
                evidence
            }
            Err(error) => {
                consecutive_poll_failures += 1;
                if consecutive_poll_failures >= 3 {
                    let cancellation_result = cancel_and_observe(adapter, job_id).await;
                    return Err(RehearsalFailure::failed(format!(
                        "could not poll Bacula restore JobId {job_id} after {consecutive_poll_failures} attempts: {error}; {cancellation_result}"
                    )));
                }
                tokio::time::sleep(Duration::from_secs(
                    2_u64.pow(u32::from(consecutive_poll_failures)),
                ))
                .await;
                continue;
            }
        };

        // A proof failure is meaningful only within one continuous Bacula
        // media-wait episode. Ordinary running states separate episodes and
        // must not let transient Director/SD propagation disagreements from
        // an earlier handoff consume the retry budget for a later volume.
        if !is_restore_media_wait(evidence.state) {
            consecutive_media_proof_failures = 0;
        }

        if evidence.is_terminal() {
            if !evidence.is_strict_success() {
                return Err(RehearsalFailure::failed(format!(
                    "Bacula restore JobId {job_id} ended as {:?} with {} errors",
                    evidence.state, evidence.errors
                )));
            }
            let Some(completed) = active_media.take() else {
                return Err(RehearsalFailure::failed(
                    "Bacula restore completed while waiting for an unfulfilled continuation tape",
                ));
            };
            complete_active_restore_media(pool, claim, completed).await?;
            if store::open_restore_rehearsal_media_request(pool, claim.run.id)
                .await
                .map_err(|error| {
                    RehearsalFailure::interrupted(format!(
                        "could not inspect final restore-media request state: {error}"
                    ))
                })?
                .is_some()
            {
                return Err(RehearsalFailure::failed(
                    "Bacula restore completed while a durable media request remained open",
                ));
            }
            let volumes = store::restore_rehearsal_volumes(pool, claim.run.id)
                .await
                .map_err(|error| {
                    RehearsalFailure::interrupted(format!(
                        "could not validate final restore-volume state: {error}"
                    ))
                })?;
            if volumes.len() != usize::try_from(claim.run.volume_count).unwrap_or(usize::MAX)
                || volumes
                    .iter()
                    .any(|volume| volume.state != "completed" || !volume.active)
            {
                return Err(RehearsalFailure::failed(
                    "Bacula restore completed before every ordered retained volume was durably read",
                ));
            }
            return Ok(evidence);
        }

        if is_restore_media_wait(evidence.state) {
            if let Some(active) = active_media {
                let requested_volume = match adapter.requested_volume(job_id, media.pool).await {
                    Ok(volume) => {
                        consecutive_media_proof_failures = 0;
                        volume
                    }
                    Err(error) => {
                        consecutive_media_proof_failures += 1;
                        if consecutive_media_proof_failures >= 3 {
                            let cancellation_result = cancel_and_observe(adapter, job_id).await;
                            return Err(RehearsalFailure::failed(format!(
                                "Bacula entered a media wait without one exact correlated requested VolumeName after {consecutive_media_proof_failures} attempts; refusing to advance ordered restore media: {error}; {cancellation_result}"
                            )));
                        }
                        // Director catalog status and Storage-Daemon DCR status
                        // are sampled separately. Tolerate a short propagation
                        // disagreement without changing any durable media state.
                        tokio::time::sleep(Duration::from_secs(
                            2_u64.pow(u32::from(consecutive_media_proof_failures)),
                        ))
                        .await;
                        continue;
                    }
                };
                let decision = match restore_media_wait_decision(
                    active,
                    media.ordered_volumes,
                    &requested_volume,
                ) {
                    Ok(decision) => decision,
                    Err(error) => {
                        let cancellation_result = cancel_and_observe(adapter, job_id).await;
                        return Err(RehearsalFailure::failed(format!(
                            "{}; {cancellation_result}",
                            error.detail
                        )));
                    }
                };
                match decision {
                    RestoreMediaWaitDecision::AdvanceTo(order) => {
                        let completed = active_media
                            .take()
                            .expect("the exact requested-next proof established active media");
                        next_volume_order = order;
                        complete_active_restore_media(pool, claim, completed).await?;
                    }
                    RestoreMediaWaitDecision::RemountCurrent => {
                        let automatic_remount_available = active_media
                            .as_mut()
                            .is_some_and(ActiveRestoreMedia::claim_automatic_remount);
                        if !automatic_remount_available {
                            let cancellation_result = cancel_and_observe(adapter, job_id).await;
                            return Err(RehearsalFailure::failed(format!(
                                "Bacula requested restore volume {} again after one automatic remount; refusing repeated tape motion. Verify the physical cartridge and drive state before starting a new restore; {cancellation_result}",
                                requested_volume.as_str()
                            )));
                        }
                        // Bacula is still asking for the current exact volume.
                        // Permit one read-only automatic mount, but never
                        // advance the ordered durable request or repeatedly
                        // cycle a rejected physical cartridge.
                        let mount =
                            adapter
                                .dispatch(&BaculaCommands::mount())
                                .await
                                .map_err(|error| {
                                    RehearsalFailure::failed(format!(
                                    "Bacula could not remount the current restore volume: {error}"
                                ))
                                })?;
                        validate_console_output(
                            "remount current restore volume",
                            &mount.stdout,
                            &mount.stderr,
                        )?;
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }

            let prepared_media = prepare_restore_media(
                pool,
                sas_config,
                claim,
                adapter,
                next_volume_order,
                cancellation,
            )
            .await?;
            if let Some(prepared_media) = prepared_media {
                if let Some(started) = operator_wait_started.take() {
                    let operator_wait = Instant::now().saturating_duration_since(started);
                    deadline = deadline.checked_add(operator_wait).ok_or_else(|| {
                        RehearsalFailure::interrupted(
                            "the restore deadline overflowed while accounting for an attended media wait",
                        )
                    })?;
                }
                active_media = Some(prepared_media);
            } else {
                operator_wait_started.get_or_insert_with(Instant::now);
                active_media = None;
            }
            // A pending request is attended operator time, not stalled tape
            // I/O. Cancellation and the lease heartbeat remain authoritative.
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn complete_active_restore_media(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    active: ActiveRestoreMedia,
) -> Result<(), RehearsalFailure> {
    let completed = if let Some(request_id) = active.request_id {
        store::complete_restore_rehearsal_media_request(pool, claim, request_id).await
    } else {
        store::complete_restore_rehearsal_volume(pool, claim, active.volume_order).await
    };
    match completed {
        Ok(true) => Ok(()),
        Ok(false) => Err(RehearsalFailure::interrupted(format!(
            "the fenced restore could not durably complete volume {}",
            active.volume_order
        ))),
        Err(error) => Err(RehearsalFailure::interrupted(format!(
            "could not durably complete restore volume {}: {error}",
            active.volume_order
        ))),
    }
}

async fn prepare_restore_media(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedRestoreRehearsalRun,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    next_volume_order: i32,
    cancellation: &CancellationToken,
) -> Result<Option<ActiveRestoreMedia>, RehearsalFailure> {
    let existing = store::open_restore_rehearsal_media_request(pool, claim.run.id)
        .await
        .map_err(|error| {
            RehearsalFailure::interrupted(format!(
                "could not inspect the durable restore-media request: {error}"
            ))
        })?;
    let request = if let Some(existing) = existing {
        if existing.state == "loaded" {
            return Err(RehearsalFailure::interrupted(
                "the worker lost in-memory state after a restore tape was loaded; refusing to guess whether Bacula consumed it",
            ));
        }
        if existing.volume_order != next_volume_order {
            return Err(RehearsalFailure::failed(format!(
                "the durable restore request is for volume {}, but Bacula requires volume {next_volume_order}",
                existing.volume_order
            )));
        }
        existing
    } else {
        let release = adapter
            .dispatch(&BaculaCommands::release())
            .await
            .map_err(|error| {
                RehearsalFailure::failed(format!(
                    "Bacula could not release the current restore tape before a manual change: {error}"
                ))
            })?;
        validate_console_output(
            "release current restore volume",
            &release.stdout,
            &release.stderr,
        )?;
        match store::ensure_restore_rehearsal_media_request(pool, claim, next_volume_order)
            .await
            .map_err(|error| {
                RehearsalFailure::interrupted(format!(
                    "could not retain Bacula's restore-media request: {error}"
                ))
            })? {
            store::RestoreRehearsalMediaRequestResult::Ready(request) => *request,
            store::RestoreRehearsalMediaRequestResult::FencedOut => {
                return Err(RehearsalFailure::interrupted(
                    "the restore was fenced out while requesting its next tape",
                ));
            }
            store::RestoreRehearsalMediaRequestResult::Blocked(reason) => {
                return Err(media_handoff_blocked_failure(pool, claim, reason).await);
            }
        }
    };

    let (authorized_request, authorized_volume) =
        match store::authorized_restore_rehearsal_media_request(pool, claim)
            .await
            .map_err(|error| {
                RehearsalFailure::interrupted(format!(
                    "could not inspect authorized restore media: {error}"
                ))
            })? {
            store::AuthorizedRestoreRehearsalMediaRequestResult::Waiting => return Ok(None),
            store::AuthorizedRestoreRehearsalMediaRequestResult::Authorized { request, volume } => {
                (*request, *volume)
            }
            store::AuthorizedRestoreRehearsalMediaRequestResult::LoadedAfterRestart { .. } => {
                return Err(RehearsalFailure::interrupted(
                    "a restore tape was durably loaded without matching worker state; drive position requires reconciliation",
                ));
            }
            store::AuthorizedRestoreRehearsalMediaRequestResult::FencedOut => {
                return Err(RehearsalFailure::interrupted(
                    "the restore was fenced out while inspecting its next tape",
                ));
            }
            store::AuthorizedRestoreRehearsalMediaRequestResult::Blocked(reason) => {
                return Err(media_handoff_blocked_failure(pool, claim, reason).await);
            }
        };
    if authorized_request.id != request.id
        || authorized_request.volume_order != next_volume_order
        || authorized_volume.volume_order != next_volume_order
        || authorized_request.expected_cartridge_id != authorized_volume.cartridge_id
        || authorized_request.authorized_cartridge_id != Some(authorized_volume.cartridge_id)
        || authorized_request.expected_engine_volume_name != authorized_volume.expected_volume_label
    {
        return Err(RehearsalFailure::failed(
            "the authorized restore tape does not match Bacula's exact ordered media request",
        ));
    }

    let loaded =
        match store::load_authorized_restore_rehearsal_media(pool, claim, authorized_request.id)
            .await
            .map_err(|error| {
                RehearsalFailure::interrupted(format!(
                    "could not durably accept the loaded restore tape: {error}"
                ))
            })? {
            store::LoadRestoreRehearsalMediaResult::Loaded(loaded) => *loaded,
            store::LoadRestoreRehearsalMediaResult::FencedOut => {
                return Err(RehearsalFailure::interrupted(
                    "the restore was fenced out while loading its next tape",
                ));
            }
            store::LoadRestoreRehearsalMediaResult::Blocked(reason) => {
                return Err(media_handoff_blocked_failure(pool, claim, reason).await);
            }
        };
    if loaded.request.id != authorized_request.id
        || loaded.volume.id != authorized_volume.id
        || loaded.volume.volume_order != next_volume_order
        || loaded.volume.media_lease_id != Some(loaded.media_lease_id)
        || loaded.volume.media_fencing_token != Some(loaded.media_fencing_token)
    {
        return Err(RehearsalFailure::interrupted(
            "the loaded restore tape does not match its new fenced media lease",
        ));
    }

    // The durable load commits the exact next cartridge and its new media
    // fencing token. Re-probe only after that boundary so no physical motion
    // can rely on a stale observation made before the lease was acquired.
    let observations = discover_and_persist(pool, sas_config)
        .await
        .map_err(|error| {
            RehearsalFailure::interrupted(format!(
                "could not revalidate the loaded drive after the restore-media lease was acquired: {error}"
            ))
        })?;
    let observation = matching_observation(
        &observations,
        &claim.run.expected_path,
        &claim.run.expected_stable_id,
    )
    .ok_or_else(|| {
        RehearsalFailure::blocked(
            "the exact assigned tape drive disappeared after the restore-media lease was acquired",
        )
    })?;
    if let Some(reason) = observation_blocker(observation, &claim.run) {
        return Err(RehearsalFailure::blocked(format!(
            "the fenced restore tape is not ready: {reason}"
        )));
    }

    execute_sas_motion(
        sas_config,
        Path::new(&claim.run.expected_path),
        SasMotionOperation::Rewind,
        cancellation,
    )
    .await
    .map_err(|error| {
        RehearsalFailure::failed(format!(
            "restore-tape rewind failed for volume {next_volume_order}: {error}"
        ))
    })?;
    let mount = adapter
        .dispatch(&BaculaCommands::mount())
        .await
        .map_err(|error| {
            RehearsalFailure::failed(format!(
                "Bacula could not mount restore volume {next_volume_order}: {error}"
            ))
        })?;
    validate_console_output("mount restore volume", &mount.stdout, &mount.stderr)?;

    Ok(Some(ActiveRestoreMedia {
        request_id: Some(loaded.request.id),
        volume_order: loaded.volume.volume_order,
        automatic_remount_attempted: false,
    }))
}

fn validate_console_output(
    operation: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(), RehearsalFailure> {
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let lowercase = combined.to_ascii_lowercase();
    if [
        "error:",
        "sql error",
        "fatal",
        "failed",
        "not found",
        "invalid command",
        "permission denied",
        "cannot mount",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
    {
        return Err(RehearsalFailure::failed(format!(
            "Bacula {operation} returned an error response: {}",
            concise_text(&combined, 512)
        )));
    }
    Ok(())
}

async fn record_engine_dispatch(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    dispatch: &CorrelatedJobDispatch,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
) -> Result<(), RehearsalFailure> {
    let recorded = store::append_restore_rehearsal_event(
        pool,
        claim,
        "engine_dispatched",
        json!({
            "phase": "restoring",
            "operation": "restore",
            "job_id": dispatch.job_id,
            "watermark": dispatch.watermark,
            "resource": dispatch.resource,
            "detail": format!("correlated Bacula restore JobId {}", dispatch.job_id),
            "media_mutation_allowed": false
        }),
    )
    .await;
    if matches!(recorded, Ok(true)) {
        return Ok(());
    }
    let cancellation_result = cancel_and_observe(adapter, dispatch.job_id).await;
    let persistence_detail = match recorded {
        Ok(false) => "the fenced evidence append was rejected".to_owned(),
        Err(error) => format!("the evidence append failed: {error}"),
        Ok(true) => unreachable!("successful append returned above"),
    };
    Err(RehearsalFailure::interrupted(format!(
        "the fenced run could not retain Bacula restore JobId {} because {persistence_detail}; {cancellation_result}",
        dispatch.job_id
    )))
}

async fn cancel_and_observe(
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    job_id: BaculaJobId,
) -> String {
    let dispatch = match adapter.dispatch(&BaculaCommands::cancel(job_id)).await {
        Ok(_) => "cancel command accepted".to_owned(),
        Err(error) => format!("cancel command failed: {error}"),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = None;
    while Instant::now() < deadline {
        match adapter.job_evidence(job_id).await {
            Ok(evidence) if evidence.is_terminal() => {
                return format!(
                    "{dispatch}; terminal state {:?} observed with {} errors",
                    evidence.state, evidence.errors
                );
            }
            Ok(evidence) => last = Some(format!("last state was {:?}", evidence.state)),
            Err(error) => last = Some(format!("last poll failed: {error}")),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    format!(
        "{dispatch}; terminal cancellation was not confirmed within 30 seconds ({})",
        last.unwrap_or_else(|| "no job evidence returned".to_owned())
    )
}

async fn cancellation_requested(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
) -> Result<bool, RehearsalFailure> {
    store::restore_rehearsal_cancellation_requested(pool, claim)
        .await
        .map_err(|error| {
            RehearsalFailure::interrupted(format!(
                "could not inspect restore-rehearsal cancellation state: {error}"
            ))
        })
}

async fn media_handoff_blocked_failure(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    reason: String,
) -> RehearsalFailure {
    match cancellation_requested(pool, claim).await {
        Ok(true) => RehearsalFailure::cancelled(reason),
        Ok(false) => RehearsalFailure::failed(reason),
        Err(failure) => failure,
    }
}

async fn renew_claim(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    context: &str,
) -> Result<(), RehearsalFailure> {
    match store::renew_restore_rehearsal_run_leases(pool, claim).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(RehearsalFailure::interrupted(format!(
            "the fenced drive or cartridge lease expired {context}"
        ))),
        Err(error) => Err(RehearsalFailure::interrupted(format!(
            "the drive/cartridge lease heartbeat failed {context}: {error}"
        ))),
    }
}

async fn transition(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    phase: store::RestoreRehearsalPhase,
    evidence: Value,
) -> Result<(), RehearsalFailure> {
    match store::transition_restore_rehearsal_phase(pool, claim, phase, evidence).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(RehearsalFailure::interrupted(
            "the fenced restore rehearsal rejected its phase transition",
        )),
        Err(error) => Err(RehearsalFailure::interrupted(format!(
            "could not retain restore-rehearsal phase evidence: {error}"
        ))),
    }
}

async fn append_manifest_artifact(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    kind: &str,
    artifact: &ManifestArtifactSummary,
    manifest: &FileManifest,
) -> Result<(), RehearsalFailure> {
    append_artifact(
        pool,
        claim,
        kind,
        &artifact.storage_path,
        &artifact.artifact_sha256,
        artifact.size_bytes,
        json!({
            "manifest_digest": manifest.digest_sha256,
            "entry_count": manifest.entry_count,
            "file_count": manifest.file_count,
            "logical_bytes": manifest.logical_bytes,
            "coverage": manifest.coverage
        }),
    )
    .await
}

async fn append_json_artifact(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    kind: &str,
    artifact: &JsonArtifactSummary,
    metadata: Value,
) -> Result<(), RehearsalFailure> {
    append_artifact(
        pool,
        claim,
        kind,
        &artifact.storage_path,
        &artifact.artifact_sha256,
        artifact.size_bytes,
        metadata,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn append_artifact(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    kind: &str,
    path: &Path,
    sha256: &str,
    size_bytes: u64,
    metadata: Value,
) -> Result<(), RehearsalFailure> {
    let storage_ref = path.to_str().ok_or_else(|| {
        RehearsalFailure::failed("restore-rehearsal artifact path is not valid UTF-8")
    })?;
    match store::append_restore_rehearsal_artifact(
        pool,
        claim,
        kind,
        storage_ref,
        sha256,
        size_bytes,
        metadata,
    )
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(RehearsalFailure::interrupted(format!(
            "the fenced claim rejected the {kind} artifact"
        ))),
        Err(error) => Err(RehearsalFailure::interrupted(format!(
            "could not retain the {kind} artifact: {error}"
        ))),
    }
}

async fn cleanup_drive(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedRestoreRehearsalRun,
) -> (bool, Value) {
    if let Err(error) = renew_claim(pool, claim, "before cleanup rewind").await {
        return (
            false,
            json!({
                "action": "rewind",
                "detail": error.detail,
                "position_uncertain": true
            }),
        );
    }
    let cleanup_cancellation = CancellationToken::new();
    let execution = execute_sas_motion(
        sas_config,
        Path::new(&claim.run.expected_path),
        SasMotionOperation::Rewind,
        &cleanup_cancellation,
    )
    .await;
    let after = match discover_and_persist(pool, sas_config).await {
        Ok(results) => results,
        Err(error) => {
            return (
                false,
                json!({
                    "action": "rewind",
                    "detail": error.to_string(),
                    "position_uncertain": true
                }),
            );
        }
    };
    let observation = matching_observation(
        &after,
        &claim.run.expected_path,
        &claim.run.expected_stable_id,
    );
    match (execution, observation) {
        (Ok(tool), Some(observation))
            if observation_blocker(observation, &claim.run).is_none()
                && observation.at_bot == Some(true) =>
        {
            (
                true,
                json!({
                    "action": "rewind",
                    "postcondition": "ready_at_bot",
                    "position_uncertain": false,
                    "tool": tool,
                    "after": observation.health()
                }),
            )
        }
        (Ok(tool), Some(observation)) => (
            false,
            json!({
                "action": "rewind",
                "detail": "cleanup did not establish the exact ready drive at BOT",
                "position_uncertain": true,
                "tool": tool,
                "after": observation.health()
            }),
        ),
        (Ok(tool), None) => (
            false,
            json!({
                "action": "rewind",
                "detail": "the exact drive identity disappeared after cleanup",
                "position_uncertain": true,
                "tool": tool
            }),
        ),
        (Err(error), observation) => (
            false,
            json!({
                "action": "rewind",
                "detail": format!("cleanup rewind failed: {error}"),
                "position_uncertain": true,
                "after": observation.map(|value| value.health())
            }),
        ),
    }
}

async fn discover_and_persist(
    pool: &PgPool,
    config: &SasDiscoveryConfig,
) -> Result<Vec<SasProbeResult>> {
    let results = discover_sas_drives(config).await;
    for result in &results {
        match result {
            SasProbeResult::Observed(observation) => {
                store::record_drive_observation(pool, observation).await?;
            }
            SasProbeResult::Missing(missing) => {
                store::mark_drive_missing(pool, missing).await?;
            }
        }
    }
    Ok(results)
}

fn matching_observation<'a>(
    results: &'a [SasProbeResult],
    expected_path: &str,
    expected_stable_id: &str,
) -> Option<&'a crate::device::SasDriveObservation> {
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

fn observation_blocker(
    observation: &crate::device::SasDriveObservation,
    run: &store::RestoreRehearsalRunRecord,
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
    if observation.position_lost != Some(false) {
        return Some("the kernel has not confirmed a known tape position");
    }
    if observation.vendor != run.expected_vendor
        || observation.model != run.expected_model
        || observation.serial != run.expected_serial
        || observation.wwn != run.expected_wwn
        || observation.firmware != run.expected_firmware
    {
        return Some("drive model, serial, WWN, or firmware changed before restore");
    }
    if observation.medium_loaded != Some(true) || observation.ready != Some(true) {
        return Some("no ready cartridge is loaded");
    }
    None
}

async fn finish_or_quarantine(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    terminal: store::RestoreRehearsalTerminalState,
    result: Value,
) -> Result<()> {
    match finish(pool, claim, terminal, result).await {
        Ok(true) => Ok(()),
        Ok(false) => {
            let detail = "restore-rehearsal completion was fenced out after physical media work; drive and cartridge require reconciliation";
            let _ = store::quarantine_fenced_restore_rehearsal_run(pool, claim, detail).await;
            Err(anyhow::anyhow!(detail))
        }
        Err(error) => {
            let detail = format!(
                "restore-rehearsal completion failed after physical media work; drive and cartridge require reconciliation: {error}"
            );
            let _ = store::quarantine_fenced_restore_rehearsal_run(pool, claim, &detail).await;
            Err(error)
        }
    }
}

async fn finish(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    terminal: store::RestoreRehearsalTerminalState,
    result: Value,
) -> Result<bool> {
    if !store::finish_restore_rehearsal_run(pool, claim, terminal, result.clone()).await? {
        warn!(
            restore_rehearsal_id = %claim.run.id,
            "restore-rehearsal completion was rejected by its fencing token"
        );
        return Ok(false);
    }
    warn!(
        restore_rehearsal_id = %claim.run.id,
        terminal_state = ?terminal,
        detail = %concise_text(
            result
                .get("detail")
                .and_then(|value| value.as_str())
                .unwrap_or("terminal restore-rehearsal evidence recorded"),
            512
        ),
        "restore rehearsal reached a durable terminal state"
    );
    Ok(true)
}

fn concise_text(value: &str, limit: usize) -> String {
    let mut output = value.chars().take(limit).collect::<String>();
    if value.chars().count() > limit {
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::*;
    use crate::data_path::{build_manifest, persist_manifest, MAX_G02_LOGICAL_BYTES};

    #[test]
    fn retained_manifest_requires_exact_path_size_and_artifact_digest() {
        let evidence = tempdir().expect("evidence root");
        let run_id = Uuid::now_v7();
        let original = tempdir().expect("source");
        fs::write(original.path().join("file.txt"), b"verified restore\n").expect("fixture");
        let manifest = build_manifest(original.path()).expect("manifest");
        let run_dir = evidence.path().join(run_id.to_string());
        fs::create_dir(&run_dir).expect("run directory");
        let artifact = persist_manifest(&run_dir, "source-manifest", &manifest).expect("artifact");
        let reference = ManifestReference {
            original_data_run_id: run_id,
            storage_ref: artifact.storage_path.display().to_string(),
            sha256: artifact.artifact_sha256,
            manifest_digest: manifest.digest_sha256.clone(),
            size_bytes: i64::try_from(artifact.size_bytes).expect("small fixture"),
        };

        let loaded = load_verified_source_manifest(evidence.path(), &reference).expect("verified");
        assert_eq!(loaded.digest_sha256, manifest.digest_sha256);

        let mut wrong_digest = reference.clone();
        wrong_digest.sha256 = "0".repeat(64);
        assert!(load_verified_source_manifest(evidence.path(), &wrong_digest).is_err());

        let mut wrong_path = reference;
        wrong_path.storage_ref = evidence.path().join("other.json").display().to_string();
        assert!(load_verified_source_manifest(evidence.path(), &wrong_path).is_err());
    }

    #[test]
    fn retained_manifest_accepts_the_operational_streaming_artifact() {
        let evidence = tempdir().expect("evidence root");
        let run_id = Uuid::now_v7();
        let original = tempdir().expect("source");
        fs::write(original.path().join("file.txt"), b"operational backup\n").expect("fixture");
        let run_dir = evidence.path().join(run_id.to_string());
        fs::create_dir(&run_dir).expect("run directory");
        let artifact = build_and_persist_manifest_with_cancel(
            original.path(),
            &run_dir,
            "source-manifest",
            &AtomicBool::new(false),
        )
        .expect("streaming artifact");
        let reference = ManifestReference {
            original_data_run_id: run_id,
            storage_ref: artifact.storage_path.display().to_string(),
            sha256: artifact.artifact_sha256,
            manifest_digest: artifact.manifest_digest.clone(),
            size_bytes: i64::try_from(artifact.size_bytes).expect("small fixture"),
        };

        let loaded = load_verified_source_manifest(evidence.path(), &reference)
            .expect("verified operational manifest");
        assert_eq!(loaded.digest_sha256, artifact.manifest_digest);
        assert_eq!(loaded.entry_count, artifact.entry_count);
    }

    #[test]
    fn operational_manifest_is_not_rejected_by_the_legacy_logical_size_cap() {
        let evidence = tempdir().expect("evidence root");
        let run_id = Uuid::now_v7();
        let original = tempdir().expect("source");
        fs::write(original.path().join("large-sparse-record.bin"), b"fixture").expect("fixture");
        let mut manifest = build_manifest(original.path()).expect("manifest");
        let logical_bytes = MAX_G02_LOGICAL_BYTES + 1;
        let file = manifest
            .entries
            .iter_mut()
            .find(|entry| matches!(entry.kind, crate::data_path::EntryKind::File))
            .expect("file entry");
        file.size = Some(logical_bytes);
        manifest.logical_bytes = logical_bytes;
        manifest.digest_sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&manifest.entries).expect("entry JSON"))
        );
        let run_dir = evidence.path().join(run_id.to_string());
        fs::create_dir(&run_dir).expect("run directory");
        let artifact = persist_manifest(&run_dir, "source-manifest", &manifest).expect("artifact");
        let reference = ManifestReference {
            original_data_run_id: run_id,
            storage_ref: artifact.storage_path.display().to_string(),
            sha256: artifact.artifact_sha256,
            manifest_digest: manifest.digest_sha256.clone(),
            size_bytes: i64::try_from(artifact.size_bytes).expect("small metadata fixture"),
        };

        let loaded = load_verified_source_manifest(evidence.path(), &reference)
            .expect("operational manifest above legacy logical limit");
        assert_eq!(loaded.logical_bytes, logical_bytes);
        assert_eq!(loaded.digest_sha256, manifest.digest_sha256);
    }

    #[test]
    fn restored_manifest_loader_requires_its_exact_evidence_directory() {
        let evidence = tempdir().expect("restore evidence");
        let source = tempdir().expect("source");
        fs::write(source.path().join("file.txt"), b"restored evidence").expect("fixture");
        let manifest = build_manifest(source.path()).expect("manifest");
        let artifact =
            persist_manifest(evidence.path(), "restored-manifest", &manifest).expect("artifact");

        let loaded = load_verified_restored_manifest(evidence.path(), &artifact)
            .expect("authenticated restored manifest");
        assert_eq!(loaded.digest_sha256, manifest.digest_sha256);

        let other = tempdir().expect("other evidence");
        assert!(load_verified_restored_manifest(other.path(), &artifact).is_err());
    }

    #[test]
    fn retained_manifest_rejects_a_symlink_artifact() {
        let evidence = tempdir().expect("evidence root");
        let outside = tempdir().expect("outside");
        let run_id = Uuid::now_v7();
        let run_dir = evidence.path().join(run_id.to_string());
        fs::create_dir(&run_dir).expect("run directory");
        fs::write(outside.path().join("manifest.json"), b"{}").expect("outside fixture");
        let path = run_dir.join("source-manifest.json");
        symlink(outside.path().join("manifest.json"), &path).expect("symlink");
        let reference = ManifestReference {
            original_data_run_id: run_id,
            storage_ref: path.display().to_string(),
            sha256: format!("{:x}", Sha256::digest(b"{}")),
            manifest_digest: "0".repeat(64),
            size_bytes: 2,
        };
        assert!(load_verified_source_manifest(evidence.path(), &reference).is_err());
    }

    #[test]
    fn retained_manifest_rejects_a_symlinked_origin_directory() {
        let evidence = tempdir().expect("evidence root");
        let outside = tempdir().expect("outside");
        let source = tempdir().expect("source");
        fs::write(source.path().join("file.txt"), b"retained evidence").expect("fixture");
        let manifest = build_manifest(source.path()).expect("manifest");
        let artifact =
            persist_manifest(outside.path(), "source-manifest", &manifest).expect("artifact");
        let run_id = Uuid::now_v7();
        symlink(outside.path(), evidence.path().join(run_id.to_string()))
            .expect("origin directory symlink");
        let reference = ManifestReference {
            original_data_run_id: run_id,
            storage_ref: evidence
                .path()
                .join(run_id.to_string())
                .join("source-manifest.json")
                .display()
                .to_string(),
            sha256: artifact.artifact_sha256,
            manifest_digest: manifest.digest_sha256,
            size_bytes: i64::try_from(artifact.size_bytes).expect("small fixture"),
        };

        assert!(load_verified_source_manifest(evidence.path(), &reference).is_err());
    }

    #[test]
    fn destination_capacity_uses_worker_observation_and_reserve() {
        let destination = tempdir().expect("restore root");
        let evidence = destination_capacity_evidence(destination.path(), 1, 1, 1, 0, 0)
            .expect("small restore fits");
        assert_eq!(evidence["checked_by_worker"], true);
        assert!(evidence["available_bytes"].as_u64().is_some());
        assert!(
            destination_capacity_evidence(destination.path(), u64::MAX, 1, 1, 0, u64::MAX).is_err()
        );
    }

    #[test]
    fn restored_source_path_matches_bacula_absolute_restore_layout() {
        assert_eq!(
            restored_source_path(
                Path::new("/restore/rehearsals/run"),
                Path::new("/source/pool/archive")
            )
            .expect("path"),
            PathBuf::from("/restore/rehearsals/run/source/pool/archive")
        );
    }

    #[test]
    fn restored_source_path_rejects_a_redirected_tree() {
        let destination = tempdir().expect("destination");
        let outside = tempdir().expect("outside");
        fs::create_dir(destination.path().join("source")).expect("source parent");
        symlink(
            outside.path(),
            destination.path().join("source").join("archive"),
        )
        .expect("redirect");

        assert!(
            verified_restored_source_path(destination.path(), Path::new("/source/archive"))
                .is_err()
        );
    }

    #[test]
    fn restore_media_handoff_requires_the_exact_ordered_requested_volume() {
        let active = ActiveRestoreMedia::initial();
        assert_eq!(active.request_id, None);
        assert_eq!(active.volume_order, 1);
        assert!(!active.automatic_remount_attempted);
        assert_eq!(active.next_volume_order().expect("next order"), 2);
        let ordered = [
            VolumeLabel::parse("TS-LTO-001").expect("volume one"),
            VolumeLabel::parse("TS-LTO-002").expect("volume two"),
            VolumeLabel::parse("TS-LTO-003").expect("volume three"),
        ];

        assert_eq!(
            restore_media_wait_decision(active, &ordered, &ordered[0]).expect("remount"),
            RestoreMediaWaitDecision::RemountCurrent
        );
        assert_eq!(
            restore_media_wait_decision(active, &ordered, &ordered[1]).expect("handoff"),
            RestoreMediaWaitDecision::AdvanceTo(2)
        );
        assert!(restore_media_wait_decision(active, &ordered, &ordered[2]).is_err());

        let second = ActiveRestoreMedia {
            request_id: Some(Uuid::now_v7()),
            volume_order: 2,
            automatic_remount_attempted: false,
        };
        assert_eq!(
            restore_media_wait_decision(second, &ordered, &ordered[1]).expect("remount"),
            RestoreMediaWaitDecision::RemountCurrent
        );
        assert_eq!(
            restore_media_wait_decision(second, &ordered, &ordered[2]).expect("handoff"),
            RestoreMediaWaitDecision::AdvanceTo(3)
        );

        let final_volume = ActiveRestoreMedia {
            request_id: Some(Uuid::now_v7()),
            volume_order: 3,
            automatic_remount_attempted: false,
        };
        let unexpected = VolumeLabel::parse("TS-LTO-004").expect("unexpected volume");
        assert!(restore_media_wait_decision(final_volume, &ordered, &unexpected).is_err());
    }

    #[test]
    fn restore_media_allows_only_one_automatic_remount_per_active_volume() {
        let mut active = ActiveRestoreMedia::initial();
        assert!(active.claim_automatic_remount());
        assert!(active.automatic_remount_attempted);
        assert!(!active.claim_automatic_remount());

        let mut next = ActiveRestoreMedia {
            request_id: Some(Uuid::now_v7()),
            volume_order: 2,
            automatic_remount_attempted: false,
        };
        assert!(next.claim_automatic_remount());
        assert!(!next.claim_automatic_remount());
    }
}
