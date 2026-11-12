use anyhow::Result;
use clap::Parser;
use tapescale::{
    config::Config,
    device::{discover_sas_drives, SasDiscoveryConfig, SasProbeResult},
    engine::{simulator::SimulatorEngine, QualificationStatus, TapeEngine},
    store,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "tapescale-worker",
    version,
    about = "TapeScale durable orchestration worker"
)]
struct Arguments {
    /// Perform one reconciliation pass and exit.
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let arguments = Arguments::parse();
    let config = Config::from_env()?;
    let pool = match &config.database_url {
        Some(url) => Some(store::connect(url, config.database_connect_timeout).await?),
        None if config.demo_mode => None,
        None => anyhow::bail!("DATABASE_URL is required outside simulation mode"),
    };
    let sas_config = SasDiscoveryConfig::from_env()?;
    if sas_config.is_enabled() {
        info!(
            configured_devices = sas_config.devices.len(),
            "read-only SAS tape discovery enabled"
        );
    }
    let engine = SimulatorEngine::new();

    loop {
        reconcile_once(pool.as_ref(), &engine, &sas_config).await?;
        if arguments.once {
            break;
        }
        tokio::select! {
            () = tokio::time::sleep(config.worker_poll_interval) => {},
            result = tokio::signal::ctrl_c() => {
                result?;
                info!("worker drain requested; no new operation will be claimed");
                break;
            }
        }
    }
    Ok(())
}

async fn reconcile_once(
    pool: Option<&sqlx::PgPool>,
    engine: &impl TapeEngine,
    sas_config: &SasDiscoveryConfig,
) -> Result<()> {
    let identity = engine.identity().await?;
    if identity.qualification != QualificationStatus::PhysicallyQualified {
        warn!(
            qualification = ?identity.qualification,
            "engine is not physically qualified; physical operations remain blocked"
        );
    }

    reconcile_sas_devices(pool, sas_config).await?;

    if let Some(pool) = pool {
        let queued = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM job_runs WHERE state IN ('queued', 'blocked', 'interrupted') AND finished_at IS NULL",
        )
        .fetch_one(pool)
        .await?;
        info!(queued_runs = queued, "durable queue reconciled");
    } else {
        info!("simulation reconciliation complete; no durable queue configured");
    }
    Ok(())
}

async fn reconcile_sas_devices(
    pool: Option<&sqlx::PgPool>,
    config: &SasDiscoveryConfig,
) -> Result<()> {
    if !config.is_enabled() {
        return Ok(());
    }
    for result in discover_sas_drives(config).await {
        match result {
            SasProbeResult::Observed(observation) => {
                info!(
                    stable_id = %observation.stable_id,
                    device = %observation.observed_path,
                    status = %observation.status,
                    identity_verified = observation.identity_verified,
                    identity_conflict = observation.identity_conflict,
                    "SAS tape drive observed"
                );
                if let Some(pool) = pool {
                    store::record_drive_observation(pool, &observation).await?;
                }
            }
            SasProbeResult::Missing(missing) => {
                warn!(
                    device = %missing.observed_path,
                    reason = %missing.reason,
                    "configured SAS tape drive is unavailable"
                );
                if let Some(pool) = pool {
                    store::mark_drive_missing(pool, &missing).await?;
                }
            }
        }
    }
    Ok(())
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("tapescale=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}
