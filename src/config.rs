use std::{env, fs, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{Context, Result};

use crate::engine::bacula::DATA_SPOOL_DIRECTORY;

// Zero deliberately means "use all capacity currently available after the
// configured and per-archive reserves".  A fixed ceiling can make Bacula
// alternate between filling and draining partial spool batches, which leaves
// a fast tape drive starved while the next batch is produced.
const DEFAULT_DATA_SPOOL_MAX_BYTES: u64 = 0;
const DEFAULT_DATA_SPOOL_RESERVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const DEFAULT_DATA_SPOOL_MIN_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const DEFAULT_NATIVE_SLICE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const DEFAULT_NATIVE_PACK_TARGET_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const DEFAULT_NATIVE_PACK_MAXIMUM_BYTES: u64 = 384 * 1024 * 1024 * 1024;
const DEFAULT_NATIVE_SPOOL_HIGH_WATER_BYTES: u64 = 512 * 1024 * 1024 * 1024;
const DEFAULT_NATIVE_SPOOL_LOW_WATER_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const DEFAULT_NATIVE_SPOOL_READY_BYTES: u64 = 128 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: Option<String>,
    pub demo_mode: bool,
    pub web_dir: PathBuf,
    pub database_connect_timeout: Duration,
    pub worker_poll_interval: Duration,
    pub lab_tape_motion_enabled: bool,
    pub lab_tape_qualification_enabled: bool,
    pub lab_file_roundtrip_enabled: bool,
    pub btape_path: PathBuf,
    pub bconsole_path: PathBuf,
    pub bacula_state_dir: PathBuf,
    pub qualification_timeout: Duration,
    pub source_root: PathBuf,
    pub restore_root: PathBuf,
    pub evidence_root: PathBuf,
    pub data_run_timeout: Duration,
    pub transfer_stall_timeout: Duration,
    pub bacula_spool_dir: PathBuf,
    pub bacula_spool_max_bytes: u64,
    pub bacula_spool_reserve_bytes: u64,
    pub bacula_spool_min_bytes: u64,
    pub dar_path: PathBuf,
    pub native_spool_dir: PathBuf,
    pub native_spool_reserve_bytes: u64,
    pub native_pack_target_bytes: u64,
    pub native_pack_maximum_bytes: u64,
    pub native_spool_high_water_bytes: u64,
    pub native_spool_low_water_bytes: u64,
    pub native_spool_ready_bytes: u64,
    pub native_slice_bytes: u64,
    pub native_compression_threads: u16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind = value("TAPESCALE_BIND", "0.0.0.0:8080")
            .parse()
            .context("TAPESCALE_BIND must be a valid socket address")?;
        let database_url = database_url()?;
        let demo_mode = parse_bool("TAPESCALE_DEMO_MODE", database_url.is_none())?;
        let web_dir = PathBuf::from(value("TAPESCALE_WEB_DIR", "web"));
        let database_connect_timeout = Duration::from_secs(parse("TAPESCALE_DB_TIMEOUT_SECS", 10)?);
        let worker_poll_interval = Duration::from_secs(parse("TAPESCALE_WORKER_POLL_SECS", 5)?);
        let lab_tape_motion_enabled = parse_bool_alias(
            "TAPESCALE_TAPE_MOTION_ENABLED",
            "TAPESCALE_LAB_TAPE_MOTION_ENABLED",
            false,
        )?;
        let lab_tape_qualification_enabled = parse_bool_alias(
            "TAPESCALE_TAPE_PREPARATION_ENABLED",
            "TAPESCALE_LAB_TAPE_QUALIFICATION_ENABLED",
            false,
        )?;
        let lab_file_roundtrip_enabled = parse_bool_alias(
            "TAPESCALE_NATIVE_BACKUP_ENABLED",
            "TAPESCALE_LAB_FILE_ROUNDTRIP_ENABLED",
            false,
        )?;
        let btape_path = PathBuf::from(value("TAPESCALE_BTAPE_PATH", "/usr/sbin/btape"));
        let bconsole_path = PathBuf::from(value("TAPESCALE_BCONSOLE_PATH", "/usr/sbin/bconsole"));
        let bacula_state_dir = PathBuf::from(value(
            "TAPESCALE_BACULA_STATE_DIR",
            "/var/lib/tapescale/bacula",
        ));
        let qualification_timeout =
            Duration::from_secs(parse("TAPESCALE_QUALIFICATION_TIMEOUT_SECS", 1_800)?);
        let source_root = PathBuf::from(value("TAPESCALE_SOURCE_ROOT", "/source"));
        let restore_root = PathBuf::from(value("TAPESCALE_RESTORE_ROOT", "/restore"));
        let evidence_root = PathBuf::from(value(
            "TAPESCALE_EVIDENCE_ROOT",
            "/var/lib/tapescale/recovery",
        ));
        let data_run_timeout =
            Duration::from_secs(parse("TAPESCALE_DATA_RUN_TIMEOUT_SECS", 604_800)?);
        let transfer_stall_timeout =
            Duration::from_secs(parse("TAPESCALE_TRANSFER_STALL_TIMEOUT_SECS", 1_800)?);
        let bacula_spool_dir = PathBuf::from(DATA_SPOOL_DIRECTORY);
        let bacula_spool_max_bytes = parse(
            "TAPESCALE_BACULA_SPOOL_MAX_BYTES",
            DEFAULT_DATA_SPOOL_MAX_BYTES,
        )?;
        let bacula_spool_reserve_bytes = parse(
            "TAPESCALE_BACULA_SPOOL_RESERVE_BYTES",
            DEFAULT_DATA_SPOOL_RESERVE_BYTES,
        )?;
        let bacula_spool_min_bytes = parse(
            "TAPESCALE_BACULA_SPOOL_MIN_BYTES",
            DEFAULT_DATA_SPOOL_MIN_BYTES,
        )?;
        let dar_path = PathBuf::from(value("TAPESCALE_DAR_PATH", "/usr/bin/dar"));
        let native_spool_dir = PathBuf::from(value(
            "TAPESCALE_NATIVE_SPOOL_DIR",
            "/var/lib/tapescale/spool",
        ));
        let native_spool_reserve_bytes = parse(
            "TAPESCALE_NATIVE_SPOOL_RESERVE_BYTES",
            DEFAULT_DATA_SPOOL_RESERVE_BYTES,
        )?;
        let native_pack_target_bytes = parse(
            "TAPESCALE_NATIVE_PACK_TARGET_BYTES",
            DEFAULT_NATIVE_PACK_TARGET_BYTES,
        )?;
        let native_pack_maximum_bytes = parse(
            "TAPESCALE_NATIVE_PACK_MAXIMUM_BYTES",
            DEFAULT_NATIVE_PACK_MAXIMUM_BYTES,
        )?;
        let native_spool_high_water_bytes = parse(
            "TAPESCALE_NATIVE_SPOOL_HIGH_WATER_BYTES",
            DEFAULT_NATIVE_SPOOL_HIGH_WATER_BYTES,
        )?;
        let native_spool_low_water_bytes = parse(
            "TAPESCALE_NATIVE_SPOOL_LOW_WATER_BYTES",
            DEFAULT_NATIVE_SPOOL_LOW_WATER_BYTES,
        )?;
        let native_spool_ready_bytes = parse(
            "TAPESCALE_NATIVE_SPOOL_READY_BYTES",
            DEFAULT_NATIVE_SPOOL_READY_BYTES,
        )?;
        let native_slice_bytes = parse("TAPESCALE_NATIVE_SLICE_BYTES", DEFAULT_NATIVE_SLICE_BYTES)?;
        let native_compression_threads = parse("TAPESCALE_NATIVE_COMPRESSION_THREADS", 4_u16)?;

        anyhow::ensure!(
            qualification_timeout >= Duration::from_secs(60),
            "TAPESCALE_QUALIFICATION_TIMEOUT_SECS must be at least 60"
        );
        anyhow::ensure!(
            qualification_timeout <= Duration::from_secs(5_400),
            "TAPESCALE_QUALIFICATION_TIMEOUT_SECS must not exceed 5400 for an attended lab run"
        );
        anyhow::ensure!(
            source_root.is_absolute() && restore_root.is_absolute(),
            "TAPESCALE_SOURCE_ROOT and TAPESCALE_RESTORE_ROOT must be absolute paths"
        );
        anyhow::ensure!(
            source_root != restore_root,
            "TAPESCALE_SOURCE_ROOT and TAPESCALE_RESTORE_ROOT must differ"
        );
        anyhow::ensure!(
            evidence_root.is_absolute(),
            "TAPESCALE_EVIDENCE_ROOT must be an absolute path"
        );
        anyhow::ensure!(
            bacula_state_dir.is_absolute(),
            "TAPESCALE_BACULA_STATE_DIR must be an absolute path"
        );
        if lab_file_roundtrip_enabled {
            anyhow::ensure!(
                source_root == PathBuf::from("/source")
                    && restore_root == PathBuf::from("/restore"),
                "the native backup worker requires TAPESCALE_SOURCE_ROOT=/source and TAPESCALE_RESTORE_ROOT=/restore"
            );
        }
        anyhow::ensure!(
            data_run_timeout >= Duration::from_secs(3_600)
                && data_run_timeout <= Duration::from_secs(2_592_000),
            "TAPESCALE_DATA_RUN_TIMEOUT_SECS must be between 3600 and 2592000"
        );
        anyhow::ensure!(
            transfer_stall_timeout >= Duration::from_secs(300)
                && transfer_stall_timeout <= Duration::from_secs(7_200),
            "TAPESCALE_TRANSFER_STALL_TIMEOUT_SECS must be between 300 and 7200"
        );
        anyhow::ensure!(
            bacula_spool_dir.is_absolute(),
            "the pinned Bacula spool directory must be an absolute path"
        );
        anyhow::ensure!(
            !bacula_spool_dir.starts_with(&source_root)
                && !source_root.starts_with(&bacula_spool_dir)
                && !bacula_spool_dir.starts_with(&restore_root)
                && !restore_root.starts_with(&bacula_spool_dir)
                && !bacula_spool_dir.starts_with(&evidence_root)
                && !evidence_root.starts_with(&bacula_spool_dir)
                && !bacula_spool_dir.starts_with(&bacula_state_dir)
                && !bacula_state_dir.starts_with(&bacula_spool_dir),
            "the Bacula spool must be isolated from source, restore, evidence, and engine-state trees"
        );
        anyhow::ensure!(
            bacula_spool_min_bytes > 0
                && (bacula_spool_max_bytes == 0
                    || bacula_spool_max_bytes >= bacula_spool_min_bytes),
            "TAPESCALE_BACULA_SPOOL_MIN_BYTES must be non-zero; TAPESCALE_BACULA_SPOOL_MAX_BYTES must be zero (automatic) or at least the minimum"
        );
        anyhow::ensure!(
            dar_path.is_absolute() && native_spool_dir.is_absolute(),
            "TAPESCALE_DAR_PATH and TAPESCALE_NATIVE_SPOOL_DIR must be absolute paths"
        );
        anyhow::ensure!(
            (64 * 1024 * 1024..=1024_u64 * 1024 * 1024 * 1024).contains(&native_slice_bytes),
            "TAPESCALE_NATIVE_SLICE_BYTES must be between 64 MiB and 1 TiB"
        );
        anyhow::ensure!(
            (64_u64 * 1024 * 1024 * 1024..=1024_u64 * 1024 * 1024 * 1024)
                .contains(&native_pack_target_bytes),
            "TAPESCALE_NATIVE_PACK_TARGET_BYTES must be between 64 GiB and 1 TiB"
        );
        anyhow::ensure!(
            native_pack_target_bytes >= native_slice_bytes,
            "TAPESCALE_NATIVE_PACK_TARGET_BYTES must be at least TAPESCALE_NATIVE_SLICE_BYTES"
        );
        anyhow::ensure!(
            native_pack_maximum_bytes >= native_pack_target_bytes
                && native_pack_maximum_bytes <= 1024_u64 * 1024 * 1024 * 1024,
            "TAPESCALE_NATIVE_PACK_MAXIMUM_BYTES must be at least the pack target and no more than 1 TiB"
        );
        anyhow::ensure!(
            native_spool_low_water_bytes > 0
                && native_spool_low_water_bytes < native_spool_ready_bytes,
            "TAPESCALE_NATIVE_SPOOL_LOW_WATER_BYTES must be non-zero and less than TAPESCALE_NATIVE_SPOOL_READY_BYTES"
        );
        anyhow::ensure!(
            native_spool_ready_bytes >= native_pack_target_bytes,
            "TAPESCALE_NATIVE_SPOOL_READY_BYTES must hold at least one normal target pack"
        );
        // Ready reserve and worst-case oversized allocation group are
        // independent. This lets ordinary packs stay small without rejecting
        // a single large file or hard-link group.
        let minimum_high_water = native_spool_ready_bytes
            .checked_add(native_pack_maximum_bytes)
            .context("the Native v2 ready reserve plus maximum pack is too large")?;
        anyhow::ensure!(
            native_spool_high_water_bytes >= minimum_high_water,
            "TAPESCALE_NATIVE_SPOOL_HIGH_WATER_BYTES must hold the ready reserve plus one maximum-size in-progress pack"
        );
        native_spool_high_water_bytes
            .checked_add(native_spool_reserve_bytes)
            .context("native spool high-water mark plus reserve overflows the capacity range")?;
        anyhow::ensure!(
            (1..=64).contains(&native_compression_threads),
            "TAPESCALE_NATIVE_COMPRESSION_THREADS must be between 1 and 64"
        );
        anyhow::ensure!(
            !native_spool_dir.starts_with(&source_root)
                && !source_root.starts_with(&native_spool_dir)
                && !native_spool_dir.starts_with(&restore_root)
                && !restore_root.starts_with(&native_spool_dir)
                && !native_spool_dir.starts_with(&evidence_root)
                && !evidence_root.starts_with(&native_spool_dir),
            "the native archive spool must be isolated from source, restore, and evidence trees"
        );

        Ok(Self {
            bind,
            database_url,
            demo_mode,
            web_dir,
            database_connect_timeout,
            worker_poll_interval,
            lab_tape_motion_enabled,
            lab_tape_qualification_enabled,
            lab_file_roundtrip_enabled,
            btape_path,
            bconsole_path,
            bacula_state_dir,
            qualification_timeout,
            source_root,
            restore_root,
            evidence_root,
            data_run_timeout,
            transfer_stall_timeout,
            bacula_spool_dir,
            bacula_spool_max_bytes,
            bacula_spool_reserve_bytes,
            bacula_spool_min_bytes,
            dar_path,
            native_spool_dir,
            native_spool_reserve_bytes,
            native_pack_target_bytes,
            native_pack_maximum_bytes,
            native_spool_high_water_bytes,
            native_spool_low_water_bytes,
            native_spool_ready_bytes,
            native_slice_bytes,
            native_compression_threads,
        })
    }
}

