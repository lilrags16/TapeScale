//! Supported Bacula Community 15.0.3 process boundary for the first physical data path.
//!
//! This module deliberately does not make the TapeScale process a Bacula implementation. It
//! renders a small, fixed Bacula configuration and communicates with a restricted Console using
//! the supported `bconsole` programmatic commands. User-controlled values are validated before
//! they can reach either a configuration file or Console stdin.

use std::{
    fmt,
    num::NonZeroU64,
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
        }
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
  Maximum Volumes = 1
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
    }}
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
  Command ACL = run, cancel, status, .status, .api, .jlist, restore, label, mount, unmount, release
  Job ACL = {BACKUP_JOB_NAME}, {VERIFY_JOB_NAME}, {RESTORE_JOB_NAME}
  Client ACL = {FILE_DAEMON_NAME}
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
        )
    }

    fn render_storage_daemon(&self) -> String {
        format!(
            r#"Storage {{
  Name = {STORAGE_DAEMON_NAME}
  SDPort = 9103
  SDAddress = 127.0.0.1
  Working Directory = "/var/lib/bacula"
  Pid Directory = "/run/bacula"
  Plugin Directory = "/usr/lib/bacula"
  Maximum Concurrent Jobs = 1
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
  Maximum Concurrent Jobs = 1
}}

Messages {{
  Name = Standard
  director = {DIRECTOR_NAME} = all
}}
"#,
            storage_password = self.credentials.storage_password,
            tape_device = self.paths.tape_device.display(),
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
    FullBackup,
    DataVerify,
    FullRestore,
    Cancel,
    QueryJob,
    QueryLatestBackup,
    QueryLatestVerify,
    QueryLatestRestore,
    QueryStorage,
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
        'M' | 'm' | 'v' => BaculaJobState::WaitingForMedia,
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
        assert!(rendered.director.contains("Level = Data"));
        assert!(rendered
            .storage_daemon
            .contains("Archive Device = \"/dev/nst0\""));
        assert!(rendered.storage_daemon.contains("Label Media = no"));
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
