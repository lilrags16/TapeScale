//! Bounded-spool TapeScale Native v2 writer.
//!
//! Native v1 deliberately staged one complete DAR archive before touching
//! tape.  That is a useful qualification shape, but it cannot represent a
//! 30 TiB archive with a 1 TiB spool.  Native v2 instead creates independently
//! testable DAR packs. On a rotational spool, pack production and tape drain
//! run as exclusive batches so competing seeks cannot starve the tape drive.
//! Disk usage is bounded at the configured high-water mark, and every handoff
//! occurs at a complete tape-file boundary.

use std::{
    collections::VecDeque,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    sync::mpsc,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::*;
use crate::{
    data_path::load_manifest,
    native_archive::{
        conservative_usable_tape_bytes, dar_pack_content_sha256, lto_native_capacity_bytes,
        open_tape_reader, read_and_hash_bounded_tape_file_with_progress,
        read_and_hash_tape_file_with_progress, read_tape_file_with_progress, sha256_bytes,
        write_dar_pack_inclusion_list, write_tape_bytes, write_tape_path, DarCompression,
        DarPackCreateInvocation, DarPackInput, DarPackSizing, DarTestInvocation,
        NativeArchiveError, PlannedDarPack, StagedDarPack, StagedDarPackDiscoveryProgress,
        StagedSlice, MAX_NATIVE_PACK_TARGET_BYTES, MIN_NATIVE_PACK_TARGET_BYTES,
        NATIVE_ROLLING_FORMAT_NAME,
    },
    native_format::{
        ordered_sha256_digest, NativeArchiveRepresentation, NativeArchiveSetSeal,
        NativePackDescriptor, NativePackKind, NativeSealedVolume, NativeV2VolumeHeader,
        NativeV2VolumeIndex, NativeV2VolumeTrailer, DEFAULT_TAPE_BLOCK_BYTES,
        NATIVE_ROLLING_FORMAT_VERSION,
    },
    native_tape::rolling_archive_volume_identity,
};

const LIVE_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const COMMAND_OUTPUT_LIMIT: usize = 64 * 1024;
const NATIVE_METADATA_READ_LIMIT: u64 = 256 * 1024 * 1024;
const VOLUME_METADATA_RESERVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_LIVE_CURRENT_FILE_BYTES: usize = 4 * 1024;

#[derive(Debug)]
struct CompletedNativeV2Backup {
    source_manifest: ManifestArtifactSummary,
    compression: DarCompression,
    volumes: Vec<CompletedVolume>,
    seal: NativeArchiveSetSeal,
    total_encoded_bytes: u64,
    pack_count: u32,
}

#[derive(Clone, Debug)]
struct CompletedVolume {
    volume_order: i32,
    cartridge_id: Uuid,
    archive_volume_id: Uuid,
    previous_archive_volume_id: Option<Uuid>,
    previous_index_sha256: Option<String>,
    volume_name: String,
    header_sha256: String,
    index_sha256: String,
    tape_file_count: i32,
    encoded_bytes: u64,
    first_record: String,
    last_record: String,
    engine_record_id: String,
    first_pack_ordinal: u32,
    pack_count: u32,
    logical_bytes: u64,
    entry_count: u64,
    pack_files: Vec<(i32, i32)>,
    archive_final: bool,
    archive_seal_sha256: Option<String>,
}

#[derive(Clone, Debug)]
struct StagedPack {
    plan: PlannedDarPack,
    pack_id: Uuid,
    directory: PathBuf,
    archive: StagedDarPack,
}

#[derive(Clone, Debug)]
struct PhysicalSlice {
    ordinal: u32,
    encoded_bytes: u64,
    sha256: String,
    display_name: String,
}

#[derive(Clone, Debug)]
struct WrittenPack {
    descriptor: NativePackDescriptor,
    descriptor_bytes: Vec<u8>,
    catalog_bytes: u64,
    catalog_sha256: String,
    slices: Vec<PhysicalSlice>,
}

#[derive(Debug)]
struct OpenVolume {
    record: store::DataRunVolumeRecord,
    capacity_bytes: u64,
    archive_volume_id: Uuid,
    previous_archive_volume_id: Option<Uuid>,
    previous_index_sha256: Option<String>,
    volume_name: String,
    header: NativeV2VolumeHeader,
    header_bytes: Vec<u8>,
    packs: Vec<WrittenPack>,
    next_tape_file_number: u32,
    physical_pack_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct SpoolWindow {
    capacity_bytes: u64,
    low_water_bytes: u64,
    ready_bytes: u64,
    high_water_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct ProducerSummary {
    pack_count: u64,
    encoded_bytes: u64,
}

/// Owns the asynchronous DAR producer. Dropping this guard on any consumer
/// error stops production immediately so a failed tape path cannot continue
/// filling the spool in a detached task.
struct NativePackProducer {
    receiver: mpsc::Receiver<StagedPack>,
    permits: mpsc::Sender<()>,
    task: Option<JoinHandle<Result<ProducerSummary, ExerciseFailure>>>,
    cancellation: CancellationToken,
}

#[derive(Debug, Default)]
struct ReadyNativePacks {
    packs: VecDeque<(StagedPack, u64)>,
    physical_bytes: u64,
}

/// `spawn_blocking` tasks cannot be force-aborted once hashing has started.
/// Make dropping their async supervisor a cooperative cancellation signal so
/// a lease failure or outer task abort cannot leave a detached spool reader.
struct BlockingDiscoveryCancellation {
    cancelled: Arc<AtomicBool>,
}

impl Drop for BlockingDiscoveryCancellation {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl ReadyNativePacks {
    fn push(&mut self, pack: StagedPack) -> Result<(), ExerciseFailure> {
        let physical_bytes = staged_pack_physical_bytes(&pack)?;
        self.physical_bytes = self
            .physical_bytes
            .checked_add(physical_bytes)
            .ok_or_else(|| ExerciseFailure::failed("Native v2 ready-queue size overflowed"))?;
        self.packs.push_back((pack, physical_bytes));
        Ok(())
    }

    fn pop(&mut self) -> Result<Option<StagedPack>, ExerciseFailure> {
        let Some((pack, physical_bytes)) = self.packs.pop_front() else {
            return Ok(None);
        };
        self.physical_bytes = self
            .physical_bytes
            .checked_sub(physical_bytes)
            .ok_or_else(|| ExerciseFailure::failed("Native v2 ready-queue size underflowed"))?;
        Ok(Some(pack))
    }
}

impl NativePackProducer {
    async fn finish(&mut self) -> Result<ProducerSummary, ExerciseFailure> {
        let task = self.task.take().ok_or_else(|| {
            ExerciseFailure::interrupted("the Native v2 pack producer was already joined")
        })?;
        task.await.map_err(|error| {
            ExerciseFailure::interrupted(format!(
                "the Native v2 pack producer task stopped unexpectedly: {error}"
            ))
        })?
    }
}

impl Drop for NativePackProducer {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn ready_queue_resume_bytes(
    spool: SpoolWindow,
    sizing: DarPackSizing,
) -> Result<u64, ExerciseFailure> {
    let resume = spool.ready_bytes;
    if resume < sizing.target_bytes
        || resume <= spool.low_water_bytes
        || resume
            .checked_add(sizing.maximum_bytes)
            .is_none_or(|required| required > spool.high_water_bytes)
    {
        return Err(ExerciseFailure::blocked(
            "Native v2 spool settings cannot hold the ready reserve plus one maximum pack",
        ));
    }
    Ok(resume)
}

const fn ready_queue_needs_refill(
    ready_bytes: u64,
    low_water_bytes: u64,
    producer_complete: bool,
) -> bool {
    !producer_complete && ready_bytes < low_water_bytes
}

async fn refill_ready_queue(
    producer: &mut NativePackProducer,
    ready: &mut ReadyNativePacks,
    resume_bytes: u64,
    producer_summary: &mut Option<ProducerSummary>,
) -> Result<(), ExerciseFailure> {
    while ready.physical_bytes < resume_bytes && producer_summary.is_none() {
        // Production is explicitly batch-gated. A single rotational spool
        // never builds a new DAR pack while tape is draining an earlier pack;
        // that seek fight was slower than either operation and made the drive
        // shoe-shine. Each permit produces exactly one complete tested pack.
        let _ = producer.permits.send(()).await;
        match producer.receiver.recv().await {
            Some(pack) => ready.push(pack)?,
            None => *producer_summary = Some(producer.finish().await?),
        }
    }
    Ok(())
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
    // The writer performs several long CPU- and disk-bound phases before the
    // first tape command.  Keep the fenced hardware assignment alive from a
    // run-level heartbeat rather than relying on each individual phase to do
    // so.  A new phase can therefore never recreate the lease-expiry gap that
    // previously existed while a large staged pack was being hashed.
    let operation_cancellation = cancellation.child_token();
    let operation = {
        let operation = perform_native_v2_backup(
            pool,
            sas_config,
            config,
            claim,
            &operation_cancellation,
            &evidence_directory,
            &stage_directory,
            &mut tape_touched,
            &mut media_write_started,
        );
        tokio::pin!(operation);
        let mut lease_heartbeat = tokio::time::interval(LIVE_SAMPLE_INTERVAL);
        lease_heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                result = &mut operation => break result,
                _ = lease_heartbeat.tick() => {
                    if let Err(error) = renew_claim(
                        pool,
                        claim,
                        "during Native v2 archive preparation and streaming",
                    ).await {
                        operation_cancellation.cancel();
                        break Err(error);
                    }
                }
            }
        }
    };
    operation_cancellation.cancel();
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
            let result = finalize_native_v2_backup(
                pool,
                claim,
                &evidence_directory,
                completed,
                cleanup.1,
            )
            .await;
            if result.is_ok() {
                if let Err(error) = fs::remove_dir_all(&stage_directory) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        warn!(path = %stage_directory.display(), %error, "could not remove completed Native v2 spool");
                    }
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
                    "detail": "Native v2 archive passed media readback, but the drive could not be proven ready at BOT",
                    "position_uncertain": true,
                    "media_write_started": media_write_started,
                    "cleanup": cleanup.1,
                    "spool_retained": stage_directory
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
            let failure_code = error.code;
            let result = json!({
                "detail": error.detail,
                "failure_code": failure_code,
                "position_uncertain": tape_touched && !cleanup.0,
                "media_write_started": media_write_started,
                "cleanup": cleanup.1,
                "archive_codec": "dar",
                "format_version": NATIVE_ROLLING_FORMAT_NAME,
                "spool_retained": tape_touched.then_some(stage_directory.clone())
            });
            if media_write_started {
                finish_or_quarantine(pool, claim, terminal, result).await
            } else {
                finish(pool, claim, terminal, result).await?;
                if let Err(cleanup_error) = fs::remove_dir_all(&stage_directory) {
                    if cleanup_error.kind() != std::io::ErrorKind::NotFound {
                        warn!(path = %stage_directory.display(), %cleanup_error, "could not remove unused Native v2 spool");
                    }
                }
                Ok(())
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn produce_native_v2_packs(
    pool: PgPool,
    sas_config: SasDiscoveryConfig,
    config: Config,
    claim: store::ClaimedSingleTapeDataRun,
    cancellation: CancellationToken,
    stage_directory: PathBuf,
    plans: Vec<PlannedDarPack>,
    sizing: DarPackSizing,
    compression: DarCompression,
    spool: SpoolWindow,
    mut permits: mpsc::Receiver<()>,
    sender: mpsc::Sender<StagedPack>,
) -> Result<ProducerSummary, ExerciseFailure> {
    let pack_total = u64::try_from(plans.len())
        .map_err(|_| ExerciseFailure::failed("too many Native v2 packs"))?;
    let mut manifest_entry_start = 1_u64;
    let mut produced_encoded_bytes = 0_u64;

    for plan in &plans {
        tokio::select! {
            permit = permits.recv() => {
                if permit.is_none() {
                    return Err(ExerciseFailure::interrupted(
                        "the Native v2 tape consumer stopped requesting pack batches",
                    ));
                }
            }
            _ = cancellation.cancelled() => {
                return Err(ExerciseFailure::cancelled(
                    "Native v2 pack production was cancelled while parked at a safe pack boundary",
                ));
            }
        }
        // Reserve room for the worst permitted in-progress pack before DAR is
        // launched. The consumer reclaims complete packs after each durable
        // tape-file close; the next permitted batch begins while tape remains
        // parked at that safe filemark boundary.
        loop {
            let usage = spool_usage(&stage_directory)?;
            if usage
                .0
                .checked_add(sizing.maximum_bytes)
                .is_some_and(|reserved| reserved <= spool.high_water_bytes)
            {
                break;
            }
            tokio::select! {
                _ = cancellation.cancelled() => {
                    return Err(ExerciseFailure::cancelled(
                        "Native v2 pack production was cancelled while waiting for spool capacity",
                    ));
                }
                _ = sender.closed() => {
                    return Err(ExerciseFailure::interrupted(
                        "the Native v2 tape consumer stopped before pack production completed",
                    ));
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }

        let staged = stage_one_pack(
            &pool,
            &sas_config,
            &config,
            &claim,
            &cancellation,
            &stage_directory,
            plan,
            sizing,
            compression,
            spool,
            manifest_entry_start,
            pack_total,
        )
        .await?;
        let staged_physical_bytes = staged_pack_physical_bytes(&staged)?;
        if staged_physical_bytes > sizing.maximum_bytes {
            return Err(ExerciseFailure::blocked(format!(
                "Native v2 pack {} produced {} physical bytes, exceeding the bounded pack maximum of {}; increase the pack target and spool window or reduce the selected allocation group",
                plan.pack_ordinal, staged_physical_bytes, sizing.maximum_bytes
            )));
        }
        produced_encoded_bytes = produced_encoded_bytes
            .checked_add(staged.archive.encoded_bytes)
            .ok_or_else(|| ExerciseFailure::failed("Native v2 encoded byte total overflowed"))?;
        manifest_entry_start = manifest_entry_start
            .checked_add(plan.source_entry_count)
            .ok_or_else(|| ExerciseFailure::failed("Native v2 entry position overflowed"))?;
        sender.send(staged).await.map_err(|_| {
            ExerciseFailure::interrupted(
                "the Native v2 tape consumer stopped before receiving a completed pack",
            )
        })?;
    }

    let ledger = store::native_v2_pack_ledger(&pool, &claim)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?;
    let producer_root = ordered_sha256(ledger.iter().map(|pack| pack.content_sha256.as_str()))?;
    if !store::mark_native_v2_producer_complete(
        &pool,
        &claim,
        pack_total,
        produced_encoded_bytes,
        &producer_root,
    )
    .await
    .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the fenced run rejected Native v2 producer completion",
        ));
    }

    Ok(ProducerSummary {
        pack_count: pack_total,
        encoded_bytes: produced_encoded_bytes,
    })
}

#[allow(clippy::too_many_arguments)]
async fn perform_native_v2_backup(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    evidence_directory: &Path,
    stage_directory: &Path,
    tape_touched: &mut bool,
    media_write_started: &mut bool,
) -> Result<CompletedNativeV2Backup, ExerciseFailure> {
    if claim.run.workflow_kind != "backup" || claim.run.format_version != NATIVE_ROLLING_FORMAT_NAME
    {
        return Err(ExerciseFailure::blocked(
            "the Native v2 writer accepts only v2 operational backup runs",
        ));
    }
    validate_paths(config, claim)?;
    verify_read_only_mount(Path::new(&claim.run.source_mount_path)).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "the worker could not prove the selected source is mounted read-only: {error}"
        ))
    })?;
    prepare_spool_root(config, stage_directory)?;

    transition(
        pool,
        claim,
        store::SingleTapeDataPhase::Inventorying,
        json!({
            "detail": "hashing the immutable snapshot and planning bounded Native v2 packs",
            "archive_codec": "dar",
            "format_version": NATIVE_ROLLING_FORMAT_NAME,
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
    let manifest = load_manifest(&source_manifest.storage_path).map_err(|error| {
        ExerciseFailure::failed(format!("could not reopen source manifest: {error}"))
    })?;
    if manifest.digest_sha256 != source_manifest.manifest_digest
        || manifest.entry_count != source_manifest.entry_count
        || manifest.logical_bytes != source_manifest.logical_bytes
    {
        return Err(ExerciseFailure::failed(
            "the authenticated source manifest changed before pack planning",
        ));
    }

    let spool = configured_spool_window(config)?;
    let sizing = effective_pack_sizing(config, claim, &source_manifest)?;
    if spool.high_water_bytes
        < spool
            .ready_bytes
            .checked_add(sizing.maximum_bytes)
            .ok_or_else(|| ExerciseFailure::blocked("Native v2 pack window overflowed"))?
    {
        return Err(ExerciseFailure::blocked(format!(
            "the configured Native v2 spool high-water mark ({}) cannot hold the ready reserve plus one maximum in-progress pack ({})",
            spool.high_water_bytes,
            spool.ready_bytes.saturating_add(sizing.maximum_bytes)
        )));
    }
    let inputs = manifest
        .entries
        .iter()
        .map(|entry| DarPackInput {
            path: entry.path.clone(),
            logical_bytes: entry.size.unwrap_or(0),
            allocation_group: entry.hard_link_group.clone(),
        })
        .collect::<Vec<_>>();
    let plans = crate::native_archive::plan_dar_packs(&inputs, sizing)
        .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;
    let estimated_archive_bytes = plans
        .iter()
        .try_fold(0_u64, |total, pack| total.checked_add(pack.estimated_bytes))
        .ok_or_else(|| ExerciseFailure::blocked("Native v2 planned archive size overflowed"))?;
    let initial_capacity = volume_payload_capacity(
        claim.run.expected_media_generation,
        claim.run.expected_media_format.as_deref(),
        source_manifest.size_bytes,
    )?;
    if !claim.run.spanning_enabled && estimated_archive_bytes > initial_capacity {
        return Err(ExerciseFailure::blocked(format!(
            "the planned archive is approximately {estimated_archive_bytes} bytes, exceeding the conservative single-tape payload capacity of {initial_capacity} bytes; enable multi-tape archives before writing"
        )));
    }

    transition(
        pool,
        claim,
        store::SingleTapeDataPhase::Writing,
        json!({
            "detail": "building independently testable DAR packs in bounded disk batches, then streaming each ready batch to tape without competing spool seeks",
            "archive_id": claim.run.archive_id,
            "archive_codec": "dar",
            "format_version": NATIVE_ROLLING_FORMAT_NAME,
            "source_manifest_sha256": source_manifest.artifact_sha256,
            "source_entries": source_manifest.entry_count,
            "logical_bytes": source_manifest.logical_bytes,
            "planned_pack_count": plans.len(),
            "pack_target_bytes": sizing.target_bytes,
            "pack_maximum_bytes": sizing.maximum_bytes,
            "spool_capacity_bytes": spool.capacity_bytes,
            "spool_low_water_bytes": spool.low_water_bytes,
            "spool_ready_bytes": spool.ready_bytes,
            "spool_high_water_bytes": spool.high_water_bytes,
            "shoe_shine_protection": "exclusive_rotational_spool_batches_at_complete_filemarks"
        }),
    )
    .await?;
    if !store::begin_native_v2_producer(pool, claim, spool.low_water_bytes, spool.high_water_bytes)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the fenced run rejected the Native v2 producer window",
        ));
    }

    let compression = DarCompression::from_resolved(&claim.run.resolved_compression);
    let pack_total = u64::try_from(plans.len())
        .map_err(|_| ExerciseFailure::failed("too many Native v2 packs"))?;
    let descriptor_capacity = plans.len();
    let (sender, receiver) = mpsc::channel(1);
    let (permit_sender, permit_receiver) = mpsc::channel(1);
    let producer_cancellation = cancellation.child_token();
    let producer_task = tokio::spawn(produce_native_v2_packs(
        pool.clone(),
        sas_config.clone(),
        config.clone(),
        claim.clone(),
        producer_cancellation.clone(),
        stage_directory.to_owned(),
        plans,
        sizing,
        compression,
        spool,
        permit_receiver,
        sender,
    ));
    let mut producer = NativePackProducer {
        receiver,
        permits: permit_sender,
        task: Some(producer_task),
        cancellation: producer_cancellation,
    };
    let mut completed_volumes = Vec::<CompletedVolume>::new();
    let mut current_volume: Option<OpenVolume> = None;
    let mut all_pack_descriptors = Vec::<NativePackDescriptor>::with_capacity(descriptor_capacity);
    let mut all_catalog_digests = Vec::<String>::with_capacity(descriptor_capacity);
    let mut produced_encoded_bytes = 0_u64;
    let resume_ready_bytes = ready_queue_resume_bytes(spool, sizing)?;
    let mut ready = ReadyNativePacks::default();
    let mut producer_summary = None;

    // Do not move the tape as soon as the first pack appears. Build a useful
    // ready reserve first, so ordinary producer jitter is absorbed before the
    // drive starts. A small archive may reach producer EOF below this target;
    // in that case all of its packs are ready before tape motion begins.
    refill_ready_queue(
        &mut producer,
        &mut ready,
        resume_ready_bytes,
        &mut producer_summary,
    )
    .await?;

    loop {
        let Some(staged) = ready.pop()? else {
            if producer_summary.is_some() {
                break;
            }
            refill_ready_queue(
                &mut producer,
                &mut ready,
                resume_ready_bytes,
                &mut producer_summary,
            )
            .await?;
            continue;
        };
        produced_encoded_bytes = produced_encoded_bytes
            .checked_add(staged.archive.encoded_bytes)
            .ok_or_else(|| ExerciseFailure::failed("Native v2 encoded byte total overflowed"))?;
        all_catalog_digests.push(staged.archive.catalog_sha256.clone());
        let pack_physical_bytes = staged_pack_physical_bytes(&staged)?;

        loop {
            if current_volume.is_none() {
                let order = i32::try_from(completed_volumes.len() + 1)
                    .map_err(|_| ExerciseFailure::failed("archive uses too many tapes"))?;
                let record = if order == 1 {
                    store::data_run_volumes(pool, claim.run.id)
                        .await
                        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
                        .into_iter()
                        .find(|volume| volume.volume_order == 1)
                        .ok_or_else(|| {
                            ExerciseFailure::interrupted(
                                "the first archive volume assignment disappeared",
                            )
                        })?
                } else {
                    native::request_next_write_volume(
                        pool,
                        sas_config,
                        config,
                        claim,
                        order,
                        cancellation,
                    )
                    .await?
                };
                revalidate_volume_before_write(
                    pool,
                    sas_config,
                    config,
                    claim,
                    &record,
                    cancellation,
                )
                .await?;
                current_volume = Some(
                    open_volume(
                        pool,
                        sas_config,
                        config,
                        claim,
                        cancellation,
                        &source_manifest,
                        record,
                        completed_volumes.last(),
                        tape_touched,
                        media_write_started,
                        spool,
                        pack_total,
                    )
                    .await?,
                );
            }

            let fits = current_volume.as_ref().is_some_and(|volume| {
                volume
                    .physical_pack_bytes
                    .saturating_add(pack_physical_bytes)
                    <= volume.capacity_bytes
            });
            if fits {
                break;
            }
            if current_volume
                .as_ref()
                .is_some_and(|volume| volume.packs.is_empty())
            {
                return Err(ExerciseFailure::blocked(format!(
                    "Native v2 pack {} needs {} physical bytes and cannot fit the conservative payload capacity of archive volume {}",
                    staged.plan.pack_ordinal,
                    pack_physical_bytes,
                    current_volume.as_ref().expect("volume exists").record.volume_order
                )));
            }
            if !claim.run.spanning_enabled {
                return Err(ExerciseFailure::blocked(
                    "the archive reached the end of its first conservative tape capacity, but multi-tape spanning is disabled",
                ));
            }
            let closed = close_and_verify_volume(
                pool,
                sas_config,
                config,
                claim,
                cancellation,
                &source_manifest,
                current_volume.take().expect("volume exists"),
                None,
                spool,
                pack_total,
            )
            .await?;
            completed_volumes.push(closed);
            execute_sas_motion(
                sas_config,
                Path::new(&claim.run.expected_path),
                SasMotionOperation::Unload,
                cancellation,
            )
            .await
            .map_err(|error| {
                ExerciseFailure::failed(format!(
                    "could not unload verified Native v2 volume for the next tape: {}",
                    sas_command_error_detail(&error)
                ))
            })?;
        }

        let descriptor = write_staged_pack(
            pool,
            sas_config,
            config,
            claim,
            cancellation,
            current_volume.as_mut().expect("volume exists"),
            &staged,
            spool,
            pack_total,
        )
        .await?;
        all_pack_descriptors.push(descriptor);
        reclaim_staged_pack(pool, claim, stage_directory, &staged).await?;

        // The pack just ended at a committed filemark. If the ready reserve is
        // below low-water, keep the drive parked at this safe boundary
        // until the producer restores the resume threshold (or reaches EOF).
        if ready_queue_needs_refill(
            ready.physical_bytes,
            spool.low_water_bytes,
            producer_summary.is_some(),
        ) {
            refill_ready_queue(
                &mut producer,
                &mut ready,
                resume_ready_bytes,
                &mut producer_summary,
            )
            .await?;
        }
    }

    let producer_summary = match producer_summary {
        Some(summary) => summary,
        None => producer.finish().await?,
    };
    if producer_summary.pack_count != pack_total
        || producer_summary.encoded_bytes != produced_encoded_bytes
        || all_pack_descriptors.len() != descriptor_capacity
        || all_catalog_digests.len() != descriptor_capacity
    {
        return Err(ExerciseFailure::failed(
            "Native v2 producer and tape-consumer totals did not reconcile",
        ));
    }
    let final_index = volume_index(
        current_volume.as_ref().expect("volume exists"),
        &source_manifest,
    )?;
    let mut sealed_volumes = completed_volumes
        .iter()
        .map(completed_to_sealed)
        .collect::<Vec<_>>();
    sealed_volumes.push(open_to_sealed(
        current_volume.as_ref().expect("volume exists"),
        &final_index,
    )?);
    let pack_catalog_root = ordered_sha256(all_catalog_digests.iter().map(String::as_str))?;
    let ordered_pack_root = ordered_sha256(
        all_pack_descriptors
            .iter()
            .map(|pack| pack.content_sha256.as_str()),
    )?;
    let seal = NativeArchiveSetSeal::new(
        claim.run.archive_id,
        source_manifest.artifact_sha256.clone(),
        pack_catalog_root,
        ordered_pack_root,
        all_pack_descriptors
            .iter()
            .map(|pack| pack.encoded_bytes)
            .sum(),
        all_pack_descriptors
            .iter()
            .map(|pack| pack.logical_bytes)
            .sum(),
        all_pack_descriptors
            .iter()
            .map(|pack| pack.entry_count)
            .sum(),
        sealed_volumes,
        Utc::now(),
    )
    .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let closed = close_and_verify_volume(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        &source_manifest,
        current_volume.take().expect("volume exists"),
        Some(&seal),
        spool,
        pack_total,
    )
    .await?;
    completed_volumes.push(closed);
    if !store::commit_native_v2_archive_seal(
        pool,
        claim,
        &store::NativeV2ArchiveSealEvidence {
            final_archive_volume_id: seal.final_archive_volume_id,
            source_manifest_sha256: seal.source_manifest_sha256.clone(),
            pack_catalog_root_sha256: seal.pack_catalog_root_sha256.clone(),
            volume_count: i32::try_from(seal.volume_count)
                .map_err(|_| ExerciseFailure::failed("too many sealed volumes"))?,
            pack_count: u64::from(seal.pack_count),
            total_encoded_bytes: seal.total_encoded_bytes,
            total_logical_bytes: seal.total_logical_bytes,
            total_entries: seal.total_entries,
            ordered_volume_index_sha256: seal.ordered_volume_index_sha256.clone(),
            ordered_pack_sha256: seal.ordered_pack_sha256.clone(),
            archive_set_sha256: seal.archive_set_sha256.clone(),
            evidence: serde_json::to_value(&seal)
                .map_err(|error| ExerciseFailure::failed(error.to_string()))?,
            completed_at: seal.completed_at,
        },
    )
    .await
    .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the fenced run rejected the verified Native v2 archive-set seal",
        ));
    }

    persist_engine_evidence(
        pool,
        claim,
        evidence_directory,
        &source_manifest,
        compression,
        &completed_volumes,
        &seal,
    )
    .await?;
    transition(
        pool,
        claim,
        store::SingleTapeDataPhase::Written,
        json!({
            "detail": "every Native v2 pack was written and every archive volume passed complete physical readback",
            "archive_id": claim.run.archive_id,
            "volume_count": completed_volumes.len(),
            "pack_count": seal.pack_count,
            "encoded_bytes": seal.total_encoded_bytes,
            "archive_set_sha256": seal.archive_set_sha256
        }),
    )
    .await?;
    transition(
        pool,
        claim,
        store::SingleTapeDataPhase::Verifying,
        json!({
            "detail": "physical readback and the archive-set seal passed; confirming the immutable source did not change",
            "physical_media_readback": true,
            "volume_count": completed_volumes.len(),
            "pack_count": seal.pack_count
        }),
    )
    .await?;
    verify_source_stability(
        pool,
        claim,
        cancellation,
        evidence_directory,
        &source_manifest,
    )
    .await?;
    Ok(CompletedNativeV2Backup {
        source_manifest,
        compression,
        volumes: completed_volumes,
        total_encoded_bytes: seal.total_encoded_bytes,
        pack_count: seal.pack_count,
        seal,
    })
}

