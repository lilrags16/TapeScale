//! Per-exercise supervision for the pinned Bacula Community data plane.
//!
//! The worker owns one database-fenced drive lease before constructing this
//! runtime. Daemons are started in a fixed order, write bounded per-run logs,
//! and are stopped before TapeScale performs its independent post-run rewind.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use rustix::process::{kill_process, Pid, Signal};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Child,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::{
    engine::bacula::{
        BaculaConfig, BaculaCredentials, BaculaDaemon, BaculaDataOptions, BaculaEndpoints,
        BaculaPaths, BaculaPoolName, RenderedBaculaConfigs, BCONSOLE_CONFIG, DIRECTOR_CONFIG,
        FILE_DAEMON_CONFIG, STORAGE_DAEMON_CONFIG,
    },
    tool_runner::{self, ToolCommand},
};

const CATALOG_SCHEMA_PROGRAM: &str = "/usr/share/bacula-director/make_postgresql_tables";
const PSQL_PROGRAM: &str = "/usr/bin/psql";
const CATALOG_SCHEMA_SCRIPT_SHA256: &str =
    "7bb209c3e84d7df73bd5b6d4844e3a34637c78ba00bce94418c7a35437a31879";
const CATALOG_SCHEMA_LOCK_ID: i64 = 0x5453_4241_4355_4c41;
const CONFIG_DIRECTORY: &str = "/etc/tapescale";
const BACULA_WORKING_DIRECTORY: &str = "/var/lib/bacula";
const MAX_DAEMON_LOG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum BaculaRuntimeError {
    #[error("invalid Bacula catalog connection: {0}")]
    CatalogConnection(String),
    #[error("Bacula catalog schema version {0} is unsupported; expected 1026")]
    CatalogVersion(i32),
    #[error("Bacula catalog schema initialization failed: {0}")]
    CatalogInitialization(String),
    #[error("Bacula configuration is invalid: {0}")]
    Configuration(String),
    #[error("could not prepare Bacula runtime path {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Bacula {daemon:?} configuration validation failed: {detail}")]
    Validation {
        daemon: BaculaDaemon,
        detail: String,
    },
    #[error("could not start Bacula {daemon:?}: {source}")]
    Spawn {
        daemon: BaculaDaemon,
        #[source]
        source: io::Error,
    },
    #[error("Bacula {daemon:?} exited during startup with status {status}")]
    EarlyExit {
        daemon: BaculaDaemon,
        status: String,
    },
}

#[derive(Clone, Debug)]
pub struct BaculaCatalogConnection {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
}

impl BaculaCatalogConnection {
    pub fn parse(database_url: &str) -> Result<Self, BaculaRuntimeError> {
        let url = Url::parse(database_url)
            .map_err(|error| BaculaRuntimeError::CatalogConnection(error.to_string()))?;
        if !matches!(url.scheme(), "postgres" | "postgresql") {
            return Err(BaculaRuntimeError::CatalogConnection(
                "DATABASE_URL must use postgres or postgresql".to_owned(),
            ));
        }
        let host = url
            .host_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| BaculaRuntimeError::CatalogConnection("host is required".to_owned()))?
            .to_owned();
        let user = percent_decode(url.username())?;
        let password = percent_decode(url.password().unwrap_or_default())?;
        let database = percent_decode(url.path().trim_start_matches('/'))?;
        if user.is_empty() || password.is_empty() || database.is_empty() {
            return Err(BaculaRuntimeError::CatalogConnection(
                "database, user, and password are required".to_owned(),
            ));
        }
        Ok(Self {
            host,
            port: url.port().unwrap_or(5432),
            database,
            user,
            password,
        })
    }
}

fn percent_decode(value: &str) -> Result<String, BaculaRuntimeError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(BaculaRuntimeError::CatalogConnection(
                    "invalid percent encoding".to_owned(),
                ));
            }
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            let (Some(high), Some(low)) = (high, low) else {
                return Err(BaculaRuntimeError::CatalogConnection(
                    "invalid percent encoding".to_owned(),
                ));
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| BaculaRuntimeError::CatalogConnection("credentials must be UTF-8".to_owned()))
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BaculaRuntimeEvidence {
    pub director_config_sha256: String,
    pub storage_config_sha256: String,
    pub file_daemon_config_sha256: String,
    pub bconsole_config_sha256: String,
    pub daemon_logs: Vec<PathBuf>,
    pub catalog_schema_version: i32,
}

