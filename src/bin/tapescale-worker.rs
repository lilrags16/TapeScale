use anyhow::Result;
use clap::Parser;
use tapescale::{
    config::Config,
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
    let engine = SimulatorEngine::new();

    loop {
        reconcile_once(pool.as_ref(), &engine).await?;
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

async fn reconcile_once(pool: Option<&sqlx::PgPool>, engine: &impl TapeEngine) -> Result<()> {
    let identity = engine.identity().await?;
    if identity.qualification != QualificationStatus::PhysicallyQualified {
        warn!(
            qualification = ?identity.qualification,
            "engine is not physically qualified; physical operations remain blocked"
        );
    }

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

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("tapescale=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}
