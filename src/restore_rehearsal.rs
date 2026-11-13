//! Attended, read-only recovery rehearsal for an already verified G0.2 point.
//!
//! This path deliberately does not generalize the Bacula laboratory adapter. It accepts only the
//! exact full, single-volume job retained by a passed G0.2 exercise, recreates that exercise's
//! fixed Bacula resources, restores the complete job into a fresh server-generated directory, and
//! independently compares the result with the retained source manifest. It never labels, erases,
//! writes, or otherwise prepares media.

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
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    bacula_catalog::{validate_backup_job, validate_restore_job, BaculaRestoreCatalogEvidence},
    bacula_runtime::{BaculaRuntime, BaculaRuntimeEvidence},
    config::Config,
    data_path::{
        build_manifest_with_cancel, compare_manifests, create_isolated_restore_destination,
        persist_json_artifact, persist_manifest, FileManifest, JsonArtifactSummary,
        ManifestArtifactSummary, ManifestComparison, MAX_G02_LOGICAL_BYTES,
        MAX_G02_MANIFEST_ENTRIES,
    },
    device::{
        discover_sas_drives, execute_sas_motion, SasDiscoveryConfig, SasMotionOperation,
        SasProbeResult,
    },
    domain::path::{resolve_link_target, validate_archive_path},
    engine::bacula::{
        BaculaAdapter, BaculaCommands, BaculaJobEvidence, BaculaJobId, BaculaJobResource,
        BaculaPaths, BaculaPoolName, BaculaRestoreSubpath, CorrelatedJobDispatch,
        DirectBconsoleExecutor, VolumeLabel,
    },
    store,
};

const MAX_RETAINED_MANIFEST_BYTES: u64 = 256 * 1024 * 1024;
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
            "the attended G0.2 restore-rehearsal gate was disabled before worker execution",
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

        if claim.run.expected_volume_label != claim.run.expected_on_tape_identity {
            return Err(RehearsalFailure::blocked(
                "the retained Bacula volume label and on-tape identity disagree",
            ));
        }
        let backup_job_id = positive_bacula_job_id(claim.run.backup_job_id)?;
        let volume = VolumeLabel::parse(claim.run.expected_volume_label.clone())
            .map_err(|error| RehearsalFailure::blocked(error.to_string()))?;
        let bacula_pool = BaculaPoolName::for_run(claim.run.original_data_run_id);
        let backup_catalog = validate_backup_job(pool, backup_job_id, &volume, &bacula_pool)
            .await
            .map_err(|error| {
                RehearsalFailure::blocked(format!(
                    "the retained Bacula backup/catalog binding is no longer valid: {error}"
                ))
            })?;
        let expected_engine_media_id = u64::try_from(claim.run.expected_engine_media_id)
            .map_err(|_| RehearsalFailure::blocked("the retained engine media id is invalid"))?;
        if backup_catalog.volume.media_id != expected_engine_media_id {
            return Err(RehearsalFailure::blocked(format!(
                "Bacula media id changed: expected {expected_engine_media_id}, observed {}",
                backup_catalog.volume.media_id
            )));
        }

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
        runtime = Some(
            BaculaRuntime::start(
                pool,
                database_url,
                claim.run.original_data_run_id,
                paths,
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
            claim,
            &adapter,
            restore_dispatch.job_id,
            deadline,
            cancellation,
        )
        .await?;
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
            "source_backup_catalog": backup_catalog,
            "restore_job": restore,
            "restore_catalog": restore_catalog,
            "volume": volume.as_str(),
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
        let restored_manifest = scan_manifest_with_lease(
            pool,
            claim,
            restored_source_root.clone(),
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
        let restored_artifact = persist_manifest(
            evidence_directory,
            "restored-manifest",
            &restored_manifest,
        )
        .map_err(|error| RehearsalFailure::failed(error.to_string()))?;
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
                "scope": "attended G0.2 verified-point full-file restore rehearsal",
                "production_eligible": false,
                "media_operation": "read_only",
                "recovery_point_id": claim.run.recovery_point_id,
                "source_data_run_id": claim.run.original_data_run_id,
                "source_backup_job_id": claim.run.backup_job_id,
                "engine_operation_id": format!("bacula-job:{}", completed.restore.job_id),
                "restore_job_id": completed.restore.job_id.get(),
                "restore_job": completed.restore,
                "restore_catalog": completed.restore_catalog,
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
            "the recovery point is not bound to the pinned G0.2 Bacula adapter and format",
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
            "retained source-manifest reference did not match the original G0.2 evidence path",
        ));
    }
    let expected_size = u64::try_from(reference.size_bytes)
        .map_err(|_| RehearsalFailure::blocked("retained source-manifest size is invalid"))?;
    if expected_size == 0 || expected_size > MAX_RETAINED_MANIFEST_BYTES {
        return Err(RehearsalFailure::blocked(format!(
            "retained source manifest exceeds the {} byte read bound",
            MAX_RETAINED_MANIFEST_BYTES
        )));
    }
    let run_directory_metadata =
        fs::symlink_metadata(&expected_run_directory).map_err(|error| {
            RehearsalFailure::blocked(format!(
                "original G0.2 evidence directory {} is unavailable: {error}",
                expected_run_directory.display()
            ))
        })?;
    if !run_directory_metadata.is_dir() || run_directory_metadata.file_type().is_symlink() {
        return Err(RehearsalFailure::blocked(
            "original G0.2 evidence directory is not a direct directory",
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
            "could not resolve the original G0.2 evidence directory: {error}"
        ))
    })?;
    if canonical_run_directory.parent() != Some(canonical_root.as_path()) {
        return Err(RehearsalFailure::blocked(
            "original G0.2 evidence directory escaped the configured evidence root",
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
            "retained source-manifest path escaped the original G0.2 evidence directory",
        ));
    }
    let descriptor = open(
        &canonical_manifest,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not safely open the retained source manifest: {error}"
        ))
    })?;
    let mut file = fs::File::from(descriptor);
    let opened_metadata = file.metadata().map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not inspect the opened source manifest: {error}"
        ))
    })?;
    if !opened_metadata.is_file() || opened_metadata.len() != expected_size {
        return Err(RehearsalFailure::blocked(
            "opened source-manifest file type or size no longer matches its catalog record",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(expected_size).unwrap_or(0));
    file.by_ref()
        .take(expected_size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            RehearsalFailure::blocked(format!(
                "could not read the retained source manifest: {error}"
            ))
        })?;
    if u64::try_from(bytes.len()).ok() != Some(expected_size) {
        return Err(RehearsalFailure::blocked(
            "retained source-manifest length changed while it was read",
        ));
    }
    let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if actual_sha256 != reference.sha256 {
        return Err(RehearsalFailure::blocked(format!(
            "retained source-manifest artifact SHA-256 mismatch: expected {}, observed {actual_sha256}",
            reference.sha256
        )));
    }
    let manifest = parse_manifest_bytes(&bytes)?;
    if manifest.digest_sha256 != reference.manifest_digest {
        return Err(RehearsalFailure::blocked(format!(
            "retained source-manifest digest mismatch: expected {}, observed {}",
            reference.manifest_digest, manifest.digest_sha256
        )));
    }
    validate_manifest_paths(&manifest)?;
    Ok(manifest)
}

