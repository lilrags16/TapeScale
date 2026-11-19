//! Closed, database-independent primitives for TapeScale native tape media.
//!
//! The public API deliberately accepts no free-form `mt` operation and no shell
//! command.  A caller must provide one of the exact configured non-rewinding
//! devices.  Archive orchestration, leases, and operator authorization remain
//! the responsibility of the worker.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    native_format::{
        NativePreparationHeader, NativeVolumeHeader, DEFAULT_TAPE_BLOCK_BYTES,
        NATIVE_FORMAT_VERSION,
    },
    tool_runner::{self, ToolCommand, ToolError, ToolOutput},
};

const TEST_FILE_COUNT: u32 = 2;
const TEST_RECORDS_PER_FILE: u32 = 16;
const METADATA_READ_LIMIT: usize = 16 * 1024 * 1024;
const MT_OUTPUT_LIMIT: usize = 16 * 1024;
const IDENTITY_PREFIX: &str = "TS-PREP-V1-";
const ARCHIVE_IDENTITY_PREFIX: &str = "TSN1-";

#[derive(Clone, Debug)]
pub struct NativeTapeConfig {
    /// Enables destructive qualification. Read-only identity probes remain
    /// available for allowlisted devices when this is false.
    pub enabled: bool,
    pub mt_program: PathBuf,
    pub allowed_devices: Vec<PathBuf>,
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeTapeOutcomeKind {
    Passed,
    Failed,
    Blocked,
    Interrupted,
}

#[derive(Clone, Debug, Serialize)]
pub struct NativeTapeOutcome {
    pub kind: NativeTapeOutcomeKind,
    pub position_uncertain: bool,
    pub evidence: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct NativeTapeIdentityOutcome {
    pub kind: NativeTapeOutcomeKind,
    pub position_uncertain: bool,
    pub volume_name: Option<String>,
    pub cartridge_id: Option<Uuid>,
    pub archive_id: Option<Uuid>,
    pub volume_ordinal: Option<u32>,
    pub evidence: Value,
}

#[derive(Clone, Copy, Debug)]
enum MtOperation {
    Rewind,
    VariableBlock,
}

impl MtOperation {
    fn args(self, device: &Path) -> Vec<String> {
        let mut args = vec!["-f".to_owned(), device.display().to_string()];
        match self {
            Self::Rewind => args.push("rewind".to_owned()),
            Self::VariableBlock => {
                args.push("setblk".to_owned());
                args.push("0".to_owned());
            }
        }
        args
    }
}

#[must_use]
pub fn preparation_volume_identity(cartridge_id: Uuid) -> String {
    format!("{IDENTITY_PREFIX}{cartridge_id}")
}

#[must_use]
pub fn preparation_identity_cartridge_id(identity: &str) -> Option<Uuid> {
    let encoded = identity.strip_prefix(IDENTITY_PREFIX)?;
    let cartridge_id = Uuid::parse_str(encoded).ok()?;
    (preparation_volume_identity(cartridge_id) == identity).then_some(cartridge_id)
}

#[must_use]
pub fn archive_volume_identity(archive_id: Uuid, volume_ordinal: u32) -> String {
    format!("{ARCHIVE_IDENTITY_PREFIX}{archive_id}-V{volume_ordinal:04}")
}

impl NativeTapeConfig {
    /// Destructively exercise multiple physical records and tape files, then
    /// replace the test payload with the cartridge's stable preparation record.
    pub async fn qualify_and_prepare(
        &self,
        run_id: Uuid,
        device: &Path,
        cartridge_id: Uuid,
        drive_stable_id: &str,
        cancellation: &CancellationToken,
    ) -> NativeTapeOutcome {
        if let Err(detail) = self.validate_destructive_access(device) {
            return blocked(detail);
        }
        let deadline = Instant::now() + self.timeout;
        let operation = self
            .qualify_and_prepare_inner(
                run_id,
                device,
                cartridge_id,
                drive_stable_id,
                deadline,
                cancellation,
            )
            .await;
        match operation {
            Ok(evidence) => NativeTapeOutcome {
                kind: NativeTapeOutcomeKind::Passed,
                position_uncertain: false,
                evidence,
            },
            Err(NativeTapeError::Blocked(detail)) => blocked(detail),
            Err(NativeTapeError::Interrupted(detail)) => NativeTapeOutcome {
                kind: NativeTapeOutcomeKind::Interrupted,
                position_uncertain: true,
                evidence: json!({
                    "test": "native_tape_test",
                    "detail": detail,
                    "command_dispatched": true,
                    "position_uncertain": true
                }),
            },
            Err(NativeTapeError::Failed(detail)) => NativeTapeOutcome {
                kind: NativeTapeOutcomeKind::Failed,
                position_uncertain: true,
                evidence: json!({
                    "test": "native_tape_test",
                    "detail": detail,
                    "command_dispatched": true,
                    "position_uncertain": true
                }),
            },
        }
    }