fn prepare_spool_root(config: &Config, stage_directory: &Path) -> Result<(), ExerciseFailure> {
    fs::create_dir_all(&config.native_spool_dir).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "could not create Native v2 spool {}: {error}",
            config.native_spool_dir.display()
        ))
    })?;
    let root = fs::symlink_metadata(&config.native_spool_dir).map_err(|error| {
        ExerciseFailure::blocked(format!("could not inspect Native v2 spool: {error}"))
    })?;
    if !root.is_dir() || root.file_type().is_symlink() {
        return Err(ExerciseFailure::blocked(
            "the Native v2 spool must be a real directory",
        ));
    }
    fs::create_dir(stage_directory).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "could not create an empty run-scoped Native v2 spool: {error}"
        ))
    })?;
    Ok(())
}

fn configured_spool_window(config: &Config) -> Result<SpoolWindow, ExerciseFailure> {
    let status = statvfs(&config.native_spool_dir).map_err(|error| {
        ExerciseFailure::blocked(format!(
            "could not inspect Native v2 spool capacity at {}: {error}",
            config.native_spool_dir.display()
        ))
    })?;
    let available = status.f_bavail.saturating_mul(status.f_frsize);
    let required = config
        .native_spool_high_water_bytes
        .checked_add(config.native_spool_reserve_bytes)
        .ok_or_else(|| ExerciseFailure::blocked("Native v2 spool capacity overflowed"))?;
    if available < required {
        return Err(ExerciseFailure::blocked(format!(
            "the Native v2 rolling spool needs {} available bytes for its configured high-water mark plus reserve, but only {available} bytes are currently available",
            required
        )));
    }
    if config.native_spool_low_water_bytes == 0
        || config.native_spool_low_water_bytes >= config.native_spool_high_water_bytes
    {
        return Err(ExerciseFailure::blocked(
            "Native v2 spool watermarks are invalid",
        ));
    }
    Ok(SpoolWindow {
        capacity_bytes: available.saturating_sub(config.native_spool_reserve_bytes),
        low_water_bytes: config.native_spool_low_water_bytes,
        ready_bytes: config.native_spool_ready_bytes,
        high_water_bytes: config.native_spool_high_water_bytes,
    })
}

