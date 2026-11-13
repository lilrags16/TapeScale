//! Attended Bacula backup and physical recovery execution, including ordered multi-volume runs.

use std::{
    fs,
    num::NonZeroU64,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use rustix::fs::statvfs;
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

// Bacula documents roughly 150 bytes of attribute spool per file. TapeScale
// reserves a much more conservative 4 KiB for every manifest entry, twice the
// serialized manifest size, and a fixed base before calculating the data-spool
// ceiling. The normal free-space reserve remains on top of this estimate.
const ATTRIBUTE_SPOOL_BYTES_PER_ENTRY: u64 = 4 * 1024;
const ATTRIBUTE_SPOOL_MANIFEST_MULTIPLIER: u64 = 2;
const ATTRIBUTE_SPOOL_BASE_BYTES: u64 = 64 * 1024 * 1024;

use crate::{
    bacula_catalog::{
        validate_backup_job_on_volumes, validate_labeled_volumes, validate_restore_job,
        validate_verify_job, BaculaBackupCatalogEvidence, BaculaRestoreCatalogEvidence,
        BaculaVerifyCatalogEvidence,
    },
    bacula_runtime::{BaculaRuntime, BaculaRuntimeEvidence},
    config::Config,
    data_path::{
        build_and_persist_manifest_with_cancel, build_manifest_with_cancel, compare_manifests,
        create_isolated_restore_destination, persist_json_artifact, persist_manifest,
        resolve_existing_zfs_snapshot_selection, source_manifests_stable, verify_read_only_mount,
        FileManifest, JsonArtifactSummary, ManifestArtifactSummary, ManifestComparison,
    },
    device::{
        clear_sas_scratch_label, discover_sas_drives, execute_sas_motion, SasDiscoveryConfig,
        SasMotionError, SasMotionOperation, SasProbeResult,
    },
    engine::bacula::{
        BaculaAdapter, BaculaCommands, BaculaCompression, BaculaDataOptions, BaculaJobEvidence,
        BaculaJobId, BaculaJobResource, BaculaJobState, BaculaPaths, BaculaPoolName,
        BaculaRestoreSubpath, CorrelatedJobDispatch, DirectBconsoleExecutor, VolumeLabel,
    },
    store,
    tool_runner::ToolError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureClass {
    Blocked,
    Failed,
    Interrupted,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BaculaMediaPhase {
    BackupWrite,
    FullReadback,
}

impl BaculaMediaPhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::BackupWrite => "backup_write",
            Self::FullReadback => "full_readback",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveManualMedia {
    request_id: Uuid,
    volume_order: i32,
    observed_job_progress: bool,
    files_at_mount: u64,
    bytes_at_mount: u64,
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

    fn cancelled(detail: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Cancelled,
            detail: detail.into(),
        }
    }
}

#[derive(Debug)]
struct CompletedExercise {
    source_manifest: RetainedSourceManifest,
    restored_manifest: Option<FileManifest>,
    comparison: Option<ManifestComparison>,
    backup: BaculaJobEvidence,
    backup_catalog: BaculaBackupCatalogEvidence,
    verify: BaculaJobEvidence,
    verify_catalog: BaculaVerifyCatalogEvidence,
    restore: Option<BaculaJobEvidence>,
    restore_catalog: Option<BaculaRestoreCatalogEvidence>,
    engine_volume_name: String,
    restore_destination: Option<PathBuf>,
}

#[derive(Debug)]
enum RetainedSourceManifest {
    Legacy {
        manifest: FileManifest,
        artifact: ManifestArtifactSummary,
    },
    Streaming(ManifestArtifactSummary),
}

impl RetainedSourceManifest {
    fn artifact(&self) -> &ManifestArtifactSummary {
        match self {
            Self::Legacy { artifact, .. } | Self::Streaming(artifact) => artifact,
        }
    }

    fn legacy(&self) -> Result<&FileManifest, ExerciseFailure> {
        match self {
            Self::Legacy { manifest, .. } => Ok(manifest),
            Self::Streaming(_) => Err(ExerciseFailure::failed(
                "a recovery comparison unexpectedly received a streaming-only source manifest",
            )),
        }
    }

    fn digest_sha256(&self) -> &str {
        &self.artifact().manifest_digest
    }

    fn logical_bytes(&self) -> u64 {
        self.artifact().logical_bytes
    }

    fn file_count(&self) -> u64 {
        self.artifact().file_count
    }

    fn coverage(&self) -> &crate::data_path::ManifestCoverage {
        &self.artifact().coverage
    }
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
            "expired physical tape operations require drive reconciliation"
        );
    }
    if !config.lab_file_roundtrip_enabled {
        let blocked = store::block_queued_single_tape_data_runs(
            pool,
            "the attended Bacula file data path was disabled before worker execution",
        )
        .await?;
        if blocked > 0 {
            warn!(
                blocked_runs = blocked,
                "queued physical tape data operations were blocked"
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
            warn!(physical_data_run_id = %id, %reason, "physical tape data operation was blocked during worker revalidation");
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
                "detail": "the exact configured drive identity was not observed immediately before the physical tape operation",
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
            "the pinned Bacula adapter requires TAPESCALE_SOURCE_ROOT=/source and TAPESCALE_RESTORE_ROOT=/restore",
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
    if claim.run.workflow_kind == "backup" {
        let source_relative = source.strip_prefix(&config.source_root).map_err(|_| {
            ExerciseFailure::blocked("the backup source path escaped the configured source root")
        })?;
        let source_relative = source_relative.to_str().ok_or_else(|| {
            ExerciseFailure::blocked("the backup source path must be valid UTF-8")
        })?;
        let resolved_source =
            resolve_existing_zfs_snapshot_selection(&config.source_root, source_relative).map_err(
                |error| {
                    ExerciseFailure::blocked(format!(
                        "the backup source is not a contained named ZFS snapshot: {error}"
                    ))
                },
            )?;
        let persisted_source = source.canonicalize().map_err(|error| {
            ExerciseFailure::blocked(format!(
                "the persisted backup source cannot be resolved: {error}"
            ))
        })?;
        if resolved_source != persisted_source {
            return Err(ExerciseFailure::blocked(
                "the persisted backup source no longer matches its named ZFS snapshot path",
            ));
        }
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
        let database_url = config.database_url.as_deref().ok_or_else(|| {
            ExerciseFailure::blocked("DATABASE_URL is required for the Bacula catalog")
        })?;
        BaculaRuntime::prepare_catalog(pool, database_url, cancellation)
            .await
            .map_err(|error| {
                ExerciseFailure::blocked(format!(
                    "Bacula catalog preflight failed before any tape write: {error}"
                ))
            })?;
        if cancellation.is_cancelled() {
            return Err(ExerciseFailure::interrupted(
                "worker shutdown was requested before source inventory",
            ));
        }

        let source_manifest = if claim.run.workflow_kind == "backup" {
            RetainedSourceManifest::Streaming(
                scan_and_persist_manifest_with_lease(
                    pool,
                    claim,
                    PathBuf::from(&claim.run.source_mount_path),
                    evidence_directory.to_owned(),
                    "source-manifest".to_owned(),
                    deadline,
                    cancellation,
                )
                .await?,
            )
        } else {
            let manifest = scan_manifest_with_lease(
                pool,
                claim,
                PathBuf::from(&claim.run.source_mount_path),
                deadline,
                cancellation,
            )
            .await?;
            let artifact = persist_manifest(evidence_directory, "source-manifest", &manifest)
                .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;
            RetainedSourceManifest::Legacy { manifest, artifact }
        };
        append_manifest_artifact(
            pool,
            claim,
            "source_manifest",
            source_manifest.artifact(),
        )
        .await?;

        if claim.run.workflow_kind != "backup" && Instant::now() >= deadline {
            return Err(ExerciseFailure::blocked(
                "source inventory consumed the entire recovery-exercise deadline; no tape command was dispatched",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(ExerciseFailure::interrupted(
                "worker shutdown was requested before any tape command",
            ));
        }
        let (spool_limit, spool_capacity) =
            bacula_spool_capacity(config, source_manifest.artifact())?;
        let data_options =
            bacula_data_options(claim)?.with_maximum_spool_size(spool_limit);
        transition(
            pool,
            claim,
            store::SingleTapeDataPhase::Writing,
            json!({
                "detail": if claim.run.spanning_enabled {
                    "source manifest retained; Bacula will stage data in the disk spool before sustained multi-tape streaming"
                } else {
                    "source manifest retained; Bacula will stage data in the disk spool before sustained tape streaming"
                },
                "source_manifest_sha256": source_manifest.artifact().manifest_digest,
                "file_count": source_manifest.artifact().file_count,
                "logical_bytes": source_manifest.artifact().logical_bytes,
                "compression_mode": claim.run.compression_mode,
                "resolved_compression": claim.run.resolved_compression,
                "spanning_enabled": claim.run.spanning_enabled,
                "data_spooling": &spool_capacity,
                "production_eligible": false
            }),
        )
        .await?;

        let volume = engine_volume_label(&claim.run.expected_cartridge_label, claim.run.id)?;
        let initial_authorized_volumes = vec![volume.clone()];
        let bacula_pool = BaculaPoolName::for_run(claim.run.id);
        renew_claim(pool, claim, "before entering the irreversible media-write boundary").await?;
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
            let detail = sas_command_error_detail(&error);
            if matches!(error, SasMotionError::Tool(ToolError::Cancelled { .. })) {
                ExerciseFailure::interrupted(format!(
                    "pre-write rewind was interrupted: {detail}"
                ))
            } else {
                ExerciseFailure::failed(format!("pre-write rewind failed: {detail}"))
            }
        })?;

        let label_clear = clear_sas_scratch_label(
            sas_config,
            Path::new(&claim.run.expected_path),
            cancellation,
        )
        .await
        .map_err(|error| {
            let detail = sas_command_error_detail(&error);
            if matches!(error, SasMotionError::Tool(ToolError::Cancelled { .. })) {
                ExerciseFailure::interrupted(format!(
                    "scratch-header preparation was interrupted after the cartridge entered the write boundary: {detail}"
                ))
            } else {
                ExerciseFailure::failed(format!(
                    "could not clear prior contents at BOT on the authorized scratch cartridge: {detail}"
                ))
            }
        })?;
        if !store::append_single_tape_data_evidence_event(
            pool,
            claim,
            "media_prepared",
            json!({
                "phase": "writing",
                "operation": "clear_prior_scratch_header",
                "device": claim.run.expected_path,
                "tool": label_clear,
                "detail": "prior scratch contents at BOT were invalidated before Director labeling"
            }),
        )
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
        {
            return Err(ExerciseFailure::interrupted(
                "the fenced run could not retain scratch-header preparation evidence",
            ));
        }

        // `weof` leaves the non-rewinding device after the new file mark. Return to
        // BOT so the Bacula Storage Daemon begins label inspection deterministically.
        execute_sas_motion(
            sas_config,
            Path::new(&claim.run.expected_path),
            SasMotionOperation::Rewind,
            cancellation,
        )
        .await
        .map_err(|error| {
            let detail = sas_command_error_detail(&error);
            if matches!(error, SasMotionError::Tool(ToolError::Cancelled { .. })) {
                ExerciseFailure::interrupted(format!(
                    "post-invalidation rewind was interrupted: {detail}"
                ))
            } else {
                ExerciseFailure::failed(format!(
                    "post-invalidation rewind failed before Bacula labeling: {detail}"
                ))
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
        runtime = Some(
            BaculaRuntime::start_with_data_options(
                pool,
                database_url,
                claim.run.id,
                paths,
                data_options,
                evidence_directory,
                cancellation,
            )
            .await
            .map_err(|error| ExerciseFailure::failed(error.to_string()))?,
        );

        let adapter = BaculaAdapter::new(DirectBconsoleExecutor::default());
        // Operational inventory may legitimately run longer than the legacy
        // G0.2 wall-clock envelope. Give daemon startup its own bounded window
        // so a very large completed inventory is not rejected by a stale
        // absolute deadline.
        let console_deadline = if claim.run.workflow_kind == "backup" {
            Instant::now() + Duration::from_secs(30)
        } else {
            deadline
        };
        wait_for_console(pool, claim, &adapter, console_deadline, cancellation).await?;
        let label_output = adapter
            .dispatch(&BaculaCommands::label(&volume, &bacula_pool))
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!("Bacula volume labeling failed: {error}"))
            })?;
        validate_console_output("label", &label_output.stdout, &label_output.stderr)?;
        let mount_output = adapter
            .dispatch(&BaculaCommands::mount())
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "Bacula could not mount the newly labeled first archive volume: {error}"
                ))
            })?;
        validate_console_output(
            "mount first archive volume",
            &mount_output.stdout,
            &mount_output.stderr,
        )?;
        let mut labeled_volumes = validate_labeled_volumes(
            pool,
            &initial_authorized_volumes,
            &bacula_pool,
            data_options.maximum_volumes.map_or(0, |limit| limit.get()),
        )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "Bacula did not catalog the exact authorized label in its isolated run pool: {error}"
                ))
            })?;
        let labeled_volume = labeled_volumes.remove(0);

        let backup_dispatch = adapter
            .dispatch_and_correlate(BaculaJobResource::Backup, &BaculaCommands::full_backup())
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!("Bacula full backup dispatch failed: {error}"))
            })?;
        record_engine_dispatch(pool, claim, "backup", &backup_dispatch, &adapter).await?;
        let backup = wait_for_job(
            pool,
            sas_config,
            claim,
            &adapter,
            backup_dispatch.job_id,
            "full backup",
            (claim.run.workflow_kind == "backup").then_some(BaculaMediaPhase::BackupWrite),
            config.data_run_timeout,
            cancellation,
        )
        .await?;
        let authorized_volumes = data_run_engine_volumes(pool, claim).await?;
        let backup_catalog = validate_backup_job_on_volumes(
            pool,
            backup.job_id,
            &authorized_volumes,
            &bacula_pool,
        )
        .await
        .map_err(|error| {
            ExerciseFailure::failed(format!(
                "Bacula backup catalog/media proof failed: {error}"
            ))
        })?;
        if claim.run.workflow_kind == "backup" {
            let catalog_volume_names = backup_catalog
                .volumes
                .iter()
                .map(|volume| volume.volume_name.clone())
                .collect::<Vec<_>>();
            if !store::mark_data_run_backup_volumes_written(pool, claim, &catalog_volume_names)
                .await
                .map_err(|error| {
                    ExerciseFailure::interrupted(format!(
                        "could not retain the catalog-proven archive write boundary: {error}"
                    ))
                })?
            {
                return Err(ExerciseFailure::interrupted(
                    "the fenced run could not match every authorized tape to the completed Bacula catalog",
                ));
            }
        }
        let engine_write = json!({
            "engine": "Bacula Community",
            "engine_version": "15.0.3",
            "volume": volume.as_str(),
            "labeled_volume": labeled_volume,
            "job": backup,
            "catalog": backup_catalog,
            "finalized": true,
            "single_volume": backup_catalog.segments.iter().all(|segment| segment.media_id == backup_catalog.volume.media_id),
            "volume_count": backup_catalog.volumes.len(),
            "compression": {
                "requested": claim.run.compression_mode,
                "resolved": claim.run.resolved_compression
            },
            "spanning_enabled": claim.run.spanning_enabled,
            "data_spooling": {
                "enabled": true,
                "directory": config.bacula_spool_dir.display().to_string(),
                "maximum_bytes": data_options.maximum_spool_size_bytes(),
                "purpose": "disk buffer for sustained tape streaming; this does not limit archive size"
            },
            "attribute_spooling": {
                "enabled": true,
                "directory": config.bacula_spool_dir.display().to_string(),
                "capacity_policy": "estimated per-entry and manifest allowance plus configured filesystem reserve"
            },
            "tape_file_boundaries": {
                "maximum_bytes": crate::engine::bacula::MAXIMUM_TAPE_FILE_BYTES,
                "purpose": "reduce EOF/filemark stops on modern LTO; this does not limit archive size"
            },
            "encryption": if claim.run.workflow_kind == "backup" {
                "disabled_by_explicit_test_slice_deviation"
            } else {
                "disabled_by_explicit_g0_2_lab_deviation"
            }
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
                "volumes": backup_catalog.volumes,
                "volume_count": backup_catalog.volumes.len(),
                "media_id": backup_catalog.volume.media_id,
                "segments": backup_catalog.segments,
                "files": backup.files,
                "job_bytes": backup.bytes,
                "volume_bytes": backup_catalog.volumes.iter().map(|volume| volume.volume_bytes).sum::<u64>(),
                "compression": claim.run.resolved_compression,
                "spanning_enabled": claim.run.spanning_enabled
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
            sas_config,
            claim,
            &adapter,
            verify_dispatch.job_id,
            "Data verify",
            (claim.run.workflow_kind == "backup").then_some(BaculaMediaPhase::FullReadback),
            config.data_run_timeout,
            cancellation,
        )
        .await?;
        let verify_catalog = validate_verify_job(pool, verify.job_id, backup.job_id)
        .await
        .map_err(|error| {
            ExerciseFailure::failed(format!("Bacula Data verify catalog proof failed: {error}"))
        })?;
        if claim.run.workflow_kind == "backup" {
            let source_after = scan_and_persist_manifest_with_lease(
                pool,
                claim,
                PathBuf::from(&claim.run.source_mount_path),
                evidence_directory.to_owned(),
                "source-stability-after".to_owned(),
                deadline,
                cancellation,
            )
            .await?;
            let stable = source_manifest
                .artifact()
                .source_stability_digest_sha256
                == source_after.source_stability_digest_sha256;
            if let Err(error) = fs::remove_file(&source_after.storage_path) {
                warn!(
                    path = %source_after.storage_path.display(),
                    %error,
                    "could not remove temporary post-backup stability manifest"
                );
            }
            if !stable {
                return Err(ExerciseFailure::failed(
                    "the supposedly immutable source changed during backup; no recovery point can be committed",
                ));
            }
            return Ok(CompletedExercise {
                source_manifest,
                restored_manifest: None,
                comparison: None,
                backup,
                backup_catalog,
                verify,
                verify_catalog,
                restore: None,
                restore_catalog: None,
                engine_volume_name: volume.as_str().to_owned(),
                restore_destination: None,
            });
        }
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
            sas_config,
            claim,
            &adapter,
            restore_dispatch.job_id,
            "full restore",
            None,
            config.data_run_timeout,
            cancellation,
        )
        .await?;
        let restore_catalog = validate_restore_job(pool, restore.job_id, backup.job_id)
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
        let legacy_source = source_manifest.legacy()?;
        if !source_manifests_stable(legacy_source, &source_after) {
            return Err(ExerciseFailure::failed(
                "the source changed during the exercise; restore proof is invalid",
            ));
        }
        let comparison = compare_manifests(legacy_source, &restored_manifest);
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
            restored_manifest: Some(restored_manifest),
            comparison: Some(comparison),
            backup,
            backup_catalog,
            verify,
            verify_catalog,
            restore: Some(restore),
            restore_catalog: Some(restore_catalog),
            engine_volume_name: volume.as_str().to_owned(),
            restore_destination: Some(destination.path().to_owned()),
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
            if claim.run.workflow_kind == "backup" {
                return complete_backup(
                    pool,
                    claim,
                    completed,
                    runtime_evidence,
                    cleanup.1,
                    evidence_directory,
                )
                .await;
            }
            let restored_manifest = completed
                .restored_manifest
                .as_ref()
                .expect("recovery exercise completion includes a restored manifest");
            let comparison = completed
                .comparison
                .as_ref()
                .expect("recovery exercise completion includes a comparison");
            let restore = completed
                .restore
                .as_ref()
                .expect("recovery exercise completion includes a restore job");
            let restore_catalog = completed
                .restore_catalog
                .as_ref()
                .expect("recovery exercise completion includes restore catalog evidence");
            let restore_destination = completed
                .restore_destination
                .as_ref()
                .expect("recovery exercise completion includes a restore destination");
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
                "restore_job": restore,
                "restore_catalog": restore_catalog,
                "source_manifest_digest": completed.source_manifest.digest_sha256(),
                "restored_manifest_digest": restored_manifest.digest_sha256,
                "comparison": comparison,
                "restore_destination": restore_destination,
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
                    logical_bytes: completed.source_manifest.logical_bytes(),
                    encoded_bytes: completed.backup_catalog.volume.volume_bytes,
                    file_count: completed.source_manifest.file_count(),
                    coverage: serde_json::to_value(completed.source_manifest.coverage())
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
                        "job": restore,
                        "catalog": restore_catalog,
                        "source_manifest_digest": completed.source_manifest.digest_sha256(),
                        "restored_manifest_digest": restored_manifest.digest_sha256,
                        "restore_destination": restore_destination
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
                    "detail": if claim.run.workflow_kind == "backup" {
                        "backup and physical readback passed, but the required cleanup rewind/postcondition failed"
                    } else {
                        "backup, physical readback, restore, and comparison passed, but the required cleanup rewind/postcondition failed"
                    },
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
                FailureClass::Cancelled => store::SingleTapeDataTerminalState::Cancelled,
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

