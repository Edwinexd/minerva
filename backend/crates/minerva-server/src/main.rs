mod auth;
mod config;
mod error;
mod routes;
mod state;
mod strategy;
mod worker;

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
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

    let config = config::Config::from_env()?;
    let state = state::AppState::new(&config).await?;

    // Start the background document-processing worker.
    worker::start(state.clone(), config.max_concurrent_ingests);

    let mut app = Router::new().nest("/api", routes::api_router(state.clone()));

    if let Some(ref static_dir) = config.static_dir {
        let index = format!("{}/index.html", static_dir);
        app = app.fallback_service(ServeDir::new(static_dir).fallback(ServeFile::new(index)));
        tracing::info!("serving static files from {}", static_dir);
    }

    let app = app
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("minerva listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
