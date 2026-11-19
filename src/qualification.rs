//! Truthful software capability and physical-qualification status.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    engine::bacula::MAXIMUM_TAPE_BLOCK_BYTES,
    tool_runner::{self, ToolCommand, ToolError, ToolOutput},
};

pub const BACULA_CANDIDATE: &str = "Bacula Community";
pub const BACULA_CANDIDATE_VERSION: &str = "15.0.3";
pub const BTAPE_PROGRAM: &str = "/usr/sbin/btape";
pub const BTAPE_SHA256_AMD64: &str =
    "3d5e3a9423406c62c0f7c5f984e63f32e88ba6af9beb2b7f444127cde51bf8a4";
// Each stream can expand up to sixfold when JSON escapes control bytes. Keeping both streams at
// 64 KiB guarantees the durable evidence remains below the schema's 1 MiB ceiling.
const RETAINED_BTAPE_OUTPUT: usize = 64 * 1024;
/// Bacula stores `VolName` in a 128-byte C buffer, including its trailing NUL.
pub const BACULA_VOLUME_NAME_MAX_BYTES: usize = 127;
const PREPARATION_VOLUME_LABEL_PREFIX: &str = "TS-PREP-V1-";

#[derive(Clone, Debug)]
pub struct BtapeQualificationConfig {
    pub enabled: bool,
    pub program: PathBuf,
    pub state_dir: PathBuf,
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BtapeOutcomeKind {
    Passed,
    Failed,
    Blocked,
    Interrupted,
}

#[derive(Clone, Debug, Serialize)]
pub struct BtapeOutcome {
    pub kind: BtapeOutcomeKind,
    pub position_uncertain: bool,
    pub evidence: Value,
}

/// Result of a closed `btape` volume-label operation.
///
/// `volume_name` is populated only when a single syntactically valid `VolName`
/// was recovered from a complete, successful `readlabel` transcript. A failed
/// write-and-read operation can therefore retain the observed mismatching name
/// without treating it as verified.
#[derive(Clone, Debug, Serialize)]
pub struct BtapeLabelOutcome {
    pub kind: BtapeOutcomeKind,
    pub position_uncertain: bool,
    pub volume_name: Option<String>,
    pub evidence: Value,
}

/// Return the deterministic Bacula volume name used to identify a cartridge
/// after destructive preparation.
///
/// The UUID is the database identity, not an operator-provided label. The fixed
/// ASCII representation is well below Bacula's 127-byte `VolName` limit and
/// contains only Bacula-supported volume-name characters.
#[must_use]
pub fn preparation_volume_label(cartridge_id: Uuid) -> String {
    format!("{PREPARATION_VOLUME_LABEL_PREFIX}{cartridge_id}")
}

/// Recover the cartridge UUID from an exact TapeScale preparation label.
///
/// Canonical round-trip comparison rejects alternate UUID spellings and any
/// suffix/prefix smuggling.  This lets reconciliation recover after the
/// physical label write succeeds but the following database commit is lost.
#[must_use]
pub fn preparation_volume_cartridge_id(volume_name: &str) -> Option<Uuid> {
    let encoded = volume_name.strip_prefix(PREPARATION_VOLUME_LABEL_PREFIX)?;
    let cartridge_id = Uuid::parse_str(encoded).ok()?;
    (preparation_volume_label(cartridge_id) == volume_name).then_some(cartridge_id)
}

impl BtapeQualificationConfig {
    /// Run Bacula's destructive general tape test with a fixed `test\n` transcript.
    ///
    /// The caller owns drive/media authorization and must hold the exclusive drive lease.
    pub async fn run(
        &self,
        run_id: Uuid,
        device: &Path,
        cancellation: &CancellationToken,
    ) -> BtapeOutcome {
        let paths = match self.prepare(run_id, device) {
            Ok(paths) => paths,
            Err(detail) => return blocked_evidence(detail),
        };
        let command = self.command(&paths, b"test\n".to_vec());
        let execution = tool_runner::run(&command, cancellation).await;
        outcome_from_execution(&self.program, &paths, execution)
    }

    /// Destructively write and immediately read back the stable preparation
    /// identity for one cartridge using a closed `btape` transcript.
    ///
    /// The caller owns destructive authorization and the exclusive drive lease.
    /// No arbitrary operator text is accepted by this primitive.
    pub async fn write_and_verify_preparation_label(
        &self,
        operation_id: Uuid,
        device: &Path,
        cartridge_id: Uuid,
        cancellation: &CancellationToken,
    ) -> BtapeLabelOutcome {
        let expected_volume_name = preparation_volume_label(cartridge_id);
        let transcript = match label_and_readlabel_transcript(&expected_volume_name) {
            Ok(transcript) => transcript,
            Err(detail) => {
                return blocked_label_evidence(
                    detail,
                    BtapeLabelOperation::WriteAndVerify(&expected_volume_name),
                );
            }
        };
        let paths = match self.prepare(operation_id, device) {
            Ok(paths) => paths,
            Err(detail) => {
                return blocked_label_evidence(
                    detail,
                    BtapeLabelOperation::WriteAndVerify(&expected_volume_name),
                );
            }
        };
        let command = self.command(&paths, transcript);
        let execution = tool_runner::run(&command, cancellation).await;
        label_outcome_from_execution(
            &self.program,
            &paths,
            BtapeLabelOperation::WriteAndVerify(&expected_volume_name),
            execution,
        )
    }

