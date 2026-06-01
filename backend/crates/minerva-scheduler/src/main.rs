//! Standalone periodic scheduler.
//!
//! Boots `AppState` and runs the four periodic pollers via
//! `minerva_app_core::schedulers::start_scheduler_loops`:
//!   - Canvas auto-sync (re-syncs `auto_sync=true` connections that are due)
//!   - LTI NRPS reconcile (roster add/remove per syncable context)
//!   - LTI platform-health probe (~daily token-endpoint ping)
//!   - pending-platform cleanup (drops unapproved dynreg rows)
//!
//! No HTTP listener and no model engine: the loops are DB queries +
//! outbound HTTP, and Canvas downloads land on the shared docs volume.
//! The fat ingest worker lives in the separate `minerva-worker` pod, so
//! an ingest OOM can't pause NRPS reconciliation and a worker roll
//! doesn't reset these scheduler clocks.

use minerva_app_core::{config::Config, schedulers, state::AppState};
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

    minerva_metrics::init("minerva-scheduler");

    let config = Config::from_env()?;
    let state = AppState::new(&config).await?;

    // Same probe as the api / worker pods. The `service` global label
    // (set in `init`) tags the gauges, and the binary's log target tags
    // the trace line, so a `memprobe` grep lands on the scheduler pod.
    minerva_metrics::spawn_memprobe("minerva-scheduler");

    tracing::info!(
        "starting minerva-scheduler (canvas / lti nrps / platform health / pending cleanup)"
    );
    schedulers::start_scheduler_loops(state);

    tokio::signal::ctrl_c().await?;
    tracing::info!("minerva-scheduler: ctrl_c received, shutting down");
    Ok(())
}
