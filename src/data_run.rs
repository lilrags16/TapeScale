//! G0.2 attended single-cartridge physical recovery exercise.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    bacula_catalog::{
        validate_backup_job, validate_labeled_volume, validate_restore_job, validate_verify_job,
        BaculaBackupCatalogEvidence, BaculaRestoreCatalogEvidence, BaculaVerifyCatalogEvidence,
    },
    bacula_runtime::{BaculaRuntime, BaculaRuntimeEvidence},
    config::Config,
    data_path::{
        build_manifest_with_cancel, compare_manifests, create_isolated_restore_destination,
        persist_json_artifact, persist_manifest, source_manifests_stable, verify_read_only_mount,
        FileManifest, JsonArtifactSummary, ManifestArtifactSummary, ManifestComparison,
    },
    device::{
        discover_sas_drives, execute_sas_motion, SasDiscoveryConfig, SasMotionError,
        SasMotionOperation, SasProbeResult,
    },
    engine::bacula::{
        BaculaAdapter, BaculaCommands, BaculaJobEvidence, BaculaJobId, BaculaJobResource,
        BaculaPaths, BaculaPoolName, BaculaRestoreSubpath, CorrelatedJobDispatch,
        DirectBconsoleExecutor, VolumeLabel,
    },
    store,
    tool_runner::ToolError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureClass {
    Blocked,
    Failed,
    Interrupted,
}

#[derive(Debug)]
struct ExerciseFailure {
    class: FailureClass,
    detail: String,
}

impl ExerciseFailure {
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
}

#[derive(Debug)]
struct CompletedExercise {
    source_manifest: FileManifest,
    restored_manifest: FileManifest,
    comparison: ManifestComparison,
    backup: BaculaJobEvidence,
    backup_catalog: BaculaBackupCatalogEvidence,
    verify: BaculaJobEvidence,
    verify_catalog: BaculaVerifyCatalogEvidence,
    restore: BaculaJobEvidence,
    restore_catalog: BaculaRestoreCatalogEvidence,
    engine_volume_name: String,
    restore_destination: PathBuf,
}

pub async fn process_single_tape_data_runs(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    worker_id: Uuid,
    cancellation: &CancellationToken,
) -> Result<()> {
    let expired = store::recover_expired_single_tape_data_runs(pool).await?;
    if expired > 0 {
        warn!(
            expired_runs = expired,
            "expired physical recovery exercises require drive reconciliation"
        );
    }
    if !config.lab_file_roundtrip_enabled {
        let blocked = store::block_queued_single_tape_data_runs(
            pool,
            "the G0.2 laboratory file roundtrip was disabled before worker execution",
        )
        .await?;
        if blocked > 0 {
            warn!(
                blocked_runs = blocked,
                "queued recovery exercises were blocked"
            );
        }
        return Ok(());
    }
    if cancellation.is_cancelled() {
        return Ok(());
    }
    match store::claim_next_single_tape_data_run(pool, worker_id).await? {
        store::ClaimSingleTapeDataRunResult::Claimed(claim) => {
            execute_claimed(pool, sas_config, config, &claim, cancellation).await?;
        }
        store::ClaimSingleTapeDataRunResult::Blocked { id, reason } => {
            warn!(recovery_exercise_id = %id, %reason, "recovery exercise was blocked during worker revalidation");
        }
        store::ClaimSingleTapeDataRunResult::Empty => {}
    }
    Ok(())
}