struct ManagedDaemon {
    kind: BaculaDaemon,
    child: Child,
    log_tasks: Vec<JoinHandle<()>>,
}

pub struct BaculaRuntime {
    daemons: Vec<ManagedDaemon>,
    evidence: BaculaRuntimeEvidence,
}

impl BaculaRuntime {
    /// Initialize and validate the Bacula catalog before any irreversible media command.
    pub async fn prepare_catalog(
        pool: &PgPool,
        database_url: &str,
        cancellation: &CancellationToken,
    ) -> Result<i32, BaculaRuntimeError> {
        let catalog = BaculaCatalogConnection::parse(database_url)?;
        ensure_catalog_schema(pool, &catalog, cancellation).await
    }

    pub async fn start(
        pool: &PgPool,
        database_url: &str,
        run_id: Uuid,
        paths: BaculaPaths,
        evidence_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Self, BaculaRuntimeError> {
        Self::start_with_data_options(
            pool,
            database_url,
            run_id,
            paths,
            BaculaDataOptions::default(),
            evidence_directory,
            cancellation,
        )
        .await
    }

    /// Start a run-scoped data plane with an explicit on-media compression and
    /// volume-spanning policy.  The legacy `start` entrypoint remains a strict
    /// uncompressed, single-volume default for recovery exercises.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_with_data_options(
        pool: &PgPool,
        database_url: &str,
        run_id: Uuid,
        paths: BaculaPaths,
        data_options: BaculaDataOptions,
        evidence_directory: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Self, BaculaRuntimeError> {
        let catalog = BaculaCatalogConnection::parse(database_url)?;
        let catalog_schema_version = ensure_catalog_schema(pool, &catalog, cancellation).await?;
        prepare_runtime_directories(evidence_directory)?;

        let token = run_id.simple().to_string();
        let credentials = BaculaCredentials::new(
            format!("dir-{token}"),
            format!("sd-{token}"),
            format!("fd-{token}"),
            format!("console-{token}"),
            catalog.password.clone(),
        )
        .map_err(|error| BaculaRuntimeError::Configuration(error.to_string()))?;
        let endpoints = BaculaEndpoints::new(
            "127.0.0.1",
            "127.0.0.1",
            "127.0.0.1",
            catalog.host,
            catalog.port,
            catalog.database,
            catalog.user,
        )
        .map_err(|error| BaculaRuntimeError::Configuration(error.to_string()))?;
        let rendered = BaculaConfig::new(
            paths,
            endpoints,
            credentials,
            BaculaPoolName::for_run(run_id),
        )
        .with_data_options(data_options)
        .render();
        let mut evidence = write_configs(&rendered)?;
        evidence.catalog_schema_version = catalog_schema_version;

        for daemon in BaculaDaemon::STARTUP_ORDER {
            validate_daemon(daemon, cancellation).await?;
        }

        let mut daemons = Vec::with_capacity(BaculaDaemon::STARTUP_ORDER.len());
        for daemon in BaculaDaemon::STARTUP_ORDER {
            match start_daemon(daemon, evidence_directory).await {
                Ok((managed, log_paths)) => {
                    evidence.daemon_logs.extend(log_paths);
                    daemons.push(managed);
                }
                Err(error) => {
                    stop_daemons(&mut daemons).await;
                    return Err(error);
                }
            }
        }
        Ok(Self { daemons, evidence })
    }

    #[must_use]
    pub const fn evidence(&self) -> &BaculaRuntimeEvidence {
        &self.evidence
    }

    pub async fn stop(mut self) -> BaculaRuntimeEvidence {
        stop_daemons(&mut self.daemons).await;
        self.evidence
    }
}

/// Validate the exact packaged Bacula catalog script without connecting to PostgreSQL.
///
/// The container CI and operational diagnostics use this entry point so a package/script parser
/// mismatch cannot remain hidden until the first physical recovery exercise.
pub fn validate_packaged_catalog_schema() -> Result<(), BaculaRuntimeError> {
    load_packaged_catalog_schema().map(drop)
}

async fn ensure_catalog_schema(
    pool: &PgPool,
    catalog: &BaculaCatalogConnection,
    cancellation: &CancellationToken,
) -> Result<i32, BaculaRuntimeError> {
    let mut schema_lock = pool.begin().await.map_err(|error| {
        BaculaRuntimeError::CatalogInitialization(format!(
            "could not begin catalog initialization lock: {error}"
        ))
    })?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(CATALOG_SCHEMA_LOCK_ID)
        .execute(&mut *schema_lock)
        .await
        .map_err(|error| BaculaRuntimeError::CatalogInitialization(error.to_string()))?;
    let relation =
        sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass('public.version')::text")
            .fetch_one(&mut *schema_lock)
            .await
            .map_err(|error| BaculaRuntimeError::CatalogInitialization(error.to_string()))?;
    if relation.is_none() {
        let schema_sql = load_packaged_catalog_schema()?;
        let command = ToolCommand::new(PSQL_PROGRAM)
            .args([
                "-h".to_owned(),
                catalog.host.clone(),
                "-p".to_owned(),
                catalog.port.to_string(),
                "-U".to_owned(),
                catalog.user.clone(),
                "-d".to_owned(),
                catalog.database.clone(),
                "-v".to_owned(),
                "ON_ERROR_STOP=1".to_owned(),
                "-f".to_owned(),
                "-".to_owned(),
            ])
            .env("PGPASSWORD", catalog.password.as_str())
            .env("PGOPTIONS", "-c search_path=public")
            .stdin_bytes(schema_sql.as_bytes())
            .timeout(Duration::from_secs(180))
            .retained_output_bytes(1024 * 1024);
        tool_runner::run(&command, cancellation)
            .await
            .map_err(|error| {
                BaculaRuntimeError::CatalogInitialization(error.diagnostic_summary(4096))
            })?;
    }
    let versions = sqlx::query_scalar::<_, i32>("SELECT versionid FROM public.version")
        .fetch_all(&mut *schema_lock)
        .await
        .map_err(|error| BaculaRuntimeError::CatalogInitialization(error.to_string()))?;
    if versions.len() != 1 {
        return Err(BaculaRuntimeError::CatalogInitialization(format!(
            "public.version contains {} rows; the pinned catalog requires exactly one",
            versions.len()
        )));
    }
    let version = versions[0];
    if version != 1026 {
        return Err(BaculaRuntimeError::CatalogVersion(version));
    }
    validate_catalog_fingerprint(&mut schema_lock).await?;
    schema_lock
        .commit()
        .await
        .map_err(|error| BaculaRuntimeError::CatalogInitialization(error.to_string()))?;
    Ok(version)
}

async fn validate_catalog_fingerprint(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), BaculaRuntimeError> {
    let missing = sqlx::query_as::<_, (String, String)>(
        "WITH required(table_name, column_name) AS (VALUES
            ('pool', 'poolid'), ('pool', 'name'), ('pool', 'maxvols'),
            ('pool', 'recycle'), ('pool', 'autoprune'),
            ('media', 'mediaid'), ('media', 'volumename'), ('media', 'poolid'),
            ('media', 'mediatype'), ('media', 'volstatus'), ('media', 'enabled'),
            ('media', 'recycle'), ('media', 'volbytes'),
            ('job', 'jobid'), ('job', 'name'), ('job', 'type'), ('job', 'level'),
            ('job', 'jobstatus'), ('job', 'priorjobid'), ('job', 'jobfiles'),
            ('job', 'jobbytes'), ('job', 'joberrors'), ('job', 'poolid'),
            ('job', 'lastreadstorageid'), ('job', 'writestorageid'),
            ('jobmedia', 'jobmediaid'), ('jobmedia', 'jobid'),
            ('jobmedia', 'mediaid'), ('jobmedia', 'volindex'),
            ('jobmedia', 'firstindex'), ('jobmedia', 'lastindex'),
            ('jobmedia', 'startfile'), ('jobmedia', 'startblock'),
            ('jobmedia', 'endfile'), ('jobmedia', 'endblock'),
            ('storage', 'storageid'), ('storage', 'name'),
            ('client', 'clientid'), ('client', 'name'),
            ('log', 'logid'), ('log', 'jobid'), ('log', 'logtext'),
            ('path', 'pathid'), ('path', 'path'),
            ('file', 'fileid'), ('file', 'jobid'), ('file', 'pathid'),
            ('file', 'filename')
         )
         SELECT required.table_name, required.column_name
         FROM required
         LEFT JOIN information_schema.columns actual
           ON actual.table_schema = 'public'
          AND actual.table_name = required.table_name
          AND actual.column_name = required.column_name
         WHERE actual.column_name IS NULL
         ORDER BY required.table_name, required.column_name",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| BaculaRuntimeError::CatalogInitialization(error.to_string()))?;
    if !missing.is_empty() {
        let detail = missing
            .iter()
            .map(|(table, column)| format!("public.{table}.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(BaculaRuntimeError::CatalogInitialization(format!(
            "catalog version 1026 is incomplete; missing {detail}"
        )));
    }

