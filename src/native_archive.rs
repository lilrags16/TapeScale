//! DAR-backed staging and physical I/O primitives for TapeScale Native archives.
//!
//! DAR is used only as a file-archive codec. TapeScale owns cartridge identity,
//! tape-file layout, volume ordering, hashing, readback, and recovery evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::native_format::{NativeArchiveRepresentation, NativeCompressionCodec};

pub const NATIVE_ENGINE_NAME: &str = "TapeScale Native";
pub const NATIVE_ENGINE_VERSION: &str = "2";
pub const NATIVE_ADAPTER_VERSION: &str = "native-v2";
pub const NATIVE_FORMAT_NAME: &str = "tapescale-native-v2";
pub const NATIVE_ROLLING_FORMAT_NAME: &str = NATIVE_FORMAT_NAME;
pub const NATIVE_V1_ENGINE_VERSION: &str = "1";
pub const NATIVE_V1_ADAPTER_VERSION: &str = "native-v1";
pub const NATIVE_V1_FORMAT_NAME: &str = "tapescale-native-v1";
pub const DAR_CODEC_VERSION: &str = "2.8.6";
pub const TAPE_RECORD_BYTES: usize = 256 * 1024;
const MIN_SLICE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SLICE_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
pub const MIN_NATIVE_PACK_TARGET_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const DEFAULT_NATIVE_PACK_TARGET_BYTES: u64 = 256 * 1024 * 1024 * 1024;
pub const DEFAULT_NATIVE_PACK_MAX_BYTES: u64 = 384 * 1024 * 1024 * 1024;
pub const MAX_NATIVE_PACK_TARGET_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const NATIVE_PACK_ENTRY_HEADROOM_BYTES: u64 = 4 * 1024;

#[must_use]
pub fn is_native_format(value: &str) -> bool {
    value == NATIVE_V1_FORMAT_NAME || value == NATIVE_ROLLING_FORMAT_NAME
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DarCompression {
    None,
    ZstdFast,
    Lz4Fast,
    GzipBalanced,
}

impl DarCompression {
    #[must_use]
    pub fn from_resolved(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "none" => Self::None,
            "gzip" | "gzip6" => Self::GzipBalanced,
            "lz4" | "lzo" => Self::Lz4Fast,
            _ => Self::ZstdFast,
        }
    }

    #[must_use]
    pub const fn native_codec(self) -> NativeCompressionCodec {
        match self {
            Self::None => NativeCompressionCodec::None,
            Self::ZstdFast => NativeCompressionCodec::Zstd,
            Self::Lz4Fast => NativeCompressionCodec::Lz4,
            Self::GzipBalanced => NativeCompressionCodec::Gzip,
        }
    }

    const fn dar_argument(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ZstdFast => Some("zstd:1:1M"),
            Self::Lz4Fast => Some("lz4:1:1M"),
            Self::GzipBalanced => Some("gzip:6:1M"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DarCreateInvocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub archive_base: PathBuf,
    pub catalog_base: PathBuf,
}

impl DarCreateInvocation {
    pub fn new(
        program: &Path,
        source: &Path,
        stage_directory: &Path,
        slice_bytes: u64,
        compression: DarCompression,
        compression_threads: u16,
    ) -> Result<Self, NativeArchiveError> {
        if !program.is_absolute() || !source.is_absolute() || !stage_directory.is_absolute() {
            return Err(NativeArchiveError::Invalid(
                "DAR, source, and staging paths must be absolute".to_owned(),
            ));
        }
        if !(MIN_SLICE_BYTES..=MAX_SLICE_BYTES).contains(&slice_bytes) {
            return Err(NativeArchiveError::Invalid(
                "DAR slice size must be from 64 MiB through 1 TiB".to_owned(),
            ));
        }
        if !(1..=64).contains(&compression_threads) {
            return Err(NativeArchiveError::Invalid(
                "DAR compression thread count must be from 1 through 64".to_owned(),
            ));
        }
        let archive_base = stage_directory.join("archive");
        let catalog_base = stage_directory.join("catalog");
        let mut args = vec![
            OsString::from("-N"),
            OsString::from("-Q"),
            OsString::from("-w"),
            OsString::from("-c"),
            archive_base.as_os_str().to_owned(),
            OsString::from("-R"),
            source.as_os_str().to_owned(),
            OsString::from("-s"),
            OsString::from(slice_bytes.to_string()),
            OsString::from("--min-digits"),
            OsString::from("6"),
            OsString::from("--on-fly-isolate"),
            catalog_base.as_os_str().to_owned(),
            OsString::from("--verbose=treated"),
        ];
        if let Some(compression) = compression.dar_argument() {
            args.push(OsString::from(format!("--compression={compression}")));
            args.push(OsString::from("--multi-thread"));
            args.push(OsString::from(compression_threads.to_string()));
        }
        Ok(Self {
            program: program.to_owned(),
            args,
            archive_base,
            catalog_base,
        })
    }
}

/// Bounded sizing policy for one independently finalized DAR pack.
///
/// `target_bytes` is the normal greedy packing boundary. A single allocation
/// group may exceed that target, but it may never exceed `maximum_bytes`. The
/// separate hard maximum lets the rolling-spool supervisor reserve enough
/// room for both a completed pack and the next pack being produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct DarPackSizing {
    pub target_bytes: u64,
    pub maximum_bytes: u64,
}

impl DarPackSizing {
    pub fn new(target_bytes: u64, maximum_bytes: u64) -> Result<Self, NativeArchiveError> {
        if !(MIN_NATIVE_PACK_TARGET_BYTES..=MAX_NATIVE_PACK_TARGET_BYTES).contains(&target_bytes) {
            return Err(NativeArchiveError::Invalid(format!(
                "native pack target must be from {} through {} bytes",
                MIN_NATIVE_PACK_TARGET_BYTES, MAX_NATIVE_PACK_TARGET_BYTES
            )));
        }
        if maximum_bytes < target_bytes || maximum_bytes > MAX_NATIVE_PACK_TARGET_BYTES {
            return Err(NativeArchiveError::Invalid(format!(
                "native pack maximum must be at least the target and at most {} bytes",
                MAX_NATIVE_PACK_TARGET_BYTES
            )));
        }
        Ok(Self {
            target_bytes,
            maximum_bytes,
        })
    }
}

impl Default for DarPackSizing {
    fn default() -> Self {
        Self {
            target_bytes: DEFAULT_NATIVE_PACK_TARGET_BYTES,
            maximum_bytes: DEFAULT_NATIVE_PACK_MAX_BYTES,
        }
    }
}

/// One authenticated-manifest entry presented to the deterministic pack
/// planner. Entries with the same non-empty allocation group remain in the
/// same pack; callers use this for hard-link closure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DarPackInput {
    pub path: String,
    pub logical_bytes: u64,
    pub allocation_group: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlannedDarPack {
    pub pack_ordinal: u32,
    pub entries: Vec<String>,
    pub source_entry_count: u64,
    pub logical_bytes: u64,
    pub estimated_bytes: u64,
}

#[derive(Debug)]
struct PackAllocationUnit {
    paths: Vec<String>,
    payload_bytes: u64,
    logical_bytes: u64,
}

/// Partition a strictly sorted manifest into deterministic, bounded packs.
///
/// The estimate intentionally includes per-entry headroom. Hard-linked paths
/// share one allocation group and count the largest member payload once,
/// matching the fact that DAR stores one inode payload with multiple names.
pub fn plan_dar_packs(
    inputs: &[DarPackInput],
    sizing: DarPackSizing,
) -> Result<Vec<PlannedDarPack>, NativeArchiveError> {
    DarPackSizing::new(sizing.target_bytes, sizing.maximum_bytes)?;
    if inputs.is_empty() {
        return Err(NativeArchiveError::Invalid(
            "native pack planning requires manifest entries".to_owned(),
        ));
    }

    let mut previous: Option<&str> = None;
    let mut group_bounds = BTreeMap::<String, (usize, usize)>::new();
    for (index, input) in inputs.iter().enumerate() {
        validate_pack_relative_path(&input.path)?;
        if previous.is_some_and(|value| value >= input.path.as_str()) {
            return Err(NativeArchiveError::Invalid(
                "native pack inputs must be unique and strictly path-sorted".to_owned(),
            ));
        }
        previous = Some(&input.path);
        if input
            .allocation_group
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.chars().any(char::is_control))
        {
            return Err(NativeArchiveError::Invalid(
                "native pack allocation group is invalid".to_owned(),
            ));
        }

        if let Some(group) = &input.allocation_group {
            group_bounds
                .entry(group.clone())
                .and_modify(|bounds| bounds.1 = index)
                .or_insert((index, index));
        }
    }

    // Build hard-link-closed intervals over the original manifest order.
    // Including every intervening entry preserves truthful contiguous
    // `manifest_entry_start/count` evidence while ensuring that all names for
    // one inode remain in one independently recoverable DAR pack.
    let mut units = Vec::<PackAllocationUnit>::new();
    let mut start = 0_usize;
    while start < inputs.len() {
        let mut end = inputs[start]
            .allocation_group
            .as_ref()
            .and_then(|group| group_bounds.get(group))
            .map_or(start, |bounds| bounds.1);
        let mut cursor = start;
        while cursor <= end {
            if let Some(group) = &inputs[cursor].allocation_group {
                end = end.max(group_bounds[group].1);
            }
            cursor += 1;
        }

        let range = &inputs[start..=end];
        let mut grouped_payloads = BTreeMap::<&str, u64>::new();
        let mut payload_bytes = 0_u64;
        let mut logical_bytes = 0_u64;
        let mut paths = Vec::with_capacity(range.len());
        for input in range {
            paths.push(input.path.clone());
            logical_bytes = logical_bytes
                .checked_add(input.logical_bytes)
                .ok_or_else(|| {
                    NativeArchiveError::Invalid(
                        "native pack logical byte total overflowed".to_owned(),
                    )
                })?;
            if let Some(group) = input.allocation_group.as_deref() {
                grouped_payloads
                    .entry(group)
                    .and_modify(|bytes| *bytes = (*bytes).max(input.logical_bytes))
                    .or_insert(input.logical_bytes);
            } else {
                payload_bytes =
                    payload_bytes
                        .checked_add(input.logical_bytes)
                        .ok_or_else(|| {
                            NativeArchiveError::Invalid(
                                "native pack byte estimate overflowed".to_owned(),
                            )
                        })?;
            }
        }
        payload_bytes = grouped_payloads
            .values()
            .try_fold(payload_bytes, |total, bytes| {
                total.checked_add(*bytes).ok_or_else(|| {
                    NativeArchiveError::Invalid("native pack byte estimate overflowed".to_owned())
                })
            })?;
        units.push(PackAllocationUnit {
            paths,
            payload_bytes,
            logical_bytes,
        });
        start = end + 1;
    }

    let mut planned = Vec::new();
    let mut current_paths = Vec::new();
    let mut current_bytes = 0_u64;
    let mut current_source_entries = 0_u64;
    let mut current_logical_bytes = 0_u64;
    for unit in units {
        let entry_headroom = u64::try_from(unit.paths.len())
            .ok()
            .and_then(|count| count.checked_mul(NATIVE_PACK_ENTRY_HEADROOM_BYTES))
            .ok_or_else(|| {
                NativeArchiveError::Invalid("native pack entry estimate overflowed".to_owned())
            })?;
        let unit_bytes = unit
            .payload_bytes
            .checked_add(entry_headroom)
            .ok_or_else(|| {
                NativeArchiveError::Invalid("native pack byte estimate overflowed".to_owned())
            })?;
        if unit_bytes > sizing.maximum_bytes {
            return Err(NativeArchiveError::Invalid(format!(
                "manifest allocation group beginning at {} needs {unit_bytes} bytes, exceeding the bounded pack maximum of {} bytes; native chunked-file records are required",
                unit.paths[0], sizing.maximum_bytes
            )));
        }
        let combined = current_bytes.checked_add(unit_bytes).ok_or_else(|| {
            NativeArchiveError::Invalid("native pack byte estimate overflowed".to_owned())
        })?;
        if !current_paths.is_empty() && combined > sizing.target_bytes {
            planned.push(build_planned_pack(
                planned.len(),
                std::mem::take(&mut current_paths),
                current_source_entries,
                current_logical_bytes,
                current_bytes,
            )?);
            current_bytes = 0;
            current_source_entries = 0;
            current_logical_bytes = 0;
        }
        current_source_entries = current_source_entries
            .checked_add(u64::try_from(unit.paths.len()).map_err(|_| {
                NativeArchiveError::Invalid("native pack has too many entries".to_owned())
            })?)
            .ok_or_else(|| {
                NativeArchiveError::Invalid("native pack entry count overflowed".to_owned())
            })?;
        current_logical_bytes = current_logical_bytes
            .checked_add(unit.logical_bytes)
            .ok_or_else(|| {
                NativeArchiveError::Invalid("native pack logical byte total overflowed".to_owned())
            })?;
        current_paths.extend(unit.paths);
        current_bytes = current_bytes.checked_add(unit_bytes).ok_or_else(|| {
            NativeArchiveError::Invalid("native pack byte estimate overflowed".to_owned())
        })?;
    }
    if !current_paths.is_empty() {
        planned.push(build_planned_pack(
            planned.len(),
            current_paths,
            current_source_entries,
            current_logical_bytes,
            current_bytes,
        )?);
    }
    Ok(planned)
}

