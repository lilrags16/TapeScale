use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use chrono::Utc;
use rustix::fs::statvfs;
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::*;
use crate::{
    native_archive::{
        conservative_usable_tape_bytes, directory_regular_file_usage, lto_native_capacity_bytes,
        open_tape_reader, plan_volumes, read_and_hash_tape_file_with_progress, read_tape_file,
        read_tape_file_with_progress, sha256_bytes, write_tape_bytes, write_tape_path,
        DarCompression, DarCreateInvocation, DarTestInvocation, NativeArchiveError, PlannedVolume,
        StagedDarArchive, StagedSlice, NATIVE_FORMAT_NAME,
    },
    native_format::{
        NativePreparationHeader, NativeSliceDescriptor, NativeVolumeHeader, NativeVolumeIndex,
        NativeVolumeTrailer, NATIVE_FORMAT_VERSION,
    },
    native_tape::archive_volume_identity,
    tool_runner::{self, ToolCommand},
};

const NATIVE_STAGE_BASE_BYTES: u64 = 64 * 1024 * 1024;
const NATIVE_STAGE_ENTRY_BYTES: u64 = 4 * 1024;
const NATIVE_METADATA_READ_LIMIT: u64 = 256 * 1024 * 1024;
const COMMAND_OUTPUT_LIMIT: usize = 64 * 1024;
const LIVE_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_NATIVE_LIVE_CURRENT_FILE_BYTES: usize = 4 * 1024;

#[derive(Debug)]
struct CompletedNativeBackup {
    source_manifest: ManifestArtifactSummary,
    archive: StagedDarArchive,
    volumes: Vec<NativeVolumeEvidence>,
    compression: DarCompression,
}

#[derive(Clone, Debug)]
struct NativeVolumeEvidence {
    volume_order: i32,
    cartridge_id: Uuid,
    archive_volume_id: Uuid,
    volume_name: String,
    header_sha256: String,
    index_sha256: String,
    tape_file_count: i32,
    encoded_bytes: u64,
    first_record: String,
    last_record: String,
    engine_record_id: String,
    segment_files: Vec<(i32, i32)>,
}

pub(super) async fn execute_claimed(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
) -> Result<()> {
    let evidence_directory = config.evidence_root.join(claim.run.id.to_string());
    let stage_directory = config.native_spool_dir.join(claim.run.id.to_string());
    let mut tape_touched = false;
    let mut media_write_started = false;
    let operation = perform_native_backup(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        &evidence_directory,
        &stage_directory,
        &mut tape_touched,
        &mut media_write_started,
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

    match operation {
        Ok(completed) if cleanup.0 => {
            let result = finalize_native_backup(
                pool,
                claim,
                &evidence_directory,
                completed,
                cleanup.1,
            )
            .await;
            if result.is_ok() {
                if let Err(error) = fs::remove_dir_all(&stage_directory) {
                    warn!(path = %stage_directory.display(), %error, "could not remove completed native archive stage");
                }
            }
            result
        }
        Ok(_) => {
            finish_or_quarantine(
                pool,
                claim,
                store::SingleTapeDataTerminalState::Failed,
                json!({
                    "detail": "native archive write and readback completed, but the drive could not be proven ready at BOT",
                    "position_uncertain": true,
                    "media_write_started": media_write_started,
                    "cleanup": cleanup.1,
                    "staged_archive_retained": stage_directory
                }),
            )
            .await
        }
        Err(error) => {
            let terminal = match error.class {
                FailureClass::Blocked => store::SingleTapeDataTerminalState::Blocked,
                FailureClass::Failed => store::SingleTapeDataTerminalState::Failed,
                FailureClass::Interrupted => store::SingleTapeDataTerminalState::Interrupted,
                FailureClass::Cancelled => store::SingleTapeDataTerminalState::Cancelled,
            };
            let result = json!({
                "detail": error.detail,
                "position_uncertain": tape_touched && !cleanup.0,
                "media_write_started": media_write_started,
                "cleanup": cleanup.1,
                "archive_codec": "dar",
                "staged_archive_retained": tape_touched.then_some(stage_directory.clone())
            });
            if media_write_started {
                finish_or_quarantine(pool, claim, terminal, result).await
            } else {
                finish(pool, claim, terminal, result).await?;
                if let Err(cleanup_error) = fs::remove_dir_all(&stage_directory) {
                    if cleanup_error.kind() != std::io::ErrorKind::NotFound {
                        warn!(path = %stage_directory.display(), %cleanup_error, "could not remove unused native archive stage");
                    }
                }
                Ok(())
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn perform_native_backup(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    evidence_directory: &Path,
    stage_directory: &Path,
    tape_touched: &mut bool,
    media_write_started: &mut bool,
) -> Result<CompletedNativeBackup, ExerciseFailure> {
    if claim.run.workflow_kind != "backup" {
        return Err(ExerciseFailure::blocked(
            "TapeScale Native v1 currently accepts operational backup runs only",
        ));
    }
    validate_paths(config, claim)?;
    verify_read_only_mount(Path::new(&claim.run.source_mount_path)).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "the worker could not prove the selected source is mounted read-only: {error}"
        ))
    })?;
    prepare_stage_root(config, stage_directory)?;

    transition(
        pool,
        claim,
        store::SingleTapeDataPhase::Inventorying,
        json!({
            "detail": "validating and hashing the immutable source before archive creation",
            "archive_codec": "dar",
            "tape_write_started": false
        }),
    )
    .await?;
    let source_manifest = scan_and_persist_manifest_with_lease(
        pool,
        claim,
        PathBuf::from(&claim.run.source_mount_path),
        evidence_directory.to_owned(),
        "source-manifest".to_owned(),
        store::SingleTapePreparationStage::SourceInventory,
        cancellation,
    )
    .await?;
    append_manifest_artifact(pool, claim, "source_manifest", &source_manifest).await?;

    let (stage_capacity, stage_target) = native_stage_capacity(config, &source_manifest)?;
    transition(
        pool,
        claim,
        store::SingleTapeDataPhase::Writing,
        json!({
            "detail": "packing and compressing the complete source into large DAR slices on disk; the tape remains idle until this finishes",
            "archive_id": claim.run.archive_id,
            "archive_codec": "dar",
            "source_manifest_sha256": source_manifest.manifest_digest,
            "file_count": source_manifest.file_count,
            "logical_bytes": source_manifest.logical_bytes,
            "stage_directory": stage_directory,
            "stage_capacity_bytes": stage_capacity,
            "stage_target_bytes": stage_target,
            "shoe_shine_protection": "complete_archive_staging"
        }),
    )
    .await?;

    let compression = DarCompression::from_resolved(&claim.run.resolved_compression);
    let invocation = DarCreateInvocation::new(
        &config.dar_path,
        Path::new(&claim.run.source_mount_path),
        stage_directory,
        config.native_slice_bytes,
        compression,
        config.native_compression_threads,
    )
    .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;
    run_dar_command(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        &invocation.program,
        &invocation.args,
        stage_directory,
        stage_capacity,
        stage_target,
        source_manifest.entry_count,
        "packing",
    )
    .await?;
    let archive = StagedDarArchive::discover(stage_directory, compression)
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    if archive.encoded_bytes > stage_capacity {
        return Err(ExerciseFailure::failed(
            "the completed native archive exceeded the fenced staging capacity",
        ));
    }

    let test = DarTestInvocation::new(
        &config.dar_path,
        &archive.archive_base,
        config.native_compression_threads,
    );
    run_dar_command(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        &test.program,
        &test.args,
        stage_directory,
        stage_capacity,
        stage_target,
        source_manifest.entry_count,
        "validating",
    )
    .await?;

    let staged_segments = archive
        .slices
        .iter()
        .map(|slice| {
            Ok(store::NativeStagedSegment {
                segment_order: i32::try_from(slice.ordinal)
                    .map_err(|_| ExerciseFailure::failed("DAR produced too many slices"))?,
                content_sha256: slice.sha256.clone(),
                encoded_bytes: slice.encoded_bytes,
                storage_ref: slice
                    .path
                    .to_str()
                    .ok_or_else(|| ExerciseFailure::failed("DAR slice path is not valid UTF-8"))?
                    .to_owned(),
            })
        })
        .collect::<Result<Vec<_>, ExerciseFailure>>()?;
    if !store::record_native_staged_segments(pool, claim, &staged_segments)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the fenced run rejected the staged native archive ledger",
        ));
    }

    let native_capacity = lto_native_capacity_bytes(
        claim.run.expected_media_generation,
        claim.run.expected_media_format.as_deref(),
    )
    .ok_or_else(|| {
        ExerciseFailure::blocked("the selected tape has no recognized uncompressed LTO capacity")
    })?;
    let repeated_metadata = source_manifest
        .size_bytes
        .checked_add(archive.catalog_bytes)
        .and_then(|value| value.checked_add(128 * 1024 * 1024))
        .ok_or_else(|| ExerciseFailure::blocked("native metadata size overflowed"))?;
    let usable = conservative_usable_tape_bytes(native_capacity, repeated_metadata)
        .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;
    let plan = plan_volumes(&archive.slices, usable, claim.run.spanning_enabled)
        .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;

    let mut completed_volumes = Vec::with_capacity(plan.len());
    for (index, volume_plan) in plan.iter().enumerate() {
        let volume = if index == 0 {
            store::data_run_volumes(pool, claim.run.id)
                .await
                .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
                .into_iter()
                .find(|volume| volume.volume_order == 1)
                .ok_or_else(|| {
                    ExerciseFailure::interrupted("the first archive volume assignment disappeared")
                })?
        } else {
            request_next_write_volume(
                pool,
                sas_config,
                config,
                claim,
                i32::try_from(index + 1)
                    .map_err(|_| ExerciseFailure::failed("archive uses too many tapes"))?,
                cancellation,
            )
            .await?
        };
        let capacity = lto_native_capacity_bytes(
            volume.expected_media_generation,
            volume.expected_media_format.as_deref(),
        )
        .and_then(|capacity| conservative_usable_tape_bytes(capacity, repeated_metadata).ok())
        .ok_or_else(|| {
            ExerciseFailure::blocked(format!(
                "archive volume {} has no usable native LTO capacity",
                volume.volume_order
            ))
        })?;
        if volume_plan.encoded_bytes > capacity {
            return Err(ExerciseFailure::blocked(format!(
                "archive volume {} is too small for its planned complete DAR slices",
                volume.volume_order
            )));
        }
        if index == 0 {
            verify_native_preparation(pool, sas_config, config, claim, &volume, cancellation)
                .await?;
        }
        let written = write_and_verify_volume(
            pool,
            sas_config,
            config,
            claim,
            cancellation,
            &source_manifest,
            &archive,
            &plan,
            volume_plan,
            &volume,
            tape_touched,
            media_write_started,
        )
        .await?;
        completed_volumes.push(written);
        if index + 1 < plan.len() {
            execute_sas_motion(
                sas_config,
                Path::new(&claim.run.expected_path),
                SasMotionOperation::Unload,
                cancellation,
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "could not unload verified archive volume {} for the next tape: {}",
                    volume.volume_order,
                    sas_command_error_detail(&error)
                ))
            })?;
        }
    }

    let engine_write = native_engine_write_json(claim, &archive, &completed_volumes);
    let engine_artifact = persist_json_artifact(evidence_directory, "engine-write", &engine_write)
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    append_json_artifact(
        pool,
        claim,
        "engine_write",
        &engine_artifact,
        engine_write.clone(),
    )
    .await?;
    transition(
        pool,
        claim,
        store::SingleTapeDataPhase::Written,
        json!({
            "detail": "every planned TapeScale Native volume was written and immediately read back",
            "archive_id": claim.run.archive_id,
            "volume_count": completed_volumes.len(),
            "encoded_bytes": archive.encoded_bytes
        }),
    )
    .await?;
    transition(
        pool,
        claim,
        store::SingleTapeDataPhase::Verifying,
        json!({
            "detail": "physical volume readback passed; confirming the source snapshot remained unchanged",
            "physical_media_readback": true,
            "volume_count": completed_volumes.len()
        }),
    )
    .await?;
    let source_after = scan_and_persist_manifest_with_lease(
        pool,
        claim,
        PathBuf::from(&claim.run.source_mount_path),
        evidence_directory.to_owned(),
        "source-stability-after".to_owned(),
        store::SingleTapePreparationStage::SourceStabilityCheck,
        cancellation,
    )
    .await?;
    let stable = source_manifest.source_stability_digest_sha256
        == source_after.source_stability_digest_sha256;
    if let Err(error) = fs::remove_file(&source_after.storage_path) {
        warn!(path = %source_after.storage_path.display(), %error, "could not remove temporary source-stability manifest");
    }
    if !stable {
        return Err(ExerciseFailure::failed(
            "the supposedly immutable source changed during backup; no recovery point can be committed",
        ));
    }

    Ok(CompletedNativeBackup {
        source_manifest,
        archive,
        volumes: completed_volumes,
        compression,
    })
}