    let inaccessible = sqlx::query_scalar::<_, String>(
        "WITH required(table_name) AS (VALUES
            ('pool'), ('media'), ('job'), ('jobmedia'), ('storage'),
            ('client'), ('log'), ('path'), ('file'), ('fileset')
         )
         SELECT table_name
         FROM required
         WHERE NOT has_table_privilege(
             current_user,
             'public.' || table_name,
             'SELECT, INSERT, UPDATE, DELETE'
         )
         ORDER BY table_name",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| BaculaRuntimeError::CatalogInitialization(error.to_string()))?;
    if !inaccessible.is_empty() {
        return Err(BaculaRuntimeError::CatalogInitialization(format!(
            "catalog role lacks required DML privileges on: {}",
            inaccessible
                .iter()
                .map(|table| format!("public.{table}"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}

fn load_packaged_catalog_schema() -> Result<String, BaculaRuntimeError> {
    let script = fs::read(CATALOG_SCHEMA_PROGRAM).map_err(|error| {
        BaculaRuntimeError::CatalogInitialization(format!(
            "could not read the pinned Bacula schema script: {error}"
        ))
    })?;
    let script_digest = digest(&script);
    if script_digest != CATALOG_SCHEMA_SCRIPT_SHA256 {
        return Err(BaculaRuntimeError::CatalogInitialization(format!(
            "Bacula schema script digest {script_digest} did not match the pinned package digest"
        )));
    }
    let script = std::str::from_utf8(&script).map_err(|error| {
        BaculaRuntimeError::CatalogInitialization(format!(
            "Bacula schema script was not UTF-8: {error}"
        ))
    })?;
    extract_catalog_schema_sql(script)
        .map(str::to_owned)
        .ok_or_else(|| {
            BaculaRuntimeError::CatalogInitialization(
                "Bacula schema script did not contain its expected SQL heredoc".to_owned(),
            )
        })
}

fn extract_catalog_schema_sql(script: &str) -> Option<&str> {
    let (_, heredoc) = script.split_once("<<END-OF-DATA\n")?;
    let (sql, _) = heredoc.rsplit_once("\nEND-OF-DATA\n")?;
    (sql.len() <= 4 * 1024 * 1024
        // The pinned Bacula 15.0.3 PostgreSQL script spells the declaration
        // with a lowercase, unquoted table name, while its later INSERT uses
        // `Version`. PostgreSQL folds both to the same public relation.
        && sql.contains("\nCREATE TABLE version\n")
        && sql.contains("INSERT INTO Version (VersionId) VALUES (1026);")
        && sql.contains("\nCOMMIT;"))
    .then_some(sql)
}

fn prepare_runtime_directories(evidence_directory: &Path) -> Result<(), BaculaRuntimeError> {
    let working_directory = Path::new(BACULA_WORKING_DIRECTORY);
    let bootstrap_directory = working_directory.join("bootstrap");
    for path in [
        Path::new(CONFIG_DIRECTORY),
        working_directory,
        bootstrap_directory.as_path(),
        Path::new("/run/bacula"),
        evidence_directory,
    ] {
        fs::create_dir_all(path).map_err(|source| BaculaRuntimeError::Io {
            path: path.to_owned(),
            source,
        })?;
    }
    Ok(())
}

fn write_configs(
    rendered: &RenderedBaculaConfigs,
) -> Result<BaculaRuntimeEvidence, BaculaRuntimeError> {
    let configs = [
        (Path::new(DIRECTOR_CONFIG), rendered.director.as_bytes()),
        (
            Path::new(STORAGE_DAEMON_CONFIG),
            rendered.storage_daemon.as_bytes(),
        ),
        (
            Path::new(FILE_DAEMON_CONFIG),
            rendered.file_daemon.as_bytes(),
        ),
        (Path::new(BCONSOLE_CONFIG), rendered.bconsole.as_bytes()),
    ];
    for (path, bytes) in configs {
        write_atomic_secret(path, bytes)?;
    }
    Ok(BaculaRuntimeEvidence {
        director_config_sha256: digest(rendered.director.as_bytes()),
        storage_config_sha256: digest(rendered.storage_daemon.as_bytes()),
        file_daemon_config_sha256: digest(rendered.file_daemon.as_bytes()),
        bconsole_config_sha256: digest(rendered.bconsole.as_bytes()),
        daemon_logs: Vec::new(),
        catalog_schema_version: 0,
    })
}

fn write_atomic_secret(path: &Path, bytes: &[u8]) -> Result<(), BaculaRuntimeError> {
    let parent = path.parent().ok_or_else(|| {
        BaculaRuntimeError::Configuration("Bacula configuration path has no parent".to_owned())
    })?;
    fs::create_dir_all(parent).map_err(|source| BaculaRuntimeError::Io {
        path: parent.to_owned(),
        source,
    })?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config"),
        Uuid::now_v7()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|source| BaculaRuntimeError::Io {
            path: temporary.clone(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| BaculaRuntimeError::Io {
            path: temporary.clone(),
            source,
        })?;
    fs::rename(&temporary, path).map_err(|source| BaculaRuntimeError::Io {
        path: path.to_owned(),
        source,
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        BaculaRuntimeError::Io {
            path: path.to_owned(),
            source,
        }
    })?;
    Ok(())
}

async fn validate_daemon(
    daemon: BaculaDaemon,
    cancellation: &CancellationToken,
) -> Result<(), BaculaRuntimeError> {
    let command = ToolCommand::new(daemon.program())
        .args(["-t", "-c", daemon.config_path()])
        .timeout(Duration::from_secs(30))
        .retained_output_bytes(256 * 1024);
    tool_runner::run(&command, cancellation)
        .await
        .map(|_| ())
        .map_err(|error| BaculaRuntimeError::Validation {
            daemon,
            detail: error.diagnostic_summary(4096),
        })
}

async fn start_daemon(
    daemon: BaculaDaemon,
    evidence_directory: &Path,
) -> Result<(ManagedDaemon, Vec<PathBuf>), BaculaRuntimeError> {
    let stem = format!("{:?}", daemon).to_ascii_lowercase();
    let stdout_path = evidence_directory.join(format!("{stem}.stdout.log"));
    let stderr_path = evidence_directory.join(format!("{stem}.stderr.log"));
    let stdout_log = tokio::fs::File::from_std(create_log(&stdout_path)?);
    let stderr_log = tokio::fs::File::from_std(create_log(&stderr_path)?);
    let mut command = daemon.foreground_command();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|source| BaculaRuntimeError::Spawn { daemon, source })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BaculaRuntimeError::Spawn {
            daemon,
            source: io::Error::other("Bacula stdout pipe was not captured"),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| BaculaRuntimeError::Spawn {
            daemon,
            source: io::Error::other("Bacula stderr pipe was not captured"),
        })?;
    let log_tasks = vec![
        spawn_bounded_log(stdout, stdout_log, stdout_path.clone()),
        spawn_bounded_log(stderr, stderr_log, stderr_path.clone()),
    ];
    tokio::time::sleep(Duration::from_millis(750)).await;
    if let Some(status) = child
        .try_wait()
        .map_err(|source| BaculaRuntimeError::Spawn { daemon, source })?
    {
        for task in log_tasks {
            let _ = task.await;
        }
        return Err(BaculaRuntimeError::EarlyExit {
            daemon,
            status: status.to_string(),
        });
    }
    Ok((
        ManagedDaemon {
            kind: daemon,
            child,
            log_tasks,
        },
        vec![stdout_path, stderr_path],
    ))
}

fn spawn_bounded_log<R>(reader: R, writer: tokio::fs::File, path: PathBuf) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(error) = drain_bounded_log(reader, writer).await {
            tracing::warn!(path = %path.display(), %error, "Bacula daemon log capture failed");
        }
    })
}

async fn drain_bounded_log<R>(mut reader: R, mut writer: tokio::fs::File) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut retained = 0_usize;
    let mut marker_written = false;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let writable = read.min(MAX_DAEMON_LOG_BYTES.saturating_sub(retained));
        if writable > 0 {
            writer.write_all(&buffer[..writable]).await?;
            retained += writable;
        }
        if writable < read && !marker_written {
            writer
                .write_all(b"\n[TapeScale: daemon log retention limit reached; remaining output discarded]\n")
                .await?;
            marker_written = true;
        }
    }
    writer.flush().await
}

fn create_log(path: &Path) -> Result<File, BaculaRuntimeError> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| BaculaRuntimeError::Io {
            path: path.to_owned(),
            source,
        })
}