fn build_planned_pack(
    zero_based_index: usize,
    entries: Vec<String>,
    source_entry_count: u64,
    logical_bytes: u64,
    estimated_bytes: u64,
) -> Result<PlannedDarPack, NativeArchiveError> {
    let entries = augment_dar_pack_entries(&entries)?;
    Ok(PlannedDarPack {
        pack_ordinal: u32::try_from(zero_based_index + 1)
            .map_err(|_| NativeArchiveError::Invalid("archive has too many packs".to_owned()))?,
        entries,
        source_entry_count,
        logical_bytes,
        estimated_bytes,
    })
}

/// Add the source root and every ancestor directory required for DAR to reach
/// exact `--include-from-file` selections. The returned list is canonical and
/// deduplicated; callers retain the original source-entry and logical-byte
/// counters separately so repeated directory scaffolding is never presented
/// as additional source data.
pub fn augment_dar_pack_entries(entries: &[String]) -> Result<Vec<String>, NativeArchiveError> {
    if entries.is_empty() {
        return Err(NativeArchiveError::Invalid(
            "DAR pack needs at least one source entry".to_owned(),
        ));
    }
    let mut augmented = BTreeSet::new();
    augmented.insert(".".to_owned());
    for entry in entries {
        validate_pack_relative_path(entry)?;
        augmented.insert(entry.clone());
        let mut parent = Path::new(entry).parent();
        while let Some(value) = parent {
            let text = value.to_str().ok_or_else(|| {
                NativeArchiveError::Invalid("DAR pack path must be UTF-8".to_owned())
            })?;
            if text.is_empty() || text == "." {
                break;
            }
            validate_pack_relative_path(text)?;
            augmented.insert(text.to_owned());
            parent = value.parent();
        }
    }
    Ok(augmented.into_iter().collect())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DarPackInclusionList {
    pub path: PathBuf,
    pub entry_count: u64,
    pub sha256: String,
}

/// Materialize an exact DAR inclusion list without shell parsing or one argv
/// item per source path. DAR 2.8's `--include-from-file` interprets each line
/// literally and, for a directory, includes only that directory entry.
pub fn write_dar_pack_inclusion_list(
    path: &Path,
    entries: &[String],
) -> Result<DarPackInclusionList, NativeArchiveError> {
    if !path.is_absolute() || entries.is_empty() {
        return Err(NativeArchiveError::Invalid(
            "DAR pack inclusion list needs an absolute path and at least one entry".to_owned(),
        ));
    }
    let entries = augment_dar_pack_entries(entries)?;
    let mut encoded = Vec::new();
    for entry in &entries {
        encoded.extend_from_slice(entry.as_bytes());
        encoded.push(b'\n');
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    Ok(DarPackInclusionList {
        path: path.to_owned(),
        entry_count: u64::try_from(entries.len()).map_err(|_| {
            NativeArchiveError::Invalid("DAR pack has too many inclusion entries".to_owned())
        })?,
        sha256: sha256_bytes(&encoded),
    })
}

#[derive(Clone, Debug)]
pub struct DarPackCreateInvocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub pack_ordinal: u32,
    pub archive_base: PathBuf,
    pub catalog_base: PathBuf,
    pub inclusion_list: PathBuf,
}

impl DarPackCreateInvocation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        program: &Path,
        source: &Path,
        pack_directory: &Path,
        inclusion_list: &Path,
        pack_ordinal: u32,
        slice_bytes: u64,
        compression: DarCompression,
        compression_threads: u16,
    ) -> Result<Self, NativeArchiveError> {
        if !program.is_absolute()
            || !source.is_absolute()
            || !pack_directory.is_absolute()
            || !inclusion_list.is_absolute()
            || !inclusion_list.starts_with(pack_directory)
        {
            return Err(NativeArchiveError::Invalid(
                "DAR, source, pack, and contained inclusion-list paths must be absolute".to_owned(),
            ));
        }
        if pack_ordinal == 0 {
            return Err(NativeArchiveError::Invalid(
                "DAR pack ordinal must be one-based".to_owned(),
            ));
        }
        if !(MIN_SLICE_BYTES..=MAX_SLICE_BYTES).contains(&slice_bytes) {
            return Err(NativeArchiveError::Invalid(
                "DAR slice size must be from 64 MiB through 1 TiB".to_owned(),
            ));
        }
        if !(1..=64).contains(&compression_threads) {
            return Err(NativeArchiveError::Invalid(
                "DAR compression thread count must be from 1 through 64".to_owned(),
            ));
        }
        let list_metadata = fs::symlink_metadata(inclusion_list)?;
        if !list_metadata.is_file() || list_metadata.file_type().is_symlink() {
            return Err(NativeArchiveError::Invalid(
                "DAR pack inclusion list must be a real regular file".to_owned(),
            ));
        }

        let archive_base = pack_directory.join("archive");
        let catalog_base = pack_directory.join("catalog");
        let mut args = vec![
            OsString::from("-N"),
            OsString::from("-Q"),
            OsString::from("-w"),
            OsString::from("-c"),
            archive_base.as_os_str().to_owned(),
            OsString::from("-R"),
            source.as_os_str().to_owned(),
            OsString::from("--include-from-file"),
            inclusion_list.as_os_str().to_owned(),
            OsString::from("-s"),
            OsString::from(slice_bytes.to_string()),
            OsString::from("--min-digits"),
            OsString::from("6"),
            OsString::from("--on-fly-isolate"),
            catalog_base.as_os_str().to_owned(),
            OsString::from("--verbose=treated"),
        ];
        if let Some(compression) = compression.dar_argument() {
            args.push(OsString::from(format!("--compression={compression}")));
            args.push(OsString::from("--multi-thread"));
            args.push(OsString::from(compression_threads.to_string()));
        }
        Ok(Self {
            program: program.to_owned(),
            args,
            pack_ordinal,
            archive_base,
            catalog_base,
            inclusion_list: inclusion_list.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RollingSpoolPlan {
    pub available_bytes: u64,
    pub reserve_bytes: u64,
    pub usable_bytes: u64,
    pub low_water_bytes: u64,
    pub high_water_bytes: u64,
    pub pack_target_bytes: u64,
    pub pack_maximum_bytes: u64,
    pub completed_pack_slots: u64,
}

/// Calculate bounded high/low watermarks. The high-water mark retains fifteen
/// percent of usable space for filesystem/DAR transients. The low-water mark
/// drives refill hysteresis: physical writing remains at a complete tape-file
/// boundary until the validated queue reaches high-water minus the reserved
/// maximum in-progress pack (or production is complete).
pub fn plan_rolling_spool(
    available_bytes: u64,
    reserve_bytes: u64,
    sizing: DarPackSizing,
) -> Result<RollingSpoolPlan, NativeArchiveError> {
    DarPackSizing::new(sizing.target_bytes, sizing.maximum_bytes)?;
    let usable_bytes = available_bytes
        .checked_sub(reserve_bytes)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            NativeArchiveError::Invalid(
                "native rolling spool reserve consumes all available space".to_owned(),
            )
        })?;
    let high_water_bytes = usable_bytes
        .checked_mul(17)
        .map(|value| value / 20)
        .ok_or_else(|| {
            NativeArchiveError::Invalid("native spool watermark overflowed".to_owned())
        })?;
    let low_water_bytes = usable_bytes / 4;
    let required_window = sizing
        .target_bytes
        .checked_add(sizing.maximum_bytes)
        .ok_or_else(|| NativeArchiveError::Invalid("native pack window overflowed".to_owned()))?;
    if high_water_bytes < required_window {
        return Err(NativeArchiveError::Invalid(format!(
            "rolling spool high-water capacity is {high_water_bytes} bytes but at least {required_window} bytes are required for one completed and one in-progress pack"
        )));
    }
    Ok(RollingSpoolPlan {
        available_bytes,
        reserve_bytes,
        usable_bytes,
        low_water_bytes,
        high_water_bytes,
        pack_target_bytes: sizing.target_bytes,
        pack_maximum_bytes: sizing.maximum_bytes,
        completed_pack_slots: high_water_bytes / sizing.target_bytes,
    })
}

#[derive(Clone, Debug)]
pub struct DarTestInvocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl DarTestInvocation {
    #[must_use]
    pub fn new(program: &Path, archive_base: &Path, compression_threads: u16) -> Self {
        Self {
            program: program.to_owned(),
            args: vec![
                OsString::from("-N"),
                OsString::from("-Q"),
                OsString::from("-t"),
                archive_base.as_os_str().to_owned(),
                OsString::from("--min-digits"),
                OsString::from("6"),
                OsString::from("--sequential-read"),
                OsString::from("--multi-thread"),
                OsString::from(compression_threads.to_string()),
                OsString::from("--verbose=treated"),
            ],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StagedSlice {
    pub ordinal: u32,
    pub path: PathBuf,
    pub encoded_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct StagedDarArchive {
    pub representation: NativeArchiveRepresentation,
    pub compression: DarCompression,
    pub archive_base: PathBuf,
    pub catalog_path: PathBuf,
    pub catalog_bytes: u64,
    pub catalog_sha256: String,
    pub slices: Vec<StagedSlice>,
    pub encoded_bytes: u64,
}

impl StagedDarArchive {
    pub fn discover(
        stage_directory: &Path,
        compression: DarCompression,
    ) -> Result<Self, NativeArchiveError> {
        let metadata = fs::symlink_metadata(stage_directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(NativeArchiveError::Invalid(
                "native archive staging root must be a real directory".to_owned(),
            ));
        }
        let mut slices = Vec::new();
        let mut catalogs = Vec::new();
        for entry in fs::read_dir(stage_directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(NativeArchiveError::Invalid(format!(
                    "unexpected non-file in native archive stage: {}",
                    path.display()
                )));
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                return Err(NativeArchiveError::Invalid(
                    "DAR produced a non-UTF-8 staging filename".to_owned(),
                ));
            };
            if let Some(ordinal) = dar_slice_ordinal(name, "archive") {
                if metadata.len() == 0 {
                    return Err(NativeArchiveError::Invalid(format!(
                        "DAR slice {ordinal} is empty"
                    )));
                }
                slices.push(StagedSlice {
                    ordinal,
                    encoded_bytes: metadata.len(),
                    sha256: sha256_file(&path)?,
                    path,
                });
            } else if dar_slice_ordinal(name, "catalog").is_some() {
                catalogs.push(path);
            } else {
                return Err(NativeArchiveError::Invalid(format!(
                    "DAR produced an unexpected staging file {name}"
                )));
            }
        }
        slices.sort_by_key(|slice| slice.ordinal);
        if slices.is_empty()
            || slices
                .iter()
                .enumerate()
                .any(|(index, slice)| slice.ordinal != u32::try_from(index + 1).unwrap_or(u32::MAX))
        {
            return Err(NativeArchiveError::Invalid(
                "DAR archive slices are missing or non-contiguous".to_owned(),
            ));
        }
        if catalogs.len() != 1 {
            return Err(NativeArchiveError::Invalid(format!(
                "DAR must produce exactly one isolated catalog slice; observed {}",
                catalogs.len()
            )));
        }
        let catalog_path = catalogs.remove(0);
        let catalog_bytes = fs::metadata(&catalog_path)?.len();
        if catalog_bytes == 0 {
            return Err(NativeArchiveError::Invalid(
                "DAR isolated catalog is empty".to_owned(),
            ));
        }
        let encoded_bytes = slices.iter().try_fold(0_u64, |total, slice| {
            total.checked_add(slice.encoded_bytes).ok_or_else(|| {
                NativeArchiveError::Invalid("staged archive byte total overflowed".to_owned())
            })
        })?;
        Ok(Self {
            representation: NativeArchiveRepresentation::FileDar,
            compression,
            archive_base: stage_directory.join("archive"),
            catalog_sha256: sha256_file(&catalog_path)?,
            catalog_path,
            catalog_bytes,
            slices,
            encoded_bytes,
        })
    }
}

/// One independently finalized and tested pack in a bounded native archive
/// set. Slice ordinals are local to this pack; the pack ordinal supplies the
/// global recovery ordering.
#[derive(Clone, Debug)]
pub struct StagedDarPack {
    pub pack_ordinal: u32,
    pub representation: NativeArchiveRepresentation,
    pub compression: DarCompression,
    pub archive_base: PathBuf,
    pub inclusion_list_path: PathBuf,
    pub inclusion_list_sha256: String,
    pub catalog_path: PathBuf,
    pub catalog_bytes: u64,
    pub catalog_sha256: String,
    pub slices: Vec<StagedSlice>,
    pub encoded_bytes: u64,
    pub pack_sha256: String,
}

/// Lock-free progress published while a completed DAR pack is discovered and
/// content-addressed. Pack slices can be hundreds of GiB, so callers must be
/// able to renew durable leases and present truthful progress while hashing is
/// running on a blocking worker thread.
#[derive(Debug, Default)]
pub struct StagedDarPackDiscoveryProgress {
    bytes_hashed: AtomicU64,
    total_bytes: AtomicU64,
    files_hashed: AtomicU64,
    files_total: AtomicU64,
    current_slice_ordinal: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct StagedDarPackDiscoverySnapshot {
    pub bytes_hashed: u64,
    pub total_bytes: u64,
    pub files_hashed: u64,
    pub files_total: u64,
    pub current_slice_ordinal: Option<u32>,
}

impl StagedDarPackDiscoveryProgress {
    #[must_use]
    pub fn snapshot(&self) -> StagedDarPackDiscoverySnapshot {
        let current_slice = self.current_slice_ordinal.load(Ordering::Acquire);
        StagedDarPackDiscoverySnapshot {
            bytes_hashed: self.bytes_hashed.load(Ordering::Acquire),
            total_bytes: self.total_bytes.load(Ordering::Acquire),
            files_hashed: self.files_hashed.load(Ordering::Acquire),
            files_total: self.files_total.load(Ordering::Acquire),
            current_slice_ordinal: u32::try_from(current_slice)
                .ok()
                .filter(|ordinal| *ordinal != 0),
        }
    }

    fn begin(&self, total_bytes: u64, files_total: u64) {
        self.bytes_hashed.store(0, Ordering::Release);
        self.files_hashed.store(0, Ordering::Release);
        self.current_slice_ordinal.store(0, Ordering::Release);
        self.total_bytes.store(total_bytes, Ordering::Release);
        self.files_total.store(files_total, Ordering::Release);
    }

    fn start_file(&self, current_slice_ordinal: Option<u32>) {
        self.current_slice_ordinal.store(
            u64::from(current_slice_ordinal.unwrap_or(0)),
            Ordering::Release,
        );
    }

    fn add_bytes(&self, bytes: u64) {
        self.bytes_hashed.fetch_add(bytes, Ordering::Release);
    }

    fn finish_file(&self) {
        self.files_hashed.fetch_add(1, Ordering::Release);
    }

    fn finish(&self) {
        self.current_slice_ordinal.store(0, Ordering::Release);
    }
}

impl StagedDarPack {
    pub fn discover(
        pack_directory: &Path,
        inclusion_list_path: &Path,
        pack_ordinal: u32,
        compression: DarCompression,
    ) -> Result<Self, NativeArchiveError> {
        let cancellation = AtomicBool::new(false);
        let progress = StagedDarPackDiscoveryProgress::default();
        Self::discover_with_cancel_and_progress(
            pack_directory,
            inclusion_list_path,
            pack_ordinal,
            compression,
            &cancellation,
            &progress,
        )
    }

    pub fn discover_with_cancel_and_progress(
        pack_directory: &Path,
        inclusion_list_path: &Path,
        pack_ordinal: u32,
        compression: DarCompression,
        cancellation: &AtomicBool,
        progress: &StagedDarPackDiscoveryProgress,
    ) -> Result<Self, NativeArchiveError> {
        if cancellation.load(Ordering::Acquire) {
            return Err(NativeArchiveError::Cancelled);
        }
        if pack_ordinal == 0 {
            return Err(NativeArchiveError::Invalid(
                "DAR pack ordinal must be one-based".to_owned(),
            ));
        }
        let metadata = fs::symlink_metadata(pack_directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(NativeArchiveError::Invalid(
                "native DAR pack root must be a real directory".to_owned(),
            ));
        }
        if !inclusion_list_path.starts_with(pack_directory) {
            return Err(NativeArchiveError::Invalid(
                "native DAR pack inclusion list escaped its pack directory".to_owned(),
            ));
        }
        let inclusion_metadata = fs::symlink_metadata(inclusion_list_path)?;
        if !inclusion_metadata.is_file() || inclusion_metadata.file_type().is_symlink() {
            return Err(NativeArchiveError::Invalid(
                "native DAR pack inclusion list must be a real regular file".to_owned(),
            ));
        }

        let mut slices = Vec::<(u32, PathBuf, u64)>::new();
        let mut catalogs = Vec::new();
        for entry in fs::read_dir(pack_directory)? {
            if cancellation.load(Ordering::Acquire) {
                return Err(NativeArchiveError::Cancelled);
            }
            let entry = entry?;
            let path = entry.path();
            let entry_metadata = fs::symlink_metadata(&path)?;
            if !entry_metadata.is_file() || entry_metadata.file_type().is_symlink() {
                return Err(NativeArchiveError::Invalid(format!(
                    "unexpected non-file in native DAR pack: {}",
                    path.display()
                )));
            }
            if path == inclusion_list_path {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                return Err(NativeArchiveError::Invalid(
                    "DAR produced a non-UTF-8 pack filename".to_owned(),
                ));
            };
            if let Some(ordinal) = dar_slice_ordinal(name, "archive") {
                if entry_metadata.len() == 0 {
                    return Err(NativeArchiveError::Invalid(format!(
                        "DAR pack slice {ordinal} is empty"
                    )));
                }
                slices.push((ordinal, path, entry_metadata.len()));
            } else if dar_slice_ordinal(name, "catalog").is_some() {
                catalogs.push((path, entry_metadata.len()));
            } else {
                return Err(NativeArchiveError::Invalid(format!(
                    "DAR produced an unexpected pack file {name}"
                )));
            }
        }
        slices.sort_by_key(|slice| slice.0);
        if slices.is_empty()
            || slices
                .iter()
                .enumerate()
                .any(|(index, slice)| slice.0 != u32::try_from(index + 1).unwrap_or(u32::MAX))
        {
            return Err(NativeArchiveError::Invalid(
                "DAR pack slices are missing or non-contiguous".to_owned(),
            ));
        }
        if catalogs.len() != 1 {
            return Err(NativeArchiveError::Invalid(format!(
                "DAR pack must contain exactly one isolated catalog slice; observed {}",
                catalogs.len()
            )));
        }
        let (catalog_path, catalog_bytes) = catalogs.remove(0);
        if catalog_bytes == 0 {
            return Err(NativeArchiveError::Invalid(
                "DAR pack isolated catalog is empty".to_owned(),
            ));
        }
        let encoded_bytes = slices.iter().try_fold(0_u64, |total, slice| {
            total.checked_add(slice.2).ok_or_else(|| {
                NativeArchiveError::Invalid("DAR pack byte total overflowed".to_owned())
            })
        })?;
        let total_bytes = encoded_bytes
            .checked_add(inclusion_metadata.len())
            .and_then(|total| total.checked_add(catalog_bytes))
            .ok_or_else(|| {
                NativeArchiveError::Invalid("DAR pack discovery byte total overflowed".to_owned())
            })?;
        let files_total = u64::try_from(slices.len())
            .ok()
            .and_then(|files| files.checked_add(2))
            .ok_or_else(|| {
                NativeArchiveError::Invalid("DAR pack discovery file total overflowed".to_owned())
            })?;
        progress.begin(total_bytes, files_total);

        // Directory enumeration order is filesystem-specific. Hash slice
        // payloads only after sorting their ordinals so progress and failure
        // evidence are deterministic across hosts.
        let mut discovered_slices = Vec::with_capacity(slices.len());
        for (ordinal, path, encoded_bytes) in slices {
            progress.start_file(Some(ordinal));
            let sha256 = sha256_file_with_cancel_and_progress(&path, cancellation, progress)?;
            progress.finish_file();
            discovered_slices.push(StagedSlice {
                ordinal,
                path,
                encoded_bytes,
                sha256,
            });
        }
        progress.start_file(None);
        let inclusion_list_sha256 =
            sha256_file_with_cancel_and_progress(inclusion_list_path, cancellation, progress)?;
        progress.finish_file();
        let catalog_sha256 =
            sha256_file_with_cancel_and_progress(&catalog_path, cancellation, progress)?;
        progress.finish_file();
        progress.finish();
        let pack_sha256 =
            dar_pack_content_sha256(pack_ordinal, &catalog_sha256, &discovered_slices)?;
        Ok(Self {
            pack_ordinal,
            representation: NativeArchiveRepresentation::FileDar,
            compression,
            archive_base: pack_directory.join("archive"),
            inclusion_list_path: inclusion_list_path.to_owned(),
            inclusion_list_sha256,
            catalog_path,
            catalog_bytes,
            catalog_sha256,
            slices: discovered_slices,
            encoded_bytes,
            pack_sha256,
        })
    }
}

/// Commitment to the exact catalog and payload tape files for one pack.
///
/// The DAR inclusion list is authenticated separately while staging, but is
/// intentionally excluded here because it is not a physical tape file. This
/// commitment can therefore be recomputed during database-independent
/// recovery using only the captured catalog and slices.
pub fn dar_pack_content_sha256(
    pack_ordinal: u32,
    catalog_sha256: &str,
    slices: &[StagedSlice],
) -> Result<String, NativeArchiveError> {
    if pack_ordinal == 0 || slices.is_empty() || !is_canonical_sha256(catalog_sha256) {
        return Err(NativeArchiveError::Invalid(
            "DAR pack commitment inputs are invalid".to_owned(),
        ));
    }
    let mut digest = Sha256::new();
    digest.update(b"tapescale-native-dar-pack-v2\0");
    digest.update(pack_ordinal.to_be_bytes());
    digest.update(catalog_sha256.as_bytes());
    for slice in slices {
        if !is_canonical_sha256(&slice.sha256) || slice.ordinal == 0 || slice.encoded_bytes == 0 {
            return Err(NativeArchiveError::Invalid(
                "DAR pack slice commitment is invalid".to_owned(),
            ));
        }
        digest.update(slice.ordinal.to_be_bytes());
        digest.update(slice.encoded_bytes.to_be_bytes());
        digest.update(slice.sha256.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlannedVolume {
    pub volume_ordinal: u32,
    pub first_slice_index: usize,
    pub slice_count: usize,
    pub encoded_bytes: u64,
}

pub fn plan_volumes(
    slices: &[StagedSlice],
    usable_bytes_per_volume: u64,
    spanning_enabled: bool,
) -> Result<Vec<PlannedVolume>, NativeArchiveError> {
    if slices.is_empty() || usable_bytes_per_volume == 0 {
        return Err(NativeArchiveError::Invalid(
            "volume planning requires staged slices and non-zero capacity".to_owned(),
        ));
    }
    let mut planned = Vec::new();
    let mut start = 0_usize;
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    for (index, slice) in slices.iter().enumerate() {
        if slice.encoded_bytes > usable_bytes_per_volume {
            return Err(NativeArchiveError::Invalid(format!(
                "DAR slice {} is larger than the usable tape capacity",
                slice.ordinal
            )));
        }
        let next = bytes.checked_add(slice.encoded_bytes).ok_or_else(|| {
            NativeArchiveError::Invalid("planned volume byte count overflowed".to_owned())
        })?;
        if count > 0 && next > usable_bytes_per_volume {
            planned.push(PlannedVolume {
                volume_ordinal: u32::try_from(planned.len() + 1).map_err(|_| {
                    NativeArchiveError::Invalid("archive uses too many tapes".to_owned())
                })?,
                first_slice_index: start,
                slice_count: count,
                encoded_bytes: bytes,
            });
            start = index;
            count = 0;
            bytes = 0;
        }
        count += 1;
        bytes = bytes.checked_add(slice.encoded_bytes).ok_or_else(|| {
            NativeArchiveError::Invalid("planned volume byte count overflowed".to_owned())
        })?;
    }
    planned.push(PlannedVolume {
        volume_ordinal: u32::try_from(planned.len() + 1)
            .map_err(|_| NativeArchiveError::Invalid("archive uses too many tapes".to_owned()))?,
        first_slice_index: start,
        slice_count: count,
        encoded_bytes: bytes,
    });
    if planned.len() > 1 && !spanning_enabled {
        return Err(NativeArchiveError::Invalid(format!(
            "the staged archive requires {} tapes but media spanning is disabled",
            planned.len()
        )));
    }
    Ok(planned)
}

#[must_use]
pub fn lto_native_capacity_bytes(
    generation: Option<i16>,
    media_format: Option<&str>,
) -> Option<u64> {
    let generation = generation.or_else(|| {
        let value = media_format?.to_ascii_uppercase();
        value
            .strip_prefix("LTO-")
            .and_then(|suffix| suffix.split_whitespace().next())
            .and_then(|suffix| suffix.trim_end_matches('M').parse().ok())
    })?;
    // LTO native capacities are specified in decimal GB/TB. GiB would
    // over-commit a cartridge before the safety reserve was even applied.
    let gb = 1_000_000_000_u64;
    match generation {
        1 => Some(100 * gb),
        2 => Some(200 * gb),
        3 => Some(400 * gb),
        4 => Some(800 * gb),
        5 => Some(1_500 * gb),
        6 => Some(2_500 * gb),
        7 => Some(6_000 * gb),
        8 if media_format.is_some_and(|value| value.to_ascii_uppercase().contains("TYPE M")) => {
            Some(9_000 * gb)
        }
        8 => Some(12_000 * gb),
        9 => Some(18_000 * gb),
        10 => Some(30_000 * gb),
        _ => None,
    }
}

pub fn conservative_usable_tape_bytes(
    native_capacity: u64,
    repeated_metadata_bytes: u64,
) -> Result<u64, NativeArchiveError> {
    let reserve = native_capacity / 20;
    native_capacity
        .checked_sub(reserve)
        .and_then(|value| value.checked_sub(repeated_metadata_bytes))
        .and_then(|value| value.checked_sub(64 * 1024 * 1024))
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            NativeArchiveError::Invalid(
                "tape capacity is smaller than metadata and safety reserves".to_owned(),
            )
        })
}

pub fn directory_regular_file_usage(directory: &Path) -> Result<(u64, u64), NativeArchiveError> {
    let mut bytes = 0_u64;
    let mut files = 0_u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(NativeArchiveError::Invalid(
                "native archive stage contains a non-regular file".to_owned(),
            ));
        }
        bytes = bytes.checked_add(metadata.len()).ok_or_else(|| {
            NativeArchiveError::Invalid("native stage byte count overflowed".to_owned())
        })?;
        files = files.checked_add(1).ok_or_else(|| {
            NativeArchiveError::Invalid("native stage file count overflowed".to_owned())
        })?;
    }
    Ok((bytes, files))
}

pub fn write_tape_bytes(
    device: &Path,
    bytes: &[u8],
    progress: &AtomicU64,
) -> Result<u64, NativeArchiveError> {
    write_tape_reader(device, io::Cursor::new(bytes), progress)
}

pub fn write_tape_path(
    device: &Path,
    path: &Path,
    progress: &AtomicU64,
) -> Result<u64, NativeArchiveError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(NativeArchiveError::Invalid(format!(
            "tape payload is not a regular file: {}",
            path.display()
        )));
    }
    write_tape_reader(device, File::open(path)?, progress)
}

fn write_tape_reader(
    device: &Path,
    mut reader: impl Read,
    progress: &AtomicU64,
) -> Result<u64, NativeArchiveError> {
    validate_non_rewinding_device_path(device)?;
    let mut tape = OpenOptions::new().write(true).open(device)?;
    let mut buffer = vec![0_u8; TAPE_RECORD_BYTES];
    let mut total = 0_u64;
    loop {
        let read = read_full_or_eof(&mut reader, &mut buffer)?;
        if read == 0 {
            break;
        }
        let written = tape.write(&buffer[..read])?;
        if written != read {
            return Err(NativeArchiveError::Io(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("tape accepted a partial record ({written} of {read} bytes)"),
            )));
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            NativeArchiveError::Invalid("tape write byte count overflowed".to_owned())
        })?;
        progress.store(total, Ordering::Release);
    }
    if total == 0 {
        return Err(NativeArchiveError::Invalid(
            "refusing to create an empty tape file".to_owned(),
        ));
    }
    tape.flush()?;
    drop(tape); // Closing /dev/nstN commits this tape file and its filemark.
    Ok(total)
}