    /// Read the Bacula volume label from the loaded tape without writing it.
    ///
    /// The caller owns the exclusive drive lease and decides whether the
    /// returned identity is acceptable for the requested cartridge.
    pub async fn probe_volume_label(
        &self,
        operation_id: Uuid,
        device: &Path,
        cancellation: &CancellationToken,
    ) -> BtapeLabelOutcome {
        let paths = match self.prepare(operation_id, device) {
            Ok(paths) => paths,
            Err(detail) => {
                return blocked_label_evidence(detail, BtapeLabelOperation::Probe);
            }
        };
        let command = self.command(&paths, readlabel_transcript());
        let execution = tool_runner::run(&command, cancellation).await;
        let outcome = label_outcome_from_execution(
            &self.program,
            &paths,
            BtapeLabelOperation::Probe,
            execution,
        );
        // Identity probes are transient status checks, not retained qualification runs. Avoid
        // accumulating one generated configuration directory every polling interval.
        if let Some(directory) = paths.config_path.parent() {
            let _ = fs::remove_dir_all(directory);
        }
        outcome
    }

    fn command(&self, paths: &BtapePaths, transcript: Vec<u8>) -> ToolCommand {
        ToolCommand::new(&self.program)
            .args([
                "-c".to_owned(),
                paths.config_path.display().to_string(),
                paths.device.display().to_string(),
            ])
            .stdin_bytes(transcript)
            // Transcript validation relies on Bacula's canonical field names.
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .timeout(self.timeout)
            .retained_output_bytes(RETAINED_BTAPE_OUTPUT)
    }

    fn prepare(&self, run_id: Uuid, device: &Path) -> Result<BtapePaths, String> {
        let program_sha256 = verify_btape_binary(&self.program)?;
        validate_state_dir(&self.state_dir)?;
        validate_non_rewinding_device(device)?;

        let working_dir = self.state_dir.join("working");
        let pid_dir = self.state_dir.join("run");
        let evidence_dir = self
            .state_dir
            .join("qualifications")
            .join(run_id.to_string());
        for directory in [&working_dir, &pid_dir, &evidence_dir] {
            fs::create_dir_all(directory).map_err(|error| {
                format!(
                    "could not create Bacula qualification directory {}: {error}",
                    directory.display()
                )
            })?;
        }

        let config_path = evidence_dir.join("bacula-sd.conf");
        let config = render_storage_config(device, &working_dir, &pid_dir, run_id)?;
        fs::write(&config_path, config.as_bytes()).map_err(|error| {
            format!(
                "could not write Bacula qualification configuration {}: {error}",
                config_path.display()
            )
        })?;
        let config_sha256 = hex_digest(config.as_bytes());
        Ok(BtapePaths {
            device: device.to_owned(),
            config_path,
            config_sha256,
            program_sha256,
        })
    }
}

#[derive(Debug)]
struct BtapePaths {
    device: PathBuf,
    config_path: PathBuf,
    config_sha256: String,
    program_sha256: String,
}

#[derive(Clone, Copy, Debug)]
enum BtapeLabelOperation<'a> {
    WriteAndVerify(&'a str),
    Probe,
}

impl<'a> BtapeLabelOperation<'a> {
    const fn test_name(self) -> &'static str {
        match self {
            Self::WriteAndVerify(_) => "btape_label_and_readlabel",
            Self::Probe => "btape_readlabel",
        }
    }

    const fn destructive(self) -> bool {
        matches!(self, Self::WriteAndVerify(_))
    }

    fn expected_volume_name(self) -> Option<&'a str> {
        match self {
            Self::WriteAndVerify(expected) => Some(expected),
            Self::Probe => None,
        }
    }

    fn transcript(self) -> String {
        match self {
            Self::WriteAndVerify(expected) => {
                format!("label\\n{expected}\\nreadlabel\\nrewind\\nquit\\n")
            }
            Self::Probe => "readlabel\\nrewind\\nquit\\n".to_owned(),
        }
    }
}

fn label_and_readlabel_transcript(volume_name: &str) -> Result<Vec<u8>, String> {
    validate_bacula_volume_name(volume_name)?;
    Ok(format!("label\n{volume_name}\nreadlabel\nrewind\nquit\n").into_bytes())
}

fn readlabel_transcript() -> Vec<u8> {
    b"readlabel\nrewind\nquit\n".to_vec()
}

fn validate_bacula_volume_name(volume_name: &str) -> Result<(), String> {
    if volume_name.is_empty() {
        return Err("Bacula volume name must not be empty".to_owned());
    }
    if volume_name.len() > BACULA_VOLUME_NAME_MAX_BYTES {
        return Err(format!(
            "Bacula volume name exceeds the {BACULA_VOLUME_NAME_MAX_BYTES}-byte limit"
        ));
    }
    if !volume_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return Err(
            "Bacula volume name contains characters outside letters, numbers, hyphen, underscore, colon, and period"
                .to_owned(),
        );
    }
    Ok(())
}

