use std::{env, fs, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{Context, Result};

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
    pub btape_path: PathBuf,
    pub bacula_state_dir: PathBuf,
    pub qualification_timeout: Duration,
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
        let lab_tape_motion_enabled = parse_bool("TAPESCALE_LAB_TAPE_MOTION_ENABLED", false)?;
        let lab_tape_qualification_enabled =
            parse_bool("TAPESCALE_LAB_TAPE_QUALIFICATION_ENABLED", false)?;
        let btape_path = PathBuf::from(value("TAPESCALE_BTAPE_PATH", "/usr/sbin/btape"));
        let bacula_state_dir = PathBuf::from(value(
            "TAPESCALE_BACULA_STATE_DIR",
            "/var/lib/tapescale/bacula",
        ));
        let qualification_timeout =
            Duration::from_secs(parse("TAPESCALE_QUALIFICATION_TIMEOUT_SECS", 1_800)?);

        anyhow::ensure!(
            qualification_timeout >= Duration::from_secs(60),
            "TAPESCALE_QUALIFICATION_TIMEOUT_SECS must be at least 60"
        );
        anyhow::ensure!(
            qualification_timeout <= Duration::from_secs(5_400),
            "TAPESCALE_QUALIFICATION_TIMEOUT_SECS must not exceed 5400 for an attended lab run"
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
            btape_path,
            bacula_state_dir,
            qualification_timeout,
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
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("{key} must be true or false"),
    }
}