fn prepare_stage_root(config: &Config, stage_directory: &Path) -> Result<(), ExerciseFailure> {
    fs::create_dir_all(&config.native_spool_dir).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "could not create native archive spool {}: {error}",
            config.native_spool_dir.display()
        ))
    })?;
    let root = fs::symlink_metadata(&config.native_spool_dir).map_err(|error| {
        ExerciseFailure::blocked(format!("could not inspect native archive spool: {error}"))
    })?;
    if !root.is_dir() || root.file_type().is_symlink() {
        return Err(ExerciseFailure::blocked(
            "the native archive spool must be a real directory",
        ));
    }
    fs::create_dir(stage_directory).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "could not create an empty run-scoped native archive stage: {error}"
        ))
    })
}

fn native_stage_capacity(
    config: &Config,
    manifest: &ManifestArtifactSummary,
) -> Result<(u64, u64), ExerciseFailure> {
    let status = statvfs(&config.native_spool_dir).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "could not inspect native archive spool capacity at {}: {error}",
            config.native_spool_dir.display()
        ))
    })?;
    let available = status.f_bavail.saturating_mul(status.f_frsize);
    let capacity = available.saturating_sub(config.native_spool_reserve_bytes);
    let payload_headroom = manifest.logical_bytes / 8;
    let entry_headroom = manifest
        .entry_count
        .checked_mul(NATIVE_STAGE_ENTRY_BYTES)
        .ok_or_else(|| ExerciseFailure::blocked("native archive staging estimate overflowed"))?;
    let target = manifest
        .logical_bytes
        .checked_add(payload_headroom)
        .and_then(|value| value.checked_add(entry_headroom))
        .and_then(|value| value.checked_add(manifest.size_bytes))
        .and_then(|value| value.checked_add(NATIVE_STAGE_BASE_BYTES))
        .ok_or_else(|| ExerciseFailure::blocked("native archive staging estimate overflowed"))?;
    if target > capacity {
        return Err(ExerciseFailure::blocked(format!(
            "complete archive staging needs approximately {target} bytes before tape motion, but only {capacity} bytes are available after the configured reserve"
        )));
    }
    Ok((capacity, target))
}

