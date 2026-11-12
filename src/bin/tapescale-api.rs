use anyhow::{Context, Result};
use tapescale::{api, config::Config, store};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = Config::from_env()?;
    let pool = match &config.database_url {
        Some(database_url) => {
            match store::connect(database_url, config.database_connect_timeout).await {
                Ok(pool) => Some(pool),
                Err(error) if config.demo_mode => {
                    warn!(error = %error, "PostgreSQL unavailable; continuing in explicit simulation mode");
                    None
                }
                Err(error) => return Err(error),
            }
        }
        None if config.demo_mode => None,
        None => anyhow::bail!("DATABASE_URL is required outside simulation mode"),
    };

    let state = api::AppState::new(pool, config.demo_mode, config.lab_tape_motion_enabled);
    let app = api::router(state, config.web_dir);
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("failed to bind {}", config.bind))?;
    info!(address = %config.bind, demo_mode = config.demo_mode, "TapeScale API listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("API server stopped unexpectedly")?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("tapescale=info,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}

async fn shutdown_signal() {
    let control_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = control_c => {},
        () = terminate => {},
    }
}