fn render_storage_config(
    device: &Path,
    working_dir: &Path,
    pid_dir: &Path,
    run_id: Uuid,
) -> Result<String, String> {
    let device = safe_config_path(device)?;
    let working_dir = safe_config_path(working_dir)?;
    let pid_dir = safe_config_path(pid_dir)?;
    Ok(format!(
        "Storage {{\n  Name = tapescale-qualification-sd\n  Working Directory = \"{working_dir}\"\n  Pid Directory = \"{pid_dir}\"\n  Plugin Directory = \"/usr/lib/bacula\"\n  Maximum Concurrent Jobs = 1\n  Messages = Standard\n}}\n\nDirector {{\n  Name = tapescale-qualification-dir\n  Password = \"qualification-{run_id}\"\n}}\n\nDevice {{\n  Name = tapescale-qualification-drive\n  Media Type = LTO\n  Archive Device = \"{device}\"\n  AutomaticMount = yes\n  AlwaysOpen = yes\n  RemovableMedia = yes\n  RandomAccess = no\n  Maximum Block Size = {MAXIMUM_TAPE_BLOCK_BYTES}\n}}\n\nMessages {{\n  Name = Standard\n  director = tapescale-qualification-dir = all\n}}\n"
    ))
}

fn verify_btape_binary(path: &Path) -> Result<String, String> {
    if path != Path::new(BTAPE_PROGRAM) {
        return Err(format!(
            "qualification is bound to the pinned Bacula binary at {BTAPE_PROGRAM}"
        ));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect pinned Bacula btape binary: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("the pinned Bacula btape path must be a regular non-symlink file".to_owned());
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("could not read pinned Bacula btape binary: {error}"))?;
    let digest = hex_digest(&bytes);
    if digest != BTAPE_SHA256_AMD64 {
        return Err(format!(
            "Bacula btape binary digest mismatch: expected {BTAPE_SHA256_AMD64}, observed {digest}"
        ));
    }
    Ok(digest)
}

fn validate_state_dir(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("TAPESCALE_BACULA_STATE_DIR must be an absolute path".to_owned());
    }
    safe_config_path(path).map(|_| ())
}

fn validate_non_rewinding_device(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new("/dev")) {
        return Err("Bacula qualification requires an exact /dev/nstN device".to_owned());
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err("Bacula qualification device is not valid UTF-8".to_owned());
    };
    let Some(index) = name.strip_prefix("nst") else {
        return Err("Bacula qualification requires a non-rewinding nst device".to_owned());
    };
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Bacula qualification requires an exact /dev/nstN device".to_owned());
    }
    Ok(())
}

fn safe_config_path(path: &Path) -> Result<String, String> {
    let Some(value) = path.to_str() else {
        return Err("Bacula configuration paths must be valid UTF-8".to_owned());
    };
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '"' | '\\'))
    {
        return Err("Bacula configuration path contains unsupported characters".to_owned());
    }
    Ok(value.to_owned())
}

fn outcome_from_execution(
    program: &Path,
    paths: &BtapePaths,
    execution: Result<ToolOutput, ToolError>,
) -> BtapeOutcome {
    let base = json!({
        "candidate": BACULA_CANDIDATE,
        "candidate_version": BACULA_CANDIDATE_VERSION,
        "test": "btape_test",
        "program": program,
        "program_sha256": paths.program_sha256,
        "device": paths.device,
        "config_path": paths.config_path,
        "config_sha256": paths.config_sha256,
        "maximum_block_size_bytes": MAXIMUM_TAPE_BLOCK_BYTES,
        "stdin_transcript": "test\\n",
        "scope": "destructive scratch-media drive compatibility test",
        "g0_gate": "not_passed"
    });
    match execution {
        Ok(output) => match validate_btape_success(&output) {
            Ok(()) => BtapeOutcome {
                kind: BtapeOutcomeKind::Passed,
                position_uncertain: false,
                evidence: merge_evidence(
                    base,
                    json!({"tool": output, "success_markers_verified": true}),
                ),
            },
            Err(detail) => BtapeOutcome {
                kind: BtapeOutcomeKind::Failed,
                position_uncertain: true,
                evidence: merge_evidence(base, json!({"detail":detail,"tool":output})),
            },
        },
        Err(ToolError::Unavailable { program }) => BtapeOutcome {
            kind: BtapeOutcomeKind::Blocked,
            position_uncertain: false,
            evidence: merge_evidence(
                base,
                json!({"detail": format!("Bacula btape is unavailable at {}", program.display())}),
            ),
        },
        Err(ToolError::Failed {
            code,
            stdout,
            stderr,
            output_truncated,
            elapsed_ms,
            ..
        }) => BtapeOutcome {
            kind: BtapeOutcomeKind::Failed,
            position_uncertain: true,
            evidence: merge_evidence(
                base,
                json!({
                    "detail": format!("Bacula btape test failed with exit code {code}"),
                    "tool": {
                        "exit_code": code,
                        "stdout": stdout,
                        "stderr": stderr,
                        "output_truncated": output_truncated,
                        "elapsed_ms": elapsed_ms
                    }
                }),
            ),
        },
        Err(ToolError::Timeout {
            timeout,
            stdout,
            stderr,
            output_truncated,
            elapsed_ms,
            ..
        }) => BtapeOutcome {
            kind: BtapeOutcomeKind::Interrupted,
            position_uncertain: true,
            evidence: merge_evidence(
                base,
                json!({
                    "detail": format!("Bacula btape exceeded its {timeout:?} deadline; tape position requires reconciliation"),
                    "tool": {
                        "exit_code": -1,
                        "stdout": stdout,
                        "stderr": stderr,
                        "output_truncated": output_truncated,
                        "elapsed_ms": elapsed_ms
                    }
                }),
            ),
        },
        Err(ToolError::Cancelled {
            stdout,
            stderr,
            output_truncated,
            elapsed_ms,
            ..
        }) => BtapeOutcome {
            kind: BtapeOutcomeKind::Interrupted,
            position_uncertain: true,
            evidence: merge_evidence(
                base,
                json!({
                    "detail": "worker shutdown interrupted Bacula btape; tape position requires reconciliation",
                    "tool": {
                        "exit_code": -1,
                        "stdout": stdout,
                        "stderr": stderr,
                        "output_truncated": output_truncated,
                        "elapsed_ms": elapsed_ms
                    }
                }),
            ),
        },
        Err(ToolError::Io { source, .. }) => BtapeOutcome {
            kind: BtapeOutcomeKind::Failed,
            position_uncertain: true,
            evidence: merge_evidence(
                base,
                json!({"detail": format!("Bacula btape process I/O failed: {source}; tape position requires reconciliation")}),
            ),
        },
    }
}