    async fn qualify_and_prepare_inner(
        &self,
        run_id: Uuid,
        device: &Path,
        cartridge_id: Uuid,
        drive_stable_id: &str,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<Value, NativeTapeError> {
        self.check(deadline, cancellation, "before native tape qualification")?;
        let variable_block = self
            .mt(device, MtOperation::VariableBlock, cancellation)
            .await?;
        let initial_rewind = self.mt(device, MtOperation::Rewind, cancellation).await?;

        let device_owned = device.to_owned();
        let cancel = cancellation.clone();
        let written =
            tokio::task::spawn_blocking(move || write_test_files(&device_owned, run_id, &cancel))
                .await
                .map_err(|error| {
                    NativeTapeError::Failed(format!("native tape writer failed: {error}"))
                })??;

        self.check(deadline, cancellation, "after native test writes")?;
        let readback_rewind = self.mt(device, MtOperation::Rewind, cancellation).await?;
        let device_owned = device.to_owned();
        let cancel = cancellation.clone();
        let read =
            tokio::task::spawn_blocking(move || read_test_files(&device_owned, run_id, &cancel))
                .await
                .map_err(|error| {
                    NativeTapeError::Failed(format!("native tape reader failed: {error}"))
                })??;
        if written != read {
            return Err(NativeTapeError::Failed(
                "native tape readback did not match every written record and file boundary"
                    .to_owned(),
            ));
        }

        let qualification_sha256 = digest_test_files(&written);
        let drive_identity_sha256 = format!("{:x}", Sha256::digest(drive_stable_id.as_bytes()));
        let preparation = NativePreparationHeader {
            format_version: NATIVE_FORMAT_VERSION,
            cartridge_id,
            prepared_at: Utc::now(),
            qualification_sha256: Some(qualification_sha256.clone()),
            drive_identity_sha256: Some(drive_identity_sha256.clone()),
        };
        let preparation_bytes = preparation
            .encode()
            .map_err(|error| NativeTapeError::Failed(error.to_string()))?;

        self.check(
            deadline,
            cancellation,
            "before writing the preparation identity",
        )?;
        let preparation_rewind = self.mt(device, MtOperation::Rewind, cancellation).await?;
        let device_owned = device.to_owned();
        let cancel = cancellation.clone();
        tokio::task::spawn_blocking(move || {
            write_bytes_file(&device_owned, &preparation_bytes, &cancel)
        })
        .await
        .map_err(|error| {
            NativeTapeError::Failed(format!("preparation identity writer failed: {error}"))
        })??;

        let identity = self.probe_identity(device, cancellation).await;
        if identity.kind != NativeTapeOutcomeKind::Passed
            || identity.cartridge_id != Some(cartridge_id)
            || identity.volume_name.as_deref()
                != Some(preparation_volume_identity(cartridge_id).as_str())
        {
            return Err(NativeTapeError::Failed(format!(
                "native preparation identity readback failed: {}",
                identity
                    .evidence
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or("identity mismatch")
            )));
        }

        Ok(json!({
            "test": "native_tape_test",
            "engine": "TapeScale Native",
            "engine_version": "1",
            "format_version": NATIVE_FORMAT_VERSION,
            "detail": "native record, filemark, rewind, readback, and cartridge identity tests passed",
            "command_dispatched": true,
            "position_uncertain": false,
            "maximum_block_size_bytes": DEFAULT_TAPE_BLOCK_BYTES,
            "block_mode": "variable",
            "test_file_count": TEST_FILE_COUNT,
            "records_per_test_file": TEST_RECORDS_PER_FILE,
            "bytes_written": u64::from(TEST_FILE_COUNT) * u64::from(TEST_RECORDS_PER_FILE) * u64::from(DEFAULT_TAPE_BLOCK_BYTES),
            "qualification_sha256": qualification_sha256,
            "drive_identity_sha256": drive_identity_sha256,
            "on_tape_identity": preparation_volume_identity(cartridge_id),
            "cartridge_id": cartridge_id,
            "mt": {
                "variable_block": variable_block,
                "initial_rewind": initial_rewind,
                "readback_rewind": readback_rewind,
                "preparation_rewind": preparation_rewind
            },
            "identity_readback": identity.evidence
        }))
    }

    /// Read file zero at BOT and return either a preparation identity or a
    /// permanent archive-volume identity.  Unformatted media is not an error.
    pub async fn probe_identity(
        &self,
        device: &Path,
        cancellation: &CancellationToken,
    ) -> NativeTapeIdentityOutcome {
        if let Err(detail) = self.validate_device(device) {
            return blocked_identity(detail);
        }
        if cancellation.is_cancelled() {
            return interrupted_identity("identity probe was cancelled before tape motion");
        }
        let initial_rewind = match self.mt(device, MtOperation::Rewind, cancellation).await {
            Ok(value) => value,
            Err(error) => return identity_error(error),
        };
        let device_owned = device.to_owned();
        let cancel = cancellation.clone();
        let bytes =
            tokio::task::spawn_blocking(move || read_first_tape_file(&device_owned, &cancel)).await;
        let bytes = match bytes {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(NativeTapeError::Failed(detail))) if is_unformatted_read_detail(&detail) => {
                Vec::new()
            }
            Ok(Err(error)) => return identity_error(error),
            Err(error) => {
                return identity_error(NativeTapeError::Failed(format!(
                    "identity reader failed: {error}"
                )))
            }
        };
        let final_rewind = match self.mt(device, MtOperation::Rewind, cancellation).await {
            Ok(value) => value,
            Err(error) => return identity_error(error),
        };
        if bytes.is_empty() {
            return NativeTapeIdentityOutcome {
                kind: NativeTapeOutcomeKind::Passed,
                position_uncertain: false,
                volume_name: None,
                cartridge_id: None,
                archive_id: None,
                volume_ordinal: None,
                evidence: json!({
                    "test": "native_identity_probe",
                    "detail": "no TapeScale native identity record was present at BOT",
                    "recognized": false,
                    "initial_rewind": initial_rewind,
                    "final_rewind": final_rewind
                }),
            };
        }
        if let Ok(header) = NativePreparationHeader::decode(&bytes) {
            let identity = preparation_volume_identity(header.cartridge_id);
            return NativeTapeIdentityOutcome {
                kind: NativeTapeOutcomeKind::Passed,
                position_uncertain: false,
                volume_name: Some(identity.clone()),
                cartridge_id: Some(header.cartridge_id),
                archive_id: None,
                volume_ordinal: None,
                evidence: json!({
                    "test": "native_identity_probe",
                    "detail": "TapeScale preparation identity read from BOT",
                    "recognized": true,
                    "record_kind": "preparation",
                    "identity": identity,
                    "header": header,
                    "initial_rewind": initial_rewind,
                    "final_rewind": final_rewind
                }),
            };
        }
        if let Ok(header) = NativeVolumeHeader::decode(&bytes) {
            let identity = archive_volume_identity(header.archive_id, header.volume_ordinal);
            return NativeTapeIdentityOutcome {
                kind: NativeTapeOutcomeKind::Passed,
                position_uncertain: false,
                volume_name: Some(identity.clone()),
                cartridge_id: Some(header.cartridge_id),
                archive_id: Some(header.archive_id),
                volume_ordinal: Some(header.volume_ordinal),
                evidence: json!({
                    "test": "native_identity_probe",
                    "detail": "TapeScale archive-volume identity read from BOT",
                    "recognized": true,
                    "record_kind": "archive_volume",
                    "identity": identity,
                    "header": header,
                    "initial_rewind": initial_rewind,
                    "final_rewind": final_rewind
                }),
            };
        }
        NativeTapeIdentityOutcome {
            kind: NativeTapeOutcomeKind::Passed,
            position_uncertain: false,
            volume_name: None,
            cartridge_id: None,
            archive_id: None,
            volume_ordinal: None,
            evidence: json!({
                "test": "native_identity_probe",
                "detail": "file zero is not a valid TapeScale native identity record",
                "recognized": false,
                "record_bytes": bytes.len(),
                "record_sha256": format!("{:x}", Sha256::digest(&bytes)),
                "initial_rewind": initial_rewind,
                "final_rewind": final_rewind
            }),
        }
    }

