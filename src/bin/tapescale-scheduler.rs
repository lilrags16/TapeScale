use anyhow::Result;
use chrono::Utc;
use chrono_tz::Tz;
use clap::Parser;
use tapescale::{
    config::Config,
    custody_ops::{self, CustodyActor, ReconcileOverdueRequest},
    domain::schedule::next_default_occurrence,
    store,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "tapescale-scheduler",
    version,
    about = "TapeScale durable policy scheduler"
)]
struct Arguments {
    /// Evaluate schedules once and exit.
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
    let timezone: Tz = std::env::var("TAPESCALE_SITE_TIMEZONE")
        .unwrap_or_else(|_| "UTC".to_owned())
        .parse()?;

    loop {
        evaluate_once(pool.as_ref(), timezone).await?;
        if arguments.once {
            break;
        }
        tokio::select! {
            () = tokio::time::sleep(config.worker_poll_interval) => {},
            result = tokio::signal::ctrl_c() => {
                result?;
                info!("scheduler drain requested");
                break;
            }
        }
    }
    Ok(())
}

async fn evaluate_once(pool: Option<&sqlx::PgPool>, timezone: Tz) -> Result<()> {
    let next = next_default_occurrence(Utc::now(), timezone);
    let (enabled_policies, newly_overdue_movements) = if let Some(pool) = pool {
        let enabled =
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM protection_policies WHERE enabled")
                .fetch_one(pool)
                .await?;
        let overdue = custody_ops::reconcile_overdue_movements(
            pool,
            ReconcileOverdueRequest {
                as_of: Utc::now(),
                actor: CustodyActor {
                    user_id: None,
                    label: "tapescale-scheduler".to_owned(),
                },
                correlation_id: uuid::Uuid::now_v7(),
            },
        )
        .await?
        .newly_overdue_task_ids
        .len();
        (enabled, overdue)
    } else {
        (0, 0)
    };
    info!(
        enabled_policies,
        newly_overdue_movements,
        occurrence_id = %next.id,
        kind = ?next.kind,
        scheduled_for = %next.scheduled_for,
        shifted_for_dst_gap = next.shifted_for_dst_gap,
        "schedule evaluation complete"
    );
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