fn effective_pack_sizing(
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    manifest: &ManifestArtifactSummary,
) -> Result<DarPackSizing, ExerciseFailure> {
    let per_volume = volume_payload_capacity(
        claim.run.expected_media_generation,
        claim.run.expected_media_format.as_deref(),
        manifest.size_bytes,
    )?;
    let target = config.native_pack_target_bytes.min(per_volume);
    if target < MIN_NATIVE_PACK_TARGET_BYTES {
        return Err(ExerciseFailure::blocked(format!(
            "the selected tape leaves only {per_volume} bytes for Native v2 packs, below the minimum supported pack target of {MIN_NATIVE_PACK_TARGET_BYTES} bytes"
        )));
    }
    let maximum = config
        .native_pack_maximum_bytes
        .min(MAX_NATIVE_PACK_TARGET_BYTES)
        .min(per_volume);
    DarPackSizing::new(target, maximum).map_err(|error| ExerciseFailure::blocked(error.to_string()))
}

fn volume_payload_capacity(
    media_generation: Option<i16>,
    media_format: Option<&str>,
    source_manifest_bytes: u64,
) -> Result<u64, ExerciseFailure> {
    let native_capacity =
        lto_native_capacity_bytes(media_generation, media_format).ok_or_else(|| {
            ExerciseFailure::blocked(
                "the selected tape has no recognized uncompressed LTO capacity",
            )
        })?;
    let repeated_metadata = source_manifest_bytes
        .checked_add(VOLUME_METADATA_RESERVE_BYTES)
        .ok_or_else(|| ExerciseFailure::blocked("Native v2 metadata reserve overflowed"))?;
    conservative_usable_tape_bytes(native_capacity, repeated_metadata)
        .map_err(|error| ExerciseFailure::blocked(error.to_string()))
}

fn spool_usage(root: &Path) -> Result<(u64, u64), ExerciseFailure> {
    let mut bytes = 0_u64;
    let mut files = 0_u64;
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            // The tape consumer reclaims completed pack directories while the
            // producer samples aggregate usage. A queued child can therefore
            // disappear between discovery and traversal without indicating a
            // corrupt spool. The run-scoped root itself must always exist.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && directory != root => {
                continue;
            }
            Err(error) => {
                return Err(ExerciseFailure::failed(format!(
                    "could not inspect Native v2 spool {}: {error}",
                    directory.display()
                )));
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(ExerciseFailure::failed(error.to_string())),
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(ExerciseFailure::failed(error.to_string())),
            };
            if metadata.file_type().is_symlink() {
                return Err(ExerciseFailure::failed(format!(
                    "Native v2 spool contains a symbolic link: {}",
                    path.display()
                )));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                bytes = bytes.checked_add(metadata.len()).ok_or_else(|| {
                    ExerciseFailure::failed("Native v2 spool byte count overflowed")
                })?;
                files = files.checked_add(1).ok_or_else(|| {
                    ExerciseFailure::failed("Native v2 spool file count overflowed")
                })?;
            } else {
                return Err(ExerciseFailure::failed(format!(
                    "Native v2 spool contains an unsupported filesystem object: {}",
                    path.display()
                )));
            }
        }
    }
    Ok((bytes, files))
}

fn staged_pack_physical_bytes(pack: &StagedPack) -> Result<u64, ExerciseFailure> {
    let descriptor_estimate = 64 * 1024_u64;
    pack.archive
        .encoded_bytes
        .checked_add(pack.archive.catalog_bytes)
        .and_then(|value| value.checked_add(descriptor_estimate))
        .ok_or_else(|| ExerciseFailure::failed("Native v2 pack physical size overflowed"))
}

