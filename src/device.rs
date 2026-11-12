//! Read-only discovery for explicitly mapped Linux SCSI tape devices.
//!
//! Only identity and `mt status` are observed here. Motion, format, erase, file-mark, and data
//! commands remain behind the qualified tape-engine boundary.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::tool_runner::{self, ToolCommand, ToolError};

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_OUTPUT_LIMIT: usize = 16 * 1024;

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
    pub scsi_address: Option<String>,
    pub generic_path: Option<String>,
    pub sysfs_state: Option<String>,
    pub position_lost: Option<bool>,
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
            "scsi_address": self.scsi_address,
            "generic_path": self.generic_path,
            "sysfs_state": self.sysfs_state,
            "position_lost": self.position_lost,
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
}

#[derive(Debug)]
struct MtStatus {
    accessible: bool,
    ready: Option<bool>,
    medium_loaded: Option<bool>,
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
    let metadata = match fs::metadata(device) {
        Ok(metadata) => metadata,
        Err(error) => {
            return SasProbeResult::Missing(MissingSasDrive {
                observed_path: device.display().to_string(),
                reason: format!("device node is unavailable: {error}"),
                observed_at,
            });
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_char_device() {
            return SasProbeResult::Missing(MissingSasDrive {
                observed_path: device.display().to_string(),
                reason: "configured path is not a character device".to_owned(),
                observed_at,
            });
        }
    }

    let device_name = device
        .file_name()
        .and_then(|name| name.to_str())
        .expect("validated tape paths always have a UTF-8 file name");
    let identity = read_sysfs_identity(&config.sysfs_root, device_name);
    let (stable_id, identity_basis, identity_verified) = stable_identity(&identity, device);
    let mut mt_status = observe_mt_status(&config.mt_path, device).await;
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
        scsi_address: identity.scsi_address,
        generic_path: identity.generic_path,
        sysfs_state: identity.state,
        position_lost: identity.position_lost,
        accessible: mt_status.accessible,
        ready: mt_status.ready,
        medium_loaded: mt_status.medium_loaded,
        status: mt_status.status,
        status_detail: mt_status.detail,
        raw_status: mt_status.raw,
        observed_at,
    }))
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
    }
}

fn parse_sysfs_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn apply_position_loss(position_lost: Option<bool>, status: &mut MtStatus) {
    if position_lost == Some(true) {
        status.ready = Some(false);
        status.status = "position_lost".to_owned();
        status.detail = Some(
            "the kernel reports tape position was lost during reset; physical operations are blocked"
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
    MtStatus {
        accessible: true,
        ready: Some(online),
        medium_loaded: Some(online),
        status: if online { "ready" } else { "no_media" }.to_owned(),
        detail: (!online).then(|| "drive responded but no ready medium was reported".to_owned()),
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

        let empty = parse_mt_success("General status bits on:\n DR_OPEN IM_REP_EN\n", "");
        assert!(empty.accessible);
        assert_eq!(empty.status, "no_media");
        assert_eq!(empty.medium_loaded, Some(false));
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
            scsi_address: None,
            generic_path: None,
            sysfs_state: None,
            position_lost: Some(false),
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
        fs::write(device.join("vendor"), "IBM     \n").expect("vendor");
        fs::write(device.join("model"), "ULT3580-TD8     \n").expect("model");
        fs::write(device.join("rev"), "J6M1\n").expect("firmware");
        fs::write(tape_class.join("position_lost_in_reset"), "1\n").expect("position flag");
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
    }

    #[test]
    fn position_loss_overrides_an_online_mt_result() {
        assert_eq!(parse_sysfs_bool("1\n"), Some(true));
        assert_eq!(parse_sysfs_bool("0\n"), Some(false));
        assert_eq!(parse_sysfs_bool("unknown"), None);

        let mut status = parse_mt_success("BOT ONLINE", "");
        assert_eq!(status.ready, Some(true));
        apply_position_loss(Some(true), &mut status);
        assert_eq!(status.ready, Some(false));
        assert_eq!(status.status, "position_lost");
    }
}