#[allow(clippy::too_many_arguments)]
async fn run_dar_command(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    program: &Path,
    args: &[std::ffi::OsString],
    stage_directory: &Path,
    stage_capacity: u64,
    stage_target: u64,
    files_total: u64,
    stage: &'static str,
) -> Result<(), ExerciseFailure> {
    let files_total = files_total.max(1);
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("HOME", "/var/empty")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            ExerciseFailure::blocked(format!(
                "could not start the pinned DAR archive codec at {}: {error}",
                program.display()
            ))
        })?;
    let current_file = Arc::new(Mutex::new(None::<String>));
    let treated = Arc::new(AtomicU64::new(0));
    let output = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let stdout_task = spawn_dar_reader(
        child.stdout.take(),
        PathBuf::from(&claim.run.source_mount_path),
        Arc::clone(&current_file),
        Arc::clone(&treated),
        Arc::clone(&output),
    );
    let stderr_task = spawn_dar_reader(
        child.stderr.take(),
        PathBuf::from(&claim.run.source_mount_path),
        Arc::clone(&current_file),
        Arc::clone(&treated),
        Arc::clone(&output),
    );
    let started = Instant::now();
    let deadline = started + config.data_run_timeout;
    let mut tick = tokio::time::interval(LIVE_SAMPLE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    let mut previous = None;
    let mut last_usage = None;
    let mut last_treated = 0_u64;
    let mut last_movement = started;
    let mut cancelled = None;
    let status = loop {
        tokio::select! {
            status = child.wait() => {
                break status.map_err(|error| ExerciseFailure::failed(format!("DAR process wait failed: {error}")))?;
            }
            () = cancellation.cancelled(), if cancelled.is_none() => {
                cancelled = Some("worker shutdown was requested while DAR was running");
                let _ = child.start_kill();
            }
            _ = tick.tick() => {
                if Instant::now() >= deadline && cancelled.is_none() {
                    cancelled = Some("DAR exceeded the configured backup deadline");
                    let _ = child.start_kill();
                }
                renew_claim(pool, claim, "while building or validating the staged native archive").await?;
                let usage = directory_regular_file_usage(stage_directory)
                    .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
                if usage.0 > stage_capacity {
                    cancelled = Some("the native archive exceeded its fenced staging capacity");
                    let _ = child.start_kill();
                }
                let now = Instant::now();
                let treated_now = treated.load(Ordering::Relaxed).min(files_total);
                if last_usage != Some(usage.0) || last_treated != treated_now {
                    last_movement = now;
                    last_usage = Some(usage.0);
                    last_treated = treated_now;
                }
                let idle = now.saturating_duration_since(last_movement);
                if idle >= config.transfer_stall_timeout && cancelled.is_none() {
                    cancelled = Some("DAR made no archive progress before the configured transfer-stall deadline");
                    let _ = child.start_kill();
                }
                let rate = previous.and_then(|(bytes, observed)| bytes_per_second(bytes, usage.0, now.saturating_duration_since(observed)));
                previous = Some((usage.0, now));
                publish_native_sample(
                    pool,
                    sas_config,
                    claim,
                    NativeSample {
                        files_processed: treated_now,
                        files_total,
                        pipeline_bytes: usage.0,
                        pipeline_total: None,
                        pipeline_rate: rate,
                        current_file: current_file.lock().ok().and_then(|value| value.clone()),
                        tape_read_rate: None,
                        tape_write_rate: None,
                        activity: if idle >= config.transfer_stall_timeout {
                            store::SingleTapeLiveActivity::Stalled
                        } else {
                            store::SingleTapeLiveActivity::Buffering
                        },
                        engine_state: stage,
                        progress_basis: "native_archive_slices_or_entries",
                        progress_scope: if stage == "packing" { 1 } else { 2 },
                        progress_idle_ms: duration_millis_u64(idle),
                        no_progress_timeout_ms: duration_millis_u64(config.transfer_stall_timeout),
                        spool: Some((usage.0, stage_capacity, stage_target, usage.1, rate)),
                    },
                ).await;
            }
        }
    };
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    if let Some(reason) = cancelled {
        return Err(if cancellation.is_cancelled() {
            ExerciseFailure::interrupted(reason)
        } else {
            ExerciseFailure::failed(reason)
        });
    }
    if !status.success() {
        let detail = output
            .lock()
            .ok()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join(" | "))
            .unwrap_or_else(|| "no diagnostic output was retained".to_owned());
        return Err(ExerciseFailure::failed(format!(
            "DAR {stage} failed with status {status}: {}",
            concise_text(&detail, 2048)
        )));
    }
    Ok(())
}

fn spawn_dar_reader(
    stream: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
    source_root: PathBuf,
    current_file: Arc<Mutex<Option<String>>>,
    treated: Arc<AtomicU64>,
    output: Arc<Mutex<VecDeque<String>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(stream) = stream else { return };
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(relative) = dar_current_file(&source_root, &line) {
                if let Ok(mut current) = current_file.lock() {
                    *current = Some(relative);
                }
                treated.fetch_add(1, Ordering::Relaxed);
            }
            if let Ok(mut retained) = output.lock() {
                retained.push_back(concise_text(&line, 512));
                let retained_bytes = retained.iter().map(String::len).sum::<usize>();
                if retained_bytes > COMMAND_OUTPUT_LIMIT {
                    retained.pop_front();
                }
            }
        }
    })
}

