//! DAR-backed staging and physical I/O primitives for TapeScale Native archives.
//!
//! DAR is used only as a file-archive codec. TapeScale owns cartridge identity,
//! tape-file layout, volume ordering, hashing, readback, and recovery evidence.

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::native_format::{NativeArchiveRepresentation, NativeCompressionCodec};

pub const NATIVE_ENGINE_NAME: &str = "TapeScale Native";
pub const NATIVE_ENGINE_VERSION: &str = "1";
pub const NATIVE_ADAPTER_VERSION: &str = "native-v1";
pub const NATIVE_FORMAT_NAME: &str = "tapescale-native-v1";
pub const DAR_CODEC_VERSION: &str = "2.8.6";
pub const TAPE_RECORD_BYTES: usize = 256 * 1024;
const MIN_SLICE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SLICE_BYTES: u64 = 1024 * 1024 * 1024 * 1024;

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

#[must_use]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
    #[error("native archive I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

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