async fn execute_claimed(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
) -> Result<()> {
    let before = discover_and_persist(pool, sas_config).await?;
    let Some(before_observation) = matching_observation(
        &before,
        &claim.run.expected_path,
        &claim.run.expected_stable_id,
    ) else {
        finish(
            pool,
            claim,
            store::SingleTapeDataTerminalState::Blocked,
            json!({
                "detail": "the exact configured drive identity was not observed immediately before the physical recovery exercise",
                "command_dispatched": false,
                "position_uncertain": false
            }),
        )
        .await?;
        return Ok(());
    };
    if let Some(reason) = observation_blocker(before_observation, &claim.run) {
        finish(
            pool,
            claim,
            store::SingleTapeDataTerminalState::Blocked,
            json!({
                "detail": reason,
                "command_dispatched": false,
                "position_uncertain": false,
                "before": before_observation.health()
            }),
        )
        .await?;
        return Ok(());
    }

    let evidence_directory = config.evidence_root.join(claim.run.id.to_string());
    let (operation, runtime_evidence, tape_touched, media_write_started) = perform_exercise(
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
    complete_exercise(
        pool,
        claim,
        operation,
        runtime_evidence,
        tape_touched,
        media_write_started,
        cleanup,
        &evidence_directory,
    )
    .await
}

fn validate_paths(
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
) -> Result<(), ExerciseFailure> {
    if config.source_root != Path::new("/source") || config.restore_root != Path::new("/restore") {
        return Err(ExerciseFailure::blocked(
            "the pinned Bacula G0.2 adapter requires TAPESCALE_SOURCE_ROOT=/source and TAPESCALE_RESTORE_ROOT=/restore",
        ));
    }
    let source = Path::new(&claim.run.source_mount_path);
    let restore = Path::new(&claim.run.restore_root_path);
    if source == config.source_root
        || !source.starts_with(&config.source_root)
        || restore == config.restore_root
        || !restore.starts_with(&config.restore_root)
    {
        return Err(ExerciseFailure::blocked(
            "persisted source and restore paths are not contained child paths",
        ));
    }
    Ok(())
}

async fn perform_exercise(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    evidence_directory: &Path,
) -> (
    Result<CompletedExercise, ExerciseFailure>,
    Option<BaculaRuntimeEvidence>,
    bool,
    bool,
) {
    let deadline = Instant::now() + config.data_run_timeout;
    let mut runtime: Option<BaculaRuntime> = None;
    let mut tape_touched = false;
    let mut media_write_started = false;
    let operation = async {
        validate_paths(config, claim)?;
        verify_read_only_mount(Path::new(&claim.run.source_mount_path)).map_err(|error| {
            ExerciseFailure::blocked(format!(
                "the worker could not prove the selected source is mounted read-only: {error}"
            ))
        })?;
        if cancellation.is_cancelled() {
            return Err(ExerciseFailure::interrupted(
                "worker shutdown was requested before source inventory",
            ));
        }

        let source_manifest = scan_manifest_with_lease(
            pool,
            claim,
            PathBuf::from(&claim.run.source_mount_path),
            deadline,
            cancellation,
        )
        .await?;
        let source_artifact = persist_manifest(
            evidence_directory,
            "source-manifest",
            &source_manifest,
        )
        .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;
        append_manifest_artifact(
            pool,
            claim,
            "source_manifest",
            &source_artifact,
            &source_manifest,
        )
        .await?;

        if Instant::now() >= deadline {
            return Err(ExerciseFailure::blocked(
                "source inventory consumed the entire recovery-exercise deadline; no tape command was dispatched",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(ExerciseFailure::interrupted(
                "worker shutdown was requested before any tape command",
            ));
        }
        transition(
            pool,
            claim,
            store::SingleTapeDataPhase::Writing,
            json!({
                "detail": "source manifest retained; beginning destructive single-volume Bacula write",
                "source_manifest_sha256": source_manifest.digest_sha256,
                "file_count": source_manifest.file_count,
                "logical_bytes": source_manifest.logical_bytes,
                "production_eligible": false
            }),
        )
        .await?;

        let volume = engine_volume_label(&claim.run.expected_cartridge_label, claim.run.id)?;
        let bacula_pool = BaculaPoolName::for_run(claim.run.id);
        if !store::mark_single_tape_media_write_started(pool, claim, volume.as_str())
            .await
            .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
        {
            return Err(ExerciseFailure::interrupted(
                "the fenced run could not durably quarantine the cartridge before tape motion",
            ));
        }
        media_write_started = true;

        // Linux tape motion can finalize prior writes, so mark the cartridge as
        // write-in-progress durably before issuing even this explicit rewind.
        tape_touched = true;
        execute_sas_motion(
            sas_config,
            Path::new(&claim.run.expected_path),
            SasMotionOperation::Rewind,
            cancellation,
        )
        .await
        .map_err(|error| {
            if matches!(error, SasMotionError::Tool(ToolError::Cancelled { .. })) {
                ExerciseFailure::interrupted(format!("pre-write rewind was interrupted: {error}"))
            } else {
                ExerciseFailure::failed(format!("pre-write rewind failed: {error}"))
            }
        })?;

        let source_relative = Path::new(&claim.run.source_mount_path)
            .strip_prefix(&config.source_root)
            .map_err(|_| ExerciseFailure::blocked("source path escaped /source"))?;
        let paths = BaculaPaths::from_relative_source(
            source_relative,
            PathBuf::from(&claim.run.expected_path),
        )
        .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;
        let database_url = config.database_url.as_deref().ok_or_else(|| {
            ExerciseFailure::blocked("DATABASE_URL is required for the Bacula catalog")
        })?;
        runtime = Some(
            BaculaRuntime::start(
                pool,
                database_url,
                claim.run.id,
                paths,
                evidence_directory,
                cancellation,
            )
            .await
            .map_err(|error| ExerciseFailure::failed(error.to_string()))?,
        );

        let adapter = BaculaAdapter::new(DirectBconsoleExecutor::default());
        wait_for_console(pool, claim, &adapter, deadline, cancellation).await?;
        let label_output = adapter
            .dispatch(&BaculaCommands::label(&volume, &bacula_pool))
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!("Bacula volume labeling failed: {error}"))
            })?;
        validate_console_output("label", &label_output.stdout, &label_output.stderr)?;
        let labeled_volume = validate_labeled_volume(pool, &volume, &bacula_pool)
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "Bacula did not catalog the exact new label in its isolated one-volume pool: {error}"
                ))
            })?;

        let backup_dispatch = adapter
            .dispatch_and_correlate(BaculaJobResource::Backup, &BaculaCommands::full_backup())
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!("Bacula full backup dispatch failed: {error}"))
            })?;
        record_engine_dispatch(pool, claim, "backup", &backup_dispatch, &adapter).await?;
        let backup = wait_for_job(
            pool,
            claim,
            &adapter,
            backup_dispatch.job_id,
            "full backup",
            deadline,
            cancellation,
        )
        .await?;
        let backup_catalog = validate_backup_job(
            pool,
            backup.job_id,
            &volume,
            &bacula_pool,
        )
        .await
        .map_err(|error| {
            ExerciseFailure::failed(format!(
                "Bacula backup catalog/media proof failed: {error}"
            ))
        })?;
        let engine_write = json!({
            "engine": "Bacula Community",
            "engine_version": "15.0.3",
            "volume": volume.as_str(),
            "labeled_volume": labeled_volume,
            "job": backup,
            "catalog": backup_catalog,
            "finalized": true,
            "single_volume": backup_catalog.segments.iter().all(|segment| segment.media_id == backup_catalog.volume.media_id),
            "encryption": "disabled_by_explicit_g0_2_lab_deviation"
        });
        let engine_artifact = persist_json_artifact(
            evidence_directory,
            "engine-write",
            &engine_write,
        )
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
        append_json_artifact(
            pool,
            claim,
            "engine_write",
            &engine_artifact,
            json!({
                "finalized": true,
                "job_id": backup.job_id,
                "volume": volume.as_str(),
                "media_id": backup_catalog.volume.media_id,
                "segments": backup_catalog.segments,
                "files": backup.files,
                "job_bytes": backup.bytes,
                "volume_bytes": backup_catalog.volume.volume_bytes
            }),
        )
        .await?;
        transition(
            pool,
            claim,
            store::SingleTapeDataPhase::Written,
            json!({
                "detail": "Bacula full backup completed with zero job errors",
                "job": backup,
                "volume": volume.as_str()
            }),
        )
        .await?;
        transition(
            pool,
            claim,
            store::SingleTapeDataPhase::Verifying,
            json!({
                "detail": "starting Bacula Level=Data full physical-media readback",
                "backup_job_id": backup.job_id
            }),
        )
        .await?;

        let verify_dispatch = adapter
            .dispatch_and_correlate(
                BaculaJobResource::Verify,
                &BaculaCommands::data_verify(backup.job_id),
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!("Bacula Data verify dispatch failed: {error}"))
            })?;
        record_engine_dispatch(pool, claim, "verify", &verify_dispatch, &adapter).await?;
        let verify = wait_for_job(
            pool,
            claim,
            &adapter,
            verify_dispatch.job_id,
            "Data verify",
            deadline,
            cancellation,
        )
        .await?;
        let verify_catalog = validate_verify_job(
            pool,
            verify.job_id,
            backup.job_id,
            &bacula_pool,
        )
        .await
        .map_err(|error| {
            ExerciseFailure::failed(format!("Bacula Data verify catalog proof failed: {error}"))
        })?;
        transition(
            pool,
            claim,
            store::SingleTapeDataPhase::Restoring,
            json!({
                "detail": "physical-media Data verify passed; starting normal Bacula restore",
                "verify_job": verify,
                "physical_media_readback": true
            }),
        )
        .await?;

        let restore_relative = Path::new(&claim.run.restore_root_path)
            .strip_prefix(&config.restore_root)
            .map_err(|_| ExerciseFailure::blocked("restore path escaped /restore"))?;
        let restore_subpath = BaculaRestoreSubpath::parse(restore_relative)
            .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;
        let destination = restore_subpath.destination(claim.run.id);
        let restore_relative_text = restore_relative.to_str().ok_or_else(|| {
            ExerciseFailure::blocked("restore path must be valid UTF-8 for the pinned adapter")
        })?;
        let secured_destination = create_isolated_restore_destination(
            &config.restore_root,
            restore_relative_text,
            claim.run.id,
        )
        .map_err(|error| {
            ExerciseFailure::failed(format!(
                "could not securely create isolated restore destination {}: {error}",
                destination.path().display()
            ))
        })?;
        if secured_destination != destination.path() {
            return Err(ExerciseFailure::blocked(
                "the secured restore destination did not match the pinned Bacula path",
            ));
        }
        let restore_dispatch = adapter
            .dispatch_and_correlate(
                BaculaJobResource::Restore,
                &BaculaCommands::full_restore(backup.job_id, &destination),
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!("Bacula restore dispatch failed: {error}"))
            })?;
        record_engine_dispatch(pool, claim, "restore", &restore_dispatch, &adapter).await?;
        let restore = wait_for_job(
            pool,
            claim,
            &adapter,
            restore_dispatch.job_id,
            "full restore",
            deadline,
            cancellation,
        )
        .await?;
        let restore_catalog = validate_restore_job(
            pool,
            restore.job_id,
            backup.job_id,
            &bacula_pool,
        )
        .await
        .map_err(|error| {
            ExerciseFailure::failed(format!("Bacula restore catalog proof failed: {error}"))
        })?;
        transition(
            pool,
            claim,
            store::SingleTapeDataPhase::Comparing,
            json!({
                "detail": "Bacula restore completed; independently hashing restored files and metadata",
                "restore_job": restore,
                "restore_destination": destination.path()
            }),
        )
        .await?;

        let restored_source_root = destination.path().join(
            Path::new(&claim.run.source_mount_path)
                .strip_prefix("/")
                .map_err(|_| ExerciseFailure::failed("invalid restored source path"))?,
        );
        let restored_manifest = scan_manifest_with_lease(
            pool,
            claim,
            restored_source_root.clone(),
            deadline,
            cancellation,
        )
        .await
        .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "restored tree was not readable at {}: {}",
                    restored_source_root.display(),
                    error.detail
                ))
            })?;
        let restored_artifact = persist_manifest(
            evidence_directory,
            "restored-manifest",
            &restored_manifest,
        )
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
        append_manifest_artifact(
            pool,
            claim,
            "restored_manifest",
            &restored_artifact,
            &restored_manifest,
        )
        .await?;

        let source_after = scan_manifest_with_lease(
            pool,
            claim,
            PathBuf::from(&claim.run.source_mount_path),
            deadline,
            cancellation,
        )
        .await?;
        if !source_manifests_stable(&source_manifest, &source_after) {
            return Err(ExerciseFailure::failed(
                "the source changed during the exercise; restore proof is invalid",
            ));
        }
        let comparison = compare_manifests(&source_manifest, &restored_manifest);
        let comparison_artifact =
            persist_json_artifact(evidence_directory, "comparison", &comparison)
                .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
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
            return Err(ExerciseFailure::failed(format!(
                "restored manifest did not match the source ({} retained differences)",
                comparison.differences.len()
            )));
        }

        Ok(CompletedExercise {
            source_manifest,
            restored_manifest,
            comparison,
            backup,
            backup_catalog,
            verify,
            verify_catalog,
            restore,
            restore_catalog,
            engine_volume_name: volume.as_str().to_owned(),
            restore_destination: destination.path().to_owned(),
        })
    }
    .await;

    let runtime_evidence = if let Some(active_runtime) = runtime.take() {
        Some(active_runtime.stop().await)
    } else {
        None
    };
    (
        operation,
        runtime_evidence,
        tape_touched,
        media_write_started,
    )
}