async fn complete_backup(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    completed: CompletedExercise,
    runtime_evidence: Option<BaculaRuntimeEvidence>,
    cleanup_evidence: Value,
    evidence_directory: &Path,
) -> Result<()> {
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
    let encoded_bytes = completed
        .backup_catalog
        .volumes
        .iter()
        .map(|volume| volume.volume_bytes)
        .sum::<u64>();
    let volume_count = completed.backup_catalog.volumes.len();
    let mut limitations = vec![
        "full_only",
        "unencrypted",
        "server_verified_read_only_source",
    ];
    if !claim.run.spanning_enabled {
        limitations.insert(0, "single_volume");
    }
    let report = json!({
        "scope": if volume_count > 1 {
            "attended multi-volume full-file backup with complete physical-media readback"
        } else {
            "attended single-volume full-file backup with complete physical-media readback"
        },
        "qualification": "test_slice",
        "production_eligible": false,
        "limitations": limitations,
        "compression": {
            "requested": claim.run.compression_mode,
            "resolved": claim.run.resolved_compression
        },
        "media": {
            "spanning_enabled": claim.run.spanning_enabled,
            "volume_count": volume_count,
            "volumes": &completed.backup_catalog.volumes,
            "encoded_bytes": encoded_bytes
        },
        "engine": {"name":"Bacula Community","version":"15.0.3"},
        "backup_job": &completed.backup,
        "backup_catalog": &completed.backup_catalog,
        "data_verify_job": &completed.verify,
        "data_verify_catalog": &completed.verify_catalog,
        "source_manifest_digest": completed.source_manifest.digest_sha256(),
        "runtime": runtime_evidence,
        "cleanup": cleanup_evidence,
        "encryption": "disabled_by_explicit_test_slice_deviation",
        "restore_test": "not_run_separate_rehearsal_required"
    });
    let report_artifact =
        match persist_json_artifact(evidence_directory, "evidence-report", &report) {
            Ok(artifact) => artifact,
            Err(error) => {
                return finish_or_quarantine(
                    pool,
                    claim,
                    store::SingleTapeDataTerminalState::Failed,
                    json!({
                        "detail": format!("could not retain the backup evidence report: {error}"),
                        "position_uncertain": false,
                        "media_write_started": true,
                        "verified_recovery_point_created": false
                    }),
                )
                .await;
            }
        };
    if let Err(error) = append_json_artifact(
        pool,
        claim,
        "evidence_report",
        &report_artifact,
        json!({
            "candidate_evidence": true,
            "complete": false,
            "pending_atomic_finalization": true,
            "workflow_kind": "backup",
            "production_eligible": false
        }),
    )
    .await
    {
        return finish_or_quarantine(
            pool,
            claim,
            store::SingleTapeDataTerminalState::Failed,
            json!({
                "detail": format!("could not retain the backup evidence artifact: {}", error.detail),
                "position_uncertain": false,
                "media_write_started": true,
                "verified_recovery_point_created": false
            }),
        )
        .await;
    }
    let pass = store::SingleTapePassEvidence {
        engine_operation_id: format!("bacula-job:{}", completed.backup.job_id),
        engine_volume_name: completed.engine_volume_name,
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
        logical_bytes: completed.source_manifest.logical_bytes(),
        encoded_bytes,
        file_count: completed.source_manifest.file_count(),
        coverage: serde_json::to_value(completed.source_manifest.coverage())
            .map_err(|error| anyhow::anyhow!(error))?,
        full_readback_evidence: json!({
            "physical_media_readback": true,
            "level": "Data",
            "job": completed.verify,
            "catalog": completed.verify_catalog,
            "backup_catalog": completed.backup_catalog
        }),
        restore_test_evidence: json!({
            "outcome": "not_run",
            "independent_claim": true
        }),
        result: report,
    };
    match store::finalize_single_tape_backup_pass(pool, claim, &pass).await? {
        store::FinalizeSingleTapeDataResult::Passed {
            recovery_point_id,
            volume_record_id,
        } => {
            info!(
                backup_run_id = %claim.run.id,
                %recovery_point_id,
                %volume_record_id,
                "attended full backup passed physical readback and created a verified recovery point"
            );
            Ok(())
        }
        store::FinalizeSingleTapeDataResult::Rejected(reason) => {
            let cancellation_requested =
                store::single_tape_data_cancel_requested(pool, claim.run.id).await?;
            finish_or_quarantine(
                pool,
                claim,
                if cancellation_requested {
                    store::SingleTapeDataTerminalState::Cancelled
                } else {
                    store::SingleTapeDataTerminalState::Failed
                },
                json!({
                    "detail": if cancellation_requested {
                        format!("backup cancellation was honored before atomic finalization: {reason}")
                    } else {
                        format!("final backup evidence was rejected: {reason}")
                    },
                    "position_uncertain": false,
                    "media_write_started": true,
                    "verified_recovery_point_created": false
                }),
            )
            .await
        }
        store::FinalizeSingleTapeDataResult::FencedOut => {
            let detail = "atomic backup finalization was fenced out after physical media work; drive and cartridge require reconciliation";
            let _ = store::quarantine_fenced_single_tape_write(pool, claim, detail).await;
            Err(anyhow::anyhow!(detail))
        }
    }
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
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
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
                match renew_claim(pool, claim, "during file inventory").await {
                    Ok(()) => {}
                    Err(error) => break error,
                }
            }
        }
    };
    stop.store(true, Ordering::Release);
    let _ = scan.await;
    Err(failure)
}

