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

use crate::tool_runner::{self, ToolCommand, ToolError, ToolOutput};

pub const BACULA_CANDIDATE: &str = "Bacula Community";
pub const BACULA_CANDIDATE_VERSION: &str = "15.0.3";
pub const BTAPE_PROGRAM: &str = "/usr/sbin/btape";
pub const BTAPE_SHA256_AMD64: &str =
    "3d5e3a9423406c62c0f7c5f984e63f32e88ba6af9beb2b7f444127cde51bf8a4";
// Each stream can expand up to sixfold when JSON escapes control bytes. Keeping both streams at
// 64 KiB guarantees the durable evidence remains below the schema's 1 MiB ceiling.
const RETAINED_BTAPE_OUTPUT: usize = 64 * 1024;

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
        let command = ToolCommand::new(&self.program)
            .args([
                "-c".to_owned(),
                paths.config_path.display().to_string(),
                paths.device.display().to_string(),
            ])
            .stdin_bytes(b"test\n".to_vec())
            .timeout(self.timeout)
            .retained_output_bytes(RETAINED_BTAPE_OUTPUT);
        let execution = tool_runner::run(&command, cancellation).await;
        outcome_from_execution(&self.program, &paths, execution)
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
        "Storage {{\n  Name = tapescale-qualification-sd\n  Working Directory = \"{working_dir}\"\n  Pid Directory = \"{pid_dir}\"\n  Plugin Directory = \"/usr/lib/bacula\"\n  Maximum Concurrent Jobs = 1\n  Messages = Standard\n}}\n\nDirector {{\n  Name = tapescale-qualification-dir\n  Password = \"qualification-{run_id}\"\n}}\n\nDevice {{\n  Name = tapescale-qualification-drive\n  Media Type = LTO\n  Archive Device = \"{device}\"\n  AutomaticMount = yes\n  AlwaysOpen = yes\n  RemovableMedia = yes\n  RandomAccess = no\n}}\n\nMessages {{\n  Name = Standard\n  director = tapescale-qualification-dir = all\n}}\n"
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
        "gate": "G0 not passed",
        "engine": {
            "candidate": format!("{BACULA_CANDIDATE} {BACULA_CANDIDATE_VERSION}"),
            "selection": "G0 laboratory candidate; not selected for production",
            "adapter": "Simulator for the legacy generic job engine; pinned Bacula 15.0.3 is available through attended btape qualification, single-volume Full backups, and G0.2 recovery exercises",
            "format": "Bacula format is exercised by attended Full backups and G0.2 recovery runs; production format selection remains open"
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

    #[test]
    fn report_never_claims_physical_qualification() {
        let value = report();
        assert_eq!(value["gate"], "G0 not passed");
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
