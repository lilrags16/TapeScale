//! Supported Bacula Community 15.0.3 process boundary for the first physical data path.
//!
//! This module deliberately does not make the TapeScale process a Bacula implementation. It
//! renders a small, fixed Bacula configuration and communicates with a restricted Console using
//! the supported `bconsole` programmatic commands. User-controlled values are validated before
//! they can reach either a configuration file or Console stdin.

use std::{
    fmt,
    num::{NonZeroU32, NonZeroU64},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Mutex as AsyncMutex,
    time::{sleep, timeout, Instant},
};
use uuid::Uuid;

pub const BACULA_VERSION: &str = "15.0.3";
pub const BCONSOLE_PROGRAM: &str = "/usr/sbin/bconsole";
pub const BCONSOLE_CONFIG: &str = "/etc/tapescale/bconsole.conf";
pub const DIRECTOR_PROGRAM: &str = "/usr/sbin/bacula-dir";
pub const DIRECTOR_CONFIG: &str = "/etc/tapescale/bacula-dir.conf";
pub const STORAGE_DAEMON_PROGRAM: &str = "/usr/sbin/bacula-sd";
pub const STORAGE_DAEMON_CONFIG: &str = "/etc/tapescale/bacula-sd.conf";
pub const FILE_DAEMON_PROGRAM: &str = "/usr/sbin/bacula-fd";
pub const FILE_DAEMON_CONFIG: &str = "/etc/tapescale/bacula-fd.conf";

pub const DIRECTOR_NAME: &str = "tapescale-dir";
pub const STORAGE_DAEMON_NAME: &str = "tapescale-sd";
pub const FILE_DAEMON_NAME: &str = "tapescale-fd";
pub const CONSOLE_NAME: &str = "tapescale-adapter";
pub const CATALOG_NAME: &str = "TS-Catalog";
pub const STORAGE_NAME: &str = "TS-LTO";
pub const DEVICE_NAME: &str = "TS-LTO-Drive";
pub const MEDIA_TYPE: &str = "LTO";
pub const POOL_NAME_PREFIX: &str = "TS-LTO-";
pub const FILESET_NAME: &str = "TS-Source";
pub const BACKUP_JOB_NAME: &str = "TS-Full";
pub const VERIFY_JOB_NAME: &str = "TS-VerifyData";
pub const RESTORE_JOB_NAME: &str = "TS-Restore";
pub const DATA_SPOOL_DIRECTORY: &str = "/var/lib/tapescale/spool";
pub const DEFAULT_DATA_SPOOL_MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// Bacula's one-gigabyte default writes an EOF/filemark far too frequently for
/// modern LTO drives. This 32 GiB alpha baseline follows current LTO-9 tuning
/// guidance but still requires generation-specific physical qualification; it
/// changes positioning granularity, not archive capacity.
pub const MAXIMUM_TAPE_FILE_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// File-level compression performed by the Bacula File Daemon before data is
/// sent to the tape drive.  The explicit levels keep the on-media format
/// reproducible instead of relying on a host-specific default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaculaCompression {
    #[default]
    None,
    GzipFast,
    GzipBalanced,
    GzipBest,
    Lzo,
}

impl BaculaCompression {
    /// Translate the stable policy/API names into an explicit Bacula codec.
    /// The packaged Bacula build and container roundtrip tests explicitly
    /// include LZO. Prefer it for `auto`: its low CPU cost is materially less
    /// likely than gzip to starve a streaming tape drive.
    pub fn from_policy_mode(value: &str) -> Result<Self, BaculaValidationError> {
        match value {
            "none" => Ok(Self::None),
            "auto" => Ok(Self::Lzo),
            "gzip" => Ok(Self::GzipBalanced),
            "lzo" => Ok(Self::Lzo),
            _ => Err(BaculaValidationError::InvalidValue {
                field: "compression_mode",
            }),
        }
    }

    /// Reconstruct the exact codec already committed to a durable run.
    ///
    /// Automatic policy changed from GZIP1 to LZO in alpha.9. Keeping the
    /// resolved value in this decision prevents an older queued run from
    /// silently changing its on-media format after an upgrade.
    pub fn from_persisted_policy(
        requested: &str,
        resolved: &str,
    ) -> Result<Self, BaculaValidationError> {
        match (requested, resolved) {
            ("auto", "gzip") => Ok(Self::GzipFast),
            ("auto", "lzo") | ("lzo", "lzo") => Ok(Self::Lzo),
            ("gzip", "gzip") => Ok(Self::GzipBalanced),
            ("none", "none") => Ok(Self::None),
            _ => Err(BaculaValidationError::InvalidValue {
                field: "resolved_compression",
            }),
        }
    }

    const fn directive(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::GzipFast => Some("GZIP1"),
            Self::GzipBalanced => Some("GZIP6"),
            Self::GzipBest => Some("GZIP9"),
            Self::Lzo => Some("LZO"),
        }
    }
}

/// How Bacula moves file data from the File Daemon to tape.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaculaDataDelivery {
    /// Stage bounded segments on disk before writing them to tape.
    #[default]
    DiskSpool,
    /// Send the File Daemon stream directly to the Storage Daemon and tape.
    Direct,
}

/// Data-plane settings that control delivery and physical representation.
/// `maximum_volumes = None` deliberately renders Bacula's value `0`, meaning
/// no configured volume-count ceiling; the finite set of TapeScale-authorized
/// cartridges remains the safety boundary for each run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BaculaDataOptions {
    pub compression: BaculaCompression,
    pub maximum_volumes: Option<NonZeroU32>,
    pub delivery: BaculaDataDelivery,
    maximum_spool_size: NonZeroU64,
}

impl BaculaDataOptions {
    #[must_use]
    pub const fn single_volume(compression: BaculaCompression) -> Self {
        Self {
            compression,
            maximum_volumes: NonZeroU32::new(1),
            delivery: BaculaDataDelivery::DiskSpool,
            maximum_spool_size: NonZeroU64::new(DEFAULT_DATA_SPOOL_MAX_BYTES)
                .expect("the fixed spool limit is non-zero"),
        }
    }

    #[must_use]
    pub const fn spanning(compression: BaculaCompression) -> Self {
        Self {
            compression,
            maximum_volumes: None,
            delivery: BaculaDataDelivery::DiskSpool,
            maximum_spool_size: NonZeroU64::new(DEFAULT_DATA_SPOOL_MAX_BYTES)
                .expect("the fixed spool limit is non-zero"),
        }
    }

    #[must_use]
    pub const fn with_delivery(mut self, delivery: BaculaDataDelivery) -> Self {
        self.delivery = delivery;
        self
    }

    #[must_use]
    pub const fn data_spooling_enabled(self) -> bool {
        matches!(self.delivery, BaculaDataDelivery::DiskSpool)
    }

    /// Bound Bacula's disk buffer without limiting the backup itself. Once
    /// this many bytes have been staged, Bacula despools them to tape and
    /// then resumes filling the buffer.
    #[must_use]
    pub const fn with_maximum_spool_size(mut self, maximum_spool_size: NonZeroU64) -> Self {
        self.maximum_spool_size = maximum_spool_size;
        self
    }

    #[must_use]
    pub const fn maximum_spool_size_bytes(self) -> u64 {
        self.maximum_spool_size.get()
    }

    const fn rendered_maximum_volumes(self) -> u32 {
        match self.maximum_volumes {
            Some(limit) => limit.get(),
            None => 0,
        }
    }
}

impl Default for BaculaDataOptions {
    fn default() -> Self {
        Self::single_volume(BaculaCompression::None)
    }
}

const SOURCE_ROOT: &str = "/source";
const RESTORE_ROOT: &str = "/restore";
const MAX_SECRET_BYTES: usize = 256;
const MAX_ADDRESS_BYTES: usize = 253;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONFIGURED_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TIMEOUT: Duration = Duration::from_secs(300);
const CORRELATION_TIMEOUT: Duration = Duration::from_secs(15);
const CORRELATION_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Fixed process specifications for an external supervisor or a dedicated data-plane entrypoint.
///
/// The adapter does not start these daemons itself: doing that during an API request could create
/// a second Storage Daemon competing for the tape. A supervisor should validate all three configs,
/// then run Storage and File daemons before the Director. `foreground_command` always uses direct
/// argv and never invokes a shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaculaDaemon {
    Director,
    StorageDaemon,
    FileDaemon,
}