fn dar_current_file(source_root: &Path, line: &str) -> Option<String> {
    let trimmed = line.trim();
    let candidate_text = if let Some(adding) = trimmed.strip_prefix("Adding ") {
        adding.split_once(" to archive: ")?.1.trim_matches('"')
    } else if let Some(root_relative) = trimmed
        .strip_prefix("OK")
        .map(str::trim_start)
        .and_then(|value| value.strip_prefix("<ROOT>/"))
    {
        root_relative
            .strip_suffix("  Data")
            .unwrap_or(root_relative)
            .trim_end()
            .trim_matches('"')
    } else {
        return None;
    };
    let candidate = Path::new(candidate_text);
    let relative = if candidate.is_absolute() {
        candidate.strip_prefix(source_root).ok()?
    } else {
        candidate.strip_prefix("./").unwrap_or(candidate)
    };
    if relative.as_os_str().is_empty()
        || relative.to_string_lossy().len() > MAX_NATIVE_LIVE_CURRENT_FILE_BYTES
        || !relative.components().all(|component| {
            matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return None;
    }
    relative.to_str().map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::dar_current_file;
    use std::path::Path;

    #[test]
    fn dar_progress_extracts_only_source_relative_paths() {
        let root = Path::new("/source/tank/.zfs/snapshot/daily");
        assert_eq!(
            dar_current_file(
                root,
                "Adding file to archive: /source/tank/.zfs/snapshot/daily/photos/one.jpg"
            )
            .as_deref(),
            Some("photos/one.jpg")
        );
        assert_eq!(
            dar_current_file(root, "OK  <ROOT>/photos/one.jpg  Data").as_deref(),
            Some("photos/one.jpg")
        );
        assert!(dar_current_file(root, "3 inode(s) saved").is_none());
        assert!(dar_current_file(root, "Adding file to archive: /etc/shadow").is_none());
    }
}

#[derive(Clone, Debug)]
struct NativeSample {
    files_processed: u64,
    files_total: u64,
    pipeline_bytes: u64,
    pipeline_total: Option<u64>,
    pipeline_rate: Option<u64>,
    current_file: Option<String>,
    tape_read_rate: Option<u64>,
    tape_write_rate: Option<u64>,
    activity: store::SingleTapeLiveActivity,
    engine_state: &'static str,
    progress_basis: &'static str,
    progress_scope: u64,
    progress_idle_ms: u64,
    no_progress_timeout_ms: u64,
    spool: Option<(u64, u64, u64, u64, Option<u64>)>,
}

async fn publish_native_sample(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedSingleTapeDataRun,
    sample: NativeSample,
) {
    let counters = read_sas_tape_io_counters(sas_config, Path::new(&claim.run.expected_path)).ok();
    let tape_read_available = counters.is_some_and(|value| value.read_bytes.is_some());
    let tape_write_available = counters.is_some_and(|value| value.write_bytes.is_some());
    let tape_available = tape_read_available || tape_write_available;
    let engine_job_id = native_job_id(claim.run.archive_id, sample.progress_scope);
    let live = store::SingleTapeLiveIoSample {
        // Native volumes are read back immediately while the aggregate backup
        // remains in its write phase. Activity and tape rates distinguish the
        // physical direction without inventing a second media-change cycle.
        phase: store::SingleTapeLiveIoPhase::BackupWrite,
        sampled_at: Utc::now(),
        sample_window_ms: Some(LIVE_SAMPLE_INTERVAL.as_millis() as u64),
        stale_after_seconds: LIVE_IO_STALE_AFTER_SECONDS,
        engine_job_id,
        files_processed: sample.files_processed,
        files_total: Some(sample.files_total),
        progress_basis: sample.progress_basis,
        archive_entries_per_second: None,
        pipeline_bytes_processed: sample.pipeline_bytes,
        pipeline_bytes_total: sample.pipeline_total,
        pipeline_bytes_per_second: sample.pipeline_rate,
        file_daemon_read_bytes_processed: None,
        file_daemon_read_bytes_per_second: None,
        current_file: sample.current_file,
        file_daemon_basis: None,
        tape_read_bytes_per_second: sample.tape_read_rate,
        tape_write_bytes_per_second: sample.tape_write_rate,
        tape_read_counter_available: tape_read_available,
        tape_write_counter_available: tape_write_available,
        tape_counters_available: tape_available,
        pipeline_basis: "native_archive_bytes",
        tape_basis: tape_available.then_some("linux_scsi_tape_sysfs_counter_delta"),
        engine_state: Some(sample.engine_state),
        activity: Some(sample.activity),
        progress_idle_ms: Some(sample.progress_idle_ms),
        no_progress_timeout_ms: Some(sample.no_progress_timeout_ms),
        spool_bytes: sample.spool.map(|value| value.0),
        spool_capacity_bytes: sample.spool.map(|value| value.1),
        spool_target_bytes: sample.spool.map(|value| value.2),
        spool_file_count: sample.spool.map(|value| value.3),
        spool_fill_bytes_per_second: sample.spool.and_then(|value| value.4),
        spool_drain_bytes_per_second: None,
        spool_basis: sample
            .spool
            .map(|_| "run_scoped_native_archive_regular_file_bytes"),
    };
    if let Err(error) = store::record_single_tape_data_live_io(pool, claim, &live).await {
        warn!(backup_run_id = %claim.run.id, %error, "could not publish native archive progress");
    }
}

const fn native_job_id(archive_id: Uuid, progress_scope: u64) -> u64 {
    let bytes = archive_id.as_bytes();
    u64::from_be_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ])
    .wrapping_add(progress_scope.rotate_left(17))
        | 1
}

async fn request_next_write_volume(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    volume_order: i32,
    cancellation: &CancellationToken,
) -> Result<store::DataRunVolumeRecord, ExerciseFailure> {
    match store::ensure_data_run_media_request(
        pool,
        claim,
        "backup_write",
        volume_order,
        None,
        None,
    )
    .await
    .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        store::DataRunMediaRequestResult::Ready(_) => {}
        store::DataRunMediaRequestResult::FencedOut => {
            return Err(ExerciseFailure::interrupted(
                "the native media request was fenced out",
            ))
        }
        store::DataRunMediaRequestResult::Blocked(reason) => {
            return Err(ExerciseFailure::blocked(reason))
        }
    }
    let mut tick = tokio::time::interval(LIVE_SAMPLE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                return Err(ExerciseFailure::interrupted("worker shutdown was requested while waiting for the next archive tape"));
            }
            _ = tick.tick() => {
                renew_claim(pool, claim, "while waiting for the next native archive tape").await?;
                if let Some((request, volume)) = store::authorized_data_run_media_request(pool, claim)
                    .await
                    .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
                {
                    if request.phase != "backup_write" || request.volume_order != volume_order {
                        return Err(ExerciseFailure::interrupted("the authorized media request changed while the worker was waiting"));
                    }
                    let observations = discover_and_persist(pool, sas_config)
                        .await
                        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?;
                    let observed = matching_observation(
                        &observations,
                        &claim.run.expected_path,
                        &claim.run.expected_stable_id,
                    )
                    .ok_or_else(|| ExerciseFailure::blocked("the selected tape drive was not observed after the media change"))?;
                    if let Some(reason) = observation_blocker(observed, &claim.run) {
                        return Err(ExerciseFailure::blocked(reason));
                    }
                    verify_native_preparation(
                        pool,
                        sas_config,
                        config,
                        claim,
                        &volume,
                        cancellation,
                    )
                    .await?;
                    if !store::mark_data_run_media_loaded(pool, claim, request.id)
                        .await
                        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
                    {
                        return Err(ExerciseFailure::interrupted("the fenced run rejected the newly loaded archive tape"));
                    }
                    return Ok(volume);
                }
            }
        }
    }
}