pub fn open_tape_reader(device: &Path) -> Result<File, NativeArchiveError> {
    validate_non_rewinding_device_path(device)?;
    Ok(OpenOptions::new().read(true).open(device)?)
}

pub fn read_tape_file(tape: &mut File, maximum_bytes: u64) -> Result<Vec<u8>, NativeArchiveError> {
    read_tape_file_inner(tape, maximum_bytes, None)
}

pub fn read_tape_file_with_progress(
    tape: &mut File,
    maximum_bytes: u64,
    progress: &AtomicU64,
) -> Result<Vec<u8>, NativeArchiveError> {
    read_tape_file_inner(tape, maximum_bytes, Some(progress))
}

fn read_tape_file_inner(
    tape: &mut File,
    maximum_bytes: u64,
    progress: Option<&AtomicU64>,
) -> Result<Vec<u8>, NativeArchiveError> {
    let mut result = Vec::new();
    let mut buffer = vec![0_u8; TAPE_RECORD_BYTES];
    loop {
        let read = tape.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if (result.len() as u64).saturating_add(read as u64) > maximum_bytes {
            return Err(NativeArchiveError::Invalid(
                "tape file exceeded its declared readback bound".to_owned(),
            ));
        }
        result.extend_from_slice(&buffer[..read]);
        if let Some(progress) = progress {
            progress.fetch_add(read as u64, Ordering::Release);
        }
    }
    if result.is_empty() {
        return Err(NativeArchiveError::Invalid(
            "physical readback encountered an empty tape file".to_owned(),
        ));
    }
    Ok(result)
}