impl BaculaDaemon {
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Director => DIRECTOR_PROGRAM,
            Self::StorageDaemon => STORAGE_DAEMON_PROGRAM,
            Self::FileDaemon => FILE_DAEMON_PROGRAM,
        }
    }

    #[must_use]
    pub const fn config_path(self) -> &'static str {
        match self {
            Self::Director => DIRECTOR_CONFIG,
            Self::StorageDaemon => STORAGE_DAEMON_CONFIG,
            Self::FileDaemon => FILE_DAEMON_CONFIG,
        }
    }

    #[must_use]
    pub fn validation_command(self) -> Command {
        let mut command = Command::new(self.program());
        command.args(["-t", "-c", self.config_path()]);
        command
    }

    #[must_use]
    pub fn foreground_command(self) -> Command {
        let mut command = Command::new(self.program());
        command
            .args(["-f", "-c", self.config_path()])
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// Safe startup ordering after the PostgreSQL catalog is ready and the schema exists.
    pub const STARTUP_ORDER: [Self; 3] = [Self::StorageDaemon, Self::FileDaemon, Self::Director];
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BaculaValidationError {
    #[error("{field} must be {expected}")]
    InvalidPath {
        field: &'static str,
        expected: &'static str,
    },
    #[error("{field} contains unsupported or unsafe characters")]
    InvalidValue { field: &'static str },
    #[error("{field} must not be empty")]
    Empty { field: &'static str },
    #[error("{field} is outside the supported range")]
    OutOfRange { field: &'static str },
}

/// A normalized, non-root selection below the read-only `/source` mount.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaculaSourceSelection(PathBuf);

impl BaculaSourceSelection {
    pub fn from_relative(path: impl AsRef<Path>) -> Result<Self, BaculaValidationError> {
        let relative = validate_relative_subpath("source_selection", path.as_ref())?;
        Ok(Self(Path::new(SOURCE_ROOT).join(relative)))
    }

    pub fn from_absolute(path: impl Into<PathBuf>) -> Result<Self, BaculaValidationError> {
        let path = path.into();
        let relative =
            path.strip_prefix(SOURCE_ROOT)
                .map_err(|_| BaculaValidationError::InvalidPath {
                    field: "source_selection",
                    expected: "a normalized child path below /source",
                })?;
        let relative = validate_relative_subpath("source_selection", relative)?;
        let normalized = Path::new(SOURCE_ROOT).join(relative);
        if path != normalized {
            return Err(BaculaValidationError::InvalidPath {
                field: "source_selection",
                expected: "a normalized child path below /source",
            });
        }
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A normalized profile-specific directory below the writable `/restore` mount.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaculaRestoreSubpath(PathBuf);

impl BaculaRestoreSubpath {
    pub fn parse(path: impl AsRef<Path>) -> Result<Self, BaculaValidationError> {
        validate_relative_subpath("restore_subpath", path.as_ref()).map(Self)
    }

    #[must_use]
    pub fn destination(&self, run_id: Uuid) -> BaculaRestoreDestination {
        BaculaRestoreDestination {
            path: Path::new(RESTORE_ROOT)
                .join(&self.0)
                .join(run_id.to_string()),
            run_id,
        }
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaRestoreDestination {
    path: PathBuf,
    run_id: Uuid,
}

impl BaculaRestoreDestination {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn run_id(&self) -> Uuid {
        self.run_id
    }
}

fn validate_relative_subpath(
    field: &'static str,
    path: &Path,
) -> Result<PathBuf, BaculaValidationError> {
    let value = path
        .to_str()
        .ok_or(BaculaValidationError::InvalidValue { field })?;
    if value.is_empty() {
        return Err(BaculaValidationError::Empty { field });
    }
    if value.len() > 512
        || path.is_absolute()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.split('/').any(|segment| {
            segment.is_empty()
                || matches!(segment, "." | "..")
                || segment
                    .chars()
                    .any(|character| character.is_control() || matches!(character, '"' | '\\'))
        })
    {
        return Err(BaculaValidationError::InvalidPath {
            field,
            expected: "a normalized relative path containing no traversal or command characters",
        });
    }
    Ok(PathBuf::from(value))
}

/// Literal container paths used by the Bacula data plane.
///
/// The mount roots remain fixed, while the FileSet must select a validated child path. Host paths
/// never enter Bacula configuration or Console commands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaculaPaths {
    source_root: PathBuf,
    source_selection: BaculaSourceSelection,
    restore_root: PathBuf,
    tape_device: PathBuf,
}

impl BaculaPaths {
    pub fn new(
        source_root: impl Into<PathBuf>,
        source_selection: BaculaSourceSelection,
        restore_root: impl Into<PathBuf>,
        tape_device: impl Into<PathBuf>,
    ) -> Result<Self, BaculaValidationError> {
        let paths = Self {
            source_root: source_root.into(),
            source_selection,
            restore_root: restore_root.into(),
            tape_device: tape_device.into(),
        };
        paths.validate()?;
        Ok(paths)
    }

    pub fn production(
        source_selection: BaculaSourceSelection,
        tape_device: impl Into<PathBuf>,
    ) -> Result<Self, BaculaValidationError> {
        Self::new(SOURCE_ROOT, source_selection, RESTORE_ROOT, tape_device)
    }

    pub fn from_relative_source(
        source_subpath: impl AsRef<Path>,
        tape_device: impl Into<PathBuf>,
    ) -> Result<Self, BaculaValidationError> {
        Self::production(
            BaculaSourceSelection::from_relative(source_subpath)?,
            tape_device,
        )
    }

    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    pub fn source_selection(&self) -> &BaculaSourceSelection {
        &self.source_selection
    }

    pub fn restore_root(&self) -> &Path {
        &self.restore_root
    }

    pub fn tape_device(&self) -> &Path {
        &self.tape_device
    }

    fn validate(&self) -> Result<(), BaculaValidationError> {
        if self.source_root != Path::new(SOURCE_ROOT) {
            return Err(BaculaValidationError::InvalidPath {
                field: "source_root",
                expected: "the literal absolute path /source",
            });
        }
        if self.source_selection.as_path() == self.source_root
            || !self
                .source_selection
                .as_path()
                .starts_with(&self.source_root)
        {
            return Err(BaculaValidationError::InvalidPath {
                field: "source_selection",
                expected: "a normalized child path below /source",
            });
        }
        if self.restore_root != Path::new(RESTORE_ROOT) {
            return Err(BaculaValidationError::InvalidPath {
                field: "restore_root",
                expected: "the literal absolute path /restore",
            });
        }
        if !is_non_rewinding_tape_path(&self.tape_device) {
            return Err(BaculaValidationError::InvalidPath {
                field: "tape_device",
                expected: "an exact non-rewinding device path /dev/nstN",
            });
        }
        Ok(())
    }
}

fn is_non_rewinding_tape_path(path: &Path) -> bool {
    let Some(value) = path.to_str() else {
        return false;
    };
    let Some(index) = value.strip_prefix("/dev/nst") else {
        return false;
    };
    !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && (index == "0" || !index.starts_with('0'))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaculaEndpoints {
    director_address: String,
    storage_address: String,
    file_daemon_address: String,
    catalog_address: String,
    catalog_port: u16,
    catalog_database: String,
    catalog_user: String,
}

impl BaculaEndpoints {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        director_address: impl Into<String>,
        storage_address: impl Into<String>,
        file_daemon_address: impl Into<String>,
        catalog_address: impl Into<String>,
        catalog_port: u16,
        catalog_database: impl Into<String>,
        catalog_user: impl Into<String>,
    ) -> Result<Self, BaculaValidationError> {
        let endpoints = Self {
            director_address: director_address.into(),
            storage_address: storage_address.into(),
            file_daemon_address: file_daemon_address.into(),
            catalog_address: catalog_address.into(),
            catalog_port,
            catalog_database: catalog_database.into(),
            catalog_user: catalog_user.into(),
        };
        endpoints.validate()?;
        Ok(endpoints)
    }

    fn validate(&self) -> Result<(), BaculaValidationError> {
        for (field, value) in [
            ("director_address", self.director_address.as_str()),
            ("storage_address", self.storage_address.as_str()),
            ("file_daemon_address", self.file_daemon_address.as_str()),
            ("catalog_address", self.catalog_address.as_str()),
        ] {
            validate_address(field, value)?;
        }
        validate_identifier("catalog_database", &self.catalog_database)?;
        validate_identifier("catalog_user", &self.catalog_user)?;
        if self.catalog_port == 0 {
            return Err(BaculaValidationError::OutOfRange {
                field: "catalog_port",
            });
        }
        Ok(())
    }
}

/// Credentials are intentionally not `Debug` so routine error reporting cannot disclose them.
#[derive(Clone, Eq, PartialEq)]
pub struct BaculaCredentials {
    director_password: String,
    storage_password: String,
    file_daemon_password: String,
    console_password: String,
    catalog_password: String,
}

impl BaculaCredentials {
    pub fn new(
        director_password: impl Into<String>,
        storage_password: impl Into<String>,
        file_daemon_password: impl Into<String>,
        console_password: impl Into<String>,
        catalog_password: impl Into<String>,
    ) -> Result<Self, BaculaValidationError> {
        let credentials = Self {
            director_password: director_password.into(),
            storage_password: storage_password.into(),
            file_daemon_password: file_daemon_password.into(),
            console_password: console_password.into(),
            catalog_password: catalog_password.into(),
        };
        credentials.validate()?;
        Ok(credentials)
    }

    fn validate(&self) -> Result<(), BaculaValidationError> {
        for (field, value) in [
            ("director_password", self.director_password.as_str()),
            ("storage_password", self.storage_password.as_str()),
            ("file_daemon_password", self.file_daemon_password.as_str()),
            ("console_password", self.console_password.as_str()),
            ("catalog_password", self.catalog_password.as_str()),
        ] {
            validate_secret(field, value)?;
        }
        Ok(())
    }
}

impl fmt::Debug for BaculaCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BaculaCredentials")
            .field("director_password", &"[redacted]")
            .field("storage_password", &"[redacted]")
            .field("file_daemon_password", &"[redacted]")
            .field("console_password", &"[redacted]")
            .field("catalog_password", &"[redacted]")
            .finish()
    }
}

fn validate_address(field: &'static str, value: &str) -> Result<(), BaculaValidationError> {
    if value.is_empty() {
        return Err(BaculaValidationError::Empty { field });
    }
    if value.len() > MAX_ADDRESS_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'[' | b']')
        })
    {
        return Err(BaculaValidationError::InvalidValue { field });
    }
    Ok(())
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), BaculaValidationError> {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(BaculaValidationError::Empty { field });
    };
    if value.len() > 63
        || !(first.is_ascii_alphabetic() || first == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(BaculaValidationError::InvalidValue { field });
    }
    Ok(())
}

fn validate_secret(field: &'static str, value: &str) -> Result<(), BaculaValidationError> {
    if value.is_empty() {
        return Err(BaculaValidationError::Empty { field });
    }
    if value.len() > MAX_SECRET_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'"' | b'\\'))
    {
        return Err(BaculaValidationError::InvalidValue { field });
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedBaculaConfigs {
    pub director: String,
    pub storage_daemon: String,
    pub file_daemon: String,
    pub bconsole: String,
}

#[derive(Clone)]
pub struct BaculaConfig {
    paths: BaculaPaths,
    endpoints: BaculaEndpoints,
    credentials: BaculaCredentials,
    pool_name: BaculaPoolName,
    data_options: BaculaDataOptions,
    data_spool_directory: PathBuf,
}

impl BaculaConfig {
    pub fn new(
        paths: BaculaPaths,
        endpoints: BaculaEndpoints,
        credentials: BaculaCredentials,
        pool_name: BaculaPoolName,
    ) -> Self {
        Self {
            paths,
            endpoints,
            credentials,
            pool_name,
            data_options: BaculaDataOptions::default(),
            data_spool_directory: PathBuf::from(DATA_SPOOL_DIRECTORY),
        }
    }

    /// Select compression and pool spanning behavior for this isolated run.
    #[must_use]
    pub const fn with_data_options(mut self, data_options: BaculaDataOptions) -> Self {
        self.data_options = data_options;
        self
    }

    /// Isolate one runtime's spool files so live occupancy can be attributed
    /// to the exact fenced run without inspecting Bacula-generated names.
    #[must_use]
    pub fn with_run_spool_directory(mut self, run_id: Uuid) -> Self {
        self.data_spool_directory =
            Path::new(DATA_SPOOL_DIRECTORY).join(run_id.simple().to_string());
        self
    }

    #[must_use]
    pub fn render(&self) -> RenderedBaculaConfigs {
        RenderedBaculaConfigs {
            director: self.render_director(),
            storage_daemon: self.render_storage_daemon(),
            file_daemon: self.render_file_daemon(),
            bconsole: self.render_bconsole(),
        }
    }

    fn render_director(&self) -> String {
        let endpoint = &self.endpoints;
        let credentials = &self.credentials;
        let pool_name = self.pool_name.as_str();
        let maximum_volumes = self.data_options.rendered_maximum_volumes();
        let spool_data = if self.data_options.data_spooling_enabled() {
            "yes"
        } else {
            "no"
        };
        let compression_directive = self
            .data_options
            .compression
            .directive()
            .map_or_else(String::new, |algorithm| {
                format!("      Compression = {algorithm}\n")
            });
        format!(
            r#"Director {{
  Name = {DIRECTOR_NAME}
  DIRport = 9101
  DirAddress = 127.0.0.1
  QueryFile = "/etc/bacula/scripts/query.sql"
  Working Directory = "/var/lib/bacula"
  Pid Directory = "/run/bacula"
  Maximum Concurrent Jobs = 1
  Password = "{director_password}"
  Messages = Daemon
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

Catalog {{
  Name = {CATALOG_NAME}
  dbname = "{catalog_database}"
  dbuser = "{catalog_user}"
  dbpassword = "{catalog_password}"
  dbaddress = "{catalog_address}"
  dbport = {catalog_port}
}}

Client {{
  Name = {FILE_DAEMON_NAME}
  Address = "{file_daemon_address}"
  FDPort = 9102
  Catalog = {CATALOG_NAME}
  Password = "{file_daemon_password}"
  Auto Prune = no
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

Storage {{
  Name = {STORAGE_NAME}
  Address = "{storage_address}"
  SDPort = 9103
  Password = "{storage_password}"
  Device = {DEVICE_NAME}
  Media Type = {MEDIA_TYPE}
  Maximum Concurrent Jobs = 1
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

Pool {{
  Name = {pool_name}
  Pool Type = Backup
  Recycle = no
  Auto Prune = no
  Maximum Volumes = {maximum_volumes}
}}

FileSet {{
  Name = {FILESET_NAME}
  Include {{
    Options {{
      Signature = SHA256
      ACL Support = yes
      XATTR Support = yes
      One FS = yes
      Sparse = yes
{compression_directive}    }}
    File = "{source_selection}"
  }}
}}

Job {{
  Name = {BACKUP_JOB_NAME}
  Type = Backup
  Level = Full
  Accurate = yes
  Client = {FILE_DAEMON_NAME}
  FileSet = {FILESET_NAME}
  Storage = {STORAGE_NAME}
  Pool = {pool_name}
  Messages = Standard
  # Data delivery is selected explicitly for every run. Direct mode bypasses
  # the batch data spool while retaining the small catalog-attribute spool.
  Spool Data = {spool_data}
  # Keep attribute spooling explicit in both delivery modes so an incomplete
  # tape job cannot publish catalog entries for data that was never finalized.
  Spool Attributes = yes
  Write Bootstrap = "/var/lib/bacula/bootstrap/%i.bsr"
}}

Job {{
  Name = {VERIFY_JOB_NAME}
  Type = Verify
  Level = Data
  Accurate = yes
  Verify Job = {BACKUP_JOB_NAME}
  Client = {FILE_DAEMON_NAME}
  FileSet = {FILESET_NAME}
  Storage = {STORAGE_NAME}
  Pool = {pool_name}
  Messages = Standard
}}

Job {{
  Name = {RESTORE_JOB_NAME}
  Type = Restore
  Client = {FILE_DAEMON_NAME}
  FileSet = {FILESET_NAME}
  Storage = {STORAGE_NAME}
  Pool = {pool_name}
  Messages = Standard
  Where = {RESTORE_ROOT}
  Replace = Always
}}

Console {{
  Name = {CONSOLE_NAME}
  Password = "{console_password}"
  Command ACL = run, cancel, status, .status, .api, .jlist, .bvfs_restore, .bvfs_cleanup, restore, label, mount, unmount, release
  # Bacula refuses Storage-Daemon running-status records when either of these
  # two visibility ACLs is narrower. The generated configuration still defines
  # only TapeScale's three fixed Jobs and one fixed Client; backup/restore
  # capabilities remain explicitly scoped below.
  Job ACL = *all*
  Client ACL = *all*
  Backup Client ACL = {FILE_DAEMON_NAME}
  Restore Client ACL = {FILE_DAEMON_NAME}
  Storage ACL = {STORAGE_NAME}
  Pool ACL = {pool_name}
  FileSet ACL = {FILESET_NAME}
  Catalog ACL = {CATALOG_NAME}
  Directory ACL = *all*
  User Id ACL = *all*
  Where ACL = {RESTORE_ROOT}, {RESTORE_ROOT}/*
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

Messages {{
  Name = Standard
  catalog = all
  console = all, !skipped, !saved
}}

Messages {{
  Name = Daemon
  console = all, !skipped, !saved
}}
"#,
            director_password = credentials.director_password,
            catalog_database = endpoint.catalog_database,
            catalog_user = endpoint.catalog_user,
            catalog_password = credentials.catalog_password,
            catalog_address = endpoint.catalog_address,
            catalog_port = endpoint.catalog_port,
            file_daemon_address = endpoint.file_daemon_address,
            file_daemon_password = credentials.file_daemon_password,
            storage_address = endpoint.storage_address,
            storage_password = credentials.storage_password,
            console_password = credentials.console_password,
            source_selection = self.paths.source_selection.as_path().display(),
            maximum_volumes = maximum_volumes,
            compression_directive = compression_directive,
            spool_data = spool_data,
        )
    }

    fn render_storage_daemon(&self) -> String {
        let maximum_spool_size = self.data_options.maximum_spool_size_bytes();
        let maximum_tape_file_size = MAXIMUM_TAPE_FILE_BYTES;
        let data_spool_directory = self.data_spool_directory.display();
        format!(
            r#"Storage {{
  Name = {STORAGE_DAEMON_NAME}
  SDPort = 9103
  SDAddress = 127.0.0.1
  # Attribute spool files always use the SD Working Directory. Co-locate them
  # with the data spool so TapeScale can measure and reserve this filesystem.
  Working Directory = "{data_spool_directory}"
  Pid Directory = "/run/bacula"
  Plugin Directory = "/usr/lib/bacula"
  # One active data job plus one read-only status connection. The Device
  # resource remains single-job, so physical tape access is still serialized.
  Maximum Concurrent Jobs = 2
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

Director {{
  Name = {DIRECTOR_NAME}
  Password = "{storage_password}"
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

Device {{
  Name = {DEVICE_NAME}
  Media Type = {MEDIA_TYPE}
  Archive Device = "{tape_device}"
  Automatic Mount = yes
  Always Open = no
  Removable Media = yes
  Random Access = no
  Label Media = no
  Maximum File Size = {maximum_tape_file_size}
  # Keep the local File/Storage stream on Bacula's documented high-throughput
  # buffer size. Both ends use the same value.
  Maximum Network Buffer Size = 65536
  Spool Directory = "{data_spool_directory}"
  Maximum Spool Size = {maximum_spool_size}
  Maximum Job Spool Size = {maximum_spool_size}
  Maximum Concurrent Jobs = 1
}}

Messages {{
  Name = Standard
  director = {DIRECTOR_NAME} = all
}}
"#,
            storage_password = self.credentials.storage_password,
            tape_device = self.paths.tape_device.display(),
            data_spool_directory = data_spool_directory,
            maximum_spool_size = maximum_spool_size,
            maximum_tape_file_size = maximum_tape_file_size,
        )
    }

    fn render_file_daemon(&self) -> String {
        format!(
            r#"Director {{
  Name = {DIRECTOR_NAME}
  Password = "{file_daemon_password}"
  Allowed Backup Directories = {SOURCE_ROOT}
  Allowed Restore Directories = {RESTORE_ROOT}
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

FileDaemon {{
  Name = {FILE_DAEMON_NAME}
  FDPort = 9102
  FDAddress = 127.0.0.1
  Working Directory = "/var/lib/bacula"
  Pid Directory = "/run/bacula"
  Plugin Directory = "/usr/lib/bacula"
  Maximum Concurrent Jobs = 1
  Maximum Network Buffer Size = 65536
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

Messages {{
  Name = Standard
  director = {DIRECTOR_NAME} = all, !skipped, !restored, !verified, !saved
}}
"#,
            file_daemon_password = self.credentials.file_daemon_password,
        )
    }

    fn render_bconsole(&self) -> String {
        format!(
            r#"Director {{
  Name = {DIRECTOR_NAME}
  DIRport = 9101
  Address = "{director_address}"
  Password = "{console_password}"
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}

Console {{
  Name = {CONSOLE_NAME}
  Director = {DIRECTOR_NAME}
  Password = "{console_password}"
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}}
"#,
            director_address = self.endpoints.director_address,
            console_password = self.credentials.console_password,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BaculaJobId(NonZeroU64);

impl BaculaJobId {
    pub fn new(value: u64) -> Result<Self, BaculaValidationError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(BaculaValidationError::OutOfRange { field: "job_id" })
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for BaculaJobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// One positive FileIndex from Bacula's catalog.
///
/// Selected restore commands never contain a user-provided pathname. The
/// catalog resolver turns an authenticated path into this numeric identity
/// before anything is rendered for bconsole.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BaculaFileIndex(NonZeroU64);

impl BaculaFileIndex {
    pub fn new(value: u64) -> Result<Self, BaculaValidationError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(BaculaValidationError::OutOfRange {
                field: "file_index",
            })
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for BaculaFileIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// One positive PathId from Bacula's catalog.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BaculaPathId(NonZeroU64);

impl BaculaPathId {
    pub fn new(value: u64) -> Result<Self, BaculaValidationError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(BaculaValidationError::OutOfRange { field: "path_id" })
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for BaculaPathId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A file selection remains paired with the exact backup JobId that owns its
/// FileIndex. Bacula's BVFS syntax accepts an alternating job,index list.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BaculaRestoreFileIndex {
    pub job_id: BaculaJobId,
    pub file_index: BaculaFileIndex,
}

/// Catalog identities for one immutable path-selection contract.
///
/// The constructor sorts and deduplicates identities, rejects an empty
/// selection, and requires every file pair to belong to the pinned backup job.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaRestoreSelection {
    backup_job_id: BaculaJobId,
    file_indexes: Vec<BaculaRestoreFileIndex>,
    directory_path_ids: Vec<BaculaPathId>,
}

impl BaculaRestoreSelection {
    pub fn new(
        backup_job_id: BaculaJobId,
        mut file_indexes: Vec<BaculaRestoreFileIndex>,
        mut directory_path_ids: Vec<BaculaPathId>,
    ) -> Result<Self, BaculaValidationError> {
        if file_indexes
            .iter()
            .any(|selection| selection.job_id != backup_job_id)
        {
            return Err(BaculaValidationError::InvalidValue {
                field: "restore_file_job_id",
            });
        }
        file_indexes.sort_unstable();
        file_indexes.dedup();
        directory_path_ids.sort_unstable();
        directory_path_ids.dedup();
        if file_indexes.is_empty() && directory_path_ids.is_empty() {
            return Err(BaculaValidationError::Empty {
                field: "restore_selection",
            });
        }
        Ok(Self {
            backup_job_id,
            file_indexes,
            directory_path_ids,
        })
    }

    #[must_use]
    pub const fn backup_job_id(&self) -> BaculaJobId {
        self.backup_job_id
    }

    #[must_use]
    pub fn file_indexes(&self) -> &[BaculaRestoreFileIndex] {
        &self.file_indexes
    }

    #[must_use]
    pub fn directory_path_ids(&self) -> &[BaculaPathId] {
        &self.directory_path_ids
    }
}

/// A BVFS restore-list table name that cannot contain user input.
///
/// Bacula 15.0.3 accepts only `b2` followed by decimal digits for these
/// tables. Encoding the server-generated run UUID as one u128 decimal integer
/// also keeps the identifier below PostgreSQL's 63-byte limit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaBvfsRestoreTable(String);

impl BaculaBvfsRestoreTable {
    #[must_use]
    pub fn for_run(run_id: Uuid) -> Self {
        Self(format!("b2{}", run_id.as_u128()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VolumeLabel(String);

impl VolumeLabel {
    pub fn parse(value: impl Into<String>) -> Result<Self, BaculaValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(BaculaValidationError::Empty {
                field: "volume_label",
            });
        }
        if value.len() > 127
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(BaculaValidationError::InvalidValue {
                field: "volume_label",
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Per-exercise Bacula pool. A unique pool keeps the one-volume ceiling useful
/// even when the catalog persists across many recovery exercises.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaPoolName(String);

impl BaculaPoolName {
    #[must_use]
    pub fn for_run(run_id: Uuid) -> Self {
        Self(format!("{POOL_NAME_PREFIX}{}", run_id.simple()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaculaOperationKind {
    Label,
    Mount,
    Release,
    FullBackup,
    DataVerify,
    FullRestore,
    PrepareSelectedRestore,
    SelectedRestore,
    CleanupSelectedRestore,
    Cancel,
    QueryJob,
    QueryLatestBackup,
    QueryLatestVerify,
    QueryLatestRestore,
    QueryStorage,
    QueryRequestedVolume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaculaJobResource {
    Backup,
    Verify,
    Restore,
}

impl BaculaJobResource {
    #[must_use]
    pub const fn resource_name(self) -> &'static str {
        match self {
            Self::Backup => BACKUP_JOB_NAME,
            Self::Verify => VERIFY_JOB_NAME,
            Self::Restore => RESTORE_JOB_NAME,
        }
    }

    const fn query_kind(self) -> BaculaOperationKind {
        match self {
            Self::Backup => BaculaOperationKind::QueryLatestBackup,
            Self::Verify => BaculaOperationKind::QueryLatestVerify,
            Self::Restore => BaculaOperationKind::QueryLatestRestore,
        }
    }

    const fn accepts_operation(self, operation: BaculaOperationKind) -> bool {
        matches!(
            (self, operation),
            (Self::Backup, BaculaOperationKind::FullBackup)
                | (Self::Verify, BaculaOperationKind::DataVerify)
                | (Self::Restore, BaculaOperationKind::FullRestore)
                | (Self::Restore, BaculaOperationKind::SelectedRestore)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BconsoleScript {
    kind: BaculaOperationKind,
    stdin: Vec<u8>,
    expects_json: bool,
}

impl BconsoleScript {
    fn console_command(
        kind: BaculaOperationKind,
        command: impl fmt::Display,
        expects_json: bool,
    ) -> Self {
        let stdin = format!(".api 2 api_opts=j\n{command}\n.quit\n").into_bytes();
        debug_assert!(!stdin.contains(&0));
        Self {
            kind,
            stdin,
            expects_json,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> BaculaOperationKind {
        self.kind
    }

    #[must_use]
    pub fn stdin(&self) -> &[u8] {
        &self.stdin
    }

    #[must_use]
    pub const fn expects_json(&self) -> bool {
        self.expects_json
    }
}

pub struct BaculaCommands;

impl BaculaCommands {
    #[must_use]
    pub fn label(volume: &VolumeLabel, pool: &BaculaPoolName) -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::Label,
            format_args!(
                "label storage={STORAGE_NAME} pool={} volume={} yes",
                pool.as_str(),
                volume.as_str()
            ),
            false,
        )
    }

    /// Ask the Storage Daemon to consume the currently loaded authorized
    /// volume after an operator or changer completes a media swap.
    #[must_use]
    pub fn mount() -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::Mount,
            format_args!("mount storage={STORAGE_NAME}"),
            false,
        )
    }

    /// Release the current volume before a manual or robotic media change.
    #[must_use]
    pub fn release() -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::Release,
            format_args!("release storage={STORAGE_NAME}"),
            false,
        )
    }

    #[must_use]
    pub fn full_backup() -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::FullBackup,
            format_args!("run job={BACKUP_JOB_NAME} level=Full yes"),
            false,
        )
    }

    #[must_use]
    pub fn data_verify(backup_job_id: BaculaJobId) -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::DataVerify,
            format_args!(
                "run job={VERIFY_JOB_NAME} level=Data accurate=yes jobid={backup_job_id} yes"
            ),
            false,
        )
    }

    #[must_use]
    pub fn full_restore(
        backup_job_id: BaculaJobId,
        destination: &BaculaRestoreDestination,
    ) -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::FullRestore,
            format_args!(
                "restore jobid={backup_job_id} restorejob={RESTORE_JOB_NAME} restoreclient={FILE_DAEMON_NAME} where=\"{}\" all done yes",
                destination.path().display()
            ),
            false,
        )
    }

    /// Ask Bacula's BVFS to materialize the exact numeric restore list.
    ///
    /// All rendered values are positive integer wrappers or the
    /// server-generated `b2<decimal UUID>` token. No pathname reaches Console
    /// stdin through this command.
    #[must_use]
    pub fn prepare_selected_restore(
        selection: &BaculaRestoreSelection,
        table: &BaculaBvfsRestoreTable,
    ) -> BconsoleScript {
        let mut command = format!(
            ".bvfs_restore path={} jobid={}",
            table.as_str(),
            selection.backup_job_id()
        );
        if !selection.file_indexes().is_empty() {
            command.push_str(" fileindex=");
            for (position, selected) in selection.file_indexes().iter().enumerate() {
                if position > 0 {
                    command.push(',');
                }
                use std::fmt::Write as _;
                write!(command, "{},{}", selected.job_id, selected.file_index)
                    .expect("writing to a String is infallible");
            }
        }
        if !selection.directory_path_ids().is_empty() {
            command.push_str(" dirid=");
            for (position, path_id) in selection.directory_path_ids().iter().enumerate() {
                if position > 0 {
                    command.push(',');
                }
                use std::fmt::Write as _;
                write!(command, "{path_id}").expect("writing to a String is infallible");
            }
        }
        BconsoleScript::console_command(BaculaOperationKind::PrepareSelectedRestore, command, false)
    }

    /// Dispatch a restore from the already materialized BVFS table.
    ///
    /// The table rows already carry the pinned backup JobId. Repeating it as a
    /// `jobid` argument makes Bacula append the same source twice to its
    /// relationship evidence; the explicit backup client keeps selection
    /// non-interactive without duplicating that identity.
    #[must_use]
    pub fn selected_restore(
        destination: &BaculaRestoreDestination,
        table: &BaculaBvfsRestoreTable,
    ) -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::SelectedRestore,
            format_args!(
                "restore client={FILE_DAEMON_NAME} restorejob={RESTORE_JOB_NAME} restoreclient={FILE_DAEMON_NAME} where=\"{}\" file=\"?{}\" done yes",
                destination.path().display(),
                table.as_str()
            ),
            false,
        )
    }

    /// Drop a server-generated BVFS restore-list table after dispatch.
    #[must_use]
    pub fn cleanup_selected_restore(table: &BaculaBvfsRestoreTable) -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::CleanupSelectedRestore,
            format_args!(".bvfs_cleanup path={}", table.as_str()),
            false,
        )
    }

    #[must_use]
    pub fn cancel(job_id: BaculaJobId) -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::Cancel,
            format_args!("cancel jobid={job_id} yes"),
            false,
        )
    }

    #[must_use]
    pub fn query_job(job_id: BaculaJobId) -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::QueryJob,
            format_args!(".jlist jobid={job_id}"),
            true,
        )
    }

    #[must_use]
    pub fn query_latest_backup(limit: u16) -> BconsoleScript {
        Self::query_latest(BaculaJobResource::Backup, limit)
    }

    #[must_use]
    pub fn query_latest_verify(limit: u16) -> BconsoleScript {
        Self::query_latest(BaculaJobResource::Verify, limit)
    }

    #[must_use]
    pub fn query_latest_restore(limit: u16) -> BconsoleScript {
        Self::query_latest(BaculaJobResource::Restore, limit)
    }

    fn query_latest(resource: BaculaJobResource, limit: u16) -> BconsoleScript {
        let limit = limit.clamp(1, 100);
        BconsoleScript::console_command(
            resource.query_kind(),
            format_args!(
                ".jlist job={} jobs order=desc limit={limit}",
                resource.resource_name()
            ),
            true,
        )
    }

    #[must_use]
    pub fn query_storage() -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::QueryStorage,
            format_args!(".status storage={STORAGE_NAME} devices"),
            true,
        )
    }

    /// Query Bacula 15's structured running-job status. Unlike JSON device
    /// status, this exposes the read DCR's exact requested VolumeName together
    /// with the correlated JobId, pool, and device.
    #[must_use]
    pub fn query_requested_volume() -> BconsoleScript {
        BconsoleScript::console_command(
            BaculaOperationKind::QueryRequestedVolume,
            format_args!(".status storage={STORAGE_NAME} running"),
            true,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BconsoleOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum BconsoleError {
    #[error("could not spawn the fixed bconsole program: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("bconsole I/O failed: {0}")]
    Io(#[source] std::io::Error),
    #[error("bconsole exceeded its {0:?} execution deadline")]
    Timeout(Duration),
    #[error("bconsole output exceeded the configured {limit}-byte bound")]
    OutputLimitExceeded { limit: usize },
    #[error("bconsole exited unsuccessfully with code {code:?}")]
    Unsuccessful { code: Option<i32> },
    #[error("bconsole executor configuration is invalid: {0}")]
    InvalidConfiguration(#[from] BaculaValidationError),
}

#[async_trait]
pub trait BconsoleExecutor: Send + Sync {
    async fn execute(&self, script: &BconsoleScript) -> Result<BconsoleOutput, BconsoleError>;
}

#[derive(Clone, Debug)]
pub struct DirectBconsoleExecutor {
    timeout: Duration,
    max_output_bytes: usize,
}

impl Default for DirectBconsoleExecutor {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl DirectBconsoleExecutor {
    pub fn new(timeout: Duration, max_output_bytes: usize) -> Result<Self, BaculaValidationError> {
        if timeout.is_zero() || timeout > MAX_TIMEOUT {
            return Err(BaculaValidationError::OutOfRange {
                field: "bconsole_timeout",
            });
        }
        if max_output_bytes == 0 || max_output_bytes > MAX_CONFIGURED_OUTPUT_BYTES {
            return Err(BaculaValidationError::OutOfRange {
                field: "bconsole_max_output_bytes",
            });
        }
        Ok(Self {
            timeout,
            max_output_bytes,
        })
    }
}

#[derive(Debug)]
struct BoundedRead {
    bytes: Vec<u8>,
    overflowed: bool,
}

async fn read_bounded<R>(mut reader: R, limit: usize) -> std::io::Result<BoundedRead>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut overflowed = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let retained = remaining.min(count);
        bytes.extend_from_slice(&chunk[..retained]);
        overflowed |= retained != count;
    }
    Ok(BoundedRead { bytes, overflowed })
}

#[async_trait]
impl BconsoleExecutor for DirectBconsoleExecutor {
    async fn execute(&self, script: &BconsoleScript) -> Result<BconsoleOutput, BconsoleError> {
        let mut child = Command::new(BCONSOLE_PROGRAM)
            .args(["-n", "-c", BCONSOLE_CONFIG])
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(BconsoleError::Spawn)?;

        let mut stdin = child.stdin.take().ok_or_else(|| {
            BconsoleError::Io(std::io::Error::other("bconsole stdin was not captured"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            BconsoleError::Io(std::io::Error::other("bconsole stdout was not captured"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            BconsoleError::Io(std::io::Error::other("bconsole stderr was not captured"))
        })?;

        stdin
            .write_all(script.stdin())
            .await
            .map_err(BconsoleError::Io)?;
        stdin.shutdown().await.map_err(BconsoleError::Io)?;
        drop(stdin);

        let completed = timeout(self.timeout, async {
            tokio::try_join!(
                read_bounded(stdout, self.max_output_bytes),
                read_bounded(stderr, self.max_output_bytes),
                child.wait()
            )
        })
        .await;

        let (stdout, stderr, status) = match completed {
            Ok(result) => result.map_err(BconsoleError::Io)?,
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(BconsoleError::Timeout(self.timeout));
            }
        };
        if stdout.overflowed || stderr.overflowed {
            return Err(BconsoleError::OutputLimitExceeded {
                limit: self.max_output_bytes,
            });
        }
        if !status.success() {
            return Err(BconsoleError::Unsuccessful {
                code: status.code(),
            });
        }
        Ok(BconsoleOutput {
            stdout: stdout.bytes,
            stderr: stderr.bytes,
            exit_code: status.code(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaculaJobState {
    Created,
    Blocked,
    WaitingForFileDaemon,
    WaitingForStorageDaemon,
    WaitingForMount,
    WaitingForMedia,
    WaitingForResource,
    Running,
    RunningWithErrors,
    Committing,
    Succeeded,
    CompletedWithErrors,
    CompletedWithWarnings,
    VerifyDifferences,
    Cancelled,
    Failed,
    Incomplete,
    Unknown,
}

impl BaculaJobState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::CompletedWithErrors
                | Self::CompletedWithWarnings
                | Self::VerifyDifferences
                | Self::Cancelled
                | Self::Failed
                | Self::Incomplete
        )
    }

    #[must_use]
    pub const fn is_strict_success(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

fn map_job_status(status: char, errors: u64) -> BaculaJobState {
    match status {
        'A' => BaculaJobState::Cancelled,
        'B' => BaculaJobState::Blocked,
        'C' | 't' => BaculaJobState::Created,
        'D' => BaculaJobState::VerifyDifferences,
        'E' | 'f' => BaculaJobState::Failed,
        'F' => BaculaJobState::WaitingForFileDaemon,
        'I' => BaculaJobState::Incomplete,
        'L' | 'a' | 'i' | 'l' => BaculaJobState::Committing,
        // Bacula's catalog contract distinguishes a request to mount the
        // current volume (`M`) from a request for a new volume (`m`). They
        // must never share a destructive continuation path.
        'M' => BaculaJobState::WaitingForMount,
        'm' => BaculaJobState::WaitingForMedia,
        'R' | 'u' | 'w' => BaculaJobState::Running,
        'S' => BaculaJobState::WaitingForStorageDaemon,
        'T' if errors == 0 => BaculaJobState::Succeeded,
        'T' => BaculaJobState::CompletedWithErrors,
        'W' => BaculaJobState::CompletedWithWarnings,
        'e' => BaculaJobState::RunningWithErrors,
        'c' | 'd' | 'j' | 'p' | 'q' | 's' => BaculaJobState::WaitingForResource,
        _ => BaculaJobState::Unknown,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaJobEvidence {
    pub job_id: BaculaJobId,
    pub resource_name: Option<String>,
    pub unique_name: Option<String>,
    pub raw_status: char,
    pub state: BaculaJobState,
    pub files: u64,
    pub bytes: u64,
    pub errors: u64,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
}

impl BaculaJobEvidence {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    #[must_use]
    pub const fn is_strict_success(&self) -> bool {
        self.state.is_strict_success()
    }
}

/// Exact in-memory counters reported by the Storage Daemon for one running
/// JobId. Unlike catalog-backed `.jlist`, these counters advance while data is
/// being staged in the disk spool.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaRunningJobEvidence {
    pub job_id: BaculaJobId,
    pub raw_status: char,
    pub state: BaculaJobState,
    pub files: u64,
    pub bytes: u64,
    pub errors: u64,
    pub average_bytes_per_second: Option<u64>,
    pub last_bytes_per_second: Option<u64>,
    pub spooling: Option<bool>,
    pub despooling: Option<bool>,
    pub despool_wait: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
pub enum BaculaJsonError {
    #[error("bconsole output was not UTF-8")]
    InvalidUtf8,
    #[error("bconsole output did not contain a structured Bacula job list")]
    MissingJobList,
    #[error("Bacula job record is missing a valid {0}")]
    MissingField(&'static str),
    #[error("Bacula returned an unsupported job status value")]
    InvalidStatus,
    #[error("Bacula returned an invalid job id")]
    InvalidJobId,
}

#[derive(Debug, thiserror::Error)]
pub enum BaculaStatusError {
    #[error("bconsole status output was not UTF-8")]
    InvalidUtf8,
    #[error("Bacula Storage status did not contain a top-level running-job array")]
    MissingRunningJobs,
    #[error("Bacula Storage running-job status was malformed")]
    MalformedRunningJobs,
    #[error("Bacula Storage status did not contain restore JobId {0}")]
    MissingJob(BaculaJobId),
    #[error("Bacula Storage status contained {actual} records for restore JobId {job_id}")]
    AmbiguousJob { job_id: BaculaJobId, actual: usize },
    #[error("Bacula Storage restore status is missing a valid {0}")]
    MissingField(&'static str),
    #[error("Bacula Storage restore status did not report waiting-for-mount status M")]
    NotWaitingForMount,
    #[error("Bacula Storage restore status reported the wrong read pool")]
    UnexpectedReadPool,
    #[error("Bacula Storage restore status reported the wrong read device")]
    UnexpectedReadDevice,
    #[error("Bacula Storage restore status unexpectedly reported a write volume")]
    UnexpectedWriteVolume,
    #[error("Bacula Storage restore status reported job errors")]
    ReportedErrors,
    #[error("Bacula Storage status exposed an invalid requested VolumeName")]
    InvalidRequestedVolume,
    #[error("Bacula Storage status wrote unexpected diagnostic output to stderr")]
    UnexpectedStderr,
}

#[derive(Debug, thiserror::Error)]
pub enum BaculaRunningStatusError {
    #[error("bconsole output was not UTF-8")]
    InvalidUtf8,
    #[error("Bacula Storage status did not contain a top-level running-job array")]
    MissingRunningJobs,
    #[error("Bacula Storage running-job status was malformed")]
    MalformedRunningJobs,
    #[error("Bacula Storage status contained {actual} records for JobId {job_id}")]
    AmbiguousJob { job_id: BaculaJobId, actual: usize },
    #[error("Bacula Storage running-job status is missing a valid {0}")]
    MissingField(&'static str),
    #[error("Bacula Storage running-job status returned an unsupported status value")]
    InvalidStatus,
    #[error("Bacula Storage status wrote unexpected diagnostic output to stderr")]
    UnexpectedStderr,
}

/// Parse the fixed Storage resource's API-v2 running list for one exact JobId.
///
/// A missing record is normal while a job is being handed between daemons and
/// returns `Ok(None)`. The parser accepts only the top-level `running` array,
/// ignores connection records without JobIds, and rejects duplicate or
/// malformed exact records rather than guessing across jobs.
pub fn parse_running_job_evidence(
    output: &[u8],
    expected_job_id: BaculaJobId,
) -> Result<Option<BaculaRunningJobEvidence>, BaculaRunningStatusError> {
    let text = std::str::from_utf8(output).map_err(|_| BaculaRunningStatusError::InvalidUtf8)?;
    let value = text
        .char_indices()
        .filter(|(_, character)| *character == '{')
        .find_map(|(offset, _)| {
            serde_json::Deserializer::from_str(&text[offset..])
                .into_iter::<Value>()
                .next()
                .and_then(Result::ok)
        })
        .ok_or(BaculaRunningStatusError::MissingRunningJobs)?;
    let root = value
        .as_object()
        .ok_or(BaculaRunningStatusError::MalformedRunningJobs)?;
    let records = root
        .get("running")
        .ok_or(BaculaRunningStatusError::MissingRunningJobs)?
        .as_array()
        .ok_or(BaculaRunningStatusError::MalformedRunningJobs)?;
    if records.iter().any(|record| !record.is_object()) {
        return Err(BaculaRunningStatusError::MalformedRunningJobs);
    }

    let mut matching = Vec::new();
    for record in records {
        let object = record
            .as_object()
            .expect("the running-job object shape was checked");
        let Some(job_id_value) = object.get("jobid") else {
            continue;
        };
        let Some(job_id) = value_as_u64(job_id_value) else {
            return Err(BaculaRunningStatusError::MalformedRunningJobs);
        };
        if job_id == expected_job_id.get() {
            matching.push(object);
        }
    }
    let object = match matching.as_slice() {
        [] => return Ok(None),
        [object] => *object,
        records => {
            return Err(BaculaRunningStatusError::AmbiguousJob {
                job_id: expected_job_id,
                actual: records.len(),
            });
        }
    };

    let raw_status = object
        .get("status")
        .and_then(Value::as_str)
        .and_then(single_status_character)
        .ok_or(BaculaRunningStatusError::InvalidStatus)?;
    let errors = required_running_u64(object, &["errors", "joberrors"], "errors")?;
    let files = required_running_u64(object, &["jobfiles", "files"], "jobfiles")?;
    let bytes = required_running_u64(object, &["jobbytes", "bytes"], "jobbytes")?;
    Ok(Some(BaculaRunningJobEvidence {
        job_id: expected_job_id,
        raw_status,
        state: map_job_status(raw_status, errors),
        files,
        bytes,
        errors,
        average_bytes_per_second: optional_running_u64(
            object,
            &["avebytes_sec", "average_bytes_per_second"],
            "avebytes_sec",
        )?,
        last_bytes_per_second: optional_running_u64(
            object,
            &["lastbytes_sec", "last_bytes_per_second"],
            "lastbytes_sec",
        )?,
        spooling: optional_running_bool(object, &["spooling"], "spooling")?,
        despooling: optional_running_bool(object, &["despooling"], "despooling")?,
        despool_wait: optional_running_bool(
            object,
            &["despool_wait", "despoolwait"],
            "despool_wait",
        )?,
    }))
}

fn single_status_character(value: &str) -> Option<char> {
    let mut characters = value.chars();
    let first = characters.next()?;
    characters.next().is_none().then_some(first)
}

fn required_running_u64(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    field: &'static str,
) -> Result<u64, BaculaRunningStatusError> {
    object_value(object, keys)
        .and_then(value_as_u64)
        .ok_or(BaculaRunningStatusError::MissingField(field))
}

fn optional_running_u64(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    field: &'static str,
) -> Result<Option<u64>, BaculaRunningStatusError> {
    object_value(object, keys)
        .map(|value| value_as_u64(value).ok_or(BaculaRunningStatusError::MissingField(field)))
        .transpose()
}

fn optional_running_bool(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    field: &'static str,
) -> Result<Option<bool>, BaculaRunningStatusError> {
    object_value(object, keys)
        .map(|value| value_as_bool(value).ok_or(BaculaRunningStatusError::MissingField(field)))
        .transpose()
}

fn value_as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_u64()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        Value::String(value) => match value.as_str() {
            "0" | "false" => Some(false),
            "1" | "true" => Some(true),
            _ => None,
        },
        _ => None,
    }
}

/// Parse Bacula 15 API-v2 Storage-Daemon running-job status and return the
/// exact read DCR VolumeName for one correlated restore job. The parser does
/// not recursively accept arbitrary object arrays and rejects write-side `m`,
/// wrong pool/device evidence, or a simultaneous write DCR.
pub fn parse_requested_volume(
    output: &[u8],
    expected_job_id: BaculaJobId,
    expected_pool: &BaculaPoolName,
) -> Result<VolumeLabel, BaculaStatusError> {
    let text = std::str::from_utf8(output).map_err(|_| BaculaStatusError::InvalidUtf8)?;
    let value = text
        .char_indices()
        .filter(|(_, character)| *character == '{')
        .find_map(|(offset, _)| {
            serde_json::Deserializer::from_str(&text[offset..])
                .into_iter::<Value>()
                .next()
                .and_then(Result::ok)
        })
        .ok_or(BaculaStatusError::MissingRunningJobs)?;
    let root = value
        .as_object()
        .ok_or(BaculaStatusError::MalformedRunningJobs)?;
    let running = root
        .get("running")
        .ok_or(BaculaStatusError::MissingRunningJobs)?;
    let records = running
        .as_array()
        .ok_or(BaculaStatusError::MalformedRunningJobs)?;
    if records.iter().any(|record| !record.is_object()) {
        return Err(BaculaStatusError::MalformedRunningJobs);
    }

    let mut matching = Vec::new();
    for record in records {
        let object = record
            .as_object()
            .expect("the running-job object shape was checked");
        let Some(job_id_value) = object.get("jobid") else {
            // Bacula may include a Director-connection record in this
            // array; it has no JobId and cannot be our restore job.
            continue;
        };
        let Some(job_id) = value_as_u64(job_id_value) else {
            return Err(BaculaStatusError::MalformedRunningJobs);
        };
        if job_id == expected_job_id.get() {
            matching.push(object);
        }
    }
    let object = match matching.as_slice() {
        [] => return Err(BaculaStatusError::MissingJob(expected_job_id)),
        [object] => *object,
        records => {
            return Err(BaculaStatusError::AmbiguousJob {
                job_id: expected_job_id,
                actual: records.len(),
            });
        }
    };

    if object.get("status").and_then(Value::as_str) != Some("M") {
        return Err(BaculaStatusError::NotWaitingForMount);
    }
    if object.get("read_pool").and_then(Value::as_str) != Some(expected_pool.as_str()) {
        return Err(BaculaStatusError::UnexpectedReadPool);
    }
    if object.get("read_device").and_then(Value::as_str) != Some(DEVICE_NAME) {
        return Err(BaculaStatusError::UnexpectedReadDevice);
    }
    if object.get("write_volume").and_then(Value::as_str) != Some("") {
        return Err(BaculaStatusError::UnexpectedWriteVolume);
    }
    let errors = object
        .get("errors")
        .and_then(value_as_u64)
        .ok_or(BaculaStatusError::MissingField("errors"))?;
    if errors != 0 {
        return Err(BaculaStatusError::ReportedErrors);
    }
    let volume = object
        .get("read_volume")
        .and_then(Value::as_str)
        .ok_or(BaculaStatusError::MissingField("read_volume"))?;
    VolumeLabel::parse(volume).map_err(|_| BaculaStatusError::InvalidRequestedVolume)
}

pub fn parse_job_list(output: &[u8]) -> Result<Vec<BaculaJobEvidence>, BaculaJsonError> {
    let text = std::str::from_utf8(output).map_err(|_| BaculaJsonError::InvalidUtf8)?;
    for (offset, character) in text.char_indices() {
        if !matches!(character, '{' | '[') {
            continue;
        }
        let Some(Ok(value)) = serde_json::Deserializer::from_str(&text[offset..])
            .into_iter::<Value>()
            .next()
        else {
            continue;
        };
        if let Some(records) = find_job_records(&value) {
            return records.iter().map(parse_job_record).collect();
        }
    }
    Err(BaculaJsonError::MissingJobList)
}

fn find_job_records(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(values) if values.iter().all(Value::is_object) => Some(values),
        Value::Object(object) => {
            if let Some(Value::Array(values)) = object.get("jobs") {
                return Some(values);
            }
            object.values().find_map(find_job_records)
        }
        _ => None,
    }
}

fn parse_job_record(value: &Value) -> Result<BaculaJobEvidence, BaculaJsonError> {
    let object = value
        .as_object()
        .ok_or(BaculaJsonError::MissingField("job object"))?;
    let job_id = object_value(object, &["jobid", "job_id"])
        .and_then(value_as_u64)
        .ok_or(BaculaJsonError::MissingField("jobid"))?;
    let job_id = BaculaJobId::new(job_id).map_err(|_| BaculaJsonError::InvalidJobId)?;
    let errors = object_value(object, &["joberrors", "errors"])
        .and_then(value_as_u64)
        .unwrap_or(0);
    let raw_status = object_value(object, &["jobstatus", "status"])
        .and_then(Value::as_str)
        .and_then(|status| {
            let mut characters = status.chars();
            let first = characters.next()?;
            characters.next().is_none().then_some(first)
        })
        .ok_or(BaculaJsonError::InvalidStatus)?;
    Ok(BaculaJobEvidence {
        job_id,
        resource_name: optional_string(object, &["name", "jobname"]),
        unique_name: optional_string(object, &["job", "ujobid"]),
        raw_status,
        state: map_job_status(raw_status, errors),
        files: object_value(object, &["jobfiles", "files"])
            .and_then(value_as_u64)
            .unwrap_or(0),
        bytes: object_value(object, &["jobbytes", "bytes"])
            .and_then(value_as_u64)
            .unwrap_or(0),
        errors,
        start_time: optional_string(object, &["starttime", "start_time"]),
        end_time: optional_string(object, &["endtime", "end_time"]),
    })
}

fn object_value<'a>(
    object: &'a serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<&'a Value> {
    keys.iter().find_map(|key| object.get(*key))
}

fn optional_string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    object_value(object, keys)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn value_as_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[derive(Debug, thiserror::Error)]
pub enum BaculaAdapterError {
    #[error(transparent)]
    Console(#[from] BconsoleError),
    #[error(transparent)]
    Json(#[from] BaculaJsonError),
    #[error(transparent)]
    Status(#[from] BaculaStatusError),
    #[error(transparent)]
    RunningStatus(#[from] BaculaRunningStatusError),
    #[error("Bacula returned {actual} records while exactly one job was required")]
    AmbiguousJob { actual: usize },
    #[error("Bacula returned JobId {actual} while JobId {expected} was requested")]
    UnexpectedJobId {
        expected: BaculaJobId,
        actual: BaculaJobId,
    },
    #[error("{operation:?} cannot create a {resource:?} job")]
    InvalidCorrelationOperation {
        operation: BaculaOperationKind,
        resource: BaculaJobResource,
    },
    #[error("multiple {resource:?} jobs appeared above watermark {watermark:?}: {job_ids:?}")]
    AmbiguousCorrelation {
        resource: BaculaJobResource,
        watermark: Option<BaculaJobId>,
        job_ids: Vec<BaculaJobId>,
    },
    #[error("no new {resource:?} JobId appeared within the correlation deadline")]
    CorrelationTimeout { resource: BaculaJobResource },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorrelatedJobDispatch {
    pub operation: BaculaOperationKind,
    pub resource: BaculaJobResource,
    pub watermark: Option<BaculaJobId>,
    pub job_id: BaculaJobId,
    pub dispatch_output: BconsoleOutput,
}

/// Injectable process adapter. Durable job persistence and scheduling remain the worker's job.
///
/// The in-process lock makes watermark/dispatch/poll atomic for one adapter. The worker must also
/// hold its database-fenced single-drive lease so another process cannot initiate the same fixed
/// Bacula Job resource concurrently.
pub struct BaculaAdapter<E> {
    executor: E,
    correlation_lock: AsyncMutex<()>,
}

impl<E> BaculaAdapter<E>
where
    E: BconsoleExecutor,
{
    pub fn new(executor: E) -> Self {
        Self {
            executor,
            correlation_lock: AsyncMutex::new(()),
        }
    }

    pub async fn dispatch(
        &self,
        script: &BconsoleScript,
    ) -> Result<BconsoleOutput, BaculaAdapterError> {
        self.executor.execute(script).await.map_err(Into::into)
    }

    pub async fn job_evidence(
        &self,
        job_id: BaculaJobId,
    ) -> Result<BaculaJobEvidence, BaculaAdapterError> {
        let output = self.dispatch(&BaculaCommands::query_job(job_id)).await?;
        let records = parse_job_list(&output.stdout)?;
        if records.len() != 1 {
            return Err(BaculaAdapterError::AmbiguousJob {
                actual: records.len(),
            });
        }
        let record = records.into_iter().next().expect("length checked");
        if record.job_id != job_id {
            return Err(BaculaAdapterError::UnexpectedJobId {
                expected: job_id,
                actual: record.job_id,
            });
        }
        Ok(record)
    }

    /// Read live Storage-Daemon counters for one exact correlated JobId.
    /// The record may be temporarily absent while daemons hand off the job.
    pub async fn running_job_evidence(
        &self,
        job_id: BaculaJobId,
    ) -> Result<Option<BaculaRunningJobEvidence>, BaculaAdapterError> {
        let output = self
            .dispatch(&BaculaCommands::query_requested_volume())
            .await?;
        if !output.stderr.is_empty() {
            return Err(BaculaRunningStatusError::UnexpectedStderr.into());
        }
        parse_running_job_evidence(&output.stdout, job_id).map_err(Into::into)
    }

    /// Return the exact volume currently requested by the fixed Storage
    /// resource. A missing, malformed, or ambiguous status record is an error;
    /// callers must never infer a media handoff from JobStatus alone.
    pub async fn requested_volume(
        &self,
        job_id: BaculaJobId,
        pool: &BaculaPoolName,
    ) -> Result<VolumeLabel, BaculaAdapterError> {
        let output = self
            .dispatch(&BaculaCommands::query_requested_volume())
            .await?;
        if !output.stderr.is_empty() {
            return Err(BaculaStatusError::UnexpectedStderr.into());
        }
        parse_requested_volume(&output.stdout, job_id, pool).map_err(Into::into)
    }

    pub async fn job_watermark(
        &self,
        resource: BaculaJobResource,
    ) -> Result<Option<BaculaJobId>, BaculaAdapterError> {
        let records = self.latest_jobs(resource, 100).await?;
        Ok(records.iter().map(|record| record.job_id).max())
    }

    /// Dispatches one fixed job and correlates its JobId without parsing the human `run` reply.
    ///
    /// Correlation is based on a structured pre-dispatch `.jlist` watermark and requires exactly
    /// one new JobId for the matching fixed Job resource. Zero or multiple candidates fail closed.
    pub async fn dispatch_and_correlate(
        &self,
        resource: BaculaJobResource,
        script: &BconsoleScript,
    ) -> Result<CorrelatedJobDispatch, BaculaAdapterError> {
        if !resource.accepts_operation(script.kind()) {
            return Err(BaculaAdapterError::InvalidCorrelationOperation {
                operation: script.kind(),
                resource,
            });
        }
        let _guard = self.correlation_lock.lock().await;
        let watermark = self.job_watermark(resource).await?;
        let dispatch_output = self.dispatch(script).await?;
        let deadline = Instant::now() + CORRELATION_TIMEOUT;
        loop {
            let mut candidates = self
                .latest_jobs(resource, 100)
                .await?
                .into_iter()
                .filter(|record| watermark.is_none_or(|value| record.job_id > value))
                .map(|record| record.job_id)
                .collect::<Vec<_>>();
            candidates.sort_unstable();
            candidates.dedup();
            match candidates.as_slice() {
                [job_id] => {
                    return Ok(CorrelatedJobDispatch {
                        operation: script.kind(),
                        resource,
                        watermark,
                        job_id: *job_id,
                        dispatch_output,
                    });
                }
                [] if Instant::now() < deadline => sleep(CORRELATION_POLL_INTERVAL).await,
                [] => return Err(BaculaAdapterError::CorrelationTimeout { resource }),
                _ => {
                    return Err(BaculaAdapterError::AmbiguousCorrelation {
                        resource,
                        watermark,
                        job_ids: candidates,
                    });
                }
            }
        }
    }

    async fn latest_jobs(
        &self,
        resource: BaculaJobResource,
        limit: u16,
    ) -> Result<Vec<BaculaJobEvidence>, BaculaAdapterError> {
        let script = match resource {
            BaculaJobResource::Backup => BaculaCommands::query_latest_backup(limit),
            BaculaJobResource::Verify => BaculaCommands::query_latest_verify(limit),
            BaculaJobResource::Restore => BaculaCommands::query_latest_restore(limit),
        };
        let output = self.dispatch(&script).await?;
        let records = parse_job_list(&output.stdout)?;
        Ok(records
            .into_iter()
            .filter(|record| record.resource_name.as_deref() == Some(resource.resource_name()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use super::*;

    fn config() -> BaculaConfig {
        let paths =
            BaculaPaths::from_relative_source("datasets/production", "/dev/nst0").expect("paths");
        let endpoints = BaculaEndpoints::new(
            "bacula-dir",
            "bacula-sd",
            "bacula-fd",
            "bacula-db",
            5432,
            "bacula",
            "bacula",
        )
        .expect("endpoints");
        let credentials = BaculaCredentials::new(
            "director-pass",
            "storage-pass",
            "file-pass",
            "console-pass",
            "catalog-pass",
        )
        .expect("credentials");
        BaculaConfig::new(
            paths,
            endpoints,
            credentials,
            BaculaPoolName::for_run(
                Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid"),
            ),
        )
    }

    #[test]
    fn renderer_uses_fixed_scoped_resources() {
        let rendered = config().render();
        assert!(rendered.director.contains("Name = TS-Full"));
        assert!(rendered
            .director
            .contains("File = \"/source/datasets/production\""));
        assert!(rendered.director.contains("Sparse = yes"));
        assert!(rendered.director.contains("One FS = yes"));
        assert!(rendered
            .director
            .contains("Where ACL = /restore, /restore/*"));
        assert!(rendered.director.contains("Directory ACL = *all*"));
        assert!(rendered
            .director
            .contains("Pool ACL = TS-LTO-018f7cac2e2a7dc5b70c78a46a6f6f01"));
        assert!(rendered.director.contains("Job ACL = *all*"));
        assert!(rendered.director.contains("Client ACL = *all*"));
        assert!(rendered
            .director
            .contains(".bvfs_restore, .bvfs_cleanup, restore"));
        assert!(rendered
            .director
            .contains("Backup Client ACL = tapescale-fd"));
        assert!(rendered
            .director
            .contains("Restore Client ACL = tapescale-fd"));
        assert!(rendered.director.contains("Level = Data"));
        assert!(rendered
            .storage_daemon
            .contains("Archive Device = \"/dev/nst0\""));
        assert!(rendered.storage_daemon.contains("Label Media = no"));
        assert!(rendered
            .storage_daemon
            .contains("Maximum File Size = 34359738368"));
        assert!(rendered
            .file_daemon
            .contains("Allowed Backup Directories = /source"));
        assert!(rendered
            .file_daemon
            .contains("Allowed Restore Directories = /restore"));
        assert!(rendered.bconsole.contains("Name = tapescale-adapter"));
        assert!(rendered.director.contains("TLS Enable = no"));
        assert!(rendered.director.contains("TLS Require = yes"));
        assert!(rendered.director.contains("TLS PSK Enable = yes"));
        assert!(rendered.director.contains("Maximum Volumes = 1"));
        assert!(rendered.director.contains("Spool Data = yes"));
        assert!(rendered.director.contains("Spool Attributes = yes"));
        assert!(rendered
            .storage_daemon
            .contains("Working Directory = \"/var/lib/tapescale/spool\""));
        assert!(rendered
            .storage_daemon
            .contains("Spool Directory = \"/var/lib/tapescale/spool\""));
        assert!(rendered
            .storage_daemon
            .contains("Maximum Spool Size = 68719476736"));
        assert!(rendered
            .storage_daemon
            .contains("Maximum Job Spool Size = 68719476736"));
        assert!(rendered
            .storage_daemon
            .contains("Maximum Concurrent Jobs = 2"));
        assert_eq!(
            rendered
                .storage_daemon
                .matches("Maximum Concurrent Jobs = 1")
                .count(),
            1,
            "the physical Device remains serialized"
        );
        assert!(!rendered.director.contains("Compression ="));
    }

    #[test]
    fn renderer_supports_unlimited_spanning_and_explicit_compression() {
        let spool_limit = NonZeroU64::new(8 * 1024 * 1024 * 1024).expect("spool limit");
        let rendered = config()
            .with_data_options(
                BaculaDataOptions::spanning(BaculaCompression::GzipBalanced)
                    .with_maximum_spool_size(spool_limit),
            )
            .render();
        assert!(rendered.director.contains("Maximum Volumes = 0"));
        assert!(rendered.director.contains("Compression = GZIP6"));
        assert!(rendered
            .storage_daemon
            .contains("Maximum Spool Size = 8589934592"));
        assert!(rendered
            .storage_daemon
            .contains("Maximum Job Spool Size = 8589934592"));
        assert!(rendered.director.contains("Recycle = no"));
        assert!(rendered.director.contains("Auto Prune = no"));
        assert_eq!(
            BaculaCompression::from_policy_mode("auto").expect("auto"),
            BaculaCompression::Lzo
        );
        assert_eq!(
            BaculaCompression::from_policy_mode("lzo").expect("lzo"),
            BaculaCompression::Lzo
        );
        assert!(BaculaCompression::from_policy_mode("zstd").is_err());
        assert_eq!(
            BaculaCompression::from_persisted_policy("auto", "gzip")
                .expect("legacy automatic codec"),
            BaculaCompression::GzipFast
        );
        assert_eq!(
            BaculaCompression::from_persisted_policy("auto", "lzo")
                .expect("current automatic codec"),
            BaculaCompression::Lzo
        );
        assert!(BaculaCompression::from_persisted_policy("auto", "none").is_err());
    }

    #[test]
    fn renderer_supports_direct_streaming_without_disabling_attribute_spooling() {
        let options = BaculaDataOptions::spanning(BaculaCompression::Lzo)
            .with_delivery(BaculaDataDelivery::Direct);
        assert!(!options.data_spooling_enabled());
        assert_eq!(
            serde_json::to_value(options.delivery).expect("delivery JSON"),
            serde_json::json!("direct")
        );

        let rendered = config().with_data_options(options).render();
        assert!(rendered.director.contains("Spool Data = no"));
        assert!(rendered.director.contains("Spool Attributes = yes"));
        assert!(rendered
            .storage_daemon
            .contains("Maximum Network Buffer Size = 65536"));
        assert!(rendered
            .file_daemon
            .contains("Maximum Network Buffer Size = 65536"));
        assert!(rendered
            .storage_daemon
            .contains("Working Directory = \"/var/lib/tapescale/spool\""));
        assert!(rendered
            .storage_daemon
            .contains("Spool Directory = \"/var/lib/tapescale/spool\""));

        let default_options = BaculaDataOptions::default();
        assert!(default_options.data_spooling_enabled());
        assert_eq!(default_options.delivery, BaculaDataDelivery::DiskSpool);
        assert_eq!(
            serde_json::to_value(default_options.delivery).expect("delivery JSON"),
            serde_json::json!("disk_spool")
        );
    }

    #[test]
    fn renderer_can_isolate_the_spool_for_one_run() {
        let run_id = Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("run id");
        let rendered = config().with_run_spool_directory(run_id).render();
        let expected = "/var/lib/tapescale/spool/018f7cac2e2a7dc5b70c78a46a6f6f01";
        assert!(rendered
            .storage_daemon
            .contains(&format!("Working Directory = \"{expected}\"")));
        assert!(rendered
            .storage_daemon
            .contains(&format!("Spool Directory = \"{expected}\"")));
    }

    #[test]
    fn path_and_configuration_injection_is_rejected() {
        assert!(BaculaSourceSelection::from_relative("../etc").is_err());
        assert!(BaculaSourceSelection::from_relative("dataset//project").is_err());
        assert!(BaculaSourceSelection::from_absolute("/source").is_err());
        assert!(BaculaSourceSelection::from_absolute("/source/data\nFile = /").is_err());
        assert!(BaculaRestoreSubpath::parse("../source").is_err());
        let selection = BaculaSourceSelection::from_relative("dataset").expect("selection");
        assert!(BaculaPaths::new(
            "/source\nFile = /",
            selection.clone(),
            "/restore",
            "/dev/nst0"
        )
        .is_err());
        assert!(BaculaPaths::new("/source", selection, "/restore/../etc", "/dev/nst0").is_err());
        assert!(BaculaPaths::from_relative_source("dataset", "/dev/st0").is_err());
        assert!(BaculaPaths::from_relative_source("dataset", "/dev/nst0;run").is_err());
        assert!(BaculaPaths::from_relative_source("dataset", "/dev/nst00").is_err());
        assert!(BaculaEndpoints::new(
            "bacula-dir\nConsole {",
            "bacula-sd",
            "bacula-fd",
            "bacula-db",
            5432,
            "bacula",
            "bacula"
        )
        .is_err());
        assert!(BaculaCredentials::new("ok", "ok", "ok", "bad\"value", "ok").is_err());
    }

    #[test]
    fn command_values_cannot_inject_console_input() {
        assert!(VolumeLabel::parse("LTO001\nyes\nrun job=TS-Full").is_err());
        assert!(VolumeLabel::parse("LTO 001").is_err());
        let label = VolumeLabel::parse("LTO-001_A").expect("label");
        let pool = BaculaPoolName::for_run(
            Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid"),
        );
        let script = BaculaCommands::label(&label, &pool);
        assert_eq!(
            std::str::from_utf8(script.stdin()).expect("utf8"),
            ".api 2 api_opts=j\nlabel storage=TS-LTO pool=TS-LTO-018f7cac2e2a7dc5b70c78a46a6f6f01 volume=LTO-001_A yes\n.quit\n"
        );
        assert!(BaculaJobId::new(0).is_err());
    }

    #[test]
    fn media_change_commands_are_fixed_and_non_interactive() {
        let mount = BaculaCommands::mount();
        assert_eq!(mount.kind(), BaculaOperationKind::Mount);
        assert_eq!(
            std::str::from_utf8(mount.stdin()).expect("utf8"),
            ".api 2 api_opts=j\nmount storage=TS-LTO\n.quit\n"
        );
        let release = BaculaCommands::release();
        assert_eq!(release.kind(), BaculaOperationKind::Release);
        assert_eq!(
            std::str::from_utf8(release.stdin()).expect("utf8"),
            ".api 2 api_opts=j\nrelease storage=TS-LTO\n.quit\n"
        );

        let requested = BaculaCommands::query_requested_volume();
        assert_eq!(requested.kind(), BaculaOperationKind::QueryRequestedVolume);
        assert_eq!(
            std::str::from_utf8(requested.stdin()).expect("utf8"),
            ".api 2 api_opts=j\n.status storage=TS-LTO running\n.quit\n"
        );
        assert!(requested.expects_json());
    }

    #[test]
    fn requested_volume_parser_correlates_exact_structured_read_evidence() {
        let job_id = BaculaJobId::new(42).expect("job id");
        let pool = BaculaPoolName::for_run(
            Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid"),
        );
        let output = format!(
            r#"Connecting to Storage daemon
{{"running":[
  {{"DirectorConnected":1700000000,"DirTLS":1}},
  {{"jobid":41,"job":"other","status":"R","read_volume":"OTHER","read_pool":"Other","read_device":"Other","write_volume":""}},
  {{"jobid":42,"job":"TS-Restore.2026","status":"M","read_volume":"TS-LTO-002","read_pool":"{}","read_device":"TS-LTO-Drive","write_volume":"","jobbytes":4096,"jobfiles":7,"errors":0}}
]}}
1000 OK: quit"#,
            pool.as_str()
        );
        assert_eq!(
            parse_requested_volume(output.as_bytes(), job_id, &pool)
                .expect("requested volume")
                .as_str(),
            "TS-LTO-002"
        );
    }

    #[test]
    fn running_job_parser_reports_live_spool_counters_for_the_exact_job() {
        let job_id = BaculaJobId::new(42).expect("job id");
        let output = br#"Connecting to Storage daemon
{"running":[
  {"DirectorConnected":1700000000,"DirTLS":1},
  {"jobid":41,"status":"R","jobbytes":999,"jobfiles":1,"errors":0},
  {"jobid":"42","status":"R","jobbytes":"67108864","jobfiles":"12","errors":"0","avebytes_sec":"5242880","lastbytes_sec":6291456,"spooling":1,"despooling":"0","despool_wait":false}
]}
1000 OK: quit"#;
        let evidence = parse_running_job_evidence(output, job_id)
            .expect("valid running status")
            .expect("exact running job");
        assert_eq!(evidence.job_id, job_id);
        assert_eq!(evidence.state, BaculaJobState::Running);
        assert_eq!(evidence.bytes, 67_108_864);
        assert_eq!(evidence.files, 12);
        assert_eq!(evidence.average_bytes_per_second, Some(5_242_880));
        assert_eq!(evidence.last_bytes_per_second, Some(6_291_456));
        assert_eq!(evidence.spooling, Some(true));
        assert_eq!(evidence.despooling, Some(false));
        assert_eq!(evidence.despool_wait, Some(false));

        assert_eq!(
            parse_running_job_evidence(br#"{"running":[]}"#, job_id)
                .expect("an absent exact job is not malformed"),
            None
        );
    }

    #[test]
    fn running_job_parser_rejects_duplicates_and_malformed_exact_records() {
        let job_id = BaculaJobId::new(42).expect("job id");
        assert!(matches!(
            parse_running_job_evidence(
                br#"{"running":[{"jobid":42,"status":"R","jobbytes":1,"jobfiles":1,"errors":0},{"jobid":42,"status":"R","jobbytes":2,"jobfiles":2,"errors":0}]}"#,
                job_id
            ),
            Err(BaculaRunningStatusError::AmbiguousJob { actual: 2, .. })
        ));
        assert!(matches!(
            parse_running_job_evidence(
                br#"{"running":[{"jobid":42,"status":"R","jobbytes":1,"jobfiles":1,"errors":0,"spooling":"maybe"}]}"#,
                job_id
            ),
            Err(BaculaRunningStatusError::MissingField("spooling"))
        ));
        assert!(matches!(
            parse_running_job_evidence(
                br#"{"running":[{"jobid":42,"status":"RR","jobbytes":1,"jobfiles":1,"errors":0}]}"#,
                job_id
            ),
            Err(BaculaRunningStatusError::InvalidStatus)
        ));
        assert!(matches!(
            parse_running_job_evidence(br#"{"jobs":[]}"#, job_id),
            Err(BaculaRunningStatusError::MissingRunningJobs)
        ));
    }

    #[test]
    fn requested_volume_parser_rejects_unstructured_or_ambiguous_evidence() {
        let job_id = BaculaJobId::new(42).expect("job id");
        let pool = BaculaPoolName::for_run(
            Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid"),
        );
        assert!(matches!(
            parse_requested_volume(
                b"Device is BLOCKED waiting for mount of volume \"TS-LTO-002\",\n",
                job_id,
                &pool
            ),
            Err(BaculaStatusError::MissingRunningJobs)
        ));
        assert!(matches!(
            parse_requested_volume(
                br#"{"devices":[{"volume":"TS-LTO-002","blocked":1}]}"#,
                job_id,
                &pool
            ),
            Err(BaculaStatusError::MissingRunningJobs)
        ));
        let duplicate = format!(
            r#"{{"running":[
  {{"jobid":42,"status":"M","read_volume":"TS-LTO-002","read_pool":"{}","read_device":"TS-LTO-Drive","write_volume":""}},
  {{"jobid":42,"status":"M","read_volume":"TS-LTO-002","read_pool":"{}","read_device":"TS-LTO-Drive","write_volume":""}}
]}}"#,
            pool.as_str(),
            pool.as_str()
        );
        assert!(matches!(
            parse_requested_volume(duplicate.as_bytes(), job_id, &pool),
            Err(BaculaStatusError::AmbiguousJob { actual: 2, .. })
        ));
    }

    #[test]
    fn requested_volume_parser_rejects_write_side_or_wrong_binding_evidence() {
        let job_id = BaculaJobId::new(42).expect("job id");
        let pool = BaculaPoolName::for_run(
            Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid"),
        );
        let status = |raw_status: &str, read_pool: &str, read_device: &str, write_volume: &str| {
            format!(
                r#"{{"running":[{{"jobid":42,"status":"{raw_status}","read_volume":"TS-LTO-002","read_pool":"{read_pool}","read_device":"{read_device}","write_volume":"{write_volume}","errors":0}}]}}"#
            )
        };
        assert!(matches!(
            parse_requested_volume(
                status("m", pool.as_str(), DEVICE_NAME, "").as_bytes(),
                job_id,
                &pool
            ),
            Err(BaculaStatusError::NotWaitingForMount)
        ));
        assert!(matches!(
            parse_requested_volume(
                status("M", "wrong-pool", DEVICE_NAME, "").as_bytes(),
                job_id,
                &pool
            ),
            Err(BaculaStatusError::UnexpectedReadPool)
        ));
        assert!(matches!(
            parse_requested_volume(
                status("M", pool.as_str(), "wrong-device", "").as_bytes(),
                job_id,
                &pool
            ),
            Err(BaculaStatusError::UnexpectedReadDevice)
        ));
        assert!(matches!(
            parse_requested_volume(
                status("M", pool.as_str(), DEVICE_NAME, "unexpected-write").as_bytes(),
                job_id,
                &pool
            ),
            Err(BaculaStatusError::UnexpectedWriteVolume)
        ));
        assert!(matches!(
            parse_requested_volume(b"{\"running\":", job_id, &pool),
            Err(BaculaStatusError::MissingRunningJobs)
        ));
    }

    #[test]
    fn requested_volume_parser_rejects_missing_job_volume_and_error_evidence() {
        let job_id = BaculaJobId::new(42).expect("job id");
        let pool = BaculaPoolName::for_run(
            Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid"),
        );
        let record = |job: &str, volume: Option<&str>, errors: u64| {
            let volume = volume
                .map(|value| format!(",\"read_volume\":\"{value}\""))
                .unwrap_or_default();
            format!(
                r#"{{"running":[{{"jobid":{job},"status":"M","read_pool":"{}","read_device":"TS-LTO-Drive","write_volume":"","errors":{errors}{volume}}}]}}"#,
                pool.as_str()
            )
        };

        assert!(matches!(
            parse_requested_volume(b"{\"running\":[]}", job_id, &pool),
            Err(BaculaStatusError::MissingJob(_))
        ));
        assert!(matches!(
            parse_requested_volume(
                record("41", Some("TS-LTO-002"), 0).as_bytes(),
                job_id,
                &pool
            ),
            Err(BaculaStatusError::MissingJob(_))
        ));
        assert!(matches!(
            parse_requested_volume(record("42", None, 0).as_bytes(), job_id, &pool),
            Err(BaculaStatusError::MissingField("read_volume"))
        ));
        assert!(matches!(
            parse_requested_volume(record("42", Some(""), 0).as_bytes(), job_id, &pool),
            Err(BaculaStatusError::InvalidRequestedVolume)
        ));
        assert!(matches!(
            parse_requested_volume(
                record("42", Some(&"A".repeat(128)), 0).as_bytes(),
                job_id,
                &pool
            ),
            Err(BaculaStatusError::InvalidRequestedVolume)
        ));
        assert!(matches!(
            parse_requested_volume(
                record("42", Some("TS-LTO-002\\nBAD"), 0).as_bytes(),
                job_id,
                &pool
            ),
            Err(BaculaStatusError::InvalidRequestedVolume)
        ));
        assert!(matches!(
            parse_requested_volume(
                record("42", Some("TS-LTO-002"), 1).as_bytes(),
                job_id,
                &pool
            ),
            Err(BaculaStatusError::ReportedErrors)
        ));
    }

    #[test]
    fn restore_destination_is_server_generated_and_contained() {
        let job_id = BaculaJobId::new(42).expect("job id");
        let run_id = Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid");
        let restore_subpath = BaculaRestoreSubpath::parse("exercises/nightly run").expect("path");
        let destination = restore_subpath.destination(run_id);
        let script = BaculaCommands::full_restore(job_id, &destination);
        let input = std::str::from_utf8(script.stdin()).expect("utf8");
        assert!(input.contains(
            "where=\"/restore/exercises/nightly run/018f7cac-2e2a-7dc5-b70c-78a46a6f6f01\" all done yes"
        ));
        assert!(destination.path().starts_with(RESTORE_ROOT));
        assert!(!input.contains("../"));
    }

    #[test]
    fn selected_restore_commands_use_only_typed_catalog_identities() {
        let backup_job_id = BaculaJobId::new(42).expect("job id");
        let run_id = Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid");
        let table = BaculaBvfsRestoreTable::for_run(run_id);
        assert_eq!(table.as_str(), format!("b2{}", run_id.as_u128()));
        assert!(table.as_str().strip_prefix("b2").is_some_and(
            |value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
        ));

        let selection = BaculaRestoreSelection::new(
            backup_job_id,
            vec![
                BaculaRestoreFileIndex {
                    job_id: backup_job_id,
                    file_index: BaculaFileIndex::new(17).expect("file index"),
                },
                BaculaRestoreFileIndex {
                    job_id: backup_job_id,
                    file_index: BaculaFileIndex::new(3).expect("file index"),
                },
                BaculaRestoreFileIndex {
                    job_id: backup_job_id,
                    file_index: BaculaFileIndex::new(17).expect("duplicate file index"),
                },
            ],
            vec![
                BaculaPathId::new(11).expect("path id"),
                BaculaPathId::new(5).expect("path id"),
                BaculaPathId::new(11).expect("duplicate path id"),
            ],
        )
        .expect("selection");
        let prepared =
            String::from_utf8(BaculaCommands::prepare_selected_restore(&selection, &table).stdin)
                .expect("utf8");
        assert_eq!(
            prepared,
            format!(
                ".api 2 api_opts=j\n.bvfs_restore path={} jobid=42 fileindex=42,3,42,17 dirid=5,11\n.quit\n",
                table.as_str()
            )
        );
        assert!(!prepared.contains("/source"));
        assert!(!prepared.contains('"'));

        let restore_subpath = BaculaRestoreSubpath::parse("selected restores").expect("path");
        let destination = restore_subpath.destination(run_id);
        let restored =
            String::from_utf8(BaculaCommands::selected_restore(&destination, &table).stdin)
                .expect("utf8");
        assert_eq!(
            restored,
            format!(
                ".api 2 api_opts=j\nrestore client=tapescale-fd restorejob=TS-Restore restoreclient=tapescale-fd where=\"{}\" file=\"?{}\" done yes\n.quit\n",
                destination.path().display(),
                table.as_str()
            )
        );
        assert!(!restored.contains("/source"));
        assert_eq!(restored.lines().count(), 3);

        let cleanup = String::from_utf8(BaculaCommands::cleanup_selected_restore(&table).stdin)
            .expect("utf8");
        assert_eq!(
            cleanup,
            format!(
                ".api 2 api_opts=j\n.bvfs_cleanup path={}\n.quit\n",
                table.as_str()
            )
        );
        assert!(BaculaJobResource::Restore.accepts_operation(BaculaOperationKind::SelectedRestore));
        assert!(!BaculaJobResource::Restore
            .accepts_operation(BaculaOperationKind::PrepareSelectedRestore));
    }

    #[test]
    fn selected_restore_identity_contract_rejects_empty_or_cross_job_values() {
        let backup_job_id = BaculaJobId::new(42).expect("job id");
        assert!(matches!(
            BaculaRestoreSelection::new(backup_job_id, Vec::new(), Vec::new()),
            Err(BaculaValidationError::Empty {
                field: "restore_selection"
            })
        ));
        assert!(matches!(
            BaculaRestoreSelection::new(
                backup_job_id,
                vec![BaculaRestoreFileIndex {
                    job_id: BaculaJobId::new(99).expect("other job id"),
                    file_index: BaculaFileIndex::new(1).expect("file index"),
                }],
                Vec::new()
            ),
            Err(BaculaValidationError::InvalidValue {
                field: "restore_file_job_id"
            })
        ));
        assert!(BaculaFileIndex::new(0).is_err());
        assert!(BaculaPathId::new(0).is_err());
    }

    #[test]
    fn each_job_type_has_a_fixed_structured_correlation_query() {
        let backup = String::from_utf8(BaculaCommands::query_latest_backup(5).stdin).expect("utf8");
        let verify = String::from_utf8(BaculaCommands::query_latest_verify(5).stdin).expect("utf8");
        let restore =
            String::from_utf8(BaculaCommands::query_latest_restore(5).stdin).expect("utf8");
        assert!(backup.contains(".jlist job=TS-Full jobs order=desc limit=5"));
        assert!(verify.contains(".jlist job=TS-VerifyData jobs order=desc limit=5"));
        assert!(restore.contains(".jlist job=TS-Restore jobs order=desc limit=5"));
    }

    #[test]
    fn status_mapping_never_promotes_warnings_or_errors() {
        let output = br#"Connecting to Director
{"jobs":[
 {"jobid":"10","name":"TS-Full","job":"TS-Full.2026-01-01_00.00.00_01","jobstatus":"T","jobfiles":"2","jobbytes":"4096","joberrors":"0"},
 {"jobid":11,"name":"TS-Full","jobstatus":"T","joberrors":1},
 {"jobid":12,"name":"TS-VerifyData","jobstatus":"W","joberrors":0},
 {"jobid":13,"name":"TS-VerifyData","jobstatus":"D","joberrors":0},
 {"jobid":14,"name":"TS-Full","jobstatus":"?","joberrors":0}
]}
1000 OK: quit"#;
        let records = parse_job_list(output).expect("jobs");
        assert_eq!(records[0].state, BaculaJobState::Succeeded);
        assert!(records[0].is_strict_success());
        assert_eq!(records[1].state, BaculaJobState::CompletedWithErrors);
        assert_eq!(records[2].state, BaculaJobState::CompletedWithWarnings);
        assert_eq!(records[3].state, BaculaJobState::VerifyDifferences);
        assert_eq!(records[4].state, BaculaJobState::Unknown);
        assert!(records[1..]
            .iter()
            .all(|record| !record.is_strict_success()));
    }

    #[test]
    fn mount_wait_is_distinct_from_new_media_wait() {
        assert_eq!(map_job_status('M', 0), BaculaJobState::WaitingForMount);
        assert_eq!(map_job_status('m', 0), BaculaJobState::WaitingForMedia);
        assert_eq!(map_job_status('v', 0), BaculaJobState::Unknown);
    }

    #[test]
    fn malformed_or_unstructured_output_is_rejected() {
        assert!(matches!(
            parse_job_list(b"Job queued. JobId=7"),
            Err(BaculaJsonError::MissingJobList)
        ));
        assert!(matches!(
            parse_job_list(br#"{"jobs":[{"jobid":7,"jobstatus":"TT"}]}"#),
            Err(BaculaJsonError::InvalidStatus)
        ));
    }

    #[derive(Clone)]
    struct FakeExecutor {
        scripts: Arc<Mutex<Vec<BconsoleScript>>>,
        output: BconsoleOutput,
    }

    #[derive(Clone)]
    struct SequenceExecutor {
        scripts: Arc<Mutex<Vec<BconsoleScript>>>,
        outputs: Arc<Mutex<VecDeque<BconsoleOutput>>>,
    }

    #[async_trait]
    impl BconsoleExecutor for SequenceExecutor {
        async fn execute(&self, script: &BconsoleScript) -> Result<BconsoleOutput, BconsoleError> {
            self.scripts
                .lock()
                .expect("scripts lock")
                .push(script.clone());
            self.outputs
                .lock()
                .expect("outputs lock")
                .pop_front()
                .ok_or_else(|| {
                    BconsoleError::Io(std::io::Error::other("missing fake bconsole output"))
                })
        }
    }

    #[async_trait]
    impl BconsoleExecutor for FakeExecutor {
        async fn execute(&self, script: &BconsoleScript) -> Result<BconsoleOutput, BconsoleError> {
            self.scripts.lock().expect("lock").push(script.clone());
            Ok(self.output.clone())
        }
    }

    #[tokio::test]
    async fn requested_volume_adapter_rejects_stderr_and_acl_denial_output() {
        let job_id = BaculaJobId::new(42).expect("job id");
        let pool = BaculaPoolName::for_run(
            Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid"),
        );
        let status = format!(
            r#"{{"running":[{{"jobid":42,"status":"M","read_volume":"TS-LTO-002","read_pool":"{}","read_device":"TS-LTO-Drive","write_volume":"","errors":0}}]}}"#,
            pool.as_str()
        );
        let adapter = BaculaAdapter::new(FakeExecutor {
            scripts: Arc::new(Mutex::new(Vec::new())),
            output: BconsoleOutput {
                stdout: status.into_bytes(),
                stderr: b"unexpected status diagnostic".to_vec(),
                exit_code: Some(0),
            },
        });
        assert!(matches!(
            adapter.requested_volume(job_id, &pool).await,
            Err(BaculaAdapterError::Status(
                BaculaStatusError::UnexpectedStderr
            ))
        ));

        let scripts = Arc::new(Mutex::new(Vec::new()));
        let adapter = BaculaAdapter::new(FakeExecutor {
            scripts: Arc::clone(&scripts),
            output: BconsoleOutput {
                stdout: b"Restricted Client or Job does not permit access to Storage daemons\n"
                    .to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
            },
        });
        assert!(matches!(
            adapter.requested_volume(job_id, &pool).await,
            Err(BaculaAdapterError::Status(
                BaculaStatusError::MissingRunningJobs
            ))
        ));
        let scripts = scripts.lock().expect("scripts lock");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].kind(), BaculaOperationKind::QueryRequestedVolume);
    }

    #[tokio::test]
    async fn running_job_adapter_uses_the_fixed_structured_status_query() {
        let scripts = Arc::new(Mutex::new(Vec::new()));
        let adapter = BaculaAdapter::new(FakeExecutor {
            scripts: Arc::clone(&scripts),
            output: BconsoleOutput {
                stdout: br#"{"running":[{"jobid":42,"status":"R","jobbytes":8192,"jobfiles":3,"errors":0,"spooling":true}]}"#.to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
            },
        });
        let evidence = adapter
            .running_job_evidence(BaculaJobId::new(42).expect("job id"))
            .await
            .expect("running evidence")
            .expect("exact job");
        assert_eq!(evidence.bytes, 8192);
        assert_eq!(evidence.spooling, Some(true));
        let scripts = scripts.lock().expect("scripts lock");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].kind(), BaculaOperationKind::QueryRequestedVolume);
    }

    #[tokio::test]
    async fn adapter_uses_injected_executor_and_structured_evidence() {
        let scripts = Arc::new(Mutex::new(Vec::new()));
        let adapter = BaculaAdapter::new(FakeExecutor {
            scripts: Arc::clone(&scripts),
            output: BconsoleOutput {
                stdout: br#"{"jobs":[{"jobid":77,"name":"TS-Full","jobstatus":"R","jobfiles":3,"jobbytes":8192,"joberrors":0}]}"#.to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
            },
        });
        let evidence = adapter
            .job_evidence(BaculaJobId::new(77).expect("job id"))
            .await
            .expect("evidence");
        assert_eq!(evidence.state, BaculaJobState::Running);
        assert_eq!(evidence.bytes, 8192);
        let scripts = scripts.lock().expect("lock");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].kind(), BaculaOperationKind::QueryJob);
        assert!(scripts[0].expects_json());
    }

    #[tokio::test]
    async fn adapter_rejects_a_different_job_id_than_requested() {
        let adapter = BaculaAdapter::new(FakeExecutor {
            scripts: Arc::new(Mutex::new(Vec::new())),
            output: BconsoleOutput {
                stdout:
                    br#"{"jobs":[{"jobid":78,"name":"TS-Full","jobstatus":"T","joberrors":0}]}"#
                        .to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
            },
        });
        assert!(matches!(
            adapter
                .job_evidence(BaculaJobId::new(77).expect("job id"))
                .await,
            Err(BaculaAdapterError::UnexpectedJobId { .. })
        ));
    }

    #[tokio::test]
    async fn dispatch_correlation_uses_a_pre_dispatch_watermark() {
        let scripts = Arc::new(Mutex::new(Vec::new()));
        let output = |stdout: &[u8]| BconsoleOutput {
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
        };
        let outputs = VecDeque::from([
            output(br#"{"jobs":[{"jobid":50,"name":"TS-Full","jobstatus":"T","joberrors":0}]}"#),
            output(b"Job queued. JobId=51\n"),
            output(br#"{"jobs":[{"jobid":51,"name":"TS-Full","jobstatus":"C","joberrors":0},{"jobid":50,"name":"TS-Full","jobstatus":"T","joberrors":0}]}"#),
        ]);
        let adapter = BaculaAdapter::new(SequenceExecutor {
            scripts: Arc::clone(&scripts),
            outputs: Arc::new(Mutex::new(outputs)),
        });
        let dispatch = adapter
            .dispatch_and_correlate(BaculaJobResource::Backup, &BaculaCommands::full_backup())
            .await
            .expect("correlated dispatch");
        assert_eq!(dispatch.watermark, Some(BaculaJobId::new(50).expect("id")));
        assert_eq!(dispatch.job_id, BaculaJobId::new(51).expect("id"));
        let kinds = scripts
            .lock()
            .expect("lock")
            .iter()
            .map(BconsoleScript::kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                BaculaOperationKind::QueryLatestBackup,
                BaculaOperationKind::FullBackup,
                BaculaOperationKind::QueryLatestBackup
            ]
        );
    }

    #[test]
    fn direct_executor_limits_are_bounded() {
        assert!(DirectBconsoleExecutor::new(Duration::ZERO, 1024).is_err());
        assert!(DirectBconsoleExecutor::new(Duration::from_secs(301), 1024).is_err());
        assert!(DirectBconsoleExecutor::new(Duration::from_secs(1), 0).is_err());
        assert!(DirectBconsoleExecutor::new(
            Duration::from_secs(1),
            MAX_CONFIGURED_OUTPUT_BYTES + 1
        )
        .is_err());
    }
}
