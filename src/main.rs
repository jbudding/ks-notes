use clap::Parser;
use ks_notes::config::Config;
use ks_notes::{build_state, db, routes};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let config = Config::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ks_notes=info,tower_http=info")),
        )
        .init();

    if let Err(e) = run(config).await {
        tracing::error!(error = ?e, "fatal");
        std::process::exit(1);
    }
}

async fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", config.bind, config.port);
    let state = build_state(config).map_err(|e| format!("startup failed: {e:?}"))?;

    // Clean up uploads that never got attached to a saved memo (>24h old).
    match db::resources::purge_orphans(&state.pool, 24 * 3600).await {
        Ok(n) if n > 0 => tracing::info!(count = n, "purged orphaned uploads"),
        Err(e) => tracing::warn!(error = ?e, "orphan purge failed"),
        _ => {}
    }

    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("shut down cleanly");
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