fn label_outcome_from_execution(
    program: &Path,
    paths: &BtapePaths,
    operation: BtapeLabelOperation<'_>,
    execution: Result<ToolOutput, ToolError>,
) -> BtapeLabelOutcome {
    let base = json!({
        "candidate": BACULA_CANDIDATE,
        "candidate_version": BACULA_CANDIDATE_VERSION,
        "test": operation.test_name(),
        "program": program,
        "program_sha256": paths.program_sha256,
        "device": paths.device,
        "config_path": paths.config_path,
        "config_sha256": paths.config_sha256,
        "maximum_block_size_bytes": MAXIMUM_TAPE_BLOCK_BYTES,
        "stdin_transcript": operation.transcript(),
        "expected_volume_name": operation.expected_volume_name(),
        "destructive": operation.destructive(),
        "scope": if operation.destructive() {
            "destructive Bacula preparation-label write and immediate readback"
        } else {
            "read-only Bacula volume-label probe"
        },
        "g0_gate": "not_passed"
    });

    match execution {
        Ok(output) => {
            let parsed = parse_readlabel_volume_name(&output);
            match parsed {
                Ok(Some(volume_name)) => {
                    let validation =
                        validate_label_operation_success(operation, &output, volume_name.as_str());
                    match validation {
                        Ok(()) => BtapeLabelOutcome {
                            kind: BtapeOutcomeKind::Passed,
                            position_uncertain: false,
                            volume_name: Some(volume_name.clone()),
                            evidence: merge_evidence(
                                base,
                                json!({
                                    "tool": output,
                                    "observed_volume_name": volume_name,
                                    "label_markers_verified": true
                                }),
                            ),
                        },
                        Err(detail) => BtapeLabelOutcome {
                            kind: BtapeOutcomeKind::Failed,
                            // Parsing already proved that the fixed transcript completed its
                            // final rewind; an identity mismatch fails closed without inventing
                            // tape-position uncertainty.
                            position_uncertain: false,
                            volume_name: Some(volume_name.clone()),
                            evidence: merge_evidence(
                                base,
                                json!({
                                    "detail": detail,
                                    "tool": output,
                                    "observed_volume_name": volume_name
                                }),
                            ),
                        },
                    }
                }
                Ok(None) if matches!(operation, BtapeLabelOperation::Probe) => BtapeLabelOutcome {
                    kind: BtapeOutcomeKind::Passed,
                    position_uncertain: false,
                    volume_name: None,
                    evidence: merge_evidence(
                        base,
                        json!({
                            "tool": output,
                            "observed_volume_name": Value::Null,
                            "label_state": "unlabeled",
                            "label_markers_verified": true
                        }),
                    ),
                },
                Ok(None) => BtapeLabelOutcome {
                    kind: BtapeOutcomeKind::Failed,
                    position_uncertain: false,
                    volume_name: None,
                    evidence: merge_evidence(
                        base,
                        json!({
                            "detail": "btape completed the label transcript but read back no Bacula volume label",
                            "tool": output,
                            "label_state": "unlabeled"
                        }),
                    ),
                },
                Err(detail) => BtapeLabelOutcome {
                    kind: BtapeOutcomeKind::Failed,
                    position_uncertain: true,
                    volume_name: None,
                    evidence: merge_evidence(base, json!({"detail": detail, "tool": output})),
                },
            }
        }
        Err(ToolError::Unavailable { program }) => BtapeLabelOutcome {
            kind: BtapeOutcomeKind::Blocked,
            position_uncertain: false,
            volume_name: None,
            evidence: merge_evidence(
                base,
                json!({"detail": format!("Bacula btape is unavailable at {}", program.display())}),
            ),
        },
        Err(ToolError::Failed {
            code,
            stdout,
            stderr,
            output_truncated,
            elapsed_ms,
            ..
        }) => BtapeLabelOutcome {
            kind: BtapeOutcomeKind::Failed,
            position_uncertain: true,
            volume_name: None,
            evidence: merge_evidence(
                base,
                json!({
                    "detail": format!("Bacula btape label operation failed with exit code {code}"),
                    "tool": {
                        "exit_code": code,
                        "stdout": stdout,
                        "stderr": stderr,
                        "output_truncated": output_truncated,
                        "elapsed_ms": elapsed_ms
                    }
                }),
            ),
        },
        Err(ToolError::Timeout {
            timeout,
            stdout,
            stderr,
            output_truncated,
            elapsed_ms,
            ..
        }) => BtapeLabelOutcome {
            kind: BtapeOutcomeKind::Interrupted,
            position_uncertain: true,
            volume_name: None,
            evidence: merge_evidence(
                base,
                json!({
                    "detail": format!("Bacula btape label operation exceeded its {timeout:?} deadline; tape position requires reconciliation"),
                    "tool": {
                        "exit_code": -1,
                        "stdout": stdout,
                        "stderr": stderr,
                        "output_truncated": output_truncated,
                        "elapsed_ms": elapsed_ms
                    }
                }),
            ),
        },
        Err(ToolError::Cancelled {
            stdout,
            stderr,
            output_truncated,
            elapsed_ms,
            ..
        }) => BtapeLabelOutcome {
            kind: BtapeOutcomeKind::Interrupted,
            position_uncertain: true,
            volume_name: None,
            evidence: merge_evidence(
                base,
                json!({
                    "detail": "worker shutdown interrupted the Bacula btape label operation; tape position requires reconciliation",
                    "tool": {
                        "exit_code": -1,
                        "stdout": stdout,
                        "stderr": stderr,
                        "output_truncated": output_truncated,
                        "elapsed_ms": elapsed_ms
                    }
                }),
            ),
        },
        Err(ToolError::Io { source, .. }) => BtapeLabelOutcome {
            kind: BtapeOutcomeKind::Failed,
            position_uncertain: true,
            volume_name: None,
            evidence: merge_evidence(
                base,
                json!({"detail": format!("Bacula btape label process I/O failed: {source}; tape position requires reconciliation")}),
            ),
        },
    }
}