    fn validate_destructive_access(&self, device: &Path) -> Result<(), String> {
        if !self.enabled {
            return Err("native destructive tape qualification is disabled".to_owned());
        }
        self.validate_device(device)
    }

    fn validate_device(&self, device: &Path) -> Result<(), String> {
        if !self.mt_program.is_absolute() {
            return Err("the configured mt-st path must be absolute".to_owned());
        }
        if !self.allowed_devices.iter().any(|allowed| allowed == device) {
            return Err(format!(
                "native tape access is not allowed for unconfigured device {}",
                device.display()
            ));
        }
        let text = device.to_string_lossy();
        let suffix = text.strip_prefix("/dev/nst").unwrap_or_default();
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("native tape access requires an exact /dev/nstN path".to_owned());
        }
        let metadata = fs::symlink_metadata(device).map_err(|error| {
            format!(
                "could not inspect configured tape device {}: {error}",
                device.display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_char_device() {
            return Err("configured tape path must be a non-symlink character device".to_owned());
        }
        Ok(())
    }

    fn check(
        &self,
        deadline: Instant,
        cancellation: &CancellationToken,
        context: &str,
    ) -> Result<(), NativeTapeError> {
        if cancellation.is_cancelled() {
            return Err(NativeTapeError::Interrupted(format!(
                "worker shutdown was requested {context}"
            )));
        }
        if Instant::now() >= deadline {
            return Err(NativeTapeError::Interrupted(format!(
                "native qualification exceeded its {:?} deadline {context}",
                self.timeout
            )));
        }
        Ok(())
    }

    async fn mt(
        &self,
        device: &Path,
        operation: MtOperation,
        cancellation: &CancellationToken,
    ) -> Result<ToolOutput, NativeTapeError> {
        let command = ToolCommand::new(&self.mt_program)
            .args(operation.args(device))
            .timeout(self.timeout.min(Duration::from_secs(15 * 60)))
            .retained_output_bytes(MT_OUTPUT_LIMIT)
            .env("LC_ALL", "C")
            .env("LANG", "C");
        tool_runner::run(&command, cancellation)
            .await
            .map_err(map_tool_error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestFileEvidence {
    file_number: u32,
    records: u32,
    bytes: u64,
    sha256: String,
}

fn write_test_files(
    device: &Path,
    run_id: Uuid,
    cancellation: &CancellationToken,
) -> Result<Vec<TestFileEvidence>, NativeTapeError> {
    let mut evidence = Vec::with_capacity(TEST_FILE_COUNT as usize);
    for file_number in 1..=TEST_FILE_COUNT {
        if cancellation.is_cancelled() {
            return Err(NativeTapeError::Interrupted(
                "native qualification was cancelled between test tape files".to_owned(),
            ));
        }
        let mut device_file = OpenOptions::new()
            .write(true)
            .open(device)
            .map_err(|error| io_failure("open tape for test write", error))?;
        let mut digest = Sha256::new();
        let mut bytes = 0_u64;
        for record_number in 1..=TEST_RECORDS_PER_FILE {
            if cancellation.is_cancelled() {
                return Err(NativeTapeError::Interrupted(
                    "native qualification was cancelled between physical records".to_owned(),
                ));
            }
            let record = test_record(run_id, file_number, record_number);
            let written = device_file
                .write(&record)
                .map_err(|error| io_failure("write native qualification record", error))?;
            if written != record.len() {
                return Err(NativeTapeError::Failed(format!(
                    "tape accepted a partial physical record: expected {}, wrote {written}",
                    record.len()
                )));
            }
            digest.update(&record);
            bytes += u64::try_from(record.len()).expect("record length fits u64");
        }
        device_file
            .flush()
            .map_err(|error| io_failure("flush native qualification tape file", error))?;
        drop(device_file); // closing /dev/nstN commits exactly one tape file mark
        evidence.push(TestFileEvidence {
            file_number,
            records: TEST_RECORDS_PER_FILE,
            bytes,
            sha256: format!("{:x}", digest.finalize()),
        });
    }
    Ok(evidence)
}

fn read_test_files(
    device: &Path,
    run_id: Uuid,
    cancellation: &CancellationToken,
) -> Result<Vec<TestFileEvidence>, NativeTapeError> {
    let mut device_file = File::open(device)
        .map_err(|error| io_failure("open tape for qualification readback", error))?;
    let mut evidence = Vec::with_capacity(TEST_FILE_COUNT as usize);
    let mut record = vec![0_u8; DEFAULT_TAPE_BLOCK_BYTES as usize];
    for file_number in 1..=TEST_FILE_COUNT {
        let mut digest = Sha256::new();
        let mut records = 0_u32;
        let mut bytes = 0_u64;
        loop {
            if cancellation.is_cancelled() {
                return Err(NativeTapeError::Interrupted(
                    "native qualification was cancelled during physical readback".to_owned(),
                ));
            }
            let read = device_file
                .read(&mut record)
                .map_err(|error| io_failure("read native qualification record", error))?;
            if read == 0 {
                break;
            }
            if read != record.len() {
                return Err(NativeTapeError::Failed(format!(
                    "tape returned a partial test record: expected {}, read {read}",
                    record.len()
                )));
            }
            records += 1;
            if records > TEST_RECORDS_PER_FILE {
                return Err(NativeTapeError::Failed(
                    "test tape file crossed its expected filemark boundary".to_owned(),
                ));
            }
            let expected = test_record(run_id, file_number, records);
            if record != expected {
                return Err(NativeTapeError::Failed(format!(
                    "physical record {records} in tape file {file_number} did not match"
                )));
            }
            digest.update(&record);
            bytes += u64::try_from(read).expect("record length fits u64");
        }
        if records != TEST_RECORDS_PER_FILE {
            return Err(NativeTapeError::Failed(format!(
                "tape file {file_number} contained {records} records; expected {TEST_RECORDS_PER_FILE}"
            )));
        }
        evidence.push(TestFileEvidence {
            file_number,
            records,
            bytes,
            sha256: format!("{:x}", digest.finalize()),
        });
    }
    Ok(evidence)
}

fn test_record(run_id: Uuid, file_number: u32, record_number: u32) -> Vec<u8> {
    let mut record = vec![0_u8; DEFAULT_TAPE_BLOCK_BYTES as usize];
    let mut offset = 0_usize;
    let mut counter = 0_u32;
    while offset < record.len() {
        let mut digest = Sha256::new();
        digest.update(b"TapeScale native qualification v1\0");
        digest.update(run_id.as_bytes());
        digest.update(file_number.to_be_bytes());
        digest.update(record_number.to_be_bytes());
        digest.update(counter.to_be_bytes());
        let block = digest.finalize();
        let remaining = record.len() - offset;
        let count = remaining.min(block.len());
        record[offset..offset + count].copy_from_slice(&block[..count]);
        offset += count;
        counter += 1;
    }
    record
}

fn digest_test_files(files: &[TestFileEvidence]) -> String {
    let mut digest = Sha256::new();
    for file in files {
        digest.update(file.file_number.to_be_bytes());
        digest.update(file.records.to_be_bytes());
        digest.update(file.bytes.to_be_bytes());
        digest.update(file.sha256.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn write_bytes_file(
    device: &Path,
    bytes: &[u8],
    cancellation: &CancellationToken,
) -> Result<(), NativeTapeError> {
    if cancellation.is_cancelled() {
        return Err(NativeTapeError::Interrupted(
            "native tape write was cancelled before dispatch".to_owned(),
        ));
    }
    let mut device_file = OpenOptions::new()
        .write(true)
        .open(device)
        .map_err(|error| io_failure("open tape for metadata write", error))?;
    let written = device_file
        .write(bytes)
        .map_err(|error| io_failure("write native tape metadata", error))?;
    if written != bytes.len() {
        return Err(NativeTapeError::Failed(format!(
            "tape accepted a partial metadata record: expected {}, wrote {written}",
            bytes.len()
        )));
    }
    device_file
        .flush()
        .map_err(|error| io_failure("flush native tape metadata", error))?;
    Ok(())
}

fn read_first_tape_file(
    device: &Path,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, NativeTapeError> {
    let mut device_file = File::open(device)
        .map_err(|error| io_failure("open tape for native identity read", error))?;
    let mut result = Vec::new();
    let mut buffer = vec![0_u8; DEFAULT_TAPE_BLOCK_BYTES as usize];
    loop {
        if cancellation.is_cancelled() {
            return Err(NativeTapeError::Interrupted(
                "native identity probe was cancelled during read".to_owned(),
            ));
        }
        let read = device_file
            .read(&mut buffer)
            .map_err(|error| io_failure("read native tape identity", error))?;
        if read == 0 {
            break;
        }
        if result.len().saturating_add(read) > METADATA_READ_LIMIT {
            return Err(NativeTapeError::Failed(format!(
                "tape file zero exceeds the {METADATA_READ_LIMIT}-byte metadata limit"
            )));
        }
        result.extend_from_slice(&buffer[..read]);
    }
    Ok(result)
}

fn map_tool_error(error: ToolError) -> NativeTapeError {
    match error {
        ToolError::Cancelled { .. } | ToolError::Timeout { .. } => {
            NativeTapeError::Interrupted(error.diagnostic_summary(2048))
        }
        ToolError::Unavailable { .. } => NativeTapeError::Blocked(error.diagnostic_summary(2048)),
        _ => NativeTapeError::Failed(error.diagnostic_summary(2048)),
    }
}

fn io_failure(context: &str, error: io::Error) -> NativeTapeError {
    NativeTapeError::Failed(format!("{context}: {error}"))
}

fn is_unformatted_read_detail(detail: &str) -> bool {
    let lowercase = detail.to_ascii_lowercase();
    lowercase.contains("input/output error")
        || lowercase.contains("no data available")
        || lowercase.contains("end of file")
}

#[derive(Debug)]
enum NativeTapeError {
    Blocked(String),
    Failed(String),
    Interrupted(String),
}

fn blocked(detail: String) -> NativeTapeOutcome {
    NativeTapeOutcome {
        kind: NativeTapeOutcomeKind::Blocked,
        position_uncertain: false,
        evidence: json!({
            "test": "native_tape_test",
            "detail": detail,
            "command_dispatched": false,
            "position_uncertain": false
        }),
    }
}

fn blocked_identity(detail: String) -> NativeTapeIdentityOutcome {
    NativeTapeIdentityOutcome {
        kind: NativeTapeOutcomeKind::Blocked,
        position_uncertain: false,
        volume_name: None,
        cartridge_id: None,
        archive_id: None,
        volume_ordinal: None,
        evidence: json!({"test":"native_identity_probe", "detail":detail}),
    }
}

fn interrupted_identity(detail: &str) -> NativeTapeIdentityOutcome {
    NativeTapeIdentityOutcome {
        kind: NativeTapeOutcomeKind::Interrupted,
        position_uncertain: false,
        volume_name: None,
        cartridge_id: None,
        archive_id: None,
        volume_ordinal: None,
        evidence: json!({"test":"native_identity_probe", "detail":detail}),
    }
}

fn identity_error(error: NativeTapeError) -> NativeTapeIdentityOutcome {
    let (kind, detail, uncertain) = match error {
        NativeTapeError::Blocked(detail) => (NativeTapeOutcomeKind::Blocked, detail, false),
        NativeTapeError::Failed(detail) => (NativeTapeOutcomeKind::Failed, detail, true),
        NativeTapeError::Interrupted(detail) => (NativeTapeOutcomeKind::Interrupted, detail, true),
    };
    NativeTapeIdentityOutcome {
        kind,
        position_uncertain: uncertain,
        volume_name: None,
        cartridge_id: None,
        archive_id: None,
        volume_ordinal: None,
        evidence: json!({"test":"native_identity_probe", "detail":detail}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_identity_is_canonical_and_reversible() {
        let cartridge = Uuid::parse_str("8b286d3c-46aa-46b2-80c8-131aeb35e43e").unwrap();
        let identity = preparation_volume_identity(cartridge);
        assert_eq!(
            preparation_identity_cartridge_id(&identity),
            Some(cartridge)
        );
        assert_eq!(
            preparation_identity_cartridge_id(&identity.to_uppercase()),
            None
        );
        assert_eq!(
            preparation_identity_cartridge_id("TS-PREP-V1-not-a-uuid"),
            None
        );
    }

    #[test]
    fn archive_identity_is_stable_and_safe_for_persistence() {
        let archive = Uuid::parse_str("7c67cabb-f13e-45e6-888d-24ca614af54e").unwrap();
        assert_eq!(
            archive_volume_identity(archive, 12),
            "TSN1-7c67cabb-f13e-45e6-888d-24ca614af54e-V0012"
        );
    }

    #[test]
    fn deterministic_records_bind_run_file_and_record() {
        let run = Uuid::parse_str("7c67cabb-f13e-45e6-888d-24ca614af54e").unwrap();
        let first = test_record(run, 1, 1);
        assert_eq!(first.len(), DEFAULT_TAPE_BLOCK_BYTES as usize);
        assert_eq!(first, test_record(run, 1, 1));
        assert_ne!(first, test_record(run, 1, 2));
        assert_ne!(first, test_record(run, 2, 1));
    }

    #[test]
    fn mt_operations_are_a_closed_argv_allowlist() {
        let device = Path::new("/dev/nst0");
        assert_eq!(
            MtOperation::VariableBlock.args(device),
            ["-f", "/dev/nst0", "setblk", "0"]
        );
        assert_eq!(
            MtOperation::Rewind.args(device),
            ["-f", "/dev/nst0", "rewind"]
        );
    }

    #[test]
    fn read_only_identity_validation_is_independent_of_destructive_gate() {
        let device = PathBuf::from("/dev/nst99999");
        let config = NativeTapeConfig {
            enabled: false,
            mt_program: PathBuf::from("/usr/bin/mt"),
            allowed_devices: vec![device.clone()],
            timeout: Duration::from_secs(1),
        };

        assert_eq!(
            config.validate_destructive_access(&device),
            Err("native destructive tape qualification is disabled".to_owned())
        );
        assert_ne!(
            config.validate_device(&device),
            Err("native destructive tape qualification is disabled".to_owned())
        );
    }
}
