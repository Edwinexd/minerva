//! Standalone lecture-video fetcher for the visual extraction pipeline.
//!
//! GitHub Actions only logs in to Play and offers each lecture's sources
//! (bearer URLs, announced sizes, JWT deadline, cues). This service does
//! the rest, earliest deadline first:
//!   - probe loops: pick the slide source of a multi-source lecture from
//!     OCR'd frames (`probe`), shrinking its reservation to that source;
//!   - download loop: stream the chosen MP4 into the staging window
//!     (`download`);
//!   - cleanup loop: remove staging files no claim or job owns.
//!
//! Runs as one `Recreate` replica with no HTTP listener. Claims live in
//! the database, so on startup every claim is from a process that is gone
//! and is released at once; nothing needs draining on shutdown.

mod download;
mod probe;

use std::time::Duration;

use minerva_app_core::config::VisualExtractionConfig;
use minerva_db::queries::visual_extraction as queue;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

/// Lectures probed at once. Only multi-source lectures are probed, and one
/// probe takes a few seconds, so one loop keeps the single download loop
/// fed; tool processes are bounded separately in `probe`.
const PROBE_LOOPS: usize = 1;
const IDLE: Duration = Duration::from_secs(5);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("minerva=debug")),
        )
        .init();

    minerva_metrics::init("minerva-fetcher");

    let database_url = std::env::var("DATABASE_URL")?;
    let staging_path = VisualExtractionConfig::from_env().staging_path;
    let db = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await?;

    minerva_metrics::spawn_memprobe("minerva-fetcher");

    let released = queue::reset_fetch_claims(&db).await?;
    tracing::info!(released, "starting minerva-fetcher");

    for _ in 0..PROBE_LOOPS {
        let db = db.clone();
        tokio::spawn(async move {
            loop {
                match probe::probe_tick(&db).await {
                    Ok(true) => continue,
                    Err(error) => tracing::warn!(%error, "visual probe queue"),
                    Ok(false) => {}
                }
                tokio::time::sleep(IDLE).await;
            }
        });
    }

    {
        let (db, staging_path) = (db.clone(), staging_path.clone());
        tokio::spawn(async move {
            loop {
                match download::download_tick(&db, &staging_path).await {
                    Ok(true) => continue,
                    Err(error) => tracing::warn!(%error, "visual download queue"),
                    Ok(false) => {}
                }
                tokio::time::sleep(IDLE).await;
            }
        });
    }

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CLEANUP_INTERVAL);
        loop {
            ticker.tick().await;
            if let Err(error) = download::cleanup_download_files(&db, &staging_path).await {
                tracing::warn!(%error, "visual staging cleanup");
            }
        }
    });

    minerva_metrics::shutdown_signal("minerva-fetcher").await;
    tracing::info!("minerva-fetcher: shutting down");
    Ok(())
}