fn parse_manifest_bytes(bytes: &[u8]) -> Result<FileManifest, RehearsalFailure> {
    let manifest: FileManifest = serde_json::from_slice(bytes).map_err(|error| {
        RehearsalFailure::blocked(format!("retained source manifest is invalid JSON: {error}"))
    })?;
    if manifest.format != "tapescale-file-manifest-v2" {
        return Err(RehearsalFailure::blocked(
            "retained source manifest uses an unsupported format",
        ));
    }
    let entry_count = u64::try_from(manifest.entries.len()).map_err(|_| {
        RehearsalFailure::blocked("retained source manifest entry count is out of range")
    })?;
    let file_count = u64::try_from(
        manifest
            .entries
            .iter()
            .filter(|entry| matches!(entry.kind, crate::data_path::EntryKind::File))
            .count(),
    )
    .map_err(|_| {
        RehearsalFailure::blocked("retained source manifest file count is out of range")
    })?;
    let logical_bytes = manifest
        .entries
        .iter()
        .filter_map(|entry| entry.size)
        .try_fold(0_u64, u64::checked_add)
        .ok_or_else(|| {
            RehearsalFailure::blocked("retained source manifest logical byte count overflowed")
        })?;
    if manifest.entry_count != entry_count
        || manifest.file_count != file_count
        || manifest.logical_bytes != logical_bytes
        || manifest.coverage != crate::data_path::ManifestCoverage::default()
    {
        return Err(RehearsalFailure::blocked(
            "retained source manifest counters or coverage declaration are invalid",
        ));
    }
    let entry_bytes = serde_json::to_vec(&manifest.entries).map_err(|error| {
        RehearsalFailure::blocked(format!(
            "could not validate source manifest entries: {error}"
        ))
    })?;
    let digest = format!("{:x}", Sha256::digest(entry_bytes));
    if digest != manifest.digest_sha256 {
        return Err(RehearsalFailure::blocked(
            "retained source manifest entry digest is invalid",
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
    if manifest.entry_count > MAX_G02_MANIFEST_ENTRIES
        || manifest.logical_bytes > MAX_G02_LOGICAL_BYTES
    {
        return Err(RehearsalFailure::blocked(
            "retained source manifest exceeds the G0.2 entry or logical-byte bound",
        ));
    }
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
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<FileManifest, RehearsalFailure> {
    let stop = Arc::new(AtomicBool::new(false));
    let scan_stop = Arc::clone(&stop);
    let mut scan =
        tokio::task::spawn_blocking(move || build_manifest_with_cancel(&path, scan_stop.as_ref()));
    let mut heartbeat = tokio::time::interval(LEASE_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let failure = loop {
        tokio::select! {
            result = &mut scan => {
                return result
                    .map_err(|error| RehearsalFailure::failed(format!("manifest worker failed: {error}")))?
                    .map_err(|error| RehearsalFailure::failed(error.to_string()));
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

async fn wait_for_job(
    pool: &PgPool,
    claim: &store::ClaimedRestoreRehearsalRun,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    job_id: BaculaJobId,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<BaculaJobEvidence, RehearsalFailure> {
    let mut consecutive_poll_failures = 0_u8;
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
        if Instant::now() >= deadline {
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
        if evidence.is_terminal() {
            return if evidence.is_strict_success() {
                Ok(evidence)
            } else {
                Err(RehearsalFailure::failed(format!(
                    "Bacula restore JobId {job_id} ended as {:?} with {} errors",
                    evidence.state, evidence.errors
                )))
            };
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
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
    use crate::data_path::build_manifest;

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
}