async fn verify_native_preparation(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    _config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    volume: &store::DataRunVolumeRecord,
    cancellation: &CancellationToken,
) -> Result<(), ExerciseFailure> {
    execute_sas_motion(
        sas_config,
        Path::new(&claim.run.expected_path),
        SasMotionOperation::Rewind,
        cancellation,
    )
    .await
    .map_err(|error| {
        ExerciseFailure::blocked(format!(
            "could not rewind the selected scratch tape for identity verification: {}",
            sas_command_error_detail(&error)
        ))
    })?;
    let device = PathBuf::from(&claim.run.expected_path);
    let expected = volume.cartridge_id;
    let preparation = tokio::task::spawn_blocking(move || {
        let mut tape = open_tape_reader(&device)?;
        let bytes = read_tape_file(&mut tape, NATIVE_METADATA_READ_LIMIT)?;
        NativePreparationHeader::decode(&bytes)
            .map_err(|error| NativeArchiveError::Invalid(error.to_string()))
    })
    .await
    .map_err(|error| ExerciseFailure::failed(format!("scratch identity reader failed: {error}")))?
    .map_err(|error| {
        ExerciseFailure::blocked(format!(
            "the loaded scratch tape has no valid TapeScale preparation identity: {error}"
        ))
    })?;
    execute_sas_motion(
        sas_config,
        Path::new(&claim.run.expected_path),
        SasMotionOperation::Rewind,
        cancellation,
    )
    .await
    .map_err(|error| ExerciseFailure::failed(sas_command_error_detail(&error)))?;
    if preparation.cartridge_id != expected {
        return Err(ExerciseFailure::blocked(format!(
            "wrong tape loaded: the drive contains cartridge {}, but archive volume {} requires {}",
            preparation.cartridge_id, volume.volume_order, expected
        )));
    }
    store::record_loaded_tape_identity(
        pool,
        claim.run.drive_id,
        &claim.run.expected_stable_id,
        &claim.run.expected_path,
        Some(&format!("TS-PREP-V1-{expected}")),
        true,
        None,
        Utc::now(),
    )
    .await
    .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn write_and_verify_volume(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    source_manifest: &ManifestArtifactSummary,
    archive: &StagedDarArchive,
    plan: &[PlannedVolume],
    volume_plan: &PlannedVolume,
    volume: &store::DataRunVolumeRecord,
    tape_touched: &mut bool,
    media_write_started: &mut bool,
) -> Result<NativeVolumeEvidence, ExerciseFailure> {
    let volume_name = archive_volume_identity(
        claim.run.archive_id,
        u32::try_from(volume.volume_order)
            .map_err(|_| ExerciseFailure::failed("invalid native volume order"))?,
    );
    if !store::mark_data_run_volume_write_started(pool, claim, volume.volume_order, &volume_name)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the fenced run could not commit the native media-write boundary",
        ));
    }
    *media_write_started = true;
    *tape_touched = true;
    set_variable_block_mode(
        sas_config,
        Path::new(&claim.run.expected_path),
        cancellation,
    )
    .await?;
    clear_sas_scratch_label(
        sas_config,
        Path::new(&claim.run.expected_path),
        cancellation,
    )
    .await
    .map_err(|error| ExerciseFailure::failed(sas_command_error_detail(&error)))?;
    execute_sas_motion(
        sas_config,
        Path::new(&claim.run.expected_path),
        SasMotionOperation::Rewind,
        cancellation,
    )
    .await
    .map_err(|error| ExerciseFailure::failed(sas_command_error_detail(&error)))?;
    renew_claim(
        pool,
        claim,
        "before streaming the staged native archive to tape",
    )
    .await?;

    let archive_volume_id = Uuid::now_v7();
    let slice_range =
        volume_plan.first_slice_index..volume_plan.first_slice_index + volume_plan.slice_count;
    let slices = &archive.slices[slice_range.clone()];
    let archive_offset = archive
        .slices
        .iter()
        .take(volume_plan.first_slice_index)
        .map(|slice| slice.encoded_bytes)
        .sum::<u64>();
    let descriptors = slices
        .iter()
        .enumerate()
        .map(|(index, slice)| NativeSliceDescriptor {
            slice_ordinal: slice.ordinal,
            tape_file_number: u32::try_from(index + 3).unwrap_or(u32::MAX),
            archive_offset_bytes: archive_offset
                + slices
                    .iter()
                    .take(index)
                    .map(|prior| prior.encoded_bytes)
                    .sum::<u64>(),
            encoded_bytes: slice.encoded_bytes,
            logical_bytes: 0,
            entry_count: 0,
            sha256: slice.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let index = NativeVolumeIndex {
        format_version: NATIVE_FORMAT_VERSION,
        cartridge_id: volume.cartridge_id,
        archive_id: claim.run.archive_id,
        archive_volume_id,
        volume_ordinal: u32::try_from(volume.volume_order)
            .map_err(|_| ExerciseFailure::failed("invalid native volume order"))?,
        volume_count: u32::try_from(plan.len())
            .map_err(|_| ExerciseFailure::failed("archive uses too many tapes"))?,
        source_manifest_sha256: source_manifest.artifact_sha256.clone(),
        archive_catalog_sha256: archive.catalog_sha256.clone(),
        total_encoded_bytes: volume_plan.encoded_bytes,
        total_logical_bytes: 0,
        slices: descriptors,
    };
    let index_bytes = index
        .encode()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let trailer = NativeVolumeTrailer::from_index(&index, Utc::now())
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let trailer_bytes = trailer
        .encode()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let header = NativeVolumeHeader {
        format_version: NATIVE_FORMAT_VERSION,
        cartridge_id: volume.cartridge_id,
        archive_id: claim.run.archive_id,
        archive_volume_id,
        volume_ordinal: u32::try_from(volume.volume_order)
            .map_err(|_| ExerciseFailure::failed("invalid native volume order"))?,
        volume_count: u32::try_from(plan.len())
            .map_err(|_| ExerciseFailure::failed("archive uses too many tapes"))?,
        slice_count: u32::try_from(slices.len())
            .map_err(|_| ExerciseFailure::failed("native volume has too many slices"))?,
        representation: archive.representation,
        compression: archive.compression.native_codec(),
        tape_block_bytes: crate::native_format::DEFAULT_TAPE_BLOCK_BYTES,
        source_manifest_sha256: source_manifest.artifact_sha256.clone(),
        archive_catalog_sha256: archive.catalog_sha256.clone(),
        created_at: Utc::now(),
    };
    let header_bytes = header
        .encode()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let device = PathBuf::from(&claim.run.expected_path);
    let progress_total = source_manifest.entry_count.max(1);
    write_one_tape_file(
        pool,
        sas_config,
        claim,
        device.clone(),
        TapePayload::Bytes(header_bytes.clone()),
        archive_offset,
        archive.encoded_bytes,
        0,
        Some("TapeScale volume header".to_owned()),
        config.transfer_stall_timeout,
        progress_total,
        cancellation,
    )
    .await?;
    if !store::mark_data_run_volume_labeled(pool, claim, volume.volume_order, &volume_name)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the native volume header was written but its identity could not be retained",
        ));
    }
    write_one_tape_file(
        pool,
        sas_config,
        claim,
        device.clone(),
        TapePayload::Path(archive.catalog_path.clone()),
        archive_offset,
        archive.encoded_bytes,
        0,
        archive
            .catalog_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned),
        config.transfer_stall_timeout,
        progress_total,
        cancellation,
    )
    .await?;
    write_one_tape_file(
        pool,
        sas_config,
        claim,
        device.clone(),
        TapePayload::Path(source_manifest.storage_path.clone()),
        archive_offset,
        archive.encoded_bytes,
        0,
        source_manifest
            .storage_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned),
        config.transfer_stall_timeout,
        progress_total,
        cancellation,
    )
    .await?;
    let mut completed_payload = archive_offset;
    for slice in slices {
        write_one_tape_file(
            pool,
            sas_config,
            claim,
            device.clone(),
            TapePayload::Path(slice.path.clone()),
            completed_payload,
            archive.encoded_bytes,
            slice.encoded_bytes,
            slice
                .path
                .file_name()
                .and_then(|value| value.to_str())
                .map(ToOwned::to_owned),
            config.transfer_stall_timeout,
            progress_total,
            cancellation,
        )
        .await?;
        completed_payload = completed_payload.saturating_add(slice.encoded_bytes);
    }
    write_one_tape_file(
        pool,
        sas_config,
        claim,
        device.clone(),
        TapePayload::Bytes(index_bytes.clone()),
        completed_payload,
        archive.encoded_bytes,
        0,
        Some("TapeScale volume index".to_owned()),
        config.transfer_stall_timeout,
        progress_total,
        cancellation,
    )
    .await?;
    write_one_tape_file(
        pool,
        sas_config,
        claim,
        device.clone(),
        TapePayload::Bytes(trailer_bytes.clone()),
        completed_payload,
        archive.encoded_bytes,
        0,
        Some("TapeScale volume trailer".to_owned()),
        config.transfer_stall_timeout,
        progress_total,
        cancellation,
    )
    .await?;

    if volume.volume_order > 1 {
        let request = store::open_data_run_media_request(pool, claim.run.id)
            .await
            .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
            .ok_or_else(|| {
                ExerciseFailure::interrupted("continuation media request disappeared")
            })?;
        if !store::complete_data_run_media_request(pool, claim, request.id, false)
            .await
            .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
        {
            return Err(ExerciseFailure::interrupted(
                "the continuation write boundary could not be retained",
            ));
        }
    }

    execute_sas_motion(
        sas_config,
        Path::new(&claim.run.expected_path),
        SasMotionOperation::Rewind,
        cancellation,
    )
    .await
    .map_err(|error| ExerciseFailure::failed(sas_command_error_detail(&error)))?;
    verify_physical_volume(
        pool,
        sas_config,
        claim,
        &device,
        &header,
        &header_bytes,
        archive,
        source_manifest,
        slices,
        &index,
        &index_bytes,
        &trailer,
        &trailer_bytes,
        progress_total,
        config.transfer_stall_timeout,
        cancellation,
    )
    .await?;
    let tape_file_count = i32::try_from(slices.len() + 5)
        .map_err(|_| ExerciseFailure::failed("native volume has too many tape files"))?;
    let first_record = format!("tape-file=3:slice={}", slices[0].ordinal);
    let last_record = format!(
        "tape-file={}:slice={}",
        slices.len() + 2,
        slices.last().expect("volume has slices").ordinal
    );
    let engine_record_id = format!("tapescale-volume:{archive_volume_id}");
    let segment_files = slices
        .iter()
        .enumerate()
        .map(|(index, slice)| {
            (
                i32::try_from(slice.ordinal).unwrap_or(i32::MAX),
                i32::try_from(index + 3).unwrap_or(i32::MAX),
            )
        })
        .collect::<Vec<_>>();
    let retained = store::NativeVerifiedVolume {
        volume_order: volume.volume_order,
        archive_volume_id,
        engine_volume_name: volume_name.clone(),
        header_sha256: sha256_bytes(&header_bytes),
        index_sha256: sha256_bytes(&index_bytes),
        tape_file_count,
        encoded_bytes: volume_plan.encoded_bytes,
        first_record: first_record.clone(),
        last_record: last_record.clone(),
        segment_files: segment_files.clone(),
    };
    if !store::mark_native_volume_verified(pool, claim, &retained)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the fenced run rejected the verified native volume ledger",
        ));
    }
    Ok(NativeVolumeEvidence {
        volume_order: volume.volume_order,
        cartridge_id: volume.cartridge_id,
        archive_volume_id,
        volume_name,
        header_sha256: retained.header_sha256,
        index_sha256: retained.index_sha256,
        tape_file_count,
        encoded_bytes: volume_plan.encoded_bytes,
        first_record,
        last_record,
        engine_record_id,
        segment_files,
    })
}