fn database_url() -> Result<Option<String>> {
    if let Some(url) = env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(url));
    }
    let Ok(password_file) = env::var("TAPESCALE_DATABASE_PASSWORD_FILE") else {
        return Ok(None);
    };
    let password = fs::read_to_string(&password_file)
        .with_context(|| format!("cannot read database password file {password_file}"))?;
    let password = password.trim_end_matches(['\r', '\n']);
    anyhow::ensure!(!password.is_empty(), "database password file is empty");

    let host = value("TAPESCALE_DATABASE_HOST", "postgres");
    let port = parse("TAPESCALE_DATABASE_PORT", 5432_u16)?;
    let user = value("TAPESCALE_DATABASE_USER", "tapescale");
    let name = value("TAPESCALE_DATABASE_NAME", "tapescale");
    let mut url = url::Url::parse("postgresql://localhost").expect("static database URL is valid");
    url.set_host(Some(&host)).context("invalid database host")?;
    url.set_port(Some(port))
        .map_err(|()| anyhow::anyhow!("invalid database port"))?;
    url.set_username(&user)
        .map_err(|()| anyhow::anyhow!("invalid database user"))?;
    url.set_password(Some(password))
        .map_err(|()| anyhow::anyhow!("invalid database password"))?;
    url.set_path(&name);
    Ok(Some(url.into()))
}

fn value(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn parse<T>(key: &str, default: T) -> Result<T>
where
    T: FromStr + Copy,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    env::var(key).map_or(Ok(default), |raw| {
        raw.parse()
            .with_context(|| format!("{key} has an invalid value"))
    })
}

fn parse_bool(key: &str, default: bool) -> Result<bool> {
    let Ok(raw) = env::var(key) else {
        return Ok(default);
    };
    parse_bool_value(key, &raw)
}

fn parse_bool_alias(primary: &str, legacy: &str, default: bool) -> Result<bool> {
    if let Ok(raw) = env::var(primary) {
        return parse_bool_value(primary, &raw);
    }
    parse_bool(legacy, default)
}

fn parse_bool_value(key: &str, raw: &str) -> Result<bool> {
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("{key} must be true or false"),
    }
}
