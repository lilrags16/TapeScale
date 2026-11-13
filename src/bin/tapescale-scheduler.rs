use anyhow::Result;
use chrono::Utc;
use clap::Parser;
use tapescale::{
    backup_ops,
    config::Config,
    custody_ops::{self, CustodyActor, ReconcileOverdueRequest},
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
    loop {
        evaluate_once(pool.as_ref(), config.lab_file_roundtrip_enabled).await?;
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

async fn evaluate_once(pool: Option<&sqlx::PgPool>, attended_backup_enabled: bool) -> Result<()> {
    let as_of = Utc::now();
    let (backup_queue, newly_overdue_movements) = if let Some(pool) = pool {
        // The queue operation owns per-policy timezones, occurrence idempotency, and policy
        // revalidation. The first operational data path is intentionally weekly-Full only;
        // incremental schedules remain persisted but cannot be dispatched by this worker.
        let backup_queue = if attended_backup_enabled {
            Some(backup_ops::enqueue_due_backup_requests(pool, as_of).await?)
        } else {
            None
        };
        let overdue = custody_ops::reconcile_overdue_movements(
            pool,
            ReconcileOverdueRequest {
                as_of,
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
        (backup_queue, overdue)
    } else {
        (None, 0)
    };
    info!(
        backup_queue = ?backup_queue,
        newly_overdue_movements,
        evaluation_as_of = %as_of,
        attended_backup_enabled,
        operational_backup_kind = "full",
        incremental_execution = "blocked",
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