fn parse_readlabel_volume_name(output: &ToolOutput) -> Result<Option<String>, String> {
    if output.output_truncated {
        return Err(
            "btape output exceeded the retained evidence bound; the complete readlabel transcript cannot be proven"
                .to_owned(),
        );
    }
    if output.exit_code != 0 {
        return Err(format!(
            "btape readlabel returned unexpected exit code {}",
            output.exit_code
        ));
    }

    let combined = format!("{}\n{}", output.stdout, output.stderr);
    const FAILURE_MARKERS: [&str; 9] = [
        "I/O error on device:",
        "Volume type error:",
        "Volume name error",
        "Error creating label.",
        "Volume version error.",
        "Bad Volume label type.",
        "Unknown error.",
        "Device open failed.",
        "Bad status from rewind.",
    ];
    if let Some(marker) = FAILURE_MARKERS
        .iter()
        .find(|marker| combined.contains(**marker))
    {
        return Err(format!(
            "btape readlabel reported a failure marker: {marker}"
        ));
    }

    const REWIND_MARKER: &str = "Rewound ";
    let rewind_markers = combined.match_indices(REWIND_MARKER).count();
    if rewind_markers != 1 {
        return Err(format!(
            "btape readlabel transcript must contain exactly one successful rewind marker; observed {rewind_markers}"
        ));
    }
    const SUCCESS_MARKER: &str = "Volume label read correctly.";
    const UNLABELED_MARKER: &str = "Volume has no label.";
    let success_markers = combined.match_indices(SUCCESS_MARKER).count();
    let unlabeled_markers = combined.match_indices(UNLABELED_MARKER).count();
    if success_markers == 0 && unlabeled_markers == 1 {
        return Ok(None);
    }
    if success_markers != 1 || unlabeled_markers != 0 {
        return Err(format!(
            "btape readlabel transcript must contain exactly one labeled or unlabeled outcome marker; observed {success_markers} labeled and {unlabeled_markers} unlabeled"
        ));
    }
    let label_headers = combined
        .lines()
        .filter(|line| line.trim() == "Volume Label:")
        .count();
    if label_headers != 1 {
        return Err(format!(
            "btape readlabel transcript must contain exactly one Volume Label header; observed {label_headers}"
        ));
    }

    let mut names = Vec::new();
    for line in combined.lines() {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field.trim() != "VolName" {
            continue;
        }
        let value = value.trim();
        validate_bacula_volume_name(value).map_err(|detail| {
            format!("btape readlabel returned an invalid VolName field: {detail}")
        })?;
        names.push(value.to_owned());
    }
    if names.len() != 1 {
        return Err(format!(
            "btape readlabel transcript must contain exactly one valid VolName field; observed {}",
            names.len()
        ));
    }
    Ok(Some(names.remove(0)))
}

