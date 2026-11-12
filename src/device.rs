//! Discovery and constrained lab motion for explicitly mapped Linux SCSI tape devices.
//!
//! Identity and `mt status` are read-only. A separately gated worker path may issue the two
//! closed non-data motion operations below. The destructive G0.2 worker also has one internal,
//! fixed file-mark operation used only to invalidate prior scratch contents before Bacula owns the
//! tape. Format, erase, arbitrary file-mark, seek, spacing, retension, read, and general write
//! commands are not exposed.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::tool_runner::{self, ToolCommand, ToolError, ToolOutput};

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_OUTPUT_LIMIT: usize = 16 * 1024;
const MOTION_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MOTION_OUTPUT_LIMIT: usize = 16 * 1024;
const MT_ST_NOWAIT: u64 = 0x2000;

#[derive(Clone, Debug)]
pub struct SasDiscoveryConfig {
    pub devices: Vec<PathBuf>,
    pub sysfs_root: PathBuf,
    pub mt_path: PathBuf,
}

impl SasDiscoveryConfig {
    pub fn from_env() -> Result<Self> {
        let devices = parse_device_list(&env::var("TAPESCALE_TAPE_DEVICES").unwrap_or_default())?;
        Ok(Self {
            devices,
            sysfs_root: absolute_setting("TAPESCALE_SYSFS_ROOT", "/sys")?,
            mt_path: absolute_setting("TAPESCALE_MT_PATH", "/usr/bin/mt-st")?,
        })
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.devices.is_empty()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SasDriveObservation {
    pub stable_id: String,
    pub identity_verified: bool,
    pub identity_conflict: bool,
    pub identity_basis: String,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub wwn: Option<String>,
    pub firmware: Option<String>,
    pub generation: Option<i16>,
    pub observed_path: String,
    pub device_major: u64,
    pub device_minor: u64,
    pub scsi_address: Option<String>,
    pub generic_path: Option<String>,
    pub sysfs_state: Option<String>,
    pub position_lost: Option<bool>,
    pub driver_options: Option<u64>,
    pub immediate_mode: Option<bool>,
    pub in_flight: Option<u64>,
    pub at_bot: Option<bool>,
    pub accessible: bool,
    pub ready: Option<bool>,
    pub medium_loaded: Option<bool>,
    pub status: String,
    pub status_detail: Option<String>,
    pub raw_status: Option<String>,
    pub observed_at: DateTime<Utc>,
}

impl SasDriveObservation {
    #[must_use]
    pub fn health(&self) -> Value {
        json!({
            "present": true,
            "accessible": self.accessible,
            "ready": self.ready,
            "medium_loaded": self.medium_loaded,
            "status": self.status,
            "status_detail": self.status_detail,
            "raw_mt_status": self.raw_status,
            "identity_verified": self.identity_verified,
            "identity_conflict": self.identity_conflict,
            "identity_basis": self.identity_basis,
            "device_major": self.device_major,
            "device_minor": self.device_minor,
            "scsi_address": self.scsi_address,
            "generic_path": self.generic_path,
            "sysfs_state": self.sysfs_state,
            "position_lost": self.position_lost,
            "driver_options": self.driver_options,
            "immediate_mode": self.immediate_mode,
            "in_flight": self.in_flight,
            "at_bot": self.at_bot,
            "capabilities": ["read_only_discovery", "mt_status"],
            "observation_source": "linux_scsi_tape",
            "observed_at": self.observed_at
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MissingSasDrive {
    pub observed_path: String,
    pub reason: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum SasProbeResult {
    Observed(Box<SasDriveObservation>),
    Missing(MissingSasDrive),
}

/// Constrained scratch-media motion that may be issued directly to a reconciled SAS drive.
///
/// This deliberately excludes positioning, media formatting, explicit file marks, erase, density,
/// compression, and every data operation. Callers cannot supply an arbitrary `mt-st` operation.
/// Linux may still finalize another controller's prior write with a filemark before motion.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SasMotionOperation {
    Rewind,
    Unload,
}

impl SasMotionOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rewind => "rewind",
            Self::Unload => "unload",
        }
    }

    const fn mt_operation(self) -> &'static str {
        match self {
            Self::Rewind => "rewind",
            // `mt-st offline` rewinds and, when supported, unloads the cartridge.
            Self::Unload => "offline",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SasMotionError {
    #[error("tape motion is not allowed for unconfigured device {device}")]
    DeviceNotConfigured { device: PathBuf },
    #[error("tape device binding is unsafe for {device}: {reason}")]
    DeviceBinding { device: PathBuf, reason: String },
    #[error(transparent)]
    Tool(#[from] ToolError),
}

/// Execute one allowlisted motion command against an exact configured non-rewinding device.
///
/// Identity reconciliation, resource leasing, media policy, and authorization remain caller
/// responsibilities. This function supplies no shell and accepts neither free-form operations nor
/// arguments.
pub async fn execute_sas_motion(
    config: &SasDiscoveryConfig,
    device: &Path,
    operation: SasMotionOperation,
    cancellation: &CancellationToken,
) -> Result<ToolOutput, SasMotionError> {
    let command = build_motion_command(config, device, operation)?;
    verify_device_binding(config, device).map_err(|reason| SasMotionError::DeviceBinding {
        device: device.to_owned(),
        reason,
    })?;
    tool_runner::run(&command, cancellation)
        .await
        .map_err(SasMotionError::from)
}

/// Invalidate the existing on-tape label at BOT before the fenced G0.2 Bacula run.
///
/// Qualification, an interrupted earlier attempt, or another scratch workflow can leave records
/// or a valid label at BOT. The Director's safe `label` command intentionally refuses a valid
/// existing label, so the already-authorized destructive recovery workflow first makes the
/// cartridge blank from Bacula's perspective. This operation is not represented by the public
/// motion API and accepts no caller-controlled command or argument.
pub(crate) async fn clear_sas_scratch_label(
    config: &SasDiscoveryConfig,
    device: &Path,
    cancellation: &CancellationToken,
) -> Result<ToolOutput, SasMotionError> {
    let command = build_clear_label_command(config, device)?;
    verify_device_binding(config, device).map_err(|reason| SasMotionError::DeviceBinding {
        device: device.to_owned(),
        reason,
    })?;
    tool_runner::run(&command, cancellation)
        .await
        .map_err(SasMotionError::from)
}

fn build_clear_label_command(
    config: &SasDiscoveryConfig,
    device: &Path,
) -> Result<ToolCommand, SasMotionError> {
    if validate_tape_path(device).is_err()
        || !config
            .devices
            .iter()
            .any(|configured| configured.as_path() == device)
    {
        return Err(SasMotionError::DeviceNotConfigured {
            device: device.to_owned(),
        });
    }

    let mut command = ToolCommand::new(&config.mt_path)
        .args([
            "-f".to_owned(),
            device.display().to_string(),
            "weof".to_owned(),
            "1".to_owned(),
        ])
        .timeout(MOTION_TIMEOUT);
    command.retained_output_bytes = MOTION_OUTPUT_LIMIT;
    Ok(command)
}

fn build_motion_command(
    config: &SasDiscoveryConfig,
    device: &Path,
    operation: SasMotionOperation,
) -> Result<ToolCommand, SasMotionError> {
    if validate_tape_path(device).is_err()
        || !config
            .devices
            .iter()
            .any(|configured| configured.as_path() == device)
    {
        return Err(SasMotionError::DeviceNotConfigured {
            device: device.to_owned(),
        });
    }

    let mut command = ToolCommand::new(&config.mt_path)
        .args([
            "-f".to_owned(),
            device.display().to_string(),
            operation.mt_operation().to_owned(),
        ])
        .timeout(MOTION_TIMEOUT);
    command.retained_output_bytes = MOTION_OUTPUT_LIMIT;
    Ok(command)
}

#[derive(Debug, Default)]
struct SysfsIdentity {
    vendor: Option<String>,
    model: Option<String>,
    serial: Option<String>,
    wwn: Option<String>,
    firmware: Option<String>,
    scsi_address: Option<String>,
    generic_path: Option<String>,
    state: Option<String>,
    position_lost: Option<bool>,
    driver_options: Option<u64>,
    immediate_mode: Option<bool>,
    in_flight: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeviceNumbers {
    major: u64,
    minor: u64,
}

#[derive(Debug)]
struct MtStatus {
    accessible: bool,
    ready: Option<bool>,
    medium_loaded: Option<bool>,
    at_bot: Option<bool>,
    status: String,
    detail: Option<String>,
    raw: Option<String>,
}

pub async fn discover_sas_drives(config: &SasDiscoveryConfig) -> Vec<SasProbeResult> {
    let mut results = Vec::with_capacity(config.devices.len());
    for device in &config.devices {
        results.push(probe_one(config, device).await);
    }
    mark_duplicate_identities(&mut results);
    results
}

async fn probe_one(config: &SasDiscoveryConfig, device: &Path) -> SasProbeResult {
    let observed_at = Utc::now();
    let device_numbers = match verify_device_binding(config, device) {
        Ok(numbers) => numbers,
        Err(reason) => {
            return SasProbeResult::Missing(MissingSasDrive {
                observed_path: device.display().to_string(),
                reason,
                observed_at,
            });
        }
    };

    let device_name = device
        .file_name()
        .and_then(|name| name.to_str())
        .expect("validated tape paths always have a UTF-8 file name");
    let identity = read_sysfs_identity(&config.sysfs_root, device_name);
    let (stable_id, identity_basis, identity_verified) = stable_identity(&identity, device);
    let mut mt_status = if identity.in_flight.is_some_and(|count| count > 0) {
        MtStatus {
            accessible: true,
            ready: None,
            medium_loaded: None,
            at_bot: None,
            status: "io_in_flight".to_owned(),
            detail: Some(
                "the kernel reports an outstanding tape I/O; status probing is deferred".to_owned(),
            ),
            raw: None,
        }
    } else {
        observe_mt_status(&config.mt_path, device).await
    };
    apply_position_loss(identity.position_lost, &mut mt_status);

    SasProbeResult::Observed(Box::new(SasDriveObservation {
        stable_id,
        identity_verified,
        identity_conflict: false,
        identity_basis,
        vendor: identity.vendor,
        model: identity.model,
        serial: identity.serial,
        wwn: identity.wwn,
        firmware: identity.firmware,
        // Model names are not authoritative generation evidence.
        generation: None,
        observed_path: device.display().to_string(),
        device_major: device_numbers.major,
        device_minor: device_numbers.minor,
        scsi_address: identity.scsi_address,
        generic_path: identity.generic_path,
        sysfs_state: identity.state,
        position_lost: identity.position_lost,
        driver_options: identity.driver_options,
        immediate_mode: identity.immediate_mode,
        in_flight: identity.in_flight,
        at_bot: mt_status.at_bot,
        accessible: mt_status.accessible,
        ready: mt_status.ready,
        medium_loaded: mt_status.medium_loaded,
        status: mt_status.status,
        status_detail: mt_status.detail,
        raw_status: mt_status.raw,
        observed_at,
    }))
}

fn verify_device_binding(
    config: &SasDiscoveryConfig,
    device: &Path,
) -> Result<DeviceNumbers, String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = fs::symlink_metadata(device)
        .map_err(|error| format!("device node is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("configured tape paths must be direct device nodes, not symlinks".to_owned());
    }
    if !metadata.file_type().is_char_device() {
        return Err("configured path is not a character device".to_owned());
    }

    let device_name = device
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "configured tape path has no UTF-8 device name".to_owned())?;
    let expected_path = config
        .sysfs_root
        .join("class")
        .join("scsi_tape")
        .join(device_name)
        .join("dev");
    let expected_raw = fs::read_to_string(&expected_path).map_err(|error| {
        format!(
            "cannot bind the device node to {}: {error}",
            expected_path.display()
        )
    })?;
    let expected = parse_sysfs_device_numbers(&expected_raw).ok_or_else(|| {
        format!(
            "{} does not contain a valid major:minor device number",
            expected_path.display()
        )
    })?;
    let actual = linux_device_numbers(metadata.rdev());
    if actual != expected {
        return Err(format!(
            "device node major:minor {}:{} does not match sysfs {}:{} for {device_name}",
            actual.major, actual.minor, expected.major, expected.minor
        ));
    }
    Ok(actual)
}

fn parse_sysfs_device_numbers(value: &str) -> Option<DeviceNumbers> {
    let (major, minor) = value.trim().split_once(':')?;
    Some(DeviceNumbers {
        major: major.parse().ok()?,
        minor: minor.parse().ok()?,
    })
}

// Linux's userspace dev_t layout, matching the kernel's MAJOR/MINOR encoding.
const fn linux_device_numbers(device: u64) -> DeviceNumbers {
    DeviceNumbers {
        major: ((device >> 8) & 0x0fff) | ((device >> 32) & 0xffff_f000),
        minor: (device & 0x00ff) | ((device >> 12) & 0xffff_ff00),
    }
}

fn parse_device_list(raw: &str) -> Result<Vec<PathBuf>> {
    let mut devices = BTreeSet::new();
    for value in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let path = PathBuf::from(value);
        validate_tape_path(&path)?;
        devices.insert(path);
    }
    Ok(devices.into_iter().collect())
}

fn validate_tape_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.parent() != Some(Path::new("/dev")) {
        bail!(
            "TAPESCALE_TAPE_DEVICES entries must be exact /dev/nstN paths; rejected {}",
            path.display()
        );
    }
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        bail!("tape device path must have a UTF-8 file name");
    };
    let Some(number) = name.strip_prefix("nst") else {
        bail!("only non-rewinding /dev/nstN tape devices are accepted");
    };
    if number.is_empty() || !number.chars().all(|character| character.is_ascii_digit()) {
        bail!("only non-rewinding /dev/nstN tape devices are accepted");
    }
    Ok(())
}

fn absolute_setting(key: &str, default: &str) -> Result<PathBuf> {
    let path = PathBuf::from(env::var(key).unwrap_or_else(|_| default.to_owned()));
    if !path.is_absolute() {
        bail!("{key} must be an absolute path");
    }
    Ok(path)
}

fn read_sysfs_identity(sysfs_root: &Path, device_name: &str) -> SysfsIdentity {
    let tape_class = sysfs_root.join("class").join("scsi_tape").join(device_name);
    let device = tape_class.join("device");
    let serial = fs::read(device.join("vpd_pg80"))
        .ok()
        .and_then(|page| parse_vpd_page_80(&page));
    let wwn = fs::read(device.join("vpd_pg83"))
        .ok()
        .and_then(|page| parse_vpd_page_83(&page));
    let scsi_address = fs::canonicalize(&device)
        .ok()
        .and_then(|path| find_scsi_address(&path));
    let generic_path = fs::read_dir(device.join("scsi_generic"))
        .ok()
        .and_then(|mut entries| entries.next())
        .and_then(Result::ok)
        .and_then(|entry| entry.file_name().into_string().ok())
        .map(|name| format!("/dev/{name}"));
    let driver_options =
        read_trimmed(tape_class.join("options")).and_then(|value| parse_sysfs_u64(&value));
    let mode_zero_name = device_name.strip_prefix('n').unwrap_or(device_name);
    let in_flight = read_trimmed(
        sysfs_root
            .join("class")
            .join("scsi_tape")
            .join(mode_zero_name)
            .join("stats")
            .join("in_flight"),
    )
    .and_then(|value| parse_sysfs_u64(&value));

    SysfsIdentity {
        vendor: read_trimmed(device.join("vendor")),
        model: read_trimmed(device.join("model")),
        serial,
        wwn,
        firmware: read_trimmed(device.join("rev")),
        scsi_address,
        generic_path,
        state: read_trimmed(device.join("state")),
        position_lost: read_trimmed(tape_class.join("position_lost_in_reset"))
            .and_then(|value| parse_sysfs_bool(&value)),
        driver_options,
        immediate_mode: driver_options.map(|options| options & MT_ST_NOWAIT != 0),
        in_flight,
    }
}

fn parse_sysfs_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn parse_sysfs_u64(value: &str) -> Option<u64> {
    let value = value.trim();
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(
            || value.parse::<u64>().ok(),
            |hex| u64::from_str_radix(hex, 16).ok(),
        )
}

fn apply_position_loss(position_lost: Option<bool>, status: &mut MtStatus) {
    if position_lost == Some(true) {
        status.status = "position_lost".to_owned();
        status.detail = Some(
            "the kernel reports tape position was lost during reset; data and relative positioning remain blocked, while rewind or unload may recover a known endpoint"
                .to_owned(),
        );
    }
}

fn read_trimmed(path: PathBuf) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim_matches(char::from(0)).trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_vpd_page_80(page: &[u8]) -> Option<String> {
    let payload = vpd_payload(page, 0x80)?;
    let value = String::from_utf8_lossy(payload)
        .trim_matches(char::from(0))
        .trim()
        .to_owned();
    (!value.is_empty()).then_some(value)
}

fn parse_vpd_page_83(page: &[u8]) -> Option<String> {
    let payload = vpd_payload(page, 0x83)?;
    let mut offset = 0;
    let mut names = Vec::new();
    while offset + 4 <= payload.len() {
        let code_set = payload[offset] & 0x0f;
        let association = (payload[offset + 1] >> 4) & 0x03;
        let identifier_type = payload[offset + 1] & 0x0f;
        let length = usize::from(payload[offset + 3]);
        let end = offset + 4 + length;
        if end > payload.len() {
            break;
        }
        let identifier = &payload[offset + 4..end];
        if association == 0 && identifier_type == 3 && code_set == 1 && !identifier.is_empty() {
            return Some(format!("naa.{}", encode_hex(identifier)));
        }
        if association == 0 && identifier_type == 8 && matches!(code_set, 2 | 3) {
            let name = String::from_utf8_lossy(identifier).trim().to_owned();
            if !name.is_empty() {
                names.push(name);
            }
        }
        offset = end;
    }
    names.into_iter().next()
}

fn vpd_payload(page: &[u8], expected_code: u8) -> Option<&[u8]> {
    if page.len() < 4 || page[1] != expected_code {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([page[2], page[3]]));
    (page.len() >= 4 + length).then_some(&page[4..4 + length])
}

fn find_scsi_address(path: &Path) -> Option<String> {
    path.components().rev().find_map(|component| {
        let value = component.as_os_str().to_str()?;
        let segments = value.split(':').collect::<Vec<_>>();
        (segments.len() == 4
            && segments.iter().all(|segment| {
                !segment.is_empty() && segment.chars().all(|character| character.is_ascii_digit())
            }))
        .then(|| value.to_owned())
    })
}

fn stable_identity(identity: &SysfsIdentity, path: &Path) -> (String, String, bool) {
    if let Some(wwn) = identity.wwn.as_deref() {
        return (
            format!("scsi:{}", normalize_identity(wwn)),
            "vpd_83".to_owned(),
            true,
        );
    }
    if let Some(serial) = identity.serial.as_deref() {
        let vendor = identity.vendor.as_deref().unwrap_or("unknown");
        let model = identity.model.as_deref().unwrap_or("unknown");
        return (
            format!(
                "serial:{}:{}:{}",
                normalize_identity(vendor),
                normalize_identity(model),
                normalize_identity(serial)
            ),
            "vpd_80".to_owned(),
            true,
        );
    }
    if let Some(address) = identity.scsi_address.as_deref() {
        return (
            format!("unverified:scsi:{address}"),
            "scsi_address_fallback".to_owned(),
            false,
        );
    }
    (
        format!(
            "unverified:path:{}",
            normalize_identity(&path.display().to_string())
        ),
        "device_path_fallback".to_owned(),
        false,
    )
}

fn normalize_identity(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | ':') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

async fn observe_mt_status(mt_path: &Path, device: &Path) -> MtStatus {
    let mut command = ToolCommand::new(mt_path)
        .args([
            "-f".to_owned(),
            device.display().to_string(),
            "status".to_owned(),
        ])
        .timeout(STATUS_TIMEOUT);
    command.retained_output_bytes = STATUS_OUTPUT_LIMIT;
    match tool_runner::run(&command, &CancellationToken::new()).await {
        Ok(output) => parse_mt_success(&output.stdout, &output.stderr),
        Err(error) => parse_mt_error(error),
    }
}

fn parse_mt_success(stdout: &str, stderr: &str) -> MtStatus {
    let raw = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let uppercase = raw.to_ascii_uppercase();
    let no_media = uppercase.contains("NO MEDIUM") || uppercase.contains("DR_OPEN");
    let online = uppercase.contains("ONLINE") && !no_media;
    let (ready, medium_loaded, status, detail) = if online {
        (Some(true), Some(true), "ready", None)
    } else if no_media {
        (
            Some(false),
            Some(false),
            "no_media",
            Some("drive affirmatively reported no medium".to_owned()),
        )
    } else {
        (
            None,
            None,
            "status_unknown",
            Some("drive responded without a recognized loaded or empty status".to_owned()),
        )
    };
    MtStatus {
        accessible: true,
        ready,
        medium_loaded,
        at_bot: online.then_some(uppercase.contains("BOT")),
        status: status.to_owned(),
        detail,
        raw: (!raw.is_empty()).then_some(raw),
    }
}

fn parse_mt_error(error: ToolError) -> MtStatus {
    let (status, detail) = match error {
        ToolError::Failed { code, stderr, .. } => {
            let lowercase = stderr.to_ascii_lowercase();
            if lowercase.contains("no medium") || lowercase.contains("medium not present") {
                (
                    "no_media",
                    format!("mt status reported no medium (exit {code})"),
                )
            } else if lowercase.contains("permission denied") {
                ("permission_denied", stderr.trim().to_owned())
            } else {
                (
                    "status_error",
                    format!("mt status exit {code}: {}", stderr.trim()),
                )
            }
        }
        ToolError::Unavailable { program } => (
            "tool_unavailable",
            format!("mt tool is unavailable: {}", program.display()),
        ),
        ToolError::Timeout { timeout, .. } => (
            "status_timeout",
            format!("mt status timed out after {timeout:?}"),
        ),
        ToolError::Cancelled { .. } => ("status_cancelled", "mt status was cancelled".to_owned()),
        ToolError::Io { source, .. } => ("status_error", source.to_string()),
    };
    let no_media = status == "no_media";
    MtStatus {
        accessible: no_media,
        ready: no_media.then_some(false),
        medium_loaded: no_media.then_some(false),
        at_bot: None,
        status: status.to_owned(),
        detail: Some(detail),
        raw: None,
    }
}

fn mark_duplicate_identities(results: &mut [SasProbeResult]) {
    let mut counts = BTreeMap::<String, usize>::new();
    for result in results.iter() {
        if let SasProbeResult::Observed(observation) = result {
            if observation.identity_verified {
                *counts.entry(observation.stable_id.clone()).or_default() += 1;
            }
        }
    }
    for result in results {
        if let SasProbeResult::Observed(observation) = result {
            if counts
                .get(&observation.stable_id)
                .copied()
                .unwrap_or_default()
                > 1
            {
                observation.identity_conflict = true;
                observation.status = "identity_conflict".to_owned();
                observation.status_detail = Some(
                    "the same stable SCSI identity was observed at multiple configured paths"
                        .to_owned(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion_config(devices: &[&str], mt_path: &str) -> SasDiscoveryConfig {
        SasDiscoveryConfig {
            devices: devices.iter().map(PathBuf::from).collect(),
            sysfs_root: PathBuf::from("/sys"),
            mt_path: PathBuf::from(mt_path),
        }
    }

    #[test]
    fn only_explicit_non_rewinding_devices_are_accepted() {
        assert_eq!(
            parse_device_list(" /dev/nst1, /dev/nst0,/dev/nst1 ").expect("valid list"),
            vec![PathBuf::from("/dev/nst0"), PathBuf::from("/dev/nst1")]
        );
        for invalid in [
            "/dev/st0",
            "/dev/nst",
            "/dev/nst0/other",
            "nst0",
            "/dev/sg0",
        ] {
            assert!(parse_device_list(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn motion_commands_are_a_closed_exact_allowlist() {
        let config = motion_config(&["/dev/nst0"], "/usr/bin/mt-st");
        for (operation, expected) in [
            (SasMotionOperation::Rewind, "rewind"),
            (SasMotionOperation::Unload, "offline"),
        ] {
            let command = build_motion_command(&config, Path::new("/dev/nst0"), operation)
                .expect("configured motion command");
            assert_eq!(command.program, PathBuf::from("/usr/bin/mt-st"));
            assert_eq!(command.args, ["-f", "/dev/nst0", expected]);
            assert_eq!(command.timeout, MOTION_TIMEOUT);
            assert_eq!(command.retained_output_bytes, MOTION_OUTPUT_LIMIT);
            assert!(command.working_dir.is_none());
        }
    }

    #[test]
    fn scratch_label_clear_is_one_fixed_internal_command() {
        let config = motion_config(&["/dev/nst0"], "/usr/bin/mt-st");
        let command = build_clear_label_command(&config, Path::new("/dev/nst0"))
            .expect("configured scratch-label command");
        assert_eq!(command.program, PathBuf::from("/usr/bin/mt-st"));
        assert_eq!(command.args, ["-f", "/dev/nst0", "weof", "1"]);
        assert_eq!(command.timeout, MOTION_TIMEOUT);
        assert_eq!(command.retained_output_bytes, MOTION_OUTPUT_LIMIT);
        assert!(command.working_dir.is_none());
    }

    #[test]
    fn scratch_label_clear_rejects_unconfigured_or_rewinding_paths() {
        let config = motion_config(&["/dev/nst0", "/dev/st0"], "/usr/bin/mt-st");
        for rejected in ["/dev/nst1", "/dev/st0", "/dev/sg0"] {
            assert!(matches!(
                build_clear_label_command(&config, Path::new(rejected)),
                Err(SasMotionError::DeviceNotConfigured { .. })
            ));
        }
    }

    #[test]
    fn motion_rejects_every_path_outside_the_exact_configuration() {
        let config = motion_config(&["/dev/nst0", "/dev/st0"], "/usr/bin/mt-st");
        for rejected in [
            "/dev/nst1",
            "/dev/st0",
            "/dev/nst0/other",
            "/dev/../dev/nst0",
        ] {
            assert!(matches!(
                build_motion_command(&config, Path::new(rejected), SasMotionOperation::Rewind),
                Err(SasMotionError::DeviceNotConfigured { .. })
            ));
        }
    }

    #[test]
    fn device_binding_rejects_symlinks_and_major_minor_mismatches() {
        use std::os::unix::fs::{symlink, MetadataExt};

        let root = tempfile::tempdir().expect("temporary sysfs root");
        let link = root.path().join("nst0");
        symlink("/dev/null", &link).expect("device symlink");
        let config = SasDiscoveryConfig {
            devices: vec![link.clone()],
            sysfs_root: root.path().to_owned(),
            mt_path: PathBuf::from("/usr/bin/mt-st"),
        };
        assert!(verify_device_binding(&config, &link)
            .expect_err("symlinks must be rejected")
            .contains("not symlinks"));

        let class = root.path().join("class/scsi_tape/null");
        fs::create_dir_all(&class).expect("fake sysfs class");
        fs::write(class.join("dev"), "999:999\n").expect("fake sysfs dev number");
        let null_config = SasDiscoveryConfig {
            devices: vec![PathBuf::from("/dev/null")],
            sysfs_root: root.path().to_owned(),
            mt_path: PathBuf::from("/usr/bin/mt-st"),
        };
        assert!(verify_device_binding(&null_config, Path::new("/dev/null"))
            .expect_err("mismatched node must be rejected")
            .contains("does not match sysfs"));

        let actual = linux_device_numbers(
            fs::symlink_metadata("/dev/null")
                .expect("null metadata")
                .rdev(),
        );
        fs::write(
            class.join("dev"),
            format!("{}:{}\n", actual.major, actual.minor),
        )
        .expect("matching sysfs dev number");
        assert_eq!(
            verify_device_binding(&null_config, Path::new("/dev/null")),
            Ok(actual)
        );
    }

    #[test]
    fn vpd_serial_and_naa_are_parsed() {
        let serial = b"IBM123456";
        let mut page80 = vec![0, 0x80, 0, serial.len() as u8];
        page80.extend_from_slice(serial);
        assert_eq!(parse_vpd_page_80(&page80).as_deref(), Some("IBM123456"));

        let naa = [0x50, 0x01, 0x43, 0x80, 0x12, 0x34, 0x56, 0x78];
        let descriptor_length = 4 + naa.len();
        let mut page83 = vec![
            0,
            0x83,
            0,
            descriptor_length as u8,
            0x01,
            0x03,
            0,
            naa.len() as u8,
        ];
        page83.extend_from_slice(&naa);
        assert_eq!(
            parse_vpd_page_83(&page83).as_deref(),
            Some("naa.5001438012345678")
        );
    }

    #[test]
    fn stable_identity_never_treats_a_path_as_verified() {
        let identity = SysfsIdentity {
            vendor: Some("IBM".to_owned()),
            model: Some("ULT3580-TD8".to_owned()),
            serial: Some("1234".to_owned()),
            ..SysfsIdentity::default()
        };
        let (stable_id, basis, verified) = stable_identity(&identity, Path::new("/dev/nst0"));
        assert_eq!(stable_id, "serial:ibm:ult3580-td8:1234");
        assert_eq!(basis, "vpd_80");
        assert!(verified);

        let fallback = SysfsIdentity::default();
        let (stable_id, _, verified) = stable_identity(&fallback, Path::new("/dev/nst0"));
        assert!(stable_id.starts_with("unverified:path:"));
        assert!(!verified);
    }

    #[test]
    fn status_parser_distinguishes_loaded_and_empty_drives() {
        let ready = parse_mt_success(
            "SCSI 2 tape drive:\nGeneral status bits on (41010000):\n BOT ONLINE IM_REP_EN\n",
            "",
        );
        assert!(ready.accessible);
        assert_eq!(ready.ready, Some(true));
        assert_eq!(ready.medium_loaded, Some(true));
        assert_eq!(ready.at_bot, Some(true));

        let empty = parse_mt_success("General status bits on:\n DR_OPEN IM_REP_EN\n", "");
        assert!(empty.accessible);
        assert_eq!(empty.status, "no_media");
        assert_eq!(empty.medium_loaded, Some(false));
        assert_eq!(empty.at_bot, None);

        let unknown = parse_mt_success("SCSI tape drive status unavailable", "");
        assert!(unknown.accessible);
        assert_eq!(unknown.status, "status_unknown");
        assert_eq!(unknown.ready, None);
        assert_eq!(unknown.medium_loaded, None);
    }

    #[test]
    fn duplicate_stable_identity_is_blocked() {
        let observation = SasDriveObservation {
            stable_id: "scsi:naa.5001".to_owned(),
            identity_verified: true,
            identity_conflict: false,
            identity_basis: "vpd_83".to_owned(),
            vendor: None,
            model: None,
            serial: None,
            wwn: Some("naa.5001".to_owned()),
            firmware: None,
            generation: None,
            observed_path: "/dev/nst0".to_owned(),
            device_major: 9,
            device_minor: 128,
            scsi_address: None,
            generic_path: None,
            sysfs_state: None,
            position_lost: Some(false),
            driver_options: Some(0),
            immediate_mode: Some(false),
            in_flight: Some(0),
            at_bot: None,
            accessible: true,
            ready: Some(false),
            medium_loaded: Some(false),
            status: "no_media".to_owned(),
            status_detail: None,
            raw_status: None,
            observed_at: Utc::now(),
        };
        let mut results = vec![
            SasProbeResult::Observed(Box::new(observation.clone())),
            SasProbeResult::Observed(Box::new(SasDriveObservation {
                observed_path: "/dev/nst1".to_owned(),
                ..observation
            })),
        ];
        mark_duplicate_identities(&mut results);
        assert!(results.iter().all(|result| matches!(
            result,
            SasProbeResult::Observed(observation) if observation.identity_conflict
        )));
    }

    #[test]
    fn scsi_address_is_found_in_a_canonical_sysfs_path() {
        let path = Path::new("/sys/devices/pci0000:00/host4/target4:0:1/4:0:1:0");
        assert_eq!(find_scsi_address(path).as_deref(), Some("4:0:1:0"));
    }

    #[test]
    fn sysfs_identity_includes_position_loss_without_guessing_generation() {
        let root = tempfile::tempdir().expect("temporary sysfs root");
        let tape_class = root.path().join("class/scsi_tape/nst0");
        let device = tape_class.join("device");
        fs::create_dir_all(device.join("scsi_generic")).expect("create fake sysfs tree");
        fs::create_dir_all(root.path().join("class/scsi_tape/st0/stats"))
            .expect("create fake stats tree");
        fs::write(device.join("vendor"), "IBM     \n").expect("vendor");
        fs::write(device.join("model"), "ULT3580-TD8     \n").expect("model");
        fs::write(device.join("rev"), "J6M1\n").expect("firmware");
        fs::write(tape_class.join("position_lost_in_reset"), "1\n").expect("position flag");
        fs::write(tape_class.join("options"), "0x2000\n").expect("driver options");
        fs::write(
            root.path().join("class/scsi_tape/st0/stats/in_flight"),
            "2\n",
        )
        .expect("in-flight count");
        let serial = b"IBM123456";
        let mut page80 = vec![0, 0x80, 0, serial.len() as u8];
        page80.extend_from_slice(serial);
        fs::write(device.join("vpd_pg80"), page80).expect("serial page");

        let identity = read_sysfs_identity(root.path(), "nst0");
        assert_eq!(identity.vendor.as_deref(), Some("IBM"));
        assert_eq!(identity.model.as_deref(), Some("ULT3580-TD8"));
        assert_eq!(identity.serial.as_deref(), Some("IBM123456"));
        assert_eq!(identity.firmware.as_deref(), Some("J6M1"));
        assert_eq!(identity.position_lost, Some(true));
        assert_eq!(identity.driver_options, Some(MT_ST_NOWAIT));
        assert_eq!(identity.immediate_mode, Some(true));
        assert_eq!(identity.in_flight, Some(2));
    }

    #[test]
    fn position_loss_preserves_rewind_and_unload_recovery_evidence() {
        assert_eq!(parse_sysfs_bool("1\n"), Some(true));
        assert_eq!(parse_sysfs_bool("0\n"), Some(false));
        assert_eq!(parse_sysfs_bool("unknown"), None);

        let mut status = parse_mt_success("BOT ONLINE", "");
        assert_eq!(status.ready, Some(true));
        apply_position_loss(Some(true), &mut status);
        assert_eq!(status.ready, Some(true));
        assert_eq!(status.status, "position_lost");
        assert_eq!(status.at_bot, Some(true));
    }
}