async fn stop_daemons(daemons: &mut Vec<ManagedDaemon>) {
    while let Some(mut daemon) = daemons.pop() {
        if let Some(pid) = daemon.child.id() {
            if let Ok(raw_pid) = i32::try_from(pid) {
                if let Some(pid) = Pid::from_raw(raw_pid) {
                    if let Err(error) = kill_process(pid, Signal::TERM) {
                        tracing::warn!(daemon = ?daemon.kind, %error, "could not send SIGTERM to Bacula daemon");
                    }
                }
            }
        }
        if tokio::time::timeout(Duration::from_secs(10), daemon.child.wait())
            .await
            .is_err()
        {
            let _ = daemon.child.kill().await;
            let _ = daemon.child.wait().await;
        }
        for task in daemon.log_tasks {
            let _ = task.await;
        }
        tracing::info!(daemon = ?daemon.kind, "Bacula data-plane daemon stopped");
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_decodes_postgresql_connection() {
        let connection = BaculaCatalogConnection::parse(
            "postgresql://tape_user:p%40ss@postgres:5544/tape_catalog",
        )
        .expect("connection");
        assert_eq!(connection.host, "postgres");
        assert_eq!(connection.port, 5544);
        assert_eq!(connection.user, "tape_user");
        assert_eq!(connection.password, "p@ss");
        assert_eq!(connection.database, "tape_catalog");
    }

    #[test]
    fn rejects_incomplete_catalog_connections() {
        assert!(BaculaCatalogConnection::parse("sqlite:///tmp/catalog").is_err());
        assert!(BaculaCatalogConnection::parse("postgresql://user@postgres/catalog").is_err());
    }

    #[test]
    fn extracts_schema_without_expanding_postgresql_dollar_quotes() {
        // Preserve the declaration/INSERT capitalization used by the exact
        // Bacula 15.0.3 package script (digest-pinned above).
        let script = "#!/bin/sh\npsql -f - <<END-OF-DATA\nBEGIN;\ndo $$ begin perform 1; end $$;\nCREATE TABLE version\n(\n    versionid integer not null\n);\nINSERT INTO Version (VersionId) VALUES (1026);\nCOMMIT;\nEND-OF-DATA\nexit $?\n";
        let sql = extract_catalog_schema_sql(script).expect("pinned heredoc");
        assert!(sql.contains("do $$ begin"));
        assert!(sql.contains("end $$;"));
        assert!(sql.contains("CREATE TABLE version"));
    }

    #[test]
    fn rejects_a_schema_heredoc_without_the_pinned_version_declaration() {
        let script = "#!/bin/sh\npsql -f - <<END-OF-DATA\nBEGIN;\nCREATE TABLE other (id integer);\nINSERT INTO Version (VersionId) VALUES (1026);\nCOMMIT;\nEND-OF-DATA\n";
        assert!(extract_catalog_schema_sql(script).is_none());
    }
}