fn validate_label_operation_success(
    operation: BtapeLabelOperation<'_>,
    output: &ToolOutput,
    observed_volume_name: &str,
) -> Result<(), String> {
    let BtapeLabelOperation::WriteAndVerify(expected_volume_name) = operation else {
        return Ok(());
    };
    let combined = format!("{}\n{}", output.stdout, output.stderr);
    let write_marker = format!("Wrote Volume label for volume \"{expected_volume_name}\".");
    let write_markers = combined.match_indices(&write_marker).count();
    if write_markers != 1 {
        return Err(format!(
            "btape label transcript must contain exactly one expected write marker; observed {write_markers}"
        ));
    }
    if observed_volume_name != expected_volume_name {
        return Err(format!(
            "btape readlabel VolName mismatch: expected {expected_volume_name}, observed {observed_volume_name}"
        ));
    }
    Ok(())
}

fn validate_btape_success(output: &ToolOutput) -> Result<(), String> {
    if output.output_truncated {
        return Err(
            "btape output exceeded the retained evidence bound; the completed transcript cannot be proven"
                .to_owned(),
        );
    }
    const REQUIRED_MARKERS: [&str; 4] = [
        "=== Write, rewind, and re-read test ===",
        "=== Test Succeeded. End Write, rewind, and re-read test ===",
        "=== Append files test ===",
        "=== End Forward space files test ===",
    ];
    let combined = format!("{}\n{}", output.stdout, output.stderr);
    let missing = REQUIRED_MARKERS
        .iter()
        .copied()
        .filter(|marker| !combined.contains(marker))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "btape exited successfully without the required completed-test transcript markers: {}",
            missing.join(", ")
        ))
    }
}

fn blocked_evidence(detail: String) -> BtapeOutcome {
    BtapeOutcome {
        kind: BtapeOutcomeKind::Blocked,
        position_uncertain: false,
        evidence: json!({
            "candidate": BACULA_CANDIDATE,
            "candidate_version": BACULA_CANDIDATE_VERSION,
            "test": "btape_test",
            "detail": detail,
            "command_dispatched": false,
            "g0_gate": "not_passed"
        }),
    }
}

fn blocked_label_evidence(detail: String, operation: BtapeLabelOperation<'_>) -> BtapeLabelOutcome {
    BtapeLabelOutcome {
        kind: BtapeOutcomeKind::Blocked,
        position_uncertain: false,
        volume_name: None,
        evidence: json!({
            "candidate": BACULA_CANDIDATE,
            "candidate_version": BACULA_CANDIDATE_VERSION,
            "test": operation.test_name(),
            "detail": detail,
            "expected_volume_name": operation.expected_volume_name(),
            "destructive": operation.destructive(),
            "command_dispatched": false,
            "g0_gate": "not_passed"
        }),
    }
}