pub fn read_and_hash_tape_file(
    tape: &mut File,
    expected_bytes: u64,
) -> Result<String, NativeArchiveError> {
    read_and_hash_tape_file_inner(tape, expected_bytes, None)
}

pub fn read_and_hash_tape_file_with_progress(
    tape: &mut File,
    expected_bytes: u64,
    progress: &AtomicU64,
) -> Result<String, NativeArchiveError> {
    read_and_hash_tape_file_inner(tape, expected_bytes, Some(progress))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HashedTapeFile {
    pub encoded_bytes: u64,
    pub sha256: String,
}

/// Hash one complete tape file when only a safe upper bound is known.
///
/// Native v2 deliberately reclaims written pack files before the containing
/// cartridge is closed. Its descriptor retains the aggregate byte bound and
/// file count, so physical readback can rediscover each slice length at its
/// filemark and recompute the pack commitment without retaining a second disk
/// copy of the volume.
pub fn read_and_hash_bounded_tape_file_with_progress(
    tape: &mut File,
    maximum_bytes: u64,
    progress: &AtomicU64,
) -> Result<HashedTapeFile, NativeArchiveError> {
    if maximum_bytes == 0 {
        return Err(NativeArchiveError::Invalid(
            "tape-file readback bound must be non-zero".to_owned(),
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; TAPE_RECORD_BYTES];
    let mut total = 0_u64;
    loop {
        let read = tape.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            NativeArchiveError::Invalid("tape read byte count overflowed".to_owned())
        })?;
        if total > maximum_bytes {
            return Err(NativeArchiveError::Invalid(
                "physical tape file exceeded its authenticated pack bound".to_owned(),
            ));
        }
        digest.update(&buffer[..read]);
        progress.fetch_add(read as u64, Ordering::Release);
    }
    if total == 0 {
        return Err(NativeArchiveError::Invalid(
            "physical readback encountered an empty tape file".to_owned(),
        ));
    }
    Ok(HashedTapeFile {
        encoded_bytes: total,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn read_and_hash_tape_file_inner(
    tape: &mut File,
    expected_bytes: u64,
    progress: Option<&AtomicU64>,
) -> Result<String, NativeArchiveError> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; TAPE_RECORD_BYTES];
    let mut total = 0_u64;
    loop {
        let read = tape.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            NativeArchiveError::Invalid("tape read byte count overflowed".to_owned())
        })?;
        if total > expected_bytes {
            return Err(NativeArchiveError::Invalid(
                "physical tape file is larger than its staged source".to_owned(),
            ));
        }
        digest.update(&buffer[..read]);
        if let Some(progress) = progress {
            progress.fetch_add(read as u64, Ordering::Release);
        }
    }
    if total != expected_bytes {
        return Err(NativeArchiveError::Invalid(format!(
            "physical tape file length mismatch: expected {expected_bytes}, read {total}"
        )));
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn sha256_file(path: &Path) -> Result<String, NativeArchiveError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sha256_file_with_cancel_and_progress(
    path: &Path,
    cancellation: &AtomicBool,
    progress: &StagedDarPackDiscoveryProgress,
) -> Result<String, NativeArchiveError> {
    sha256_reader_with_cancel_and_progress(File::open(path)?, cancellation, progress)
}

fn sha256_reader_with_cancel_and_progress(
    mut reader: impl Read,
    cancellation: &AtomicBool,
    progress: &StagedDarPackDiscoveryProgress,
) -> Result<String, NativeArchiveError> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if cancellation.load(Ordering::Acquire) {
            return Err(NativeArchiveError::Cancelled);
        }
        let read = reader.read(&mut buffer)?;
        if cancellation.load(Ordering::Acquire) {
            return Err(NativeArchiveError::Cancelled);
        }
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        progress.add_bytes(read as u64);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[must_use]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_pack_relative_path(value: &str) -> Result<(), NativeArchiveError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(NativeArchiveError::Invalid(
            "DAR pack path must be a non-empty control-free relative path".to_owned(),
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(NativeArchiveError::Invalid(format!(
            "DAR pack path is not contained beneath the source root: {value}"
        )));
    }
    Ok(())
}

fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn dar_slice_ordinal(name: &str, basename: &str) -> Option<u32> {
    let number = name
        .strip_prefix(&format!("{basename}."))?
        .strip_suffix(".dar")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    number.parse::<u32>().ok().filter(|value| *value > 0)
}

fn read_full_or_eof(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 => break,
            count => filled += count,
        }
    }
    Ok(filled)
}