#[allow(clippy::too_many_arguments)]
async fn complete_exercise(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    operation: Result<CompletedExercise, ExerciseFailure>,
    runtime_evidence: Option<BaculaRuntimeEvidence>,
    tape_touched: bool,
    media_write_started: bool,
    cleanup: (bool, Value),
    evidence_directory: &Path,
) -> Result<()> {
    match operation {
        Ok(completed) if cleanup.0 => {
            let first_segment = completed
                .backup_catalog
                .segments
                .first()
                .expect("catalog validation requires JobMedia evidence");
            let last_segment = completed
                .backup_catalog
                .segments
                .last()
                .expect("catalog validation requires JobMedia evidence");
            let report = json!({
                "scope": "G0.2 laboratory single-tape full-file physical recovery exercise",
                "production_eligible": false,
                "engine": {"name":"Bacula Community","version":"15.0.3"},
                "backup_job": completed.backup,
                "backup_catalog": completed.backup_catalog,
                "data_verify_job": completed.verify,
                "data_verify_catalog": completed.verify_catalog,
                "restore_job": completed.restore,
                "restore_catalog": completed.restore_catalog,
                "source_manifest_digest": completed.source_manifest.digest_sha256,
                "restored_manifest_digest": completed.restored_manifest.digest_sha256,
                "comparison": completed.comparison,
                "restore_destination": completed.restore_destination,
                "runtime": runtime_evidence,
                "cleanup": cleanup.1,
                "encryption": "disabled_by_explicit_g0_2_lab_deviation"
            });
            let finalize_attempt: Result<store::FinalizeSingleTapeDataResult, String> = async {
                let report_artifact =
                    persist_json_artifact(evidence_directory, "evidence-report", &report)
                        .map_err(|error| error.to_string())?;
                append_json_artifact(
                    pool,
                    claim,
                    "evidence_report",
                    &report_artifact,
                    json!({
                        "candidate_evidence": true,
                        "complete": false,
                        "pending_atomic_finalization": true,
                        "production_eligible": false
                    }),
                )
                .await
                .map_err(|error| error.detail)?;
                let pass = store::SingleTapePassEvidence {
                    engine_operation_id: format!("bacula-job:{}", completed.backup.job_id),
                    engine_volume_name: completed.engine_volume_name.clone(),
                    engine_record_id: format!(
                        "bacula-media:{}:job:{}",
                        completed.backup_catalog.volume.media_id, completed.backup.job_id
                    ),
                    first_record: format!(
                        "file={}:block={}",
                        first_segment.start_file, first_segment.start_block
                    ),
                    last_record: Some(format!(
                        "file={}:block={}",
                        last_segment.end_file, last_segment.end_block
                    )),
                    logical_bytes: completed.source_manifest.logical_bytes,
                    encoded_bytes: completed.backup_catalog.volume.volume_bytes,
                    file_count: completed.source_manifest.file_count,
                    coverage: serde_json::to_value(&completed.source_manifest.coverage)
                        .map_err(|error| error.to_string())?,
                    full_readback_evidence: json!({
                        "physical_media_readback": true,
                        "level": "Data",
                        "job": completed.verify,
                        "catalog": completed.verify_catalog,
                        "backup_catalog": completed.backup_catalog
                    }),
                    restore_test_evidence: json!({
                        "matched": true,
                        "job": completed.restore,
                        "catalog": completed.restore_catalog,
                        "source_manifest_digest": completed.source_manifest.digest_sha256,
                        "restored_manifest_digest": completed.restored_manifest.digest_sha256,
                        "restore_destination": completed.restore_destination
                    }),
                    result: report,
                };
                store::finalize_single_tape_data_pass(pool, claim, &pass)
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            match finalize_attempt {
                Ok(store::FinalizeSingleTapeDataResult::Passed {
                    recovery_point_id,
                    volume_record_id,
                }) => info!(
                    recovery_exercise_id = %claim.run.id,
                    %recovery_point_id,
                    %volume_record_id,
                    "first recoverable tape exercise passed and created a verified recovery point"
                ),
                Ok(store::FinalizeSingleTapeDataResult::Rejected(reason)) => {
                    finish_or_quarantine(
                        pool,
                        claim,
                        store::SingleTapeDataTerminalState::Failed,
                        json!({
                            "detail": format!("final pass evidence was rejected: {reason}"),
                            "position_uncertain": false,
                            "media_write_started": true,
                            "cleanup": cleanup.1
                        }),
                    )
                    .await?;
                }
                Ok(store::FinalizeSingleTapeDataResult::FencedOut) => {
                    let detail = "atomic pass finalization was fenced out after physical media work; drive and cartridge require reconciliation";
                    let _ = store::quarantine_fenced_single_tape_write(pool, claim, detail).await;
                    return Err(anyhow::anyhow!(detail));
                }
                Err(reason) => {
                    finish_or_quarantine(
                        pool,
                        claim,
                        store::SingleTapeDataTerminalState::Failed,
                        json!({
                            "detail": format!("could not retain and atomically finalize pass evidence: {reason}"),
                            "position_uncertain": false,
                            "media_write_started": true,
                            "cleanup": cleanup.1,
                            "verified_recovery_point_created": false
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
                store::SingleTapeDataTerminalState::Failed,
                json!({
                    "detail": "backup, physical readback, restore, and comparison passed, but the required cleanup rewind/postcondition failed",
                    "position_uncertain": true,
                    "media_write_started": true,
                    "cleanup": cleanup.1,
                    "runtime": runtime_evidence
                }),
            )
            .await?;
        }
        Err(failure) => {
            let terminal = match failure.class {
                FailureClass::Blocked if !tape_touched => {
                    store::SingleTapeDataTerminalState::Blocked
                }
                FailureClass::Interrupted => store::SingleTapeDataTerminalState::Interrupted,
                FailureClass::Blocked | FailureClass::Failed => {
                    store::SingleTapeDataTerminalState::Failed
                }
            };
            let result = json!({
                "detail": failure.detail,
                "position_uncertain": tape_touched && !cleanup.0,
                "tape_command_dispatched": tape_touched,
                "media_write_started": media_write_started,
                "cleanup": cleanup.1,
                "runtime": runtime_evidence,
                "production_eligible": false,
                "verified_recovery_point_created": false
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

async fn finish_or_quarantine(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    terminal: store::SingleTapeDataTerminalState,
    result: Value,
) -> Result<()> {
    match finish(pool, claim, terminal, result).await {
        Ok(true) => Ok(()),
        Ok(false) => {
            let detail = "terminal completion was fenced out after physical media work; drive and cartridge require reconciliation";
            let _ = store::quarantine_fenced_single_tape_write(pool, claim, detail).await;
            Err(anyhow::anyhow!(detail))
        }
        Err(error) => {
            let detail = format!(
                "terminal completion failed after physical media work; drive and cartridge require reconciliation: {error}"
            );
            let _ = store::quarantine_fenced_single_tape_write(pool, claim, &detail).await;
            Err(error)
        }
    }
}

async fn scan_manifest_with_lease(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    path: PathBuf,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<FileManifest, ExerciseFailure> {
    let stop = Arc::new(AtomicBool::new(false));
    let scan_stop = Arc::clone(&stop);
    let mut scan =
        tokio::task::spawn_blocking(move || build_manifest_with_cancel(&path, scan_stop.as_ref()));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(60));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let failure = loop {
        tokio::select! {
            result = &mut scan => {
                return result
                    .map_err(|error| ExerciseFailure::failed(format!("manifest worker failed: {error}")))?
                    .map_err(|error| ExerciseFailure::blocked(error.to_string()));
            }
            () = cancellation.cancelled() => {
                break ExerciseFailure::interrupted(
                    "worker shutdown was requested during file inventory",
                );
            }
            () = tokio::time::sleep_until(deadline.into()) => {
                break ExerciseFailure::interrupted(
                    "file inventory exceeded the recovery-exercise deadline",
                );
            }
            _ = heartbeat.tick() => {
                match store::renew_single_tape_data_run_leases(pool, claim).await {
                    Ok(true) => {}
                    Ok(false) => break ExerciseFailure::interrupted(
                        "the fenced drive or cartridge lease expired during file inventory",
                    ),
                    Err(error) => break ExerciseFailure::interrupted(format!(
                        "the drive/cartridge lease heartbeat failed during file inventory: {error}"
                    )),
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
    claim: &store::ClaimedSingleTapeDataRun,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(), ExerciseFailure> {
    let mut last_error = None;
    for _ in 0..20 {
        if cancellation.is_cancelled() {
            return Err(ExerciseFailure::interrupted(
                "worker shutdown was requested while the Bacula Director was starting",
            ));
        }
        if Instant::now() >= deadline {
            break;
        }
        match adapter.job_watermark(BaculaJobResource::Backup).await {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error.to_string()),
        }
        renew_claim(pool, claim, "during Bacula startup").await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(ExerciseFailure::failed(format!(
        "Bacula Director did not become ready: {}",
        last_error.unwrap_or_else(|| "deadline reached".to_owned())
    )))
}

#[allow(clippy::too_many_arguments)]
async fn wait_for_job(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    job_id: BaculaJobId,
    operation: &str,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<BaculaJobEvidence, ExerciseFailure> {
    let mut consecutive_poll_failures = 0_u8;
    loop {
        if cancellation.is_cancelled() {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(ExerciseFailure::interrupted(format!(
                "worker shutdown requested cancellation of Bacula {operation} JobId {job_id}; {cancellation_result}"
            )));
        }
        if Instant::now() >= deadline {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(ExerciseFailure::interrupted(format!(
                "Bacula {operation} JobId {job_id} exceeded the recovery-exercise deadline; {cancellation_result}"
            )));
        }
        if let Err(error) = renew_claim(pool, claim, &format!("during Bacula {operation}")).await {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(ExerciseFailure::interrupted(format!(
                "{}; {}",
                error.detail, cancellation_result
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
                    return Err(ExerciseFailure::failed(format!(
                        "could not poll Bacula {operation} JobId {job_id} after {consecutive_poll_failures} attempts: {error}; {cancellation_result}"
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
                Err(ExerciseFailure::failed(format!(
                    "Bacula {operation} JobId {job_id} ended as {:?} with {} errors",
                    evidence.state, evidence.errors
                )))
            };
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn record_engine_dispatch(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    operation: &str,
    dispatch: &CorrelatedJobDispatch,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
) -> Result<(), ExerciseFailure> {
    let recorded = store::append_single_tape_data_evidence_event(
        pool,
        claim,
        "engine_dispatched",
        json!({
            "phase": match operation {
                "backup" => "writing",
                "verify" => "verifying",
                "restore" => "restoring",
                _ => "engine",
            },
            "operation": operation,
            "job_id": dispatch.job_id,
            "watermark": dispatch.watermark,
            "resource": dispatch.resource,
            "detail": format!("correlated Bacula {operation} JobId {}", dispatch.job_id)
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
    Err(ExerciseFailure::interrupted(format!(
        "the fenced run could not retain Bacula {operation} JobId {} because {persistence_detail}; {cancellation_result}",
        dispatch.job_id,
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

async fn renew_claim(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    context: &str,
) -> Result<(), ExerciseFailure> {
    match store::renew_single_tape_data_run_leases(pool, claim).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ExerciseFailure::interrupted(format!(
            "the fenced drive or cartridge lease expired {context}"
        ))),
        Err(error) => Err(ExerciseFailure::interrupted(format!(
            "the drive/cartridge lease heartbeat failed {context}: {error}"
        ))),
    }
}

fn engine_volume_label(
    registered_label: &str,
    run_id: Uuid,
) -> Result<VolumeLabel, ExerciseFailure> {
    let mut normalized = registered_label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .take(96)
        .collect::<String>();
    if normalized.trim_matches('-').is_empty() {
        normalized = "TAPESCALE".to_owned();
    }
    normalized.push('-');
    normalized.push_str(&run_id.simple().to_string()[..12]);
    VolumeLabel::parse(normalized).map_err(|error| ExerciseFailure::blocked(error.to_string()))
}

fn validate_console_output(
    operation: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(), ExerciseFailure> {
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let lowercase = combined.to_ascii_lowercase();
    if [
        "error:",
        "failed",
        "not found",
        "invalid command",
        "permission denied",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
    {
        return Err(ExerciseFailure::failed(format!(
            "Bacula {operation} returned an error response: {}",
            concise_text(&combined, 512)
        )));
    }
    Ok(())
}

async fn append_manifest_artifact(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    kind: &str,
    artifact: &ManifestArtifactSummary,
    manifest: &FileManifest,
) -> Result<(), ExerciseFailure> {
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
    claim: &store::ClaimedSingleTapeDataRun,
    kind: &str,
    artifact: &JsonArtifactSummary,
    metadata: Value,
) -> Result<(), ExerciseFailure> {
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
    claim: &store::ClaimedSingleTapeDataRun,
    kind: &str,
    path: &Path,
    sha256: &str,
    size_bytes: u64,
    metadata: Value,
) -> Result<(), ExerciseFailure> {
    let storage_ref = path
        .to_str()
        .ok_or_else(|| ExerciseFailure::failed("evidence artifact path is not valid UTF-8"))?;
    if store::append_single_tape_data_artifact(
        pool,
        claim,
        kind,
        storage_ref,
        sha256,
        size_bytes,
        metadata,
    )
    .await
    .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        Ok(())
    } else {
        Err(ExerciseFailure::interrupted(format!(
            "the fenced claim rejected the {kind} evidence artifact"
        )))
    }
}

async fn transition(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    phase: store::SingleTapeDataPhase,
    evidence: Value,
) -> Result<(), ExerciseFailure> {
    if store::transition_single_tape_data_phase(pool, claim, phase, evidence)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        Ok(())
    } else {
        Err(ExerciseFailure::interrupted(
            "the fenced claim rejected a recovery-exercise phase transition",
        ))
    }
}

async fn cleanup_drive(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedSingleTapeDataRun,
) -> (bool, Value) {
    // Cleanup is intentionally independent of the shutdown token so a SIGTERM
    // still gets one bounded attempt to leave the cartridge at BOT.
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
                    "action":"rewind",
                    "detail":error.to_string(),
                    "position_uncertain":true
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
                "action":"rewind",
                "detail":"post-run rewind did not establish the exact ready drive at BOT",
                "position_uncertain":true,
                "tool":tool,
                "after":observation.health()
            }),
        ),
        (Ok(tool), None) => (
            false,
            json!({
                "action":"rewind",
                "detail":"the exact drive identity disappeared after cleanup",
                "position_uncertain":true,
                "tool":tool
            }),
        ),
        (Err(error), observation) => (
            false,
            json!({
                "action":"rewind",
                "detail":format!("post-run rewind failed: {error}"),
                "position_uncertain":true,
                "after":observation.map(|value| value.health())
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
    run: &store::SingleTapeDataRunRecord,
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
        return Some("drive model, serial, WWN, or firmware changed before the exercise");
    }
    if observation.medium_loaded != Some(true) || observation.ready != Some(true) {
        return Some("no ready cartridge is loaded");
    }
    None
}

async fn finish(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    terminal: store::SingleTapeDataTerminalState,
    result: Value,
) -> Result<bool> {
    if !store::finish_single_tape_data_run(pool, claim, terminal, result.clone()).await? {
        warn!(
            recovery_exercise_id = %claim.run.id,
            "recovery-exercise completion was rejected by its fencing token"
        );
    } else {
        warn!(
            recovery_exercise_id = %claim.run.id,
            terminal_state = ?terminal,
            detail = %concise_text(
                result
                    .get("detail")
                    .and_then(|value| value.as_str())
                    .unwrap_or("terminal evidence recorded"),
                512
            ),
            "physical recovery exercise reached a durable terminal state"
        );
        return Ok(true);
    }
    Ok(false)
}

fn concise_text(value: &str, limit: usize) -> String {
    let mut characters = value.chars().map(|character| {
        if character.is_control() {
            ' '
        } else {
            character
        }
    });
    let mut concise = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        concise.push('…');
    }
    concise
}