fn merge_evidence(mut left: Value, right: Value) -> Value {
    let (Some(left), Some(right)) = (left.as_object_mut(), right.as_object()) else {
        return left;
    };
    left.extend(right.clone());
    Value::Object(left.clone())
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns product capability metadata without inventing hardware evidence.
#[must_use]
pub fn report() -> Value {
    json!({
        "release": crate::VERSION,
        "gate": "hardware qualification is installation-specific",
        "engine": {
            "candidate": "TapeScale Native 1",
            "selection": "native archive and cartridge format selected for alpha testing",
            "adapter": "TapeScale directly exercises the configured non-rewinding SCSI tape device with closed record, filemark, rewind, and readback operations",
            "format": "TapeScale Native Volume v1 writes a machine-readable cartridge UUID at tape file zero"
        },
        "claims": {
            "software_tests": "Automated domain and service tests",
            "physical_lto": "No drive or media generation qualified",
            "truenas": "25.10.8 target; this installation has not been qualified"
        },
        "generations": [
            {"generation":"LTO-1","reads":"LTO-1","writes":"LTO-1","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-2","reads":"LTO-1, LTO-2","writes":"LTO-1, LTO-2","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-3","reads":"LTO-1–3","writes":"LTO-2–3","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-4","reads":"LTO-2–4","writes":"LTO-3–4","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-5","reads":"LTO-3–5","writes":"LTO-4–5","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-6","reads":"LTO-4–6","writes":"LTO-5–6","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-7","reads":"LTO-5–7","writes":"LTO-6–7","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-8","reads":"LTO-7, LTO-7 Type M, LTO-8","writes":"LTO-7, LTO-7 Type M, LTO-8","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-9","reads":"LTO-8, LTO-9","writes":"LTO-8, LTO-9","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-10","reads":"LTO-10","writes":"LTO-10","model":"Capability model","physical":"Not run"}
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readlabel_output(volume_name: &str) -> ToolOutput {
        ToolOutput {
            exit_code: 0,
            stdout: format!(
                "*readlabel\nbtape: btape.c:525 Volume label read correctly.\n\nVolume Label:\nId                : Bacula 1.0 immortal\nVerNo             : 11\nVolName           : {volume_name}\nLabelType         : VOL_LABEL\n*rewind\nRewound \"tapescale-qualification-drive\" (/dev/nst0)\n*"
            ),
            stderr: String::new(),
            output_truncated: false,
            elapsed_ms: 1,
        }
    }

    fn label_and_readlabel_output(volume_name: &str) -> ToolOutput {
        let mut output = readlabel_output(volume_name);
        output.stdout = format!(
            "*label\nEnter Volume Name: Wrote Volume label for volume \"{volume_name}\".\n{}",
            output.stdout
        );
        output
    }

    fn label_test_paths() -> BtapePaths {
        BtapePaths {
            device: PathBuf::from("/dev/nst0"),
            config_path: PathBuf::from(
                "/var/lib/tapescale/bacula/qualifications/test/bacula-sd.conf",
            ),
            config_sha256: "config-digest".to_owned(),
            program_sha256: BTAPE_SHA256_AMD64.to_owned(),
        }
    }

    #[test]
    fn report_never_claims_physical_qualification() {
        let value = report();
        assert_eq!(
            value["gate"],
            "hardware qualification is installation-specific"
        );
        assert_eq!(value["generations"].as_array().map(Vec::len), Some(10));
        assert!(value["generations"]
            .as_array()
            .expect("generation array")
            .iter()
            .all(|generation| generation["physical"] == "Not run"));
    }

    #[test]
    fn btape_config_is_bound_to_the_exact_non_rewinding_device() {
        let config = render_storage_config(
            Path::new("/dev/nst0"),
            Path::new("/var/lib/tapescale/bacula/working"),
            Path::new("/var/lib/tapescale/bacula/run"),
            Uuid::nil(),
        )
        .expect("config");
        assert!(config.contains("Archive Device = \"/dev/nst0\""));
        assert!(config.contains("RandomAccess = no"));
        assert!(config.contains("Maximum Block Size = 262144"));
    }

    #[test]
    fn qualification_rejects_rewinding_or_injected_device_paths() {
        assert!(validate_non_rewinding_device(Path::new("/dev/st0")).is_err());
        assert!(validate_non_rewinding_device(Path::new("/dev/nst0\nDevice {}")).is_err());
        assert!(validate_non_rewinding_device(Path::new("/dev/nst7")).is_ok());
    }

    #[test]
    fn qualification_rejects_substituted_btape_programs() {
        assert!(verify_btape_binary(Path::new("/usr/bin/true")).is_err());
    }

    #[test]
    fn preparation_volume_label_is_stable_safe_and_uuid_bound() {
        let cartridge_id =
            Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").expect("valid cartridge UUID");
        let label = preparation_volume_label(cartridge_id);

        assert_eq!(label, "TS-PREP-V1-01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(label, preparation_volume_label(cartridge_id));
        assert!(label.len() <= BACULA_VOLUME_NAME_MAX_BYTES);
        assert!(validate_bacula_volume_name(&label).is_ok());
        assert_ne!(label, preparation_volume_label(Uuid::nil()));
        assert_eq!(preparation_volume_cartridge_id(&label), Some(cartridge_id));
        assert_eq!(
            preparation_volume_cartridge_id("TS-PREP-V1-0123456789abcdef0123456789abcdef"),
            None
        );
        assert_eq!(
            preparation_volume_cartridge_id(&format!("{label}-suffix")),
            None
        );
        assert_eq!(preparation_volume_cartridge_id("OTHER-0123"), None);
    }

    #[test]
    fn label_transcripts_are_closed_and_validate_volume_names() {
        let label = preparation_volume_label(Uuid::nil());
        assert_eq!(
            label_and_readlabel_transcript(&label).expect("safe transcript"),
            format!("label\n{label}\nreadlabel\nrewind\nquit\n").into_bytes()
        );
        assert_eq!(
            readlabel_transcript(),
            b"readlabel\nrewind\nquit\n".to_vec()
        );

        for invalid in [
            "",
            "label with spaces",
            "label/with/slashes",
            "label\nreadlabel",
            "tape-💾",
        ] {
            assert!(validate_bacula_volume_name(invalid).is_err(), "{invalid:?}");
            assert!(label_and_readlabel_transcript(invalid).is_err());
        }
        assert!(validate_bacula_volume_name(&"A".repeat(127)).is_ok());
        assert!(validate_bacula_volume_name(&"A".repeat(128)).is_err());
    }

    #[test]
    fn readlabel_parser_requires_one_complete_valid_volume_label() {
        let expected = preparation_volume_label(Uuid::nil());
        assert_eq!(
            parse_readlabel_volume_name(&readlabel_output(&expected))
                .expect("complete readlabel transcript")
                .as_deref(),
            Some(expected.as_str())
        );

        let mut no_success = readlabel_output(&expected);
        no_success.stdout = no_success
            .stdout
            .replace("Volume label read correctly.", "Volume has no label.");
        assert_eq!(
            parse_readlabel_volume_name(&no_success).expect("exact unlabeled outcome"),
            None
        );

        let mut duplicate = readlabel_output(&expected);
        duplicate
            .stdout
            .push_str(&format!("\nVolName : {expected}\n"));
        assert!(parse_readlabel_volume_name(&duplicate).is_err());

        let mut not_rewound = readlabel_output(&expected);
        not_rewound.stdout = not_rewound.stdout.replace(
            "Rewound \"tapescale-qualification-drive\" (/dev/nst0)",
            "Bad status from rewind. ERR=Input/output error",
        );
        assert!(parse_readlabel_volume_name(&not_rewound).is_err());

        let mut invalid_name = readlabel_output(&expected);
        invalid_name.stdout = invalid_name.stdout.replace(&expected, "bad label");
        assert!(parse_readlabel_volume_name(&invalid_name).is_err());

        let mut truncated = readlabel_output(&expected);
        truncated.output_truncated = true;
        assert!(parse_readlabel_volume_name(&truncated).is_err());
    }

    #[test]
    fn label_outcome_passes_only_for_the_exact_written_and_read_name() {
        let cartridge_id =
            Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").expect("valid cartridge UUID");
        let expected = preparation_volume_label(cartridge_id);
        let paths = label_test_paths();
        let passed = label_outcome_from_execution(
            Path::new(BTAPE_PROGRAM),
            &paths,
            BtapeLabelOperation::WriteAndVerify(&expected),
            Ok(label_and_readlabel_output(&expected)),
        );
        assert_eq!(passed.kind, BtapeOutcomeKind::Passed);
        assert_eq!(passed.volume_name.as_deref(), Some(expected.as_str()));
        assert!(!passed.position_uncertain);

        let observed = preparation_volume_label(Uuid::nil());
        let mut mismatched = label_and_readlabel_output(&observed);
        mismatched.stdout = mismatched.stdout.replacen(
            &format!("Wrote Volume label for volume \"{observed}\"."),
            &format!("Wrote Volume label for volume \"{expected}\"."),
            1,
        );
        let failed = label_outcome_from_execution(
            Path::new(BTAPE_PROGRAM),
            &paths,
            BtapeLabelOperation::WriteAndVerify(&expected),
            Ok(mismatched),
        );
        assert_eq!(failed.kind, BtapeOutcomeKind::Failed);
        assert_eq!(failed.volume_name.as_deref(), Some(observed.as_str()));
        assert!(!failed.position_uncertain);

        let mut unlabeled = label_and_readlabel_output(&expected);
        unlabeled.stdout = unlabeled
            .stdout
            .replace("Volume label read correctly.", "Volume has no label.");
        let failed = label_outcome_from_execution(
            Path::new(BTAPE_PROGRAM),
            &paths,
            BtapeLabelOperation::WriteAndVerify(&expected),
            Ok(unlabeled),
        );
        assert_eq!(failed.kind, BtapeOutcomeKind::Failed);
        assert!(failed.volume_name.is_none());
        assert!(!failed.position_uncertain);
    }

    #[test]
    fn readlabel_probe_is_read_only_and_rejects_false_success() {
        let expected = preparation_volume_label(Uuid::nil());
        let paths = label_test_paths();
        let passed = label_outcome_from_execution(
            Path::new(BTAPE_PROGRAM),
            &paths,
            BtapeLabelOperation::Probe,
            Ok(readlabel_output(&expected)),
        );
        assert_eq!(passed.kind, BtapeOutcomeKind::Passed);
        assert_eq!(passed.volume_name.as_deref(), Some(expected.as_str()));
        assert_eq!(passed.evidence["destructive"], false);

        let mut unlabeled = readlabel_output(&expected);
        unlabeled.stdout = unlabeled
            .stdout
            .replace("Volume label read correctly.", "Volume has no label.");
        let unlabeled = label_outcome_from_execution(
            Path::new(BTAPE_PROGRAM),
            &paths,
            BtapeLabelOperation::Probe,
            Ok(unlabeled),
        );
        assert_eq!(unlabeled.kind, BtapeOutcomeKind::Passed);
        assert!(unlabeled.volume_name.is_none());
        assert!(!unlabeled.position_uncertain);
        assert_eq!(unlabeled.evidence["label_state"], "unlabeled");
    }

    #[test]
    fn exit_zero_without_the_complete_btape_transcript_is_not_a_pass() {
        let incomplete = ToolOutput {
            exit_code: 0,
            stdout: "*".to_owned(),
            stderr: String::new(),
            output_truncated: false,
            elapsed_ms: 1,
        };
        assert!(validate_btape_success(&incomplete).is_err());

        let complete = ToolOutput {
            exit_code: 0,
            stdout: [
                "=== Write, rewind, and re-read test ===",
                "=== Test Succeeded. End Write, rewind, and re-read test ===",
                "=== Append files test ===",
                "=== End Forward space files test ===",
            ]
            .join("\n"),
            stderr: String::new(),
            output_truncated: false,
            elapsed_ms: 1,
        };
        assert!(validate_btape_success(&complete).is_ok());
    }
}