fn validate_non_rewinding_device_path(device: &Path) -> Result<(), NativeArchiveError> {
    let text = device
        .to_str()
        .ok_or_else(|| NativeArchiveError::Invalid("tape device path must be UTF-8".to_owned()))?;
    if !text.starts_with("/dev/nst")
        || text[8..].is_empty()
        || !text[8..].bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(NativeArchiveError::Invalid(
            "native tape I/O requires an exact /dev/nstN path".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum NativeArchiveError {
    #[error("native archive input is invalid: {0}")]
    Invalid(String),
    #[error("native archive operation was cancelled")]
    Cancelled,
    #[error("native archive I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    struct CancellingReader<'a> {
        cancellation: &'a AtomicBool,
        requested_bytes: &'a AtomicU64,
    }

    impl Read for CancellingReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.requested_bytes
                .store(buffer.len() as u64, Ordering::Release);
            buffer.fill(b'x');
            self.cancellation.store(true, Ordering::Release);
            Ok(buffer.len())
        }
    }

    fn slice(ordinal: u32, bytes: u64) -> StagedSlice {
        StagedSlice {
            ordinal,
            path: PathBuf::from(format!("/stage/archive.{ordinal:06}.dar")),
            encoded_bytes: bytes,
            sha256: "0".repeat(64),
        }
    }

    #[test]
    fn dar_invocation_is_closed_and_non_interactive() {
        let invocation = DarCreateInvocation::new(
            Path::new("/usr/bin/dar"),
            Path::new("/source/pool/.zfs/snapshot/one"),
            Path::new("/spool/run"),
            64 * 1024 * 1024,
            DarCompression::ZstdFast,
            4,
        )
        .expect("valid invocation");
        let args = invocation
            .args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.starts_with(&["-N".into(), "-Q".into(), "-w".into()]));
        assert!(args.contains(&"--on-fly-isolate".to_owned()));
        assert!(args.contains(&"--compression=zstd:1:1M".to_owned()));
        assert!(!args.iter().any(|value| value.contains(';')));
    }

    #[test]
    fn dar_pack_planning_is_bounded_and_keeps_allocation_groups_together() {
        let inputs = vec![
            DarPackInput {
                path: ".".to_owned(),
                logical_bytes: 0,
                allocation_group: None,
            },
            DarPackInput {
                path: "alpha.bin".to_owned(),
                logical_bytes: 200 * GIB,
                allocation_group: Some("hard-link-one".to_owned()),
            },
            DarPackInput {
                path: "beta.bin".to_owned(),
                logical_bytes: 200 * GIB,
                allocation_group: Some("hard-link-one".to_owned()),
            },
            DarPackInput {
                path: "charlie.bin".to_owned(),
                logical_bytes: 100 * GIB,
                allocation_group: None,
            },
            DarPackInput {
                path: "delta.bin".to_owned(),
                logical_bytes: 200 * GIB,
                allocation_group: None,
            },
        ];
        let first = plan_dar_packs(&inputs, DarPackSizing::default()).expect("bounded packs");
        let second = plan_dar_packs(&inputs, DarPackSizing::default()).expect("determinism");
        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        assert!(first[0].entries.contains(&"alpha.bin".to_owned()));
        assert!(first[0].entries.contains(&"beta.bin".to_owned()));
        assert_eq!(first[0].source_entry_count, 3);
        assert_eq!(first[0].logical_bytes, 400 * GIB);
        assert_eq!(first[0].pack_ordinal, 1);
        assert_eq!(first[2].pack_ordinal, 3);
        assert!(first
            .iter()
            .all(|pack| pack.entries.first().is_some_and(|path| path == ".")));

        assert_eq!(
            augment_dar_pack_entries(&["deep/tree/file.bin".to_owned()]).expect("ancestor closure"),
            vec![
                ".".to_owned(),
                "deep".to_owned(),
                "deep/tree".to_owned(),
                "deep/tree/file.bin".to_owned(),
            ]
        );

        let oversized = vec![DarPackInput {
            path: "oversized.bin".to_owned(),
            logical_bytes: DEFAULT_NATIVE_PACK_MAX_BYTES + 1,
            allocation_group: None,
        }];
        assert!(plan_dar_packs(&oversized, DarPackSizing::default())
            .expect_err("oversized file must require native chunking")
            .to_string()
            .contains("chunked-file"));
    }

    #[test]
    fn dar_pack_planning_keeps_nonadjacent_hardlinks_in_truthful_manifest_ranges() {
        let inputs = vec![
            DarPackInput {
                path: "a.bin".to_owned(),
                logical_bytes: 40 * GIB,
                allocation_group: Some("inode-7".to_owned()),
            },
            DarPackInput {
                path: "b.bin".to_owned(),
                logical_bytes: 40 * GIB,
                allocation_group: None,
            },
            DarPackInput {
                path: "c.bin".to_owned(),
                logical_bytes: 40 * GIB,
                allocation_group: Some("inode-7".to_owned()),
            },
            DarPackInput {
                path: "d.bin".to_owned(),
                logical_bytes: 40 * GIB,
                allocation_group: None,
            },
        ];
        let packs = plan_dar_packs(
            &inputs,
            DarPackSizing::new(64 * GIB, 96 * GIB).expect("test sizing"),
        )
        .expect("hard-link interval should fit");
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].source_entry_count, 3);
        assert!(packs[0].entries.iter().any(|entry| entry == "a.bin"));
        assert!(packs[0].entries.iter().any(|entry| entry == "b.bin"));
        assert!(packs[0].entries.iter().any(|entry| entry == "c.bin"));
        assert!(!packs[0].entries.iter().any(|entry| entry == "d.bin"));
        assert_eq!(packs[1].source_entry_count, 1);
        assert!(packs[1].entries.iter().any(|entry| entry == "d.bin"));
    }

    #[test]
    fn dar_pack_uses_one_contained_exact_inclusion_list() {
        let temporary = tempfile::tempdir().expect("temporary pack");
        let pack_directory = temporary.path().join("pack-000001");
        fs::create_dir(&pack_directory).expect("pack directory");
        let inclusion_path = pack_directory.join("include.list");
        let inclusion = write_dar_pack_inclusion_list(
            &inclusion_path,
            &[".".to_owned(), "dir".to_owned(), "dir/file.bin".to_owned()],
        )
        .expect("exact inclusion list");
        assert_eq!(inclusion.entry_count, 3);
        assert_eq!(inclusion.sha256.len(), 64);
        assert_eq!(
            fs::read_to_string(&inclusion_path).expect("read inclusion list"),
            ".\ndir\ndir/file.bin\n"
        );

        let invocation = DarPackCreateInvocation::new(
            Path::new("/usr/bin/dar"),
            Path::new("/source/snapshot"),
            &pack_directory,
            &inclusion_path,
            1,
            64 * 1024 * 1024,
            DarCompression::ZstdFast,
            4,
        )
        .expect("safe pack invocation");
        let args = invocation
            .args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let option = args
            .iter()
            .position(|value| value == "--include-from-file")
            .expect("include-list option");
        assert_eq!(
            args[option + 1],
            inclusion_path.to_string_lossy().into_owned()
        );
        assert!(!args.iter().any(|value| value == "dir/file.bin"));
        assert!(!args.iter().any(|value| value.contains(';')));

        assert!(write_dar_pack_inclusion_list(
            &pack_directory.join("unsafe.list"),
            &["../escape".to_owned()]
        )
        .is_err());
    }

    #[test]
    fn completed_dar_pack_has_one_canonical_commitment() {
        let temporary = tempfile::tempdir().expect("temporary pack");
        let pack_directory = temporary.path().join("pack-000007");
        fs::create_dir(&pack_directory).expect("pack directory");
        let inclusion_path = pack_directory.join("include.list");
        write_dar_pack_inclusion_list(&inclusion_path, &["file.bin".to_owned()])
            .expect("inclusion list");
        fs::write(pack_directory.join("archive.000002.dar"), b"second").expect("second slice");
        fs::write(pack_directory.join("archive.000001.dar"), b"first").expect("first slice");
        fs::write(pack_directory.join("catalog.000001.dar"), b"catalog").expect("catalog");
        let expected_hashed_bytes = [
            inclusion_path.clone(),
            pack_directory.join("archive.000001.dar"),
            pack_directory.join("archive.000002.dar"),
            pack_directory.join("catalog.000001.dar"),
        ]
        .iter()
        .map(|path| fs::metadata(path).expect("pack input metadata").len())
        .sum::<u64>();

        let cancellation = AtomicBool::new(false);
        let progress = StagedDarPackDiscoveryProgress::default();
        let pack = StagedDarPack::discover_with_cancel_and_progress(
            &pack_directory,
            &inclusion_path,
            7,
            DarCompression::ZstdFast,
            &cancellation,
            &progress,
        )
        .expect("completed pack");
        assert_eq!(pack.pack_ordinal, 7);
        assert_eq!(pack.slices.len(), 2);
        assert_eq!(
            pack.slices
                .iter()
                .map(|slice| slice.ordinal)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(pack.encoded_bytes, 11);
        assert_eq!(pack.pack_sha256.len(), 64);
        let snapshot = progress.snapshot();
        assert_eq!(snapshot.bytes_hashed, snapshot.total_bytes);
        assert_eq!(snapshot.total_bytes, expected_hashed_bytes);
        assert_eq!(snapshot.files_hashed, 4);
        assert_eq!(snapshot.files_total, 4);
        assert_eq!(snapshot.current_slice_ordinal, None);

        let compatibility = StagedDarPack::discover(
            &pack_directory,
            &inclusion_path,
            7,
            DarCompression::ZstdFast,
        )
        .expect("compatibility discovery");
        assert_eq!(compatibility.pack_sha256, pack.pack_sha256);
        assert_eq!(compatibility.slices, pack.slices);

        fs::write(pack_directory.join("unexpected"), b"nope").expect("unexpected file");
        assert!(StagedDarPack::discover(
            &pack_directory,
            &inclusion_path,
            7,
            DarCompression::ZstdFast,
        )
        .is_err());
    }

    #[test]
    fn pack_hashing_checks_cancellation_at_one_mebibyte_read_boundaries() {
        let cancellation = AtomicBool::new(false);
        let requested_bytes = AtomicU64::new(0);
        let progress = StagedDarPackDiscoveryProgress::default();
        progress.begin(2 * 1024 * 1024, 1);
        progress.start_file(Some(9));
        let error = sha256_reader_with_cancel_and_progress(
            CancellingReader {
                cancellation: &cancellation,
                requested_bytes: &requested_bytes,
            },
            &cancellation,
            &progress,
        )
        .expect_err("cancellation raised during a hash read must stop discovery");
        assert!(matches!(error, NativeArchiveError::Cancelled));
        assert_eq!(requested_bytes.load(Ordering::Acquire), 1024 * 1024);
        assert_eq!(
            progress.snapshot(),
            StagedDarPackDiscoverySnapshot {
                bytes_hashed: 0,
                total_bytes: 2 * 1024 * 1024,
                files_hashed: 0,
                files_total: 1,
                current_slice_ordinal: Some(9),
            }
        );
    }

    #[test]
    fn pack_discovery_honors_cancellation_before_filesystem_work() {
        let cancellation = AtomicBool::new(true);
        let progress = StagedDarPackDiscoveryProgress::default();
        let error = StagedDarPack::discover_with_cancel_and_progress(
            Path::new("/not/inspected"),
            Path::new("/not/inspected/include.list"),
            1,
            DarCompression::ZstdFast,
            &cancellation,
            &progress,
        )
        .expect_err("pre-cancelled discovery must stop before path inspection");
        assert!(matches!(error, NativeArchiveError::Cancelled));
        assert_eq!(
            progress.snapshot(),
            StagedDarPackDiscoverySnapshot::default()
        );
    }

    #[test]
    fn one_terabyte_spool_supports_a_three_pack_high_water_window() {
        let plan = plan_rolling_spool(1_000_000_000_000, 8 * GIB, DarPackSizing::default())
            .expect("one terabyte rolling spool");
        assert!((750 * GIB..=800 * GIB).contains(&plan.high_water_bytes));
        assert!((150 * GIB..=250 * GIB).contains(&plan.low_water_bytes));
        assert_eq!(plan.completed_pack_slots, 3);

        assert!(plan_rolling_spool(500 * GIB, 8 * GIB, DarPackSizing::default(),).is_err());
    }

    #[test]
    fn volume_planning_keeps_every_slice_whole_and_ordered() {
        let slices = vec![slice(1, 40), slice(2, 40), slice(3, 40), slice(4, 10)];
        let planned = plan_volumes(&slices, 90, true).expect("spanning plan");
        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].first_slice_index, 0);
        assert_eq!(planned[0].slice_count, 2);
        assert_eq!(planned[0].encoded_bytes, 80);
        assert_eq!(planned[1].first_slice_index, 2);
        assert_eq!(planned[1].slice_count, 2);
        assert_eq!(planned[1].encoded_bytes, 50);
        assert!(plan_volumes(&slices, 90, false).is_err());
    }

    #[test]
    fn lto_capacity_is_native_not_compressed_marketing_capacity() {
        let gb = 1_000_000_000_u64;
        assert_eq!(
            lto_native_capacity_bytes(Some(8), Some("LTO-8")),
            Some(12_000 * gb)
        );
        assert_eq!(
            lto_native_capacity_bytes(Some(8), Some("LTO-7 Type M")),
            Some(9_000 * gb)
        );
        assert_eq!(
            lto_native_capacity_bytes(Some(9), Some("LTO-9")),
            Some(18_000 * gb)
        );
    }

    #[test]
    fn tape_device_path_is_exactly_non_rewinding() {
        assert!(validate_non_rewinding_device_path(Path::new("/dev/nst0")).is_ok());
        for invalid in ["/dev/st0", "/dev/nst", "/dev/nst0/extra", "nst0"] {
            assert!(validate_non_rewinding_device_path(Path::new(invalid)).is_err());
        }
    }
}