async fn scan_and_persist_manifest_with_lease(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    path: PathBuf,
    run_directory: PathBuf,
    artifact_name: String,
    _deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<ManifestArtifactSummary, ExerciseFailure> {
    let stop = Arc::new(AtomicBool::new(false));
    let scan_stop = Arc::clone(&stop);
    let mut scan = tokio::task::spawn_blocking(move || {
        build_and_persist_manifest_with_cancel(
            &path,
            &run_directory,
            &artifact_name,
            scan_stop.as_ref(),
        )
    });
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let failure = loop {
        tokio::select! {
            result = &mut scan => {
                return result
                    .map_err(|error| ExerciseFailure::failed(format!("streaming manifest worker failed: {error}")))?
                    .map_err(|error| ExerciseFailure::blocked(error.to_string()));
            }
            () = cancellation.cancelled() => {
                break ExerciseFailure::interrupted(
                    "worker shutdown was requested during streaming file inventory",
                );
            }
            _ = heartbeat.tick() => {
                match renew_claim(pool, claim, "during streaming file inventory").await {
                    Ok(()) => {}
                    Err(error) => break error,
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

fn observe_manual_media_progress(active: &mut ActiveManualMedia, evidence: &BaculaJobEvidence) {
    if matches!(
        evidence.state,
        BaculaJobState::Running | BaculaJobState::Committing
    ) || evidence.files != active.files_at_mount
        || evidence.bytes != active.bytes_at_mount
    {
        active.observed_job_progress = true;
    }
}

const fn mount_wait_can_request_media(phase: Option<BaculaMediaPhase>) -> bool {
    matches!(phase, Some(BaculaMediaPhase::FullReadback))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExistingWriteMountAction {
    WaitForAuthorization,
    FulfillAuthorization,
    LostLoadedState,
    Conflict,
}

fn existing_write_mount_action(phase: &str, state: &str) -> ExistingWriteMountAction {
    if phase != BaculaMediaPhase::BackupWrite.as_str() {
        ExistingWriteMountAction::Conflict
    } else {
        match state {
            "pending" => ExistingWriteMountAction::WaitForAuthorization,
            "authorized" => ExistingWriteMountAction::FulfillAuthorization,
            "loaded" => ExistingWriteMountAction::LostLoadedState,
            _ => ExistingWriteMountAction::Conflict,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn wait_for_job(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedSingleTapeDataRun,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    job_id: BaculaJobId,
    operation: &str,
    media_phase: Option<BaculaMediaPhase>,
    no_progress_timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<BaculaJobEvidence, ExerciseFailure> {
    let mut consecutive_poll_failures = 0_u8;
    let mut last_progress = None;
    let mut last_progress_at = Instant::now();
    let mut active_manual_media: Option<ActiveManualMedia> = None;
    let mut next_readback_volume = 1_i32;
    loop {
        if cancellation.is_cancelled() {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(ExerciseFailure::interrupted(format!(
                "worker shutdown requested cancellation of Bacula {operation} JobId {job_id}; {cancellation_result}"
            )));
        }
        if let Err(error) = renew_claim(pool, claim, &format!("during Bacula {operation}")).await {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(engine_wait_failure(error, &cancellation_result));
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

        let progress = (
            evidence.state,
            evidence.files,
            evidence.bytes,
            evidence.errors,
        );
        if last_progress.as_ref() != Some(&progress) {
            last_progress = Some(progress);
            last_progress_at = Instant::now();
        }

        if evidence.is_terminal() {
            if !evidence.is_strict_success() {
                return Err(ExerciseFailure::failed(format!(
                    "Bacula {operation} JobId {job_id} ended as {:?} with {} errors",
                    evidence.state, evidence.errors
                )));
            }
            if active_manual_media.is_none()
                && store::open_data_run_media_request(pool, claim.run.id)
                    .await
                    .map_err(|error| {
                        ExerciseFailure::interrupted(format!(
                            "could not inspect final manual-media state: {error}"
                        ))
                    })?
                    .is_some()
            {
                return Err(ExerciseFailure::failed(
                    "Bacula completed while an unfulfilled manual-media request remained open",
                ));
            }
            if media_phase == Some(BaculaMediaPhase::FullReadback) {
                let volumes =
                    store::data_run_volumes(pool, claim.run.id)
                        .await
                        .map_err(|error| {
                            ExerciseFailure::interrupted(format!(
                                "could not validate the final ordered readback volume: {error}"
                            ))
                        })?;
                if volumes.len() > 1 {
                    let expected_last = i32::try_from(volumes.len()).map_err(|_| {
                        ExerciseFailure::failed("archive volume count exceeds the order range")
                    })?;
                    if active_manual_media
                        .as_ref()
                        .is_none_or(|active| active.volume_order != expected_last)
                    {
                        return Err(ExerciseFailure::failed(
                            "Bacula completed before the final ordered archive volume was presented for readback",
                        ));
                    }
                }
            }
            if let Some(active) = active_manual_media {
                complete_manual_media_request(pool, claim, active.request_id, false).await?;
            }
            return Ok(evidence);
        }

        if evidence.state == BaculaJobState::WaitingForMount {
            if mount_wait_can_request_media(media_phase) {
                if let Some(active) = active_manual_media.as_mut() {
                    observe_manual_media_progress(active, &evidence);
                }
                if active_manual_media
                    .as_ref()
                    .is_some_and(|active| active.observed_job_progress)
                {
                    let completed = active_manual_media
                        .take()
                        .expect("the progress check established active readback media");
                    complete_manual_media_request(pool, claim, completed.request_id, true).await?;
                    next_readback_volume =
                        completed.volume_order.checked_add(1).ok_or_else(|| {
                            ExerciseFailure::failed("archive volume order overflowed")
                        })?;
                } else if active_manual_media.is_some() {
                    // The known read volume has been presented but Bacula has
                    // not yet been observed accepting it. Retrying a mount is
                    // non-destructive; do not advance the ordered request.
                    let mount = adapter
                        .dispatch(&BaculaCommands::mount())
                        .await
                        .map_err(|error| {
                            ExerciseFailure::failed(format!(
                                "Bacula could not mount the current readback volume for {operation}: {error}"
                            ))
                        })?;
                    validate_console_output(
                        "mount current readback volume",
                        &mount.stdout,
                        &mount.stderr,
                    )?;
                    if last_progress_at.elapsed() >= no_progress_timeout {
                        let cancellation_result = cancel_and_observe(adapter, job_id).await;
                        return Err(ExerciseFailure::interrupted(format!(
                            "Bacula {operation} JobId {job_id} did not accept the current readback volume within {} seconds; {cancellation_result}",
                            no_progress_timeout.as_secs()
                        )));
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }

                // A read job's `M` state names a known volume to mount. It may
                // create or advance an exact, non-destructive ordered readback
                // request. Destructive write continuations are never created
                // from this state; only Bacula's `m` state may trigger those.
                active_manual_media = prepare_manual_media(
                    pool,
                    sas_config,
                    claim,
                    adapter,
                    BaculaMediaPhase::FullReadback,
                    next_readback_volume,
                    cancellation,
                )
                .await?;
                if let Some(active) = active_manual_media.as_mut() {
                    active.files_at_mount = evidence.files;
                    active.bytes_at_mount = evidence.bytes;
                }
                last_progress_at = Instant::now();
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }

            if media_phase == Some(BaculaMediaPhase::BackupWrite) && active_manual_media.is_none() {
                if let Some(request) = store::open_data_run_media_request(pool, claim.run.id)
                    .await
                    .map_err(|error| {
                        ExerciseFailure::interrupted(format!(
                            "could not inspect the destructive media request while Bacula was waiting for a mount: {error}"
                        ))
                    })?
                {
                    match existing_write_mount_action(&request.phase, &request.state) {
                        ExistingWriteMountAction::WaitForAuthorization => {
                            // The destructive request was created only while
                            // Bacula reported `m`; a later `M` must not create
                            // or allocate another tape while the operator waits.
                            last_progress_at = Instant::now();
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                        ExistingWriteMountAction::FulfillAuthorization => {
                            // `M` may fulfill an exact, already-persisted and
                            // operator-authorized request whose origin was `m`.
                            // It still cannot originate a destructive request.
                            active_manual_media = prepare_manual_media(
                                pool,
                                sas_config,
                                claim,
                                adapter,
                                BaculaMediaPhase::BackupWrite,
                                next_readback_volume,
                                cancellation,
                            )
                            .await?;
                            if let Some(active) = active_manual_media.as_mut() {
                                active.files_at_mount = evidence.files;
                                active.bytes_at_mount = evidence.bytes;
                            }
                            last_progress_at = Instant::now();
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                        ExistingWriteMountAction::LostLoadedState => {
                            return Err(ExerciseFailure::interrupted(
                                "the worker lost in-memory state after a destructive continuation tape was loaded",
                            ));
                        }
                        ExistingWriteMountAction::Conflict => {
                            return Err(ExerciseFailure::failed(
                                "Bacula's write mount wait conflicted with its durable media request",
                            ));
                        }
                    }
                }
            }

            // `M` means Bacula wants the currently authorized volume mounted;
            // it does not mean the write tape is full. Never originate or
            // allocate a new BackupWrite cartridge from this state.
            let mount = adapter
                .dispatch(&BaculaCommands::mount())
                .await
                .map_err(|error| {
                    ExerciseFailure::failed(format!(
                        "Bacula could not mount the current archive volume for {operation}: {error}"
                    ))
                })?;
            validate_console_output("mount current archive volume", &mount.stdout, &mount.stderr)?;
            if let Some(active) = active_manual_media.as_mut() {
                observe_manual_media_progress(active, &evidence);
            }
            if last_progress_at.elapsed() >= no_progress_timeout {
                let cancellation_result = cancel_and_observe(adapter, job_id).await;
                return Err(ExerciseFailure::interrupted(format!(
                    "Bacula {operation} JobId {job_id} remained waiting to mount the current volume for {} seconds; {cancellation_result}",
                    no_progress_timeout.as_secs()
                )));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        if evidence.state == BaculaJobState::WaitingForMedia {
            if let Some(phase) = media_phase {
                if let Some(active) = active_manual_media.as_mut() {
                    observe_manual_media_progress(active, &evidence);
                    if !active.observed_job_progress {
                        if last_progress_at.elapsed() >= no_progress_timeout {
                            let cancellation_result = cancel_and_observe(adapter, job_id).await;
                            return Err(ExerciseFailure::interrupted(format!(
                                "Bacula {operation} JobId {job_id} did not accept archive volume {} within the no-progress window; {cancellation_result}",
                                active.volume_order
                            )));
                        }
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }

                if let Some(active) = active_manual_media.take() {
                    complete_manual_media_request(pool, claim, active.request_id, true).await?;
                    if phase == BaculaMediaPhase::FullReadback {
                        next_readback_volume =
                            active.volume_order.checked_add(1).ok_or_else(|| {
                                ExerciseFailure::failed("archive volume order overflowed")
                            })?;
                    }
                }

                active_manual_media = prepare_manual_media(
                    pool,
                    sas_config,
                    claim,
                    adapter,
                    phase,
                    next_readback_volume,
                    cancellation,
                )
                .await?;
                if let Some(active) = active_manual_media.as_mut() {
                    active.files_at_mount = evidence.files;
                    active.bytes_at_mount = evidence.bytes;
                }

                // A pending request is an operator wait, not stalled tape I/O.
                // A newly mounted tape gets a fresh watchdog window.
                last_progress_at = Instant::now();
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        } else if let Some(active) = active_manual_media.as_mut() {
            observe_manual_media_progress(active, &evidence);
        }

        if last_progress_at.elapsed() >= no_progress_timeout {
            let cancellation_result = cancel_and_observe(adapter, job_id).await;
            return Err(ExerciseFailure::interrupted(format!(
                "Bacula {operation} JobId {job_id} made no observable progress for {} seconds; {cancellation_result}",
                no_progress_timeout.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn complete_manual_media_request(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    request_id: Uuid,
    media_exhausted: bool,
) -> Result<(), ExerciseFailure> {
    match store::complete_data_run_media_request(pool, claim, request_id, media_exhausted).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ExerciseFailure::interrupted(
            "the fenced run could not durably complete its manual-media request",
        )),
        Err(error) => Err(ExerciseFailure::interrupted(format!(
            "could not durably complete the manual-media request: {error}"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare_manual_media(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedSingleTapeDataRun,
    adapter: &BaculaAdapter<DirectBconsoleExecutor>,
    phase: BaculaMediaPhase,
    next_readback_volume: i32,
    cancellation: &CancellationToken,
) -> Result<Option<ActiveManualMedia>, ExerciseFailure> {
    let existing = store::open_data_run_media_request(pool, claim.run.id)
        .await
        .map_err(|error| {
            ExerciseFailure::interrupted(format!(
                "could not inspect the durable manual-media request: {error}"
            ))
        })?;

    let request = if let Some(existing) = existing {
        if existing.phase != phase.as_str() {
            return Err(ExerciseFailure::failed(format!(
                "the durable media request is for {}, but Bacula is waiting during {}",
                existing.phase,
                phase.as_str()
            )));
        }
        if existing.state == "loaded" {
            return Err(ExerciseFailure::interrupted(
                "the worker lost in-memory state after a tape was loaded; refusing to guess whether the media was already changed",
            ));
        }
        if phase == BaculaMediaPhase::FullReadback && existing.volume_order != next_readback_volume
        {
            return Err(ExerciseFailure::failed(format!(
                "the durable readback request is for volume {}, but the ordered readback expects volume {next_readback_volume}",
                existing.volume_order
            )));
        }
        existing
    } else {
        let release = adapter
            .dispatch(&BaculaCommands::release())
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "Bacula could not release the current tape before a manual change: {error}"
                ))
            })?;
        validate_console_output(
            "release current archive volume",
            &release.stdout,
            &release.stderr,
        )?;

        let volumes = store::data_run_volumes(pool, claim.run.id)
            .await
            .map_err(|error| {
                ExerciseFailure::interrupted(format!(
                    "could not load the ordered archive volumes: {error}"
                ))
            })?;
        let (volume_order, expected_cartridge_id, expected_engine_volume_name) = match phase {
            BaculaMediaPhase::BackupWrite => {
                let volume_order = volumes
                    .iter()
                    .map(|volume| volume.volume_order)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| ExerciseFailure::failed("archive volume order overflowed"))?;
                (volume_order, None, None)
            }
            BaculaMediaPhase::FullReadback => {
                let volume = volumes
                    .iter()
                    .find(|volume| volume.volume_order == next_readback_volume)
                    .ok_or_else(|| {
                        ExerciseFailure::failed(format!(
                            "Bacula requested another readback tape after all {} retained archive volumes were presented",
                            volumes.len()
                        ))
                    })?;
                let engine_name = volume.engine_volume_name.clone().ok_or_else(|| {
                    ExerciseFailure::failed(format!(
                        "archive volume {} has no retained Bacula label",
                        volume.volume_order
                    ))
                })?;
                (
                    volume.volume_order,
                    Some(volume.cartridge_id),
                    Some(engine_name),
                )
            }
        };

        match store::ensure_data_run_media_request(
            pool,
            claim,
            phase.as_str(),
            volume_order,
            expected_cartridge_id,
            expected_engine_volume_name.as_deref(),
        )
        .await
        .map_err(|error| {
            ExerciseFailure::interrupted(format!(
                "could not retain Bacula's manual-media request: {error}"
            ))
        })? {
            store::DataRunMediaRequestResult::Ready(request) => *request,
            store::DataRunMediaRequestResult::FencedOut => {
                return Err(ExerciseFailure::interrupted(
                    "the run was fenced out while requesting another archive tape",
                ))
            }
            store::DataRunMediaRequestResult::Blocked(reason) => {
                return Err(ExerciseFailure::blocked(reason))
            }
        }
    };

    let Some((authorized_request, volume)) = store::authorized_data_run_media_request(pool, claim)
        .await
        .map_err(|error| {
            ExerciseFailure::interrupted(format!(
                "could not inspect authorized continuation media: {error}"
            ))
        })?
    else {
        return Ok(None);
    };
    if authorized_request.id != request.id
        || authorized_request.phase != phase.as_str()
        || authorized_request.volume_order != request.volume_order
        || volume.volume_order != request.volume_order
    {
        return Err(ExerciseFailure::failed(
            "the authorized continuation tape does not match Bacula's active media request",
        ));
    }

    let observations = discover_and_persist(pool, sas_config)
        .await
        .map_err(|error| {
            ExerciseFailure::interrupted(format!(
                "could not revalidate the loaded drive before media motion: {error}"
            ))
        })?;
    let observation = matching_observation(
        &observations,
        &claim.run.expected_path,
        &claim.run.expected_stable_id,
    )
    .ok_or_else(|| {
        ExerciseFailure::blocked(
            "the exact assigned tape drive disappeared before the media change",
        )
    })?;
    if let Some(reason) = observation_blocker(observation, &claim.run) {
        return Err(ExerciseFailure::blocked(format!(
            "the loaded continuation tape is not ready: {reason}"
        )));
    }

    if !store::mark_data_run_media_loaded(pool, claim, request.id)
        .await
        .map_err(|error| {
            ExerciseFailure::interrupted(format!(
                "could not durably accept the loaded archive tape: {error}"
            ))
        })?
    {
        return Err(ExerciseFailure::interrupted(
            "the fenced run could not accept the loaded archive tape",
        ));
    }

    match phase {
        BaculaMediaPhase::BackupWrite => {
            let engine_volume = engine_volume_label_for_order(
                &volume.expected_cartridge_label,
                claim.run.id,
                volume.volume_order,
            )?;
            if !store::mark_data_run_volume_write_started(
                pool,
                claim,
                volume.volume_order,
                engine_volume.as_str(),
            )
            .await
            .map_err(|error| {
                ExerciseFailure::interrupted(format!(
                    "could not commit the continuation-tape write boundary: {error}"
                ))
            })? {
                return Err(ExerciseFailure::interrupted(
                    "the continuation tape was not durably quarantined before tape motion",
                ));
            }

            execute_sas_motion(
                sas_config,
                Path::new(&claim.run.expected_path),
                SasMotionOperation::Rewind,
                cancellation,
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "continuation-tape pre-write rewind failed: {}",
                    sas_command_error_detail(&error)
                ))
            })?;
            let label_clear = clear_sas_scratch_label(
                sas_config,
                Path::new(&claim.run.expected_path),
                cancellation,
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "could not clear prior contents on archive volume {}: {}",
                    volume.volume_order,
                    sas_command_error_detail(&error)
                ))
            })?;
            if !store::append_single_tape_data_evidence_event(
                pool,
                claim,
                "media_prepared",
                json!({
                    "phase": "writing",
                    "volume_order": volume.volume_order,
                    "cartridge_id": volume.cartridge_id,
                    "operation": "clear_prior_scratch_header",
                    "device": claim.run.expected_path,
                    "tool": label_clear,
                    "detail": "continuation scratch contents at BOT were invalidated before Bacula labeling"
                }),
            )
            .await
            .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
            {
                return Err(ExerciseFailure::interrupted(
                    "the fenced run could not retain continuation-media preparation evidence",
                ));
            }
            execute_sas_motion(
                sas_config,
                Path::new(&claim.run.expected_path),
                SasMotionOperation::Rewind,
                cancellation,
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "continuation-tape post-invalidation rewind failed: {}",
                    sas_command_error_detail(&error)
                ))
            })?;

            let bacula_pool = BaculaPoolName::for_run(claim.run.id);
            let label = adapter
                .dispatch(&BaculaCommands::label(&engine_volume, &bacula_pool))
                .await
                .map_err(|error| {
                    ExerciseFailure::failed(format!(
                        "Bacula could not label archive volume {}: {error}",
                        volume.volume_order
                    ))
                })?;
            validate_console_output(
                "label continuation archive volume",
                &label.stdout,
                &label.stderr,
            )?;
            let authorized_volumes = data_run_engine_volumes(pool, claim).await?;
            validate_labeled_volumes(
                pool,
                &authorized_volumes,
                &bacula_pool,
                if claim.run.spanning_enabled { 0 } else { 1 },
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "Bacula did not retain the exact ordered archive-volume set after labeling: {error}"
                ))
            })?;
        }
        BaculaMediaPhase::FullReadback => {
            if volume.engine_volume_name.as_deref()
                != authorized_request.expected_engine_volume_name.as_deref()
            {
                return Err(ExerciseFailure::failed(
                    "the authorized readback tape does not match its retained Bacula label",
                ));
            }
            execute_sas_motion(
                sas_config,
                Path::new(&claim.run.expected_path),
                SasMotionOperation::Rewind,
                cancellation,
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "archive readback rewind failed for volume {}: {}",
                    volume.volume_order,
                    sas_command_error_detail(&error)
                ))
            })?;
        }
    }

    let mount = adapter
        .dispatch(&BaculaCommands::mount())
        .await
        .map_err(|error| {
            ExerciseFailure::failed(format!(
                "Bacula could not mount archive volume {}: {error}",
                volume.volume_order
            ))
        })?;
    validate_console_output("mount archive volume", &mount.stdout, &mount.stderr)?;

    Ok(Some(ActiveManualMedia {
        request_id: request.id,
        volume_order: volume.volume_order,
        observed_job_progress: false,
        files_at_mount: 0,
        bytes_at_mount: 0,
    }))
}

fn engine_wait_failure(error: ExerciseFailure, cancellation_result: &str) -> ExerciseFailure {
    let detail = format!("{}; {cancellation_result}", error.detail);
    if error.class == FailureClass::Cancelled {
        ExerciseFailure::cancelled(detail)
    } else {
        ExerciseFailure::interrupted(detail)
    }
}

fn bacula_data_options(
    claim: &store::ClaimedSingleTapeDataRun,
) -> Result<BaculaDataOptions, ExerciseFailure> {
    if claim.run.workflow_kind != "backup" {
        return Ok(BaculaDataOptions::single_volume(BaculaCompression::None));
    }
    let compression =
        BaculaCompression::from_policy_mode(&claim.run.compression_mode).map_err(|error| {
            ExerciseFailure::blocked(format!(
                "the persisted backup compression mode is not executable: {error}"
            ))
        })?;
    Ok(if claim.run.spanning_enabled {
        BaculaDataOptions::spanning(compression)
    } else {
        BaculaDataOptions::single_volume(compression)
    })
}

/// Select a run-scoped spool ceiling from the configured policy and the
/// filesystem's current free space. Keeping a reserve prevents the data spool
/// from exhausting its filesystem. The ceiling is a buffer size, not an
/// archive-size limit: Bacula despools a full buffer and continues the same
/// job.
fn bacula_spool_capacity(
    config: &Config,
    manifest: &ManifestArtifactSummary,
) -> Result<(NonZeroU64, Value), ExerciseFailure> {
    let metadata = fs::symlink_metadata(&config.bacula_spool_dir).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "the Bacula spool directory {} is unavailable: {error}",
            config.bacula_spool_dir.display()
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ExerciseFailure::blocked(format!(
            "the Bacula spool path {} must be a real directory",
            config.bacula_spool_dir.display()
        )));
    }
    let write_probe = config.bacula_spool_dir.join(format!(
        ".tapescale-write-probe-{}",
        Uuid::now_v7().simple()
    ));
    let probe = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&write_probe)
        .map_err(|error| {
            ExerciseFailure::blocked(format!(
                "the Bacula spool directory {} is not writable: {error}",
                config.bacula_spool_dir.display()
            ))
        })?;
    let sync_result = probe.sync_all();
    drop(probe);
    let remove_result = fs::remove_file(&write_probe);
    sync_result.map_err(|error| {
        ExerciseFailure::blocked(format!(
            "the Bacula spool filesystem could not persist a preflight write: {error}"
        ))
    })?;
    remove_result.map_err(|error| {
        ExerciseFailure::blocked(format!(
            "the Bacula spool preflight file {} could not be removed: {error}",
            write_probe.display()
        ))
    })?;
    let status = statvfs(&config.bacula_spool_dir).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "could not inspect Bacula spool capacity at {}: {error}",
            config.bacula_spool_dir.display()
        ))
    })?;
    let available_bytes = status.f_bavail.saturating_mul(status.f_frsize);
    let filesystem_bytes = status.f_blocks.saturating_mul(status.f_frsize);
    let attribute_spool_reserve_bytes =
        estimated_attribute_spool_reserve(manifest.entry_count, manifest.size_bytes).ok_or_else(
            || {
                ExerciseFailure::blocked(
                    "the Bacula attribute-spool capacity estimate overflowed for this manifest",
                )
            },
        )?;
    let total_reserved_bytes = config
        .bacula_spool_reserve_bytes
        .checked_add(attribute_spool_reserve_bytes)
        .ok_or_else(|| ExerciseFailure::blocked("the combined Bacula spool reserve overflowed"))?;
    let Some(limit) = effective_spool_limit(
        available_bytes,
        total_reserved_bytes,
        config.bacula_spool_max_bytes,
        config.bacula_spool_min_bytes,
    ) else {
        return Err(ExerciseFailure::blocked(format!(
            "Bacula spooling needs at least {} data bytes plus {} attribute-spool bytes and a {} byte filesystem reserve, but {} has only {} bytes available",
            config.bacula_spool_min_bytes,
            attribute_spool_reserve_bytes,
            config.bacula_spool_reserve_bytes,
            config.bacula_spool_dir.display(),
            available_bytes
        )));
    };
    let effective_maximum_bytes = limit.get();
    Ok((
        limit,
        json!({
            "enabled": true,
            "directory": config.bacula_spool_dir.display().to_string(),
            "filesystem_bytes": filesystem_bytes,
            "available_bytes_at_preflight": available_bytes,
            "configured_filesystem_reserve_bytes": config.bacula_spool_reserve_bytes,
            "estimated_attribute_spool_reserve_bytes": attribute_spool_reserve_bytes,
            "total_reserved_bytes": total_reserved_bytes,
            "attribute_spool_estimate": {
                "manifest_entries": manifest.entry_count,
                "manifest_size_bytes": manifest.size_bytes,
                "bytes_per_entry": ATTRIBUTE_SPOOL_BYTES_PER_ENTRY,
                "manifest_multiplier": ATTRIBUTE_SPOOL_MANIFEST_MULTIPLIER,
                "base_bytes": ATTRIBUTE_SPOOL_BASE_BYTES
            },
            "configured_maximum_bytes": config.bacula_spool_max_bytes,
            "effective_maximum_bytes": effective_maximum_bytes,
            "minimum_required_bytes": config.bacula_spool_min_bytes,
            "archive_size_limited": false,
            "behavior_at_limit": "despool_to_tape_then_continue",
            "attribute_spooling_enabled": true,
            "storage_daemon_working_directory": config.bacula_spool_dir.display().to_string()
        }),
    ))
}

fn estimated_attribute_spool_reserve(entry_count: u64, manifest_size_bytes: u64) -> Option<u64> {
    ATTRIBUTE_SPOOL_BASE_BYTES
        .checked_add(entry_count.checked_mul(ATTRIBUTE_SPOOL_BYTES_PER_ENTRY)?)?
        .checked_add(manifest_size_bytes.checked_mul(ATTRIBUTE_SPOOL_MANIFEST_MULTIPLIER)?)
}

fn effective_spool_limit(
    available_bytes: u64,
    reserve_bytes: u64,
    configured_maximum_bytes: u64,
    minimum_bytes: u64,
) -> Option<NonZeroU64> {
    let effective = available_bytes
        .saturating_sub(reserve_bytes)
        .min(configured_maximum_bytes);
    if effective < minimum_bytes {
        None
    } else {
        NonZeroU64::new(effective)
    }
}

async fn data_run_engine_volumes(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
) -> Result<Vec<VolumeLabel>, ExerciseFailure> {
    let rows = store::data_run_volumes(pool, claim.run.id)
        .await
        .map_err(|error| {
            ExerciseFailure::failed(format!(
                "could not load the authorized archive-volume set: {error}"
            ))
        })?;
    let mut labels = Vec::new();
    for row in rows {
        let Some(engine_volume_name) = row.engine_volume_name else {
            continue;
        };
        labels.push(VolumeLabel::parse(engine_volume_name).map_err(|error| {
            ExerciseFailure::failed(format!(
                "persisted archive volume {} has an invalid Bacula label: {error}",
                row.volume_order
            ))
        })?);
    }
    if labels.is_empty() {
        return Err(ExerciseFailure::failed(
            "the completed Bacula job has no durably authorized archive volumes",
        ));
    }
    Ok(labels)
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
    match store::single_tape_data_cancel_requested(pool, claim.run.id).await {
        Ok(true) => {
            return Err(ExerciseFailure::cancelled(format!(
                "operator cancellation was requested {context}"
            )))
        }
        Ok(false) => {}
        Err(error) => {
            return Err(ExerciseFailure::interrupted(format!(
                "could not check durable cancellation state {context}: {error}"
            )))
        }
    }
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
    engine_volume_label_for_order(registered_label, run_id, 1)
}

fn engine_volume_label_for_order(
    registered_label: &str,
    run_id: Uuid,
    volume_order: i32,
) -> Result<VolumeLabel, ExerciseFailure> {
    if volume_order <= 0 {
        return Err(ExerciseFailure::blocked(
            "archive volume order must be positive",
        ));
    }
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
    if volume_order > 1 {
        normalized.push_str(&format!("-V{volume_order:04}"));
    }
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
        "sql error",
        "fatal",
        "failed",
        "not found",
        "invalid command",
        "permission denied",
        "cannot label",
        "already labeled",
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

fn sas_command_error_detail(error: &SasMotionError) -> String {
    match error {
        SasMotionError::Tool(tool) => tool.diagnostic_summary(2048),
        SasMotionError::DeviceNotConfigured { .. } | SasMotionError::DeviceBinding { .. } => {
            error.to_string()
        }
    }
}

async fn append_manifest_artifact(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    kind: &str,
    artifact: &ManifestArtifactSummary,
) -> Result<(), ExerciseFailure> {
    append_artifact(
        pool,
        claim,
        kind,
        &artifact.storage_path,
        &artifact.artifact_sha256,
        artifact.size_bytes,
        json!({
            "manifest_digest": artifact.manifest_digest,
            "entry_count": artifact.entry_count,
            "file_count": artifact.file_count,
            "logical_bytes": artifact.logical_bytes,
            "coverage": artifact.coverage
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
    if store::single_tape_data_cancel_requested(pool, claim.run.id)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::cancelled(
            "operator cancellation was requested before the next phase",
        ));
    }
    if store::transition_single_tape_data_phase(pool, claim, phase, evidence)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        Ok(())
    } else {
        Err(ExerciseFailure::interrupted(
            "the fenced claim rejected a physical-data phase transition",
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
        return Some("drive model, serial, WWN, or firmware changed before the operation");
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
            physical_data_run_id = %claim.run.id,
            "physical-data completion was rejected by its fencing token"
        );
    } else {
        warn!(
            physical_data_run_id = %claim.run.id,
            terminal_state = ?terminal,
            detail = %concise_text(
                result
                    .get("detail")
                    .and_then(|value| value.as_str())
                    .unwrap_or("terminal evidence recorded"),
                512
            ),
            "physical tape data operation reached a durable terminal state"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_validation_rejects_existing_media_and_sql_failures() {
        for output in [
            "3920 Cannot label Volume because it is already labeled: TEST001",
            "SQL error. Expected one MediaId",
            "Fatal catalog error",
        ] {
            assert!(
                validate_console_output("label", output.as_bytes(), b"").is_err(),
                "accepted Bacula failure output: {output}"
            );
        }
    }

    #[test]
    fn label_validation_accepts_success() {
        let output = b"3000 OK label. VolBytes=283 Volume=TEST001 Device=TS-LTO-Drive";
        assert!(validate_console_output("label", output, b"").is_ok());
    }

    #[test]
    fn engine_wait_preserves_operator_cancellation_class() {
        let failure = engine_wait_failure(
            ExerciseFailure::cancelled("operator cancellation was requested during Bacula verify"),
            "cancel command accepted",
        );
        assert_eq!(failure.class, FailureClass::Cancelled);
        assert!(failure.detail.contains("cancel command accepted"));
    }

    #[test]
    fn engine_wait_keeps_lease_failure_interrupted() {
        let failure = engine_wait_failure(
            ExerciseFailure::interrupted("the fenced lease expired during Bacula backup"),
            "cancel command accepted",
        );
        assert_eq!(failure.class, FailureClass::Interrupted);
    }

    #[test]
    fn mount_wait_advances_only_non_destructive_readback_media() {
        assert!(mount_wait_can_request_media(Some(
            BaculaMediaPhase::FullReadback
        )));
        assert!(!mount_wait_can_request_media(Some(
            BaculaMediaPhase::BackupWrite
        )));
        assert!(!mount_wait_can_request_media(None));
    }

    #[test]
    fn mount_wait_can_fulfill_but_never_originate_a_write_continuation() {
        assert_eq!(
            existing_write_mount_action("backup_write", "pending"),
            ExistingWriteMountAction::WaitForAuthorization
        );
        assert_eq!(
            existing_write_mount_action("backup_write", "authorized"),
            ExistingWriteMountAction::FulfillAuthorization
        );
        assert_eq!(
            existing_write_mount_action("backup_write", "loaded"),
            ExistingWriteMountAction::LostLoadedState
        );
        assert_eq!(
            existing_write_mount_action("full_readback", "authorized"),
            ExistingWriteMountAction::Conflict
        );
    }

    #[test]
    fn active_job_state_proves_a_mounted_tape_was_accepted_without_byte_progress() {
        let mut active = ActiveManualMedia {
            request_id: Uuid::now_v7(),
            volume_order: 2,
            observed_job_progress: false,
            files_at_mount: 7,
            bytes_at_mount: 4096,
        };
        let mut evidence = BaculaJobEvidence {
            job_id: BaculaJobId::new(42).unwrap(),
            resource_name: None,
            unique_name: None,
            raw_status: 'R',
            state: BaculaJobState::Running,
            files: 7,
            bytes: 4096,
            errors: 0,
            start_time: None,
            end_time: None,
        };
        observe_manual_media_progress(&mut active, &evidence);
        assert!(active.observed_job_progress);

        active.observed_job_progress = false;
        evidence.raw_status = 'M';
        evidence.state = BaculaJobState::WaitingForMount;
        observe_manual_media_progress(&mut active, &evidence);
        assert!(!active.observed_job_progress);

        evidence.bytes += 1;
        observe_manual_media_progress(&mut active, &evidence);
        assert!(active.observed_job_progress);
    }

    #[test]
    fn archive_volume_labels_are_stable_and_unique_per_tape() {
        let run_id = Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").unwrap();
        let first = engine_volume_label_for_order("AA 0344", run_id, 1).unwrap();
        let second = engine_volume_label_for_order("AA 0345", run_id, 2).unwrap();
        assert_eq!(first.as_str(), "AA-0344-018f7cac2e2a");
        assert_eq!(second.as_str(), "AA-0345-018f7cac2e2a-V0002");
        assert_ne!(first, second);
        assert!(engine_volume_label_for_order("AA0346", run_id, 0).is_err());
    }

    #[test]
    fn spool_limit_preserves_reserve_and_does_not_limit_archive_size() {
        let gib = 1024_u64 * 1024 * 1024;
        assert_eq!(
            effective_spool_limit(200 * gib, 8 * gib, 64 * gib, 8 * gib)
                .expect("ample spool capacity")
                .get(),
            64 * gib
        );
        assert_eq!(
            effective_spool_limit(48 * gib, 8 * gib, 64 * gib, 8 * gib)
                .expect("available space caps the buffer")
                .get(),
            40 * gib
        );
        assert!(effective_spool_limit(15 * gib, 8 * gib, 64 * gib, 8 * gib).is_none());
    }

    #[test]
    fn attribute_spool_estimate_scales_without_an_entry_ceiling() {
        let estimate = estimated_attribute_spool_reserve(1_000_000, 256 * 1024 * 1024)
            .expect("bounded estimate");
        assert_eq!(
            estimate,
            ATTRIBUTE_SPOOL_BASE_BYTES
                + 1_000_000 * ATTRIBUTE_SPOOL_BYTES_PER_ENTRY
                + 512 * 1024 * 1024
        );
        assert!(estimated_attribute_spool_reserve(u64::MAX, u64::MAX).is_none());
    }
}
