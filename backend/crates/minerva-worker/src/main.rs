//! Standalone document-ingest worker.
//!
//! Boots `AppState` and runs the doc-claim loop + stale-doc sweeper +
//! relink sweeper (`minerva_app_core::worker::start_worker_loops`).
//! Embedding / reranking go to the model-server pods over gRPC (set
//! `MINERVA_EMBEDDER_URL` / `MINERVA_RERANKER_URL`). The periodic
//! Canvas / NRPS schedulers run in the dedicated `minerva-scheduler`
//! pod, not here.

use minerva_app_core::{config::Config, memprobe, state::AppState, worker};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("minerva=debug,tower_http=debug")),
        )
        .init();

    let config = Config::from_env()?;
    let state = AppState::new(&config).await?;

    // Same probe as the api / scheduler pods, tagged with this binary's
    // log target so a `memprobe` grep lands on the worker pod.
    memprobe::spawn();

    tracing::info!("starting minerva-worker (doc claim + stale/relink sweepers)");
    worker::start_worker_loops(state, config.max_concurrent_ingests);

    tokio::signal::ctrl_c().await?;
    tracing::info!("minerva-worker: ctrl_c received, shutting down");
    Ok(())
}