async fn set_variable_block_mode(
    sas_config: &SasDiscoveryConfig,
    device: &Path,
    cancellation: &CancellationToken,
) -> Result<(), ExerciseFailure> {
    let command = ToolCommand::new(&sas_config.mt_path)
        .args([
            "-f".to_owned(),
            device.display().to_string(),
            "setblk".to_owned(),
            "0".to_owned(),
        ])
        .timeout(Duration::from_secs(15 * 60))
        .retained_output_bytes(16 * 1024);
    tool_runner::run(&command, cancellation)
        .await
        .map_err(|error| {
            ExerciseFailure::failed(format!(
                "could not enable variable-block tape I/O: {}",
                error.diagnostic_summary(1024)
            ))
        })?;
    Ok(())
}

enum TapePayload {
    Bytes(Vec<u8>),
    Path(PathBuf),
}

#[allow(clippy::too_many_arguments)]
async fn write_one_tape_file(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedSingleTapeDataRun,
    device: PathBuf,
    payload: TapePayload,
    completed_bytes: u64,
    total_bytes: u64,
    archive_payload_bytes: u64,
    current_file: Option<String>,
    no_progress_timeout: Duration,
    files_total: u64,
    cancellation: &CancellationToken,
) -> Result<(), ExerciseFailure> {
    let expected = match &payload {
        TapePayload::Bytes(bytes) => bytes.len() as u64,
        TapePayload::Path(path) => fs::metadata(path)
            .map_err(|error| ExerciseFailure::failed(error.to_string()))?
            .len(),
    };
    let write_progress = Arc::new(AtomicU64::new(0));
    let task_progress = Arc::clone(&write_progress);
    let mut write = tokio::task::spawn_blocking(move || match payload {
        TapePayload::Bytes(bytes) => write_tape_bytes(&device, &bytes, task_progress.as_ref()),
        TapePayload::Path(path) => write_tape_path(&device, &path, task_progress.as_ref()),
    });
    let mut tick = tokio::time::interval(LIVE_SAMPLE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut previous_counter: Option<(u64, Instant)> = None;
    let mut previous_progress: Option<(u64, Instant)> = None;
    let mut last_pipeline_rate = None;
    let mut last_tape_rate = None;
    let mut last_physical_written = 0_u64;
    let mut last_movement = Instant::now();
    let mut cancellation_seen = false;
    let mut deferred_failure = None;
    let mut stalled_failure = None;
    loop {
        tokio::select! {
            result = &mut write => {
                let written = result
                    .map_err(|error| ExerciseFailure::failed(format!("native tape writer failed: {error}")))?
                    .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
                if written != expected {
                    return Err(ExerciseFailure::failed("native tape writer returned an incomplete tape file"));
                }
                break;
            }
            () = cancellation.cancelled(), if !cancellation_seen => {
                // A physical record cannot safely be abandoned mid-write. Finish the
                // current tape file, then honor shutdown before opening another one.
                cancellation_seen = true;
            }
            _ = tick.tick() => {
                if deferred_failure.is_some() {
                    continue;
                }
                if let Err(error) = renew_claim(pool, claim, "while streaming a completed archive slice to tape").await {
                    // Dropping a spawn_blocking JoinHandle does not stop the kernel tape write.
                    // Latch the fencing failure and wait for this tape file to close before the
                    // caller is allowed to rewind or unload the drive.
                    deferred_failure = Some(error);
                    continue;
                }
                let counter = read_sas_tape_io_counters(sas_config, Path::new(&claim.run.expected_path))
                    .ok()
                    .and_then(|value| value.write_bytes);
                let now = Instant::now();
                let tape_rate = previous_counter.and_then(|(before, observed)| counter.and_then(|after| bytes_per_second(before, after, now.saturating_duration_since(observed))));
                let counter_advanced = previous_counter
                    .zip(counter)
                    .is_some_and(|((before, _), after)| after > before);
                if let Some(counter) = counter { previous_counter = Some((counter, now)); }
                let physical_written = write_progress.load(Ordering::Acquire);
                if physical_written > last_physical_written || counter_advanced {
                    last_movement = now;
                    last_physical_written = physical_written;
                }
                let idle = now.saturating_duration_since(last_movement);
                if idle >= no_progress_timeout && stalled_failure.is_none() {
                    stalled_failure = Some(ExerciseFailure::failed(
                        "native tape writing made no physical progress before the configured transfer-stall deadline; the current tape file was allowed to close safely",
                    ));
                }
                let written = physical_written.min(archive_payload_bytes);
                let pipeline_rate = previous_progress.and_then(|(before, observed)| bytes_per_second(before, written, now.saturating_duration_since(observed)));
                previous_progress = Some((written, now));
                last_pipeline_rate = pipeline_rate;
                last_tape_rate = tape_rate.or(pipeline_rate);
                publish_native_sample(pool, sas_config, claim, NativeSample {
                    files_processed: 0,
                    files_total,
                    pipeline_bytes: completed_bytes.saturating_add(written).min(total_bytes),
                    pipeline_total: Some(total_bytes),
                    pipeline_rate,
                    current_file: current_file.clone(),
                    tape_read_rate: None,
                    tape_write_rate: last_tape_rate,
                    activity: if idle >= no_progress_timeout {
                        store::SingleTapeLiveActivity::Stalled
                    } else {
                        store::SingleTapeLiveActivity::Streaming
                    },
                    engine_state: "writing_native_volume",
                    progress_basis: "native_archive_bytes_over_staged_bytes",
                    progress_scope: 3,
                    progress_idle_ms: duration_millis_u64(idle),
                    no_progress_timeout_ms: duration_millis_u64(no_progress_timeout),
                    spool: None,
                }).await;
            }
        }
    }
    if let Some(error) = deferred_failure {
        return Err(error);
    }
    if let Some(error) = stalled_failure {
        return Err(error);
    }
    if cancellation_seen {
        return Err(ExerciseFailure::interrupted(
            "worker shutdown was honored after completing the current native tape file",
        ));
    }
    publish_native_sample(
        pool,
        sas_config,
        claim,
        NativeSample {
            files_processed: 0,
            files_total,
            pipeline_bytes: completed_bytes
                .saturating_add(archive_payload_bytes)
                .min(total_bytes),
            pipeline_total: Some(total_bytes),
            pipeline_rate: last_pipeline_rate,
            current_file,
            tape_read_rate: None,
            tape_write_rate: last_tape_rate,
            activity: store::SingleTapeLiveActivity::Streaming,
            engine_state: "writing_native_volume",
            progress_basis: "native_archive_bytes_over_staged_bytes",
            progress_scope: 3,
            progress_idle_ms: 0,
            no_progress_timeout_ms: duration_millis_u64(no_progress_timeout),
            spool: None,
        },
    )
    .await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn verify_physical_volume(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedSingleTapeDataRun,
    device: &Path,
    expected_header: &NativeVolumeHeader,
    header_bytes: &[u8],
    archive: &StagedDarArchive,
    source_manifest: &ManifestArtifactSummary,
    slices: &[StagedSlice],
    expected_index: &NativeVolumeIndex,
    index_bytes: &[u8],
    expected_trailer: &NativeVolumeTrailer,
    trailer_bytes: &[u8],
    files_total: u64,
    no_progress_timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<(), ExerciseFailure> {
    renew_claim(pool, claim, "before native physical-media readback").await?;
    let device = device.to_owned();
    let expected_catalog = archive.catalog_sha256.clone();
    let catalog_bytes = archive.catalog_bytes;
    let expected_manifest = source_manifest.artifact_sha256.clone();
    let manifest_bytes = source_manifest.size_bytes;
    let expected_slices = slices.to_vec();
    let header_expected = expected_header.clone();
    let index_expected = expected_index.clone();
    let trailer_expected = expected_trailer.clone();
    let header_digest = sha256_bytes(header_bytes);
    let index_digest = sha256_bytes(index_bytes);
    let trailer_digest = sha256_bytes(trailer_bytes);
    let fixed_readback_bytes = u64::try_from(header_bytes.len())
        .ok()
        .and_then(|value| value.checked_add(catalog_bytes))
        .and_then(|value| value.checked_add(manifest_bytes))
        .and_then(|value| value.checked_add(u64::try_from(index_bytes.len()).ok()?))
        .and_then(|value| value.checked_add(u64::try_from(trailer_bytes.len()).ok()?))
        .ok_or_else(|| ExerciseFailure::failed("native readback byte total overflowed"))?;
    let total_readback_bytes = expected_slices
        .iter()
        .try_fold(fixed_readback_bytes, |total, slice| {
            total.checked_add(slice.encoded_bytes)
        })
        .ok_or_else(|| ExerciseFailure::failed("native readback byte total overflowed"))?;
    let read_progress = Arc::new(AtomicU64::new(0));
    let task_progress = Arc::clone(&read_progress);
    let current_file = Arc::new(Mutex::new(Some("TapeScale volume header".to_owned())));
    let task_current_file = Arc::clone(&current_file);
    let mut readback = tokio::task::spawn_blocking(move || -> Result<(), NativeArchiveError> {
        let set_current = |name: String| {
            if let Ok(mut current) = task_current_file.lock() {
                *current = Some(name);
            }
        };
        let mut tape = open_tape_reader(&device)?;
        let physical_header = read_tape_file_with_progress(
            &mut tape,
            NATIVE_METADATA_READ_LIMIT,
            task_progress.as_ref(),
        )?;
        if sha256_bytes(&physical_header) != header_digest
            || NativeVolumeHeader::decode(&physical_header)
                .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?
                != header_expected
        {
            return Err(NativeArchiveError::Invalid(
                "native volume header readback mismatch".to_owned(),
            ));
        }
        set_current("catalog.1.dar".to_owned());
        if read_and_hash_tape_file_with_progress(&mut tape, catalog_bytes, task_progress.as_ref())?
            != expected_catalog
        {
            return Err(NativeArchiveError::Invalid(
                "isolated DAR catalog readback mismatch".to_owned(),
            ));
        }
        set_current("source-manifest.json".to_owned());
        if read_and_hash_tape_file_with_progress(&mut tape, manifest_bytes, task_progress.as_ref())?
            != expected_manifest
        {
            return Err(NativeArchiveError::Invalid(
                "source manifest readback mismatch".to_owned(),
            ));
        }
        for slice in expected_slices {
            set_current(
                slice
                    .path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("DAR archive slice")
                    .to_owned(),
            );
            if read_and_hash_tape_file_with_progress(
                &mut tape,
                slice.encoded_bytes,
                task_progress.as_ref(),
            )? != slice.sha256
            {
                return Err(NativeArchiveError::Invalid(format!(
                    "DAR slice {} physical readback mismatch",
                    slice.ordinal
                )));
            }
        }
        set_current("TapeScale volume index".to_owned());
        let physical_index = read_tape_file_with_progress(
            &mut tape,
            NATIVE_METADATA_READ_LIMIT,
            task_progress.as_ref(),
        )?;
        if sha256_bytes(&physical_index) != index_digest
            || NativeVolumeIndex::decode(&physical_index)
                .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?
                != index_expected
        {
            return Err(NativeArchiveError::Invalid(
                "native volume index readback mismatch".to_owned(),
            ));
        }
        set_current("TapeScale volume trailer".to_owned());
        let physical_trailer = read_tape_file_with_progress(
            &mut tape,
            NATIVE_METADATA_READ_LIMIT,
            task_progress.as_ref(),
        )?;
        let decoded_trailer = NativeVolumeTrailer::decode(&physical_trailer)
            .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?;
        if sha256_bytes(&physical_trailer) != trailer_digest || decoded_trailer != trailer_expected
        {
            return Err(NativeArchiveError::Invalid(
                "native volume trailer readback mismatch".to_owned(),
            ));
        }
        decoded_trailer
            .verify_index(&index_expected)
            .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?;
        Ok(())
    });
    let mut tick = tokio::time::interval(LIVE_SAMPLE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut previous_counter: Option<(u64, Instant)> = None;
    let mut previous_progress: Option<(u64, Instant)> = None;
    let mut last_progress = 0_u64;
    let mut last_movement = Instant::now();
    let mut last_pipeline_rate = None;
    let mut last_tape_rate = None;
    let mut cancellation_seen = false;
    let mut deferred_failure = None;
    let mut stalled_failure = None;
    loop {
        tokio::select! {
            result = &mut readback => {
                result
                    .map_err(|error| ExerciseFailure::failed(format!("native tape reader failed: {error}")))?
                    .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
                break;
            }
            () = cancellation.cancelled(), if !cancellation_seen => {
                cancellation_seen = true;
            }
            _ = tick.tick() => {
                if deferred_failure.is_some() {
                    continue;
                }
                if let Err(error) = renew_claim(pool, claim, "during native physical-media readback").await {
                    // As with writes, a blocking tape read cannot be cancelled by dropping its
                    // JoinHandle. Wait until the device file closes before cleanup can move tape.
                    deferred_failure = Some(error);
                    continue;
                }
                let counter = read_sas_tape_io_counters(sas_config, Path::new(&claim.run.expected_path))
                    .ok()
                    .and_then(|value| value.read_bytes);
                let now = Instant::now();
                let tape_rate = previous_counter.and_then(|(before, observed)| counter.and_then(|after| bytes_per_second(before, after, now.saturating_duration_since(observed))));
                let counter_advanced = previous_counter
                    .zip(counter)
                    .is_some_and(|((before, _), after)| after > before);
                if let Some(counter) = counter { previous_counter = Some((counter, now)); }
                let read = read_progress.load(Ordering::Acquire).min(total_readback_bytes);
                let pipeline_rate = previous_progress.and_then(|(before, observed)| bytes_per_second(before, read, now.saturating_duration_since(observed)));
                previous_progress = Some((read, now));
                if read > last_progress || counter_advanced {
                    last_progress = read;
                    last_movement = now;
                }
                let idle = now.saturating_duration_since(last_movement);
                if idle >= no_progress_timeout && stalled_failure.is_none() {
                    stalled_failure = Some(ExerciseFailure::failed(
                        "native physical readback made no progress before the configured transfer-stall deadline; the volume read was allowed to close safely",
                    ));
                }
                last_pipeline_rate = pipeline_rate;
                last_tape_rate = tape_rate.or(pipeline_rate);
                publish_native_sample(pool, sas_config, claim, NativeSample {
                    files_processed: 0,
                    files_total,
                    pipeline_bytes: read,
                    pipeline_total: Some(total_readback_bytes),
                    pipeline_rate,
                    current_file: current_file.lock().ok().and_then(|value| value.clone()),
                    tape_read_rate: last_tape_rate,
                    tape_write_rate: None,
                    activity: if idle >= no_progress_timeout {
                        store::SingleTapeLiveActivity::Stalled
                    } else {
                        store::SingleTapeLiveActivity::Reading
                    },
                    engine_state: "reading_native_volume",
                    progress_basis: "native_archive_bytes_over_staged_bytes",
                    progress_scope: 10_000 + u64::from(expected_header.volume_ordinal),
                    progress_idle_ms: duration_millis_u64(idle),
                    no_progress_timeout_ms: duration_millis_u64(no_progress_timeout),
                    spool: None,
                }).await;
            }
        }
    }
    if let Some(error) = deferred_failure {
        return Err(error);
    }
    if let Some(error) = stalled_failure {
        return Err(error);
    }
    if cancellation_seen {
        return Err(ExerciseFailure::interrupted(
            "worker shutdown was honored after completing native volume readback",
        ));
    }
    publish_native_sample(
        pool,
        sas_config,
        claim,
        NativeSample {
            files_processed: 0,
            files_total,
            pipeline_bytes: total_readback_bytes,
            pipeline_total: Some(total_readback_bytes),
            pipeline_rate: last_pipeline_rate,
            current_file: current_file.lock().ok().and_then(|value| value.clone()),
            tape_read_rate: last_tape_rate,
            tape_write_rate: None,
            activity: store::SingleTapeLiveActivity::Reading,
            engine_state: "reading_native_volume",
            progress_basis: "native_archive_bytes_over_staged_bytes",
            progress_scope: 10_000 + u64::from(expected_header.volume_ordinal),
            progress_idle_ms: 0,
            no_progress_timeout_ms: duration_millis_u64(no_progress_timeout),
            spool: None,
        },
    )
    .await;
    Ok(())
}

fn native_engine_write_json(
    claim: &store::ClaimedSingleTapeDataRun,
    archive: &StagedDarArchive,
    volumes: &[NativeVolumeEvidence],
) -> Value {
    json!({
        "finalized": true,
        "engine": "TapeScale Native",
        "engine_version": "1",
        "format_version": NATIVE_FORMAT_NAME,
        "archive_id": claim.run.archive_id,
        "archive_codec": "dar",
        "dar_version": crate::native_archive::DAR_CODEC_VERSION,
        "encoded_bytes": archive.encoded_bytes,
        "catalog_sha256": archive.catalog_sha256,
        "slice_count": archive.slices.len(),
        "compression": claim.run.resolved_compression,
        "volumes": volumes.iter().map(|volume| json!({
            "volume_order": volume.volume_order,
            "cartridge_id": volume.cartridge_id,
            "archive_volume_id": volume.archive_volume_id,
            "volume_name": volume.volume_name,
            "header_sha256": volume.header_sha256,
            "index_sha256": volume.index_sha256,
            "tape_file_count": volume.tape_file_count,
            "encoded_bytes": volume.encoded_bytes,
            "first_record": volume.first_record,
            "last_record": volume.last_record,
            "engine_record_id": volume.engine_record_id,
            "segments": volume.segment_files
        })).collect::<Vec<_>>()
    })
}

async fn finalize_native_backup(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    evidence_directory: &Path,
    completed: CompletedNativeBackup,
    cleanup: Value,
) -> Result<()> {
    let first = completed
        .volumes
        .first()
        .ok_or_else(|| anyhow::anyhow!("native archive completed without a volume"))?;
    let last = completed
        .volumes
        .last()
        .expect("first volume established a non-empty list");
    let report = json!({
        "scope": "attended full-file backup with complete per-volume physical readback",
        "engine": {"name": "TapeScale Native", "version": "1"},
        "format_version": NATIVE_FORMAT_NAME,
        "archive_id": claim.run.archive_id,
        "archive_codec": "dar",
        "dar_version": crate::native_archive::DAR_CODEC_VERSION,
        "compression": completed.compression,
        "slice_count": completed.archive.slices.len(),
        "encoded_bytes": completed.archive.encoded_bytes,
        "source_manifest_digest": completed.source_manifest.manifest_digest,
        "volumes": completed.volumes.iter().map(|volume| json!({
            "volume_order": volume.volume_order,
            "cartridge_id": volume.cartridge_id,
            "archive_volume_id": volume.archive_volume_id,
            "volume_name": volume.volume_name,
            "header_sha256": volume.header_sha256,
            "index_sha256": volume.index_sha256,
            "tape_file_count": volume.tape_file_count,
            "encoded_bytes": volume.encoded_bytes,
            "first_record": volume.first_record,
            "last_record": volume.last_record,
            "engine_record_id": volume.engine_record_id,
            "physical_media_readback": true
        })).collect::<Vec<_>>(),
        "cleanup": cleanup,
        "restore_test": "not_run_separate_rehearsal_required"
    });
    let report_artifact = persist_json_artifact(evidence_directory, "evidence-report", &report)?;
    append_json_artifact(
        pool,
        claim,
        "evidence_report",
        &report_artifact,
        json!({
            "candidate_evidence": true,
            "pending_atomic_finalization": true,
            "archive_codec": "dar"
        }),
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.detail))?;
    let pass = store::SingleTapePassEvidence {
        engine_operation_id: format!("tapescale-archive:{}", claim.run.archive_id),
        engine_volume_name: first.volume_name.clone(),
        engine_record_id: first.engine_record_id.clone(),
        first_record: first.first_record.clone(),
        last_record: Some(last.last_record.clone()),
        logical_bytes: completed.source_manifest.logical_bytes,
        encoded_bytes: completed.archive.encoded_bytes,
        file_count: completed.source_manifest.file_count,
        coverage: serde_json::to_value(&completed.source_manifest.coverage)?,
        full_readback_evidence: json!({
            "physical_media_readback": true,
            "archive_id": claim.run.archive_id,
            "archive_codec": "dar",
            "volume_count": completed.volumes.len(),
            "volumes": completed.volumes.iter().map(|volume| json!({
                "volume_order": volume.volume_order,
                "archive_volume_id": volume.archive_volume_id,
                "header_sha256": volume.header_sha256,
                "index_sha256": volume.index_sha256
            })).collect::<Vec<_>>()
        }),
        restore_test_evidence: json!({"outcome": "not_run", "independent_claim": true}),
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
                "TapeScale Native backup passed physical readback and created a recovery point"
            );
            Ok(())
        }
        store::FinalizeSingleTapeDataResult::Rejected(reason) => {
            finish_or_quarantine(
                pool,
                claim,
                store::SingleTapeDataTerminalState::Failed,
                json!({
                    "detail": format!("final native backup evidence was rejected: {reason}"),
                    "media_write_started": true,
                    "position_uncertain": false,
                    "verified_recovery_point_created": false
                }),
            )
            .await
        }
        store::FinalizeSingleTapeDataResult::FencedOut => {
            let detail =
                "native backup finalization was fenced out after verified physical media work";
            let _ = store::quarantine_fenced_single_tape_write(pool, claim, detail).await;
            Err(anyhow::anyhow!(detail))
        }
    }
}