fn ordered_sha256<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> Result<String, ExerciseFailure> {
    ordered_sha256_digest(values).map_err(|error| ExerciseFailure::failed(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn discover_staged_pack_with_progress(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    stage_directory: &Path,
    pack_directory: &Path,
    inclusion_path: &Path,
    plan: &PlannedDarPack,
    compression: DarCompression,
    spool: SpoolWindow,
) -> Result<StagedDarPack, ExerciseFailure> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancellation_guard = BlockingDiscoveryCancellation {
        cancelled: Arc::clone(&cancelled),
    };
    let progress = Arc::new(StagedDarPackDiscoveryProgress::default());
    let worker_cancelled = Arc::clone(&cancelled);
    let worker_progress = Arc::clone(&progress);
    let worker_pack_directory = pack_directory.to_owned();
    let worker_inclusion_path = inclusion_path.to_owned();
    let pack_ordinal = plan.pack_ordinal;
    let mut discovery = tokio::task::spawn_blocking(move || {
        StagedDarPack::discover_with_cancel_and_progress(
            &worker_pack_directory,
            &worker_inclusion_path,
            pack_ordinal,
            compression,
            &worker_cancelled,
            &worker_progress,
        )
    });
    let mut tick = tokio::time::interval(LIVE_SAMPLE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    let mut previous = (0_u64, Instant::now());
    let mut last_movement = previous.1;

    loop {
        tokio::select! {
            result = &mut discovery => {
                return result
                    .map_err(|error| ExerciseFailure::failed(format!(
                        "Native v2 pack {} hash worker stopped unexpectedly: {error}",
                        plan.pack_ordinal,
                    )))?
                    .map_err(|error| match error {
                        NativeArchiveError::Cancelled if cancellation.is_cancelled() => {
                            ExerciseFailure::interrupted(format!(
                                "worker shutdown interrupted hashing Native v2 pack {}",
                                plan.pack_ordinal,
                            ))
                        }
                        other => ExerciseFailure::failed(format!(
                            "could not hash Native v2 pack {}: {other}",
                            plan.pack_ordinal,
                        )),
                    });
            }
            () = cancellation.cancelled() => {
                cancelled.store(true, Ordering::Release);
                let _ = discovery.await;
                return Err(ExerciseFailure::interrupted(format!(
                    "worker shutdown interrupted hashing Native v2 pack {}",
                    plan.pack_ordinal,
                )));
            }
            _ = tick.tick() => {
                // The run-level heartbeat is authoritative. This boundary
                // renewal also makes this long operation safe if it is ever
                // reused outside the current producer lifecycle.
                if let Err(error) = renew_claim(
                    pool,
                    claim,
                    "while hashing a completed Native v2 pack",
                ).await {
                    cancelled.store(true, Ordering::Release);
                    let _ = discovery.await;
                    return Err(error);
                }
                let snapshot = progress.snapshot();
                if snapshot.total_bytes == 0 || snapshot.files_total == 0 {
                    continue;
                }
                let now = Instant::now();
                if snapshot.bytes_hashed != previous.0 {
                    last_movement = now;
                }
                let rate = bytes_per_second(
                    previous.0,
                    snapshot.bytes_hashed,
                    now.saturating_duration_since(previous.1),
                );
                previous = (snapshot.bytes_hashed, now);
                let idle = now.saturating_duration_since(last_movement);
                if idle >= config.transfer_stall_timeout {
                    cancelled.store(true, Ordering::Release);
                    let _ = discovery.await;
                    return Err(ExerciseFailure::failed(format!(
                        "hashing Native v2 pack {} made no disk-read progress before the transfer-stall deadline",
                        plan.pack_ordinal,
                    )));
                }
                let current_file = snapshot.current_slice_ordinal.map(|ordinal| {
                    format!(
                        "pack-{:08}/archive.{ordinal:06}.dar",
                        plan.pack_ordinal,
                    )
                });
                let usage = match spool_usage(stage_directory) {
                    Ok(usage) => usage,
                    Err(error) => {
                        cancelled.store(true, Ordering::Release);
                        let _ = discovery.await;
                        return Err(error);
                    }
                };
                let mut sample = V2Sample::spool(
                    "hashing_pack",
                    store::SingleTapeLiveActivity::Buffering,
                    usage,
                    spool,
                    snapshot.files_hashed,
                    snapshot.files_total,
                    snapshot.bytes_hashed,
                    current_file,
                );
                sample.pipeline_total = Some(snapshot.total_bytes);
                sample.pipeline_rate = rate;
                sample.progress_scope = u64::from(plan.pack_ordinal)
                    .saturating_mul(1_000_000)
                    .saturating_add(2);
                sample.progress_idle_ms = duration_millis_u64(idle);
                sample.no_progress_timeout_ms = duration_millis_u64(config.transfer_stall_timeout);
                publish_v2_sample(pool, sas_config, claim, sample).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn stage_one_pack(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    stage_directory: &Path,
    plan: &PlannedDarPack,
    sizing: DarPackSizing,
    compression: DarCompression,
    spool: SpoolWindow,
    manifest_entry_start: u64,
    pack_total: u64,
) -> Result<StagedPack, ExerciseFailure> {
    let pack_directory = stage_directory.join(format!("pack-{:08}", plan.pack_ordinal));
    fs::create_dir(&pack_directory).map_err(|error| {
        ExerciseFailure::failed(format!(
            "could not create Native v2 pack {} spool directory: {error}",
            plan.pack_ordinal
        ))
    })?;
    let inclusion_path = pack_directory.join("include.paths");
    write_dar_pack_inclusion_list(&inclusion_path, &plan.entries)
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let invocation = DarPackCreateInvocation::new(
        &config.dar_path,
        Path::new(&claim.run.source_mount_path),
        &pack_directory,
        &inclusion_path,
        plan.pack_ordinal,
        config.native_slice_bytes,
        compression,
        config.native_compression_threads,
    )
    .map_err(|error| ExerciseFailure::blocked(error.to_string()))?;
    run_pack_command(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        &invocation.program,
        &invocation.args,
        stage_directory,
        spool,
        plan,
        pack_total,
        "packing_pack",
        Some(sizing.maximum_bytes),
    )
    .await?;
    let archive = discover_staged_pack_with_progress(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        stage_directory,
        &pack_directory,
        &inclusion_path,
        plan,
        compression,
        spool,
    )
    .await?;
    if archive.encoded_bytes > sizing.maximum_bytes {
        return Err(ExerciseFailure::blocked(format!(
            "Native v2 pack {} produced {} bytes, exceeding its hard {} byte pack boundary; the pack was stopped before tape motion",
            plan.pack_ordinal, archive.encoded_bytes, sizing.maximum_bytes
        )));
    }
    let test = DarTestInvocation::new(
        &config.dar_path,
        &archive.archive_base,
        config.native_compression_threads,
    );
    run_pack_command(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        &test.program,
        &test.args,
        stage_directory,
        spool,
        plan,
        pack_total,
        "validating_pack",
        None,
    )
    .await?;
    let usage = spool_usage(stage_directory)?;
    if usage.0 > spool.high_water_bytes {
        return Err(ExerciseFailure::failed(format!(
            "completed Native v2 pack {} exceeded the configured spool high-water boundary",
            plan.pack_ordinal
        )));
    }
    let pack_id = Uuid::now_v7();
    let storage_ref = pack_directory
        .to_str()
        .ok_or_else(|| ExerciseFailure::failed("Native v2 spool path is not valid UTF-8"))?
        .to_owned();
    let tape_file_count = i32::try_from(2_usize.saturating_add(archive.slices.len()))
        .map_err(|_| ExerciseFailure::failed("Native v2 pack has too many tape files"))?;
    let staged_record = store::NativeV2StagedPack {
        pack_ordinal: i32::try_from(plan.pack_ordinal)
            .map_err(|_| ExerciseFailure::failed("Native v2 pack ordinal overflowed"))?,
        pack_id,
        content_sha256: archive.pack_sha256.clone(),
        catalog_sha256: archive.catalog_sha256.clone(),
        logical_bytes: plan.logical_bytes,
        encoded_bytes: archive.encoded_bytes,
        storage_ref,
        manifest_entry_start,
        manifest_entry_count: plan.source_entry_count,
        tape_file_count,
    };
    if !store::record_native_v2_staged_pack(pool, claim, &staged_record)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(format!(
            "the fenced run rejected completed Native v2 pack {}",
            plan.pack_ordinal
        )));
    }
    publish_v2_sample(
        pool,
        sas_config,
        claim,
        V2Sample::spool(
            "validating_pack",
            store::SingleTapeLiveActivity::Buffering,
            usage,
            spool,
            u64::from(plan.pack_ordinal),
            pack_total,
            archive.encoded_bytes,
            plan.entries.last().cloned(),
        ),
    )
    .await;
    Ok(StagedPack {
        plan: plan.clone(),
        pack_id,
        directory: pack_directory,
        archive,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_pack_command(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    program: &Path,
    args: &[OsString],
    stage_directory: &Path,
    spool: SpoolWindow,
    plan: &PlannedDarPack,
    pack_total: u64,
    engine_state: &'static str,
    pack_maximum_bytes: Option<u64>,
) -> Result<(), ExerciseFailure> {
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
                "could not start pinned DAR for Native v2 pack {}: {error}",
                plan.pack_ordinal
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
    let mut previous: Option<(u64, Instant)> = None;
    let mut last_usage = spool_usage(stage_directory)?.0;
    let mut last_treated = 0_u64;
    let mut last_movement = started;
    let mut cancelled: Option<&'static str> = None;
    let status = loop {
        tokio::select! {
            status = child.wait() => {
                break status.map_err(|error| ExerciseFailure::failed(format!("DAR process wait failed: {error}")))?;
            }
            () = cancellation.cancelled(), if cancelled.is_none() => {
                cancelled = Some("worker shutdown was requested while DAR was building a Native v2 pack");
                let _ = child.start_kill();
            }
            _ = tick.tick() => {
                if Instant::now() >= deadline && cancelled.is_none() {
                    cancelled = Some("DAR exceeded the configured Native v2 backup deadline");
                    let _ = child.start_kill();
                }
                renew_claim(pool, claim, "while building or testing a Native v2 pack").await?;
                let usage = spool_usage(stage_directory)?;
                if let Some(maximum) = pack_maximum_bytes {
                    let pack_directory = stage_directory.join(format!("pack-{:08}", plan.pack_ordinal));
                    let pack_usage = spool_usage(&pack_directory)?.0;
                    if pack_usage > maximum && cancelled.is_none() {
                        cancelled = Some("DAR exceeded the hard per-pack byte boundary before tape motion");
                        let _ = child.start_kill();
                    }
                }
                let available = statvfs(stage_directory)
                    .map(|status| status.f_bavail.saturating_mul(status.f_frsize))
                    .unwrap_or(0);
                if available < config.native_spool_reserve_bytes && cancelled.is_none() {
                    cancelled = Some("Native v2 stopped pack production before consuming the spool safety reserve");
                    let _ = child.start_kill();
                }
                if usage.0 > spool.high_water_bytes && cancelled.is_none() {
                    cancelled = Some("Native v2 pack production exceeded the configured spool high-water boundary");
                    let _ = child.start_kill();
                }
                let now = Instant::now();
                let treated_now = treated.load(Ordering::Relaxed).min(plan.source_entry_count.max(1));
                if usage.0 != last_usage || treated_now != last_treated {
                    last_movement = now;
                    last_usage = usage.0;
                    last_treated = treated_now;
                }
                let idle = now.saturating_duration_since(last_movement);
                if idle >= config.transfer_stall_timeout && cancelled.is_none() {
                    cancelled = Some("DAR made no Native v2 pack progress before the transfer-stall deadline");
                    let _ = child.start_kill();
                }
                let rate = previous.and_then(|(bytes, observed)| {
                    bytes_per_second(bytes, usage.0, now.saturating_duration_since(observed))
                });
                previous = Some((usage.0, now));
                let mut sample = V2Sample::spool(
                    engine_state,
                    if idle >= config.transfer_stall_timeout {
                        store::SingleTapeLiveActivity::Stalled
                    } else {
                        store::SingleTapeLiveActivity::Buffering
                    },
                    usage,
                    spool,
                    u64::from(plan.pack_ordinal.saturating_sub(1)),
                    pack_total,
                    usage.0,
                    current_file.lock().ok().and_then(|value| value.clone()),
                );
                sample.pipeline_rate = rate;
                sample.progress_idle_ms = duration_millis_u64(idle);
                sample.no_progress_timeout_ms = duration_millis_u64(config.transfer_stall_timeout);
                publish_v2_sample(pool, sas_config, claim, sample).await;
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
            "DAR {engine_state} for pack {} failed with status {status}: {}",
            plan.pack_ordinal,
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
                while retained.iter().map(String::len).sum::<usize>() > COMMAND_OUTPUT_LIMIT {
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
        || relative.to_string_lossy().len() > MAX_LIVE_CURRENT_FILE_BYTES
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

#[derive(Clone, Debug)]
struct V2Sample {
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
    progress_scope: u64,
    progress_idle_ms: u64,
    no_progress_timeout_ms: u64,
    spool_bytes: Option<u64>,
    spool_capacity_bytes: Option<u64>,
    spool_high_water_bytes: Option<u64>,
    spool_low_water_bytes: Option<u64>,
    spool_ready_bytes: Option<u64>,
    spool_file_count: Option<u64>,
    spool_pack_count: Option<u64>,
    spool_fill_rate: Option<u64>,
    spool_drain_rate: Option<u64>,
    spool_reclaimed_bytes: Option<u64>,
}

impl V2Sample {
    #[allow(clippy::too_many_arguments)]
    fn spool(
        engine_state: &'static str,
        activity: store::SingleTapeLiveActivity,
        usage: (u64, u64),
        spool: SpoolWindow,
        files_processed: u64,
        files_total: u64,
        pipeline_bytes: u64,
        current_file: Option<String>,
    ) -> Self {
        Self {
            files_processed,
            files_total,
            pipeline_bytes,
            pipeline_total: None,
            pipeline_rate: None,
            current_file,
            tape_read_rate: None,
            tape_write_rate: None,
            activity,
            engine_state,
            progress_scope: files_processed.saturating_add(1),
            progress_idle_ms: 0,
            // Activity samples must always carry a usable stall horizon. Long-running
            // operations replace this default with the configured timeout on every
            // observation tick.
            no_progress_timeout_ms: 1_800_000,
            spool_bytes: Some(usage.0),
            spool_capacity_bytes: Some(spool.capacity_bytes),
            spool_high_water_bytes: Some(spool.high_water_bytes),
            spool_low_water_bytes: Some(spool.low_water_bytes),
            spool_ready_bytes: Some(spool.ready_bytes),
            spool_file_count: Some(usage.1),
            spool_pack_count: None,
            spool_fill_rate: None,
            spool_drain_rate: None,
            spool_reclaimed_bytes: None,
        }
    }
}

fn native_v2_spool_telemetry<'a>(
    packs: impl IntoIterator<Item = (&'a str, &'a str, i64)>,
) -> Result<(u64, u64), ExerciseFailure> {
    let mut ready_packs = 0_u64;
    let mut reclaimed_bytes = 0_u64;
    for (state, spool_state, encoded_bytes) in packs {
        let encoded_bytes = u64::try_from(encoded_bytes).map_err(|_| {
            ExerciseFailure::failed("Native v2 spool ledger contained a negative byte count")
        })?;
        if state == "staged" && spool_state == "present" {
            ready_packs = ready_packs
                .checked_add(1)
                .ok_or_else(|| ExerciseFailure::failed("Native v2 ready-pack count overflowed"))?;
        }
        if spool_state == "reclaimed" {
            reclaimed_bytes = reclaimed_bytes.checked_add(encoded_bytes).ok_or_else(|| {
                ExerciseFailure::failed("Native v2 reclaimed byte count overflowed")
            })?;
        }
    }
    Ok((ready_packs, reclaimed_bytes))
}

async fn publish_v2_sample(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    claim: &store::ClaimedSingleTapeDataRun,
    sample: V2Sample,
) {
    let ledger_telemetry = match store::native_v2_pack_ledger(pool, claim).await {
        Ok(packs) => match native_v2_spool_telemetry(packs.iter().map(|pack| {
            (
                pack.state.as_str(),
                pack.spool_state.as_str(),
                pack.encoded_bytes,
            )
        })) {
            Ok(telemetry) => Some(telemetry),
            Err(error) => {
                warn!(backup_run_id = %claim.run.id, detail = %error.detail, "could not summarize Native v2 spool telemetry");
                None
            }
        },
        Err(error) => {
            warn!(backup_run_id = %claim.run.id, %error, "could not load Native v2 spool telemetry");
            None
        }
    };
    let counters = read_sas_tape_io_counters(sas_config, Path::new(&claim.run.expected_path)).ok();
    let tape_read_available = counters.is_some_and(|value| value.read_bytes.is_some());
    let tape_write_available = counters.is_some_and(|value| value.write_bytes.is_some());
    let tape_available = tape_read_available || tape_write_available;
    let live = store::SingleTapeLiveIoSample {
        phase: store::SingleTapeLiveIoPhase::BackupWrite,
        sampled_at: Utc::now(),
        sample_window_ms: Some(LIVE_SAMPLE_INTERVAL.as_millis() as u64),
        stale_after_seconds: LIVE_IO_STALE_AFTER_SECONDS,
        engine_job_id: native_job_id(claim.run.archive_id, sample.progress_scope),
        files_processed: sample.files_processed,
        files_total: Some(sample.files_total),
        progress_basis: "native_v2_complete_packs",
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
        pipeline_basis: "native_v2_pack_bytes",
        tape_basis: tape_available.then_some("linux_scsi_tape_sysfs_counter_delta"),
        engine_state: Some(sample.engine_state),
        activity: Some(sample.activity),
        progress_idle_ms: Some(sample.progress_idle_ms),
        no_progress_timeout_ms: Some(sample.no_progress_timeout_ms),
        spool_bytes: sample.spool_bytes,
        spool_capacity_bytes: sample.spool_capacity_bytes,
        spool_target_bytes: sample.spool_ready_bytes,
        spool_file_count: sample.spool_file_count,
        spool_fill_bytes_per_second: sample.spool_fill_rate,
        spool_drain_bytes_per_second: sample.spool_drain_rate,
        spool_basis: sample
            .spool_bytes
            .map(|_| "run_scoped_native_v2_regular_file_bytes"),
        spool_high_water_bytes: sample.spool_high_water_bytes,
        spool_low_water_bytes: sample.spool_low_water_bytes,
        spool_pack_count: sample
            .spool_pack_count
            .or_else(|| ledger_telemetry.map(|telemetry| telemetry.0)),
        spool_reclaimed_bytes: sample
            .spool_reclaimed_bytes
            .or_else(|| ledger_telemetry.map(|telemetry| telemetry.1)),
    };
    match store::record_single_tape_data_live_io(pool, claim, &live).await {
        Ok(true) => {}
        Ok(false) => warn!(
            backup_run_id = %claim.run.id,
            engine_state = sample.engine_state,
            "Native v2 progress sample was rejected"
        ),
        Err(error) => {
            warn!(backup_run_id = %claim.run.id, %error, "could not publish Native v2 progress");
        }
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

enum TapePayload {
    Bytes(Vec<u8>),
    Path(PathBuf),
}

fn native_tape_write_failure(
    error: NativeArchiveError,
    display_name: &str,
    written: u64,
    expected: u64,
) -> ExerciseFailure {
    if matches!(&error, NativeArchiveError::Io(source) if source.raw_os_error() == Some(28)) {
        return ExerciseFailure::tape_capacity(format!(
            "physical tape reached end of media while writing {display_name} after {written} of {expected} bytes; the loaded cartridge's observed capacity did not match its catalog record, or this archive needs another tape"
        ));
    }
    ExerciseFailure::failed(error.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn write_tape_file(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    payload: TapePayload,
    display_name: String,
    completed_bytes: u64,
    total_bytes: u64,
    files_processed: u64,
    files_total: u64,
    progress_scope: u64,
    stage_directory: &Path,
    spool: SpoolWindow,
    engine_state: &'static str,
) -> Result<(), ExerciseFailure> {
    let expected = match &payload {
        TapePayload::Bytes(bytes) => bytes.len() as u64,
        TapePayload::Path(path) => fs::metadata(path)
            .map_err(|error| ExerciseFailure::failed(error.to_string()))?
            .len(),
    };
    let device = PathBuf::from(&claim.run.expected_path);
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
    let mut last_progress = 0_u64;
    let mut last_movement = Instant::now();
    let mut last_pipeline_rate = None;
    let mut last_tape_rate = None;
    let mut cancellation_seen = false;
    let mut deferred_failure = None;
    let mut stalled_failure = None;
    loop {
        tokio::select! {
            result = &mut write => {
                let outcome = result
                    .map_err(|error| ExerciseFailure::failed(format!("Native v2 tape writer failed: {error}")))?;
                let written = match outcome {
                    Ok(written) => written,
                    Err(error) => {
                        let written = write_progress.load(Ordering::Acquire).min(expected);
                        return Err(native_tape_write_failure(
                            error,
                            &display_name,
                            written,
                            expected,
                        ));
                    }
                };
                if written != expected {
                    return Err(ExerciseFailure::failed("Native v2 tape writer returned an incomplete tape file"));
                }
                break;
            }
            () = cancellation.cancelled(), if !cancellation_seen => {
                cancellation_seen = true;
            }
            _ = tick.tick() => {
                if deferred_failure.is_some() {
                    continue;
                }
                if let Err(error) = renew_claim(pool, claim, "while writing a complete Native v2 tape file").await {
                    deferred_failure = Some(error);
                    continue;
                }
                let counter = read_sas_tape_io_counters(sas_config, Path::new(&claim.run.expected_path))
                    .ok()
                    .and_then(|value| value.write_bytes);
                let now = Instant::now();
                let tape_rate = previous_counter.and_then(|(before, observed)| {
                    counter.and_then(|after| bytes_per_second(before, after, now.saturating_duration_since(observed)))
                });
                let counter_advanced = previous_counter
                    .zip(counter)
                    .is_some_and(|((before, _), after)| after > before);
                if let Some(counter) = counter {
                    previous_counter = Some((counter, now));
                }
                let written = write_progress.load(Ordering::Acquire).min(expected);
                let pipeline_rate = previous_progress.and_then(|(before, observed)| {
                    bytes_per_second(before, written, now.saturating_duration_since(observed))
                });
                previous_progress = Some((written, now));
                if written > last_progress || counter_advanced {
                    last_progress = written;
                    last_movement = now;
                }
                let idle = now.saturating_duration_since(last_movement);
                if idle >= config.transfer_stall_timeout && stalled_failure.is_none() {
                    stalled_failure = Some(ExerciseFailure::failed(
                        "Native v2 tape writing made no physical progress before the transfer-stall deadline; the current complete tape file was allowed to close safely",
                    ));
                }
                last_pipeline_rate = pipeline_rate;
                last_tape_rate = tape_rate.or(pipeline_rate);
                let usage = spool_usage(stage_directory).unwrap_or((0, 0));
                let mut sample = V2Sample::spool(
                    engine_state,
                    if idle >= config.transfer_stall_timeout {
                        store::SingleTapeLiveActivity::Stalled
                    } else {
                        store::SingleTapeLiveActivity::Streaming
                    },
                    usage,
                    spool,
                    files_processed,
                    files_total,
                    completed_bytes.saturating_add(written),
                    Some(display_name.clone()),
                );
                sample.pipeline_total = Some(total_bytes);
                sample.pipeline_rate = pipeline_rate;
                sample.tape_write_rate = last_tape_rate;
                sample.progress_scope = progress_scope;
                sample.progress_idle_ms = duration_millis_u64(idle);
                sample.no_progress_timeout_ms = duration_millis_u64(config.transfer_stall_timeout);
                sample.spool_drain_rate = pipeline_rate;
                publish_v2_sample(pool, sas_config, claim, sample).await;
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
            "worker shutdown was honored after completing the current Native v2 tape file",
        ));
    }
    let usage = spool_usage(stage_directory).unwrap_or((0, 0));
    let mut sample = V2Sample::spool(
        engine_state,
        store::SingleTapeLiveActivity::Streaming,
        usage,
        spool,
        files_processed,
        files_total,
        completed_bytes.saturating_add(expected),
        Some(display_name),
    );
    sample.pipeline_total = Some(total_bytes);
    sample.pipeline_rate = last_pipeline_rate;
    sample.tape_write_rate = last_tape_rate;
    sample.progress_scope = progress_scope;
    sample.no_progress_timeout_ms = duration_millis_u64(config.transfer_stall_timeout);
    sample.spool_drain_rate = last_pipeline_rate;
    publish_v2_sample(pool, sas_config, claim, sample).await;
    Ok(())
}

/// Re-establish the complete physical precondition immediately before a
/// volume crosses the durable write boundary.  Pack production may run for
/// hours after the original assignment, so neither the startup observation
/// nor an earlier attended-media check is sufficient evidence here.
async fn revalidate_volume_before_write(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    record: &store::DataRunVolumeRecord,
    cancellation: &CancellationToken,
) -> Result<(), ExerciseFailure> {
    let observations = discover_and_persist(pool, sas_config)
        .await
        .map_err(|error| {
            ExerciseFailure::interrupted(format!(
                "could not refresh the assigned drive immediately before tape writing: {error}"
            ))
        })?;
    let observation = matching_observation(
        &observations,
        &claim.run.expected_path,
        &claim.run.expected_stable_id,
    )
    .ok_or_else(|| {
        ExerciseFailure::blocked(
            "the exact assigned tape drive disappeared before the Native v2 write",
        )
    })?;
    if let Some(reason) = observation_blocker(observation, &claim.run) {
        return Err(ExerciseFailure::blocked(format!(
            "the selected drive or loaded tape changed before the Native v2 write: {reason}"
        )));
    }
    validate_loaded_media_generation(observation, record)?;

    // This reads TapeScale's preparation identity from the physical medium;
    // the UUID, rather than an operator acknowledgement, proves which tape is
    // actually loaded.
    native::verify_native_preparation(pool, sas_config, config, claim, record, cancellation)
        .await?;

    let observations = discover_and_persist(pool, sas_config)
        .await
        .map_err(|error| {
            ExerciseFailure::interrupted(format!(
                "could not verify the assigned drive after reading the tape identity: {error}"
            ))
        })?;
    let observation = matching_observation(
        &observations,
        &claim.run.expected_path,
        &claim.run.expected_stable_id,
    )
    .ok_or_else(|| {
        ExerciseFailure::blocked(
            "the exact assigned tape drive disappeared after loaded-tape verification",
        )
    })?;
    if let Some(reason) = observation_blocker(observation, &claim.run) {
        return Err(ExerciseFailure::blocked(format!(
            "the selected drive or loaded tape changed during loaded-tape verification: {reason}"
        )));
    }
    validate_loaded_media_generation(observation, record)?;
    if observation.at_bot != Some(true) || observation.position_lost == Some(true) {
        return Err(ExerciseFailure::blocked(
            "loaded-tape verification did not leave the exact selected tape at a known beginning-of-tape position",
        ));
    }
    renew_claim(
        pool,
        claim,
        "after proving the loaded TapeScale identity and before tape writing",
    )
    .await
}

fn validate_loaded_media_generation(
    observation: &crate::device::SasDriveObservation,
    record: &store::DataRunVolumeRecord,
) -> Result<(), ExerciseFailure> {
    let expected = record.expected_media_generation.ok_or_else(|| {
        ExerciseFailure::blocked(
            "the assigned cartridge has no recorded LTO generation; prepare the loaded tape again",
        )
    })?;
    let observed = observation.loaded_media_generation.ok_or_else(|| {
        ExerciseFailure::blocked(
            "the loaded tape did not report a recognized LTO generation immediately before writing; TapeScale will not guess its capacity",
        )
    })?;
    if observed != expected {
        return Err(ExerciseFailure::blocked(format!(
            "loaded tape density is LTO-{observed}, but the assigned cartridge is recorded as LTO-{expected}; prepare the loaded tape again so TapeScale can correct its capacity before writing"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn open_volume(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    source_manifest: &ManifestArtifactSummary,
    record: store::DataRunVolumeRecord,
    previous: Option<&CompletedVolume>,
    tape_touched: &mut bool,
    media_write_started: &mut bool,
    spool: SpoolWindow,
    pack_total: u64,
) -> Result<OpenVolume, ExerciseFailure> {
    let volume_ordinal = u32::try_from(record.volume_order)
        .map_err(|_| ExerciseFailure::failed("invalid Native v2 volume order"))?;
    let volume_name = rolling_archive_volume_identity(claim.run.archive_id, volume_ordinal);
    renew_claim(
        pool,
        claim,
        "after loaded-tape verification and immediately before the Native v2 media-write boundary",
    )
    .await?;
    if !store::mark_data_run_volume_write_started(pool, claim, record.volume_order, &volume_name)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the fenced run could not commit the Native v2 media-write boundary",
        ));
    }
    *media_write_started = true;
    *tape_touched = true;
    native::set_variable_block_mode(
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
    renew_claim(pool, claim, "before opening a Native v2 archive volume").await?;

    let archive_volume_id = Uuid::now_v7();
    let header = NativeV2VolumeHeader {
        format_version: NATIVE_ROLLING_FORMAT_VERSION,
        cartridge_id: record.cartridge_id,
        archive_id: claim.run.archive_id,
        archive_volume_id,
        volume_ordinal,
        previous_archive_volume_id: previous.map(|value| value.archive_volume_id),
        previous_volume_index_sha256: previous.map(|value| value.index_sha256.clone()),
        declared_pack_count: None,
        representation: NativeArchiveRepresentation::FileDar,
        compression: DarCompression::from_resolved(&claim.run.resolved_compression).native_codec(),
        tape_block_bytes: DEFAULT_TAPE_BLOCK_BYTES,
        source_manifest_sha256: source_manifest.artifact_sha256.clone(),
        created_at: Utc::now(),
    };
    let header_bytes = header
        .encode()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let metadata_total = u64::try_from(header_bytes.len())
        .ok()
        .and_then(|value| value.checked_add(source_manifest.size_bytes))
        .ok_or_else(|| ExerciseFailure::failed("Native v2 opening metadata size overflowed"))?;
    let stage_directory = config.native_spool_dir.join(claim.run.id.to_string());
    write_tape_file(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        TapePayload::Bytes(header_bytes.clone()),
        format!("Native v2 volume {volume_ordinal} header"),
        0,
        metadata_total,
        0,
        pack_total,
        100_000 + u64::from(volume_ordinal),
        &stage_directory,
        spool,
        "writing_native_volume_header",
    )
    .await?;
    if !store::mark_data_run_volume_labeled(pool, claim, record.volume_order, &volume_name)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(
            "the Native v2 volume header was written but its identity could not be retained",
        ));
    }
    write_tape_file(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        TapePayload::Path(source_manifest.storage_path.clone()),
        "source-manifest.json".to_owned(),
        header_bytes.len() as u64,
        metadata_total,
        0,
        pack_total,
        100_000 + u64::from(volume_ordinal),
        &stage_directory,
        spool,
        "writing_native_volume_header",
    )
    .await?;

    Ok(OpenVolume {
        capacity_bytes: volume_payload_capacity(
            record.expected_media_generation,
            record.expected_media_format.as_deref(),
            source_manifest.size_bytes,
        )?,
        record,
        archive_volume_id,
        previous_archive_volume_id: previous.map(|value| value.archive_volume_id),
        previous_index_sha256: previous.map(|value| value.index_sha256.clone()),
        volume_name,
        header,
        header_bytes,
        packs: Vec::new(),
        next_tape_file_number: 2,
        physical_pack_bytes: 0,
    })
}

#[allow(clippy::too_many_arguments)]
async fn write_staged_pack(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    volume: &mut OpenVolume,
    staged: &StagedPack,
    spool: SpoolWindow,
    pack_total: u64,
) -> Result<NativePackDescriptor, ExerciseFailure> {
    let tape_file_count = u32::try_from(2_usize.saturating_add(staged.archive.slices.len()))
        .map_err(|_| ExerciseFailure::failed("Native v2 pack has too many tape files"))?;
    let archive_offset_bytes = volume.packs.last().map(|pack| {
        pack.descriptor
            .archive_offset_bytes
            .saturating_add(pack.descriptor.encoded_bytes)
    });
    let prior_global_bytes = store::native_v2_pack_ledger(pool, claim)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
        .into_iter()
        .filter(|record| record.pack_ordinal < staged.plan.pack_ordinal as i32)
        .try_fold(0_u64, |total, record| {
            u64::try_from(record.encoded_bytes)
                .ok()
                .and_then(|bytes| total.checked_add(bytes))
        })
        .ok_or_else(|| ExerciseFailure::failed("Native v2 archive offset overflowed"))?;
    let descriptor = NativePackDescriptor {
        pack_id: staged.pack_id,
        pack_ordinal: staged.plan.pack_ordinal,
        kind: NativePackKind::DarArchive,
        first_tape_file_number: volume.next_tape_file_number,
        tape_file_count,
        archive_offset_bytes: archive_offset_bytes.unwrap_or(prior_global_bytes),
        encoded_bytes: staged.archive.encoded_bytes,
        logical_bytes: staged.plan.logical_bytes,
        entry_count: staged.plan.source_entry_count,
        content_sha256: staged.archive.pack_sha256.clone(),
        catalog_sha256: Some(staged.archive.catalog_sha256.clone()),
    };
    let descriptor_bytes = descriptor
        .encode()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let physical_total = u64::try_from(descriptor_bytes.len())
        .ok()
        .and_then(|value| value.checked_add(staged.archive.catalog_bytes))
        .and_then(|value| value.checked_add(staged.archive.encoded_bytes))
        .ok_or_else(|| ExerciseFailure::failed("Native v2 pack tape byte total overflowed"))?;
    if volume.physical_pack_bytes.saturating_add(physical_total) > volume.capacity_bytes {
        return Err(ExerciseFailure::failed(
            "Native v2 pack no longer fits after its exact descriptor was encoded",
        ));
    }
    let pack_order = i32::try_from(staged.plan.pack_ordinal)
        .map_err(|_| ExerciseFailure::failed("Native v2 pack ordinal overflowed"))?;
    let first_tape_file = i32::try_from(volume.next_tape_file_number)
        .map_err(|_| ExerciseFailure::failed("Native v2 tape file number overflowed"))?;
    if !store::assign_native_v2_pack(
        pool,
        claim,
        pack_order,
        volume.record.volume_order,
        first_tape_file,
    )
    .await
    .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(format!(
            "the fenced run rejected the tape position for Native v2 pack {}",
            staged.plan.pack_ordinal
        )));
    }
    if !store::mark_native_v2_pack_write_started(pool, claim, pack_order)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(format!(
            "the fenced run rejected the write boundary for Native v2 pack {}",
            staged.plan.pack_ordinal
        )));
    }

    let stage_directory = config.native_spool_dir.join(claim.run.id.to_string());
    let scope = 1_000_000 + u64::from(staged.plan.pack_ordinal);
    let mut completed = 0_u64;
    write_tape_file(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        TapePayload::Bytes(descriptor_bytes.clone()),
        format!("pack {:08} descriptor", staged.plan.pack_ordinal),
        completed,
        physical_total,
        u64::from(staged.plan.pack_ordinal.saturating_sub(1)),
        pack_total,
        scope,
        &stage_directory,
        spool,
        "writing_native_pack",
    )
    .await?;
    completed = completed.saturating_add(descriptor_bytes.len() as u64);
    write_tape_file(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        TapePayload::Path(staged.archive.catalog_path.clone()),
        format!("pack {:08} catalog.1.dar", staged.plan.pack_ordinal),
        completed,
        physical_total,
        u64::from(staged.plan.pack_ordinal.saturating_sub(1)),
        pack_total,
        scope,
        &stage_directory,
        spool,
        "writing_native_pack",
    )
    .await?;
    completed = completed.saturating_add(staged.archive.catalog_bytes);
    for slice in &staged.archive.slices {
        let display_name = slice
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .map(|value| format!("pack {:08} {value}", staged.plan.pack_ordinal))
            .unwrap_or_else(|| {
                format!(
                    "pack {:08} DAR slice {}",
                    staged.plan.pack_ordinal, slice.ordinal
                )
            });
        write_tape_file(
            pool,
            sas_config,
            config,
            claim,
            cancellation,
            TapePayload::Path(slice.path.clone()),
            display_name,
            completed,
            physical_total,
            u64::from(staged.plan.pack_ordinal.saturating_sub(1)),
            pack_total,
            scope,
            &stage_directory,
            spool,
            "writing_native_pack",
        )
        .await?;
        completed = completed.saturating_add(slice.encoded_bytes);
    }
    if !store::mark_native_v2_pack_written(pool, claim, pack_order)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(format!(
            "the fenced run rejected completed Native v2 pack {}",
            staged.plan.pack_ordinal
        )));
    }
    let slices = staged
        .archive
        .slices
        .iter()
        .map(|slice| PhysicalSlice {
            ordinal: slice.ordinal,
            encoded_bytes: slice.encoded_bytes,
            sha256: slice.sha256.clone(),
            display_name: slice
                .path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("archive slice")
                .to_owned(),
        })
        .collect();
    volume.physical_pack_bytes = volume.physical_pack_bytes.saturating_add(physical_total);
    volume.next_tape_file_number = volume
        .next_tape_file_number
        .checked_add(tape_file_count)
        .ok_or_else(|| ExerciseFailure::failed("Native v2 tape file number overflowed"))?;
    volume.packs.push(WrittenPack {
        descriptor: descriptor.clone(),
        descriptor_bytes,
        catalog_bytes: staged.archive.catalog_bytes,
        catalog_sha256: staged.archive.catalog_sha256.clone(),
        slices,
    });
    Ok(descriptor)
}

async fn reclaim_staged_pack(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    stage_directory: &Path,
    staged: &StagedPack,
) -> Result<(), ExerciseFailure> {
    fs::remove_dir_all(&staged.directory).map_err(|error| {
        ExerciseFailure::failed(format!(
            "Native v2 pack {} was written but its spool storage could not be reclaimed: {error}",
            staged.plan.pack_ordinal
        ))
    })?;
    if let Ok(directory) = fs::File::open(stage_directory) {
        let _ = directory.sync_all();
    }
    if !store::mark_native_v2_pack_spool_reclaimed(
        pool,
        claim,
        i32::try_from(staged.plan.pack_ordinal)
            .map_err(|_| ExerciseFailure::failed("Native v2 pack ordinal overflowed"))?,
    )
    .await
    .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(format!(
            "the fenced run rejected spool reclamation for Native v2 pack {}",
            staged.plan.pack_ordinal
        )));
    }
    Ok(())
}

fn volume_index(
    volume: &OpenVolume,
    source_manifest: &ManifestArtifactSummary,
) -> Result<NativeV2VolumeIndex, ExerciseFailure> {
    let packs = volume
        .packs
        .iter()
        .map(|pack| pack.descriptor.clone())
        .collect::<Vec<_>>();
    let index = NativeV2VolumeIndex {
        format_version: NATIVE_ROLLING_FORMAT_VERSION,
        cartridge_id: volume.record.cartridge_id,
        archive_id: volume.header.archive_id,
        archive_volume_id: volume.archive_volume_id,
        volume_ordinal: volume.header.volume_ordinal,
        previous_archive_volume_id: volume.previous_archive_volume_id,
        previous_volume_index_sha256: volume.previous_index_sha256.clone(),
        source_manifest_sha256: source_manifest.artifact_sha256.clone(),
        total_encoded_bytes: packs.iter().map(|pack| pack.encoded_bytes).sum(),
        total_logical_bytes: packs.iter().map(|pack| pack.logical_bytes).sum(),
        total_entries: packs.iter().map(|pack| pack.entry_count).sum(),
        packs,
    };
    index
        .verify_header(&volume.header)
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    Ok(index)
}

fn completed_to_sealed(volume: &CompletedVolume) -> NativeSealedVolume {
    NativeSealedVolume {
        volume_ordinal: u32::try_from(volume.volume_order).unwrap_or(u32::MAX),
        archive_volume_id: volume.archive_volume_id,
        previous_archive_volume_id: volume.previous_archive_volume_id,
        previous_volume_index_sha256: volume.previous_index_sha256.clone(),
        index_sha256: volume.index_sha256.clone(),
        first_pack_ordinal: volume.first_pack_ordinal,
        pack_count: volume.pack_count,
        encoded_bytes: volume.encoded_bytes,
        logical_bytes: volume.logical_bytes,
        entry_count: volume.entry_count,
    }
}

fn open_to_sealed(
    _volume: &OpenVolume,
    index: &NativeV2VolumeIndex,
) -> Result<NativeSealedVolume, ExerciseFailure> {
    let first = index
        .packs
        .first()
        .ok_or_else(|| ExerciseFailure::failed("cannot seal an empty Native v2 volume"))?;
    Ok(NativeSealedVolume {
        volume_ordinal: index.volume_ordinal,
        archive_volume_id: index.archive_volume_id,
        previous_archive_volume_id: index.previous_archive_volume_id,
        previous_volume_index_sha256: index.previous_volume_index_sha256.clone(),
        index_sha256: index
            .payload_sha256()
            .map_err(|error| ExerciseFailure::failed(error.to_string()))?,
        first_pack_ordinal: first.pack_ordinal,
        pack_count: u32::try_from(index.packs.len())
            .map_err(|_| ExerciseFailure::failed("Native v2 pack count overflowed"))?,
        encoded_bytes: index.total_encoded_bytes,
        logical_bytes: index.total_logical_bytes,
        entry_count: index.total_entries,
    })
}

#[allow(clippy::too_many_arguments)]
async fn close_and_verify_volume(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    source_manifest: &ManifestArtifactSummary,
    volume: OpenVolume,
    seal: Option<&NativeArchiveSetSeal>,
    spool: SpoolWindow,
    pack_total: u64,
) -> Result<CompletedVolume, ExerciseFailure> {
    let index = volume_index(&volume, source_manifest)?;
    let index_bytes = index
        .encode()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let seal_bytes = seal
        .map(NativeArchiveSetSeal::encode)
        .transpose()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let trailer = if let Some(seal) = seal {
        NativeV2VolumeTrailer::from_index_with_archive_set_seal(&index, seal, Utc::now())
    } else {
        NativeV2VolumeTrailer::from_index(&index, Utc::now())
    }
    .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let trailer_bytes = trailer
        .encode()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let closing_total = u64::try_from(index_bytes.len())
        .ok()
        .and_then(|value| {
            value.checked_add(seal_bytes.as_ref().map_or(0, |bytes| bytes.len() as u64))
        })
        .and_then(|value| value.checked_add(trailer_bytes.len() as u64))
        .ok_or_else(|| ExerciseFailure::failed("Native v2 closing metadata size overflowed"))?;
    let stage_directory = config.native_spool_dir.join(claim.run.id.to_string());
    let progress_scope = 2_000_000 + u64::from(index.volume_ordinal);
    write_tape_file(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        TapePayload::Bytes(index_bytes.clone()),
        format!("Native v2 volume {} index", index.volume_ordinal),
        0,
        closing_total,
        u64::from(index.packs.last().expect("nonempty index").pack_ordinal),
        pack_total,
        progress_scope,
        &stage_directory,
        spool,
        "closing_native_volume",
    )
    .await?;
    let mut completed_closing = index_bytes.len() as u64;
    if let Some(bytes) = &seal_bytes {
        write_tape_file(
            pool,
            sas_config,
            config,
            claim,
            cancellation,
            TapePayload::Bytes(bytes.clone()),
            "Native v2 archive-set seal".to_owned(),
            completed_closing,
            closing_total,
            u64::from(index.packs.last().expect("nonempty index").pack_ordinal),
            pack_total,
            progress_scope,
            &stage_directory,
            spool,
            "closing_native_volume",
        )
        .await?;
        completed_closing = completed_closing.saturating_add(bytes.len() as u64);
    }
    write_tape_file(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        TapePayload::Bytes(trailer_bytes.clone()),
        format!("Native v2 volume {} trailer", index.volume_ordinal),
        completed_closing,
        closing_total,
        u64::from(index.packs.last().expect("nonempty index").pack_ordinal),
        pack_total,
        progress_scope,
        &stage_directory,
        spool,
        "closing_native_volume",
    )
    .await?;

    execute_sas_motion(
        sas_config,
        Path::new(&claim.run.expected_path),
        SasMotionOperation::Rewind,
        cancellation,
    )
    .await
    .map_err(|error| {
        ExerciseFailure::failed(format!(
            "could not rewind Native v2 volume {} for readback: {}",
            index.volume_ordinal,
            sas_command_error_detail(&error)
        ))
    })?;
    verify_physical_volume(
        pool,
        sas_config,
        config,
        claim,
        cancellation,
        source_manifest,
        &volume,
        &index,
        &index_bytes,
        seal,
        seal_bytes.as_deref(),
        &trailer,
        &trailer_bytes,
        spool,
        pack_total,
    )
    .await?;

    let index_sha256 = index
        .payload_sha256()
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    let tape_file_count_u32 = volume
        .next_tape_file_number
        .checked_add(2)
        .and_then(|value| value.checked_add(if seal.is_some() { 1 } else { 0 }))
        .ok_or_else(|| ExerciseFailure::failed("Native v2 tape file count overflowed"))?;
    let first_pack = index.packs.first().expect("index is nonempty");
    let first_record = format!(
        "TSNV2:{}:{}:header",
        claim.run.archive_id, index.volume_ordinal
    );
    let last_record = format!(
        "TSNV2:{}:{}:trailer",
        claim.run.archive_id, index.volume_ordinal
    );
    let pack_files = index
        .packs
        .iter()
        .map(|pack| {
            Ok((
                i32::try_from(pack.pack_ordinal)
                    .map_err(|_| ExerciseFailure::failed("Native v2 pack ordinal overflowed"))?,
                i32::try_from(pack.first_tape_file_number).map_err(|_| {
                    ExerciseFailure::failed("Native v2 tape file number overflowed")
                })?,
            ))
        })
        .collect::<Result<Vec<_>, ExerciseFailure>>()?;
    let evidence = store::NativeV2VerifiedVolume {
        volume_order: volume.record.volume_order,
        archive_volume_id: volume.archive_volume_id,
        previous_archive_volume_id: volume.previous_archive_volume_id,
        previous_index_sha256: volume.previous_index_sha256.clone(),
        engine_volume_name: volume.volume_name.clone(),
        header_sha256: sha256_bytes(&volume.header_bytes),
        index_sha256: index_sha256.clone(),
        tape_file_count: i32::try_from(tape_file_count_u32)
            .map_err(|_| ExerciseFailure::failed("Native v2 tape file count overflowed"))?,
        encoded_bytes: index.total_encoded_bytes,
        first_record: first_record.clone(),
        last_record: last_record.clone(),
        archive_final: seal.is_some(),
        archive_seal_sha256: seal.map(|value| value.archive_set_sha256.clone()),
        pack_files: pack_files.clone(),
    };
    if !store::mark_native_v2_volume_verified(pool, claim, &evidence)
        .await
        .map_err(|error| ExerciseFailure::interrupted(error.to_string()))?
    {
        return Err(ExerciseFailure::interrupted(format!(
            "the fenced run rejected verified Native v2 volume {}",
            index.volume_ordinal
        )));
    }
    Ok(CompletedVolume {
        volume_order: volume.record.volume_order,
        cartridge_id: volume.record.cartridge_id,
        archive_volume_id: volume.archive_volume_id,
        previous_archive_volume_id: volume.previous_archive_volume_id,
        previous_index_sha256: volume.previous_index_sha256,
        volume_name: volume.volume_name,
        header_sha256: sha256_bytes(&volume.header_bytes),
        index_sha256: index_sha256.clone(),
        tape_file_count: evidence.tape_file_count,
        encoded_bytes: index.total_encoded_bytes,
        first_record,
        last_record,
        engine_record_id: index_sha256,
        first_pack_ordinal: first_pack.pack_ordinal,
        pack_count: u32::try_from(index.packs.len())
            .map_err(|_| ExerciseFailure::failed("Native v2 pack count overflowed"))?,
        logical_bytes: index.total_logical_bytes,
        entry_count: index.total_entries,
        pack_files,
        archive_final: seal.is_some(),
        archive_seal_sha256: seal.map(|value| value.archive_set_sha256.clone()),
    })
}

#[allow(clippy::too_many_arguments)]
async fn verify_physical_volume(
    pool: &PgPool,
    sas_config: &SasDiscoveryConfig,
    config: &Config,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    source_manifest: &ManifestArtifactSummary,
    volume: &OpenVolume,
    expected_index: &NativeV2VolumeIndex,
    index_bytes: &[u8],
    expected_seal: Option<&NativeArchiveSetSeal>,
    seal_bytes: Option<&[u8]>,
    expected_trailer: &NativeV2VolumeTrailer,
    trailer_bytes: &[u8],
    spool: SpoolWindow,
    pack_total: u64,
) -> Result<(), ExerciseFailure> {
    renew_claim(pool, claim, "before Native v2 physical-media readback").await?;
    let total_readback_bytes = u64::try_from(volume.header_bytes.len())
        .ok()
        .and_then(|value| value.checked_add(source_manifest.size_bytes))
        .and_then(|value| {
            volume.packs.iter().try_fold(value, |total, pack| {
                total
                    .checked_add(pack.descriptor_bytes.len() as u64)?
                    .checked_add(pack.catalog_bytes)?
                    .checked_add(
                        pack.slices
                            .iter()
                            .try_fold(0_u64, |sum, slice| sum.checked_add(slice.encoded_bytes))?,
                    )
            })
        })
        .and_then(|value| value.checked_add(index_bytes.len() as u64))
        .and_then(|value| value.checked_add(seal_bytes.map_or(0, |bytes| bytes.len() as u64)))
        .and_then(|value| value.checked_add(trailer_bytes.len() as u64))
        .ok_or_else(|| ExerciseFailure::failed("Native v2 readback size overflowed"))?;
    let device = PathBuf::from(&claim.run.expected_path);
    let expected_header = volume.header.clone();
    let expected_header_bytes = volume.header_bytes.clone();
    let expected_manifest_sha256 = source_manifest.artifact_sha256.clone();
    let manifest_bytes = source_manifest.size_bytes;
    let expected_packs = volume.packs.clone();
    let index_expected = expected_index.clone();
    let index_expected_bytes = index_bytes.to_vec();
    let seal_expected = expected_seal.cloned();
    let seal_expected_bytes = seal_bytes.map(ToOwned::to_owned);
    let trailer_expected = expected_trailer.clone();
    let trailer_expected_bytes = trailer_bytes.to_vec();
    let read_progress = Arc::new(AtomicU64::new(0));
    let task_progress = Arc::clone(&read_progress);
    let current_file = Arc::new(Mutex::new(Some("Native v2 volume header".to_owned())));
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
        if physical_header != expected_header_bytes
            || NativeV2VolumeHeader::decode(&physical_header)
                .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?
                != expected_header
        {
            return Err(NativeArchiveError::Invalid(
                "Native v2 volume header readback mismatch".to_owned(),
            ));
        }
        set_current("source-manifest.json".to_owned());
        if read_and_hash_tape_file_with_progress(&mut tape, manifest_bytes, task_progress.as_ref())?
            != expected_manifest_sha256
        {
            return Err(NativeArchiveError::Invalid(
                "Native v2 source manifest readback mismatch".to_owned(),
            ));
        }
        for pack in expected_packs {
            set_current(format!(
                "pack {:08} descriptor",
                pack.descriptor.pack_ordinal
            ));
            let physical_descriptor = read_tape_file_with_progress(
                &mut tape,
                NATIVE_METADATA_READ_LIMIT,
                task_progress.as_ref(),
            )?;
            if physical_descriptor != pack.descriptor_bytes
                || NativePackDescriptor::decode(&physical_descriptor)
                    .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?
                    != pack.descriptor
            {
                return Err(NativeArchiveError::Invalid(format!(
                    "Native v2 pack {} descriptor readback mismatch",
                    pack.descriptor.pack_ordinal
                )));
            }
            set_current(format!(
                "pack {:08} catalog.1.dar",
                pack.descriptor.pack_ordinal
            ));
            let physical_catalog = read_and_hash_bounded_tape_file_with_progress(
                &mut tape,
                pack.catalog_bytes,
                task_progress.as_ref(),
            )?;
            if physical_catalog.encoded_bytes != pack.catalog_bytes
                || physical_catalog.sha256 != pack.catalog_sha256
            {
                return Err(NativeArchiveError::Invalid(format!(
                    "Native v2 pack {} catalog readback mismatch",
                    pack.descriptor.pack_ordinal
                )));
            }
            let mut physical_slices = Vec::with_capacity(pack.slices.len());
            for expected_slice in pack.slices {
                set_current(format!(
                    "pack {:08} {}",
                    pack.descriptor.pack_ordinal, expected_slice.display_name
                ));
                let physical = read_and_hash_bounded_tape_file_with_progress(
                    &mut tape,
                    pack.descriptor.encoded_bytes,
                    task_progress.as_ref(),
                )?;
                if physical.encoded_bytes != expected_slice.encoded_bytes
                    || physical.sha256 != expected_slice.sha256
                {
                    return Err(NativeArchiveError::Invalid(format!(
                        "Native v2 pack {} slice {} readback mismatch",
                        pack.descriptor.pack_ordinal, expected_slice.ordinal
                    )));
                }
                physical_slices.push(StagedSlice {
                    ordinal: expected_slice.ordinal,
                    encoded_bytes: physical.encoded_bytes,
                    sha256: physical.sha256,
                    path: PathBuf::from(&expected_slice.display_name),
                });
            }
            if dar_pack_content_sha256(
                pack.descriptor.pack_ordinal,
                &physical_catalog.sha256,
                &physical_slices,
            )? != pack.descriptor.content_sha256
            {
                return Err(NativeArchiveError::Invalid(format!(
                    "Native v2 pack {} content commitment mismatch",
                    pack.descriptor.pack_ordinal
                )));
            }
        }
        set_current("Native v2 volume index".to_owned());
        let physical_index = read_tape_file_with_progress(
            &mut tape,
            NATIVE_METADATA_READ_LIMIT,
            task_progress.as_ref(),
        )?;
        if physical_index != index_expected_bytes
            || NativeV2VolumeIndex::decode(&physical_index)
                .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?
                != index_expected
        {
            return Err(NativeArchiveError::Invalid(
                "Native v2 volume index readback mismatch".to_owned(),
            ));
        }
        if let (Some(expected), Some(expected_bytes)) =
            (seal_expected.as_ref(), seal_expected_bytes.as_ref())
        {
            set_current("Native v2 archive-set seal".to_owned());
            let physical_seal = read_tape_file_with_progress(
                &mut tape,
                NATIVE_METADATA_READ_LIMIT,
                task_progress.as_ref(),
            )?;
            if &physical_seal != expected_bytes
                || NativeArchiveSetSeal::decode(&physical_seal)
                    .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?
                    != *expected
            {
                return Err(NativeArchiveError::Invalid(
                    "Native v2 archive-set seal readback mismatch".to_owned(),
                ));
            }
        }
        set_current("Native v2 volume trailer".to_owned());
        let physical_trailer = read_tape_file_with_progress(
            &mut tape,
            NATIVE_METADATA_READ_LIMIT,
            task_progress.as_ref(),
        )?;
        let decoded_trailer = NativeV2VolumeTrailer::decode(&physical_trailer)
            .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?;
        if physical_trailer != trailer_expected_bytes || decoded_trailer != trailer_expected {
            return Err(NativeArchiveError::Invalid(
                "Native v2 volume trailer readback mismatch".to_owned(),
            ));
        }
        decoded_trailer
            .verify_index(&index_expected)
            .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?;
        if let Some(seal) = seal_expected.as_ref() {
            decoded_trailer
                .verify_archive_set_seal(seal)
                .map_err(|error| NativeArchiveError::Invalid(error.to_string()))?;
        }
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
                    .map_err(|error| ExerciseFailure::failed(format!("Native v2 tape reader failed: {error}")))?
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
                if let Err(error) = renew_claim(pool, claim, "during Native v2 physical-media readback").await {
                    deferred_failure = Some(error);
                    continue;
                }
                let counter = read_sas_tape_io_counters(sas_config, Path::new(&claim.run.expected_path))
                    .ok()
                    .and_then(|value| value.read_bytes);
                let now = Instant::now();
                let tape_rate = previous_counter.and_then(|(before, observed)| {
                    counter.and_then(|after| bytes_per_second(before, after, now.saturating_duration_since(observed)))
                });
                let counter_advanced = previous_counter
                    .zip(counter)
                    .is_some_and(|((before, _), after)| after > before);
                if let Some(counter) = counter {
                    previous_counter = Some((counter, now));
                }
                let read = read_progress.load(Ordering::Acquire).min(total_readback_bytes);
                let pipeline_rate = previous_progress.and_then(|(before, observed)| {
                    bytes_per_second(before, read, now.saturating_duration_since(observed))
                });
                previous_progress = Some((read, now));
                if read > last_progress || counter_advanced {
                    last_progress = read;
                    last_movement = now;
                }
                let idle = now.saturating_duration_since(last_movement);
                if idle >= config.transfer_stall_timeout && stalled_failure.is_none() {
                    stalled_failure = Some(ExerciseFailure::failed(
                        "Native v2 physical readback made no progress before the transfer-stall deadline; the tape reader was allowed to close safely",
                    ));
                }
                last_pipeline_rate = pipeline_rate;
                last_tape_rate = tape_rate.or(pipeline_rate);
                let usage = spool_usage(&config.native_spool_dir.join(claim.run.id.to_string())).unwrap_or((0, 0));
                let mut sample = V2Sample::spool(
                    "reading_native_pack",
                    if idle >= config.transfer_stall_timeout {
                        store::SingleTapeLiveActivity::Stalled
                    } else {
                        store::SingleTapeLiveActivity::Reading
                    },
                    usage,
                    spool,
                    u64::from(expected_index.packs.last().expect("index nonempty").pack_ordinal),
                    pack_total,
                    read,
                    current_file.lock().ok().and_then(|value| value.clone()),
                );
                sample.pipeline_total = Some(total_readback_bytes);
                sample.pipeline_rate = pipeline_rate;
                sample.tape_read_rate = last_tape_rate;
                sample.progress_scope = 10_000_000 + u64::from(expected_index.volume_ordinal);
                sample.progress_idle_ms = duration_millis_u64(idle);
                sample.no_progress_timeout_ms = duration_millis_u64(config.transfer_stall_timeout);
                publish_v2_sample(pool, sas_config, claim, sample).await;
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
            "worker shutdown was honored after completing Native v2 volume readback",
        ));
    }
    let usage =
        spool_usage(&config.native_spool_dir.join(claim.run.id.to_string())).unwrap_or((0, 0));
    let mut sample = V2Sample::spool(
        "reading_native_pack",
        store::SingleTapeLiveActivity::Reading,
        usage,
        spool,
        u64::from(
            expected_index
                .packs
                .last()
                .expect("index nonempty")
                .pack_ordinal,
        ),
        pack_total,
        total_readback_bytes,
        current_file.lock().ok().and_then(|value| value.clone()),
    );
    sample.pipeline_total = Some(total_readback_bytes);
    sample.pipeline_rate = last_pipeline_rate;
    sample.tape_read_rate = last_tape_rate;
    sample.progress_scope = 10_000_000 + u64::from(expected_index.volume_ordinal);
    sample.no_progress_timeout_ms = duration_millis_u64(config.transfer_stall_timeout);
    publish_v2_sample(pool, sas_config, claim, sample).await;
    Ok(())
}

async fn persist_engine_evidence(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    evidence_directory: &Path,
    source_manifest: &ManifestArtifactSummary,
    compression: DarCompression,
    volumes: &[CompletedVolume],
    seal: &NativeArchiveSetSeal,
) -> Result<(), ExerciseFailure> {
    let engine_write = json!({
        "finalized": true,
        "engine": "TapeScale Native",
        "engine_version": "2",
        "format_version": NATIVE_ROLLING_FORMAT_NAME,
        "native_layout_version": 2,
        "archive_id": claim.run.archive_id,
        "archive_codec": "dar",
        "dar_version": crate::native_archive::DAR_CODEC_VERSION,
        "compression": compression,
        "source_manifest_sha256": source_manifest.artifact_sha256,
        "pack_count": seal.pack_count,
        "encoded_bytes": seal.total_encoded_bytes,
        "archive_set_sha256": seal.archive_set_sha256,
        "pack_catalog_root_sha256": seal.pack_catalog_root_sha256,
        "ordered_pack_sha256": seal.ordered_pack_sha256,
        "volumes": volumes.iter().map(|volume| json!({
            "volume_order": volume.volume_order,
            "cartridge_id": volume.cartridge_id,
            "archive_volume_id": volume.archive_volume_id,
            "previous_archive_volume_id": volume.previous_archive_volume_id,
            "previous_index_sha256": volume.previous_index_sha256,
            "volume_name": volume.volume_name,
            "header_sha256": volume.header_sha256,
            "index_sha256": volume.index_sha256,
            "tape_file_count": volume.tape_file_count,
            "encoded_bytes": volume.encoded_bytes,
            "first_record": volume.first_record,
            "last_record": volume.last_record,
            "engine_record_id": volume.engine_record_id,
            "first_pack_ordinal": volume.first_pack_ordinal,
            "pack_count": volume.pack_count,
            "pack_files": volume.pack_files,
            "archive_final": volume.archive_final,
            "archive_seal_sha256": volume.archive_seal_sha256
        })).collect::<Vec<_>>()
    });
    let engine_artifact = persist_json_artifact(evidence_directory, "engine-write", &engine_write)
        .map_err(|error| ExerciseFailure::failed(error.to_string()))?;
    append_json_artifact(pool, claim, "engine_write", &engine_artifact, engine_write).await
}

async fn verify_source_stability(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    cancellation: &CancellationToken,
    evidence_directory: &Path,
    source_manifest: &ManifestArtifactSummary,
) -> Result<(), ExerciseFailure> {
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
        warn!(path = %source_after.storage_path.display(), %error, "could not remove temporary Native v2 source-stability manifest");
    }
    if !stable {
        return Err(ExerciseFailure::failed(
            "the supposedly immutable source changed during the Native v2 backup; no recovery point can be committed",
        ));
    }
    Ok(())
}

async fn finalize_native_v2_backup(
    pool: &PgPool,
    claim: &store::ClaimedSingleTapeDataRun,
    evidence_directory: &Path,
    completed: CompletedNativeV2Backup,
    cleanup: Value,
) -> Result<()> {
    let first = completed
        .volumes
        .first()
        .ok_or_else(|| anyhow::anyhow!("Native v2 archive completed without a volume"))?;
    let last = completed
        .volumes
        .last()
        .expect("first volume established a non-empty list");
    let report = json!({
        "scope": "attended bounded-spool multi-volume full-file backup with complete per-volume physical readback",
        "engine": {"name": "TapeScale Native", "version": "2"},
        "format_version": NATIVE_ROLLING_FORMAT_NAME,
        "native_layout_version": 2,
        "archive_id": claim.run.archive_id,
        "archive_codec": "dar",
        "dar_version": crate::native_archive::DAR_CODEC_VERSION,
        "compression": completed.compression,
        "pack_count": completed.pack_count,
        "encoded_bytes": completed.total_encoded_bytes,
        "source_manifest_digest": completed.source_manifest.manifest_digest,
        "source_manifest_sha256": completed.source_manifest.artifact_sha256,
        "archive_set_sha256": completed.seal.archive_set_sha256,
        "pack_catalog_root_sha256": completed.seal.pack_catalog_root_sha256,
        "ordered_pack_sha256": completed.seal.ordered_pack_sha256,
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
            "pack_count": volume.pack_count,
            "archive_final": volume.archive_final,
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
            "archive_codec": "dar",
            "native_layout_version": 2,
            "archive_set_sha256": completed.seal.archive_set_sha256
        }),
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.detail))?;
    let pass = store::SingleTapePassEvidence {
        engine_operation_id: format!("tapescale-native-v2:{}", claim.run.archive_id),
        engine_volume_name: first.volume_name.clone(),
        engine_record_id: first.engine_record_id.clone(),
        first_record: first.first_record.clone(),
        last_record: Some(last.last_record.clone()),
        logical_bytes: completed.source_manifest.logical_bytes,
        encoded_bytes: completed.total_encoded_bytes,
        file_count: completed.source_manifest.file_count,
        coverage: serde_json::to_value(&completed.source_manifest.coverage)?,
        full_readback_evidence: json!({
            "physical_media_readback": true,
            "archive_id": claim.run.archive_id,
            "archive_codec": "dar",
            "native_layout_version": 2,
            "archive_set_sha256": completed.seal.archive_set_sha256,
            "volume_count": completed.volumes.len(),
            "pack_count": completed.pack_count,
            "volumes": completed.volumes.iter().map(|volume| json!({
                "volume_order": volume.volume_order,
                "archive_volume_id": volume.archive_volume_id,
                "header_sha256": volume.header_sha256,
                "index_sha256": volume.index_sha256,
                "pack_count": volume.pack_count
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
                archive_set_sha256 = %completed.seal.archive_set_sha256,
                "TapeScale Native v2 backup created a sealed recovery point"
            );
            Ok(())
        }
        store::FinalizeSingleTapeDataResult::Rejected(reason) => {
            finish_or_quarantine(
                pool,
                claim,
                store::SingleTapeDataTerminalState::Failed,
                json!({
                    "detail": format!("final Native v2 backup evidence was rejected: {reason}"),
                    "media_write_started": true,
                    "position_uncertain": false,
                    "verified_recovery_point_created": false,
                    "archive_set_sha256": completed.seal.archive_set_sha256
                }),
            )
            .await
        }
        store::FinalizeSingleTapeDataResult::FencedOut => {
            let detail =
                "Native v2 backup finalization was fenced out after verified physical media work";
            let _ = store::quarantine_fenced_single_tape_write(pool, claim, detail).await;
            Err(anyhow::anyhow!(detail))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dar_current_file, native_tape_write_failure, native_v2_spool_telemetry,
        ready_queue_needs_refill, ready_queue_resume_bytes, BlockingDiscoveryCancellation,
        SpoolWindow,
    };
    use crate::native_archive::{DarPackSizing, NativeArchiveError};
    use std::{
        io,
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    #[test]
    fn dar_progress_is_source_relative() {
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
        assert!(dar_current_file(root, "Adding file to archive: /etc/shadow").is_none());
    }

    #[test]
    fn dropping_pack_hash_supervision_requests_blocking_cancellation() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = BlockingDiscoveryCancellation {
            cancelled: Arc::clone(&cancelled),
        };
        assert!(!cancelled.load(Ordering::Acquire));
        drop(guard);
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn ready_queue_uses_a_real_startup_and_refill_window() {
        let gib = 1024_u64 * 1024 * 1024;
        let spool = SpoolWindow {
            capacity_bytes: 900 * gib,
            low_water_bytes: 32 * gib,
            ready_bytes: 128 * gib,
            high_water_bytes: 512 * gib,
        };
        let sizing = DarPackSizing::new(64 * gib, 384 * gib).expect("valid sizing");
        assert_eq!(
            ready_queue_resume_bytes(spool, sizing).expect("hysteresis window"),
            128 * gib
        );
        assert!(ready_queue_needs_refill(31 * gib, 32 * gib, false));
        assert!(!ready_queue_needs_refill(32 * gib, 32 * gib, false));
        assert!(!ready_queue_needs_refill(0, 32 * gib, true));
    }

    #[test]
    fn ready_queue_rejects_watermarks_without_hysteresis() {
        let gib = 1024_u64 * 1024 * 1024;
        let spool = SpoolWindow {
            capacity_bytes: 700 * gib,
            low_water_bytes: 32 * gib,
            ready_bytes: 128 * gib,
            high_water_bytes: 511 * gib,
        };
        let sizing = DarPackSizing::new(64 * gib, 384 * gib).expect("valid sizing");
        assert!(ready_queue_resume_bytes(spool, sizing).is_err());
    }

    #[test]
    fn spool_telemetry_counts_only_ready_packs_and_reclaimed_payload() {
        let telemetry = native_v2_spool_telemetry([
            ("staged", "present", 100),
            ("assigned", "present", 200),
            ("written", "reclaimed", 300),
            ("verified", "reclaimed", 400),
        ])
        .expect("valid ledger");
        assert_eq!(telemetry, (1, 700));
        assert!(native_v2_spool_telemetry([("staged", "present", -1)]).is_err());
    }

    #[test]
    fn tape_enospc_is_reported_as_physical_end_of_media() {
        let failure = native_tape_write_failure(
            NativeArchiveError::Io(io::Error::from_raw_os_error(28)),
            "pack-00000006.slice-000001",
            212_370_157_722,
            273_738_824_267,
        );
        assert_eq!(failure.code, Some("tape_capacity_exhausted"));
        assert!(failure
            .detail
            .contains("physical tape reached end of media"));
        assert!(failure.detail.contains("212370157722 of 273738824267"));
    }
}
