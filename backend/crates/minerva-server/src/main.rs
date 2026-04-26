mod auth;
mod classification;
mod config;
mod error;
mod ext_obfuscate;
mod feature_flags;
pub mod lti;
mod relink_scheduler;
mod routes;
mod rules;
mod state;
mod strategy;
mod worker;

use axum::response::{IntoResponse, Response};
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

    // One-shot backfill of the document_id payload index across pre-existing
    // course_* collections. New collections get the index at creation time.
    // Runs in the background so a slow/unhealthy Qdrant doesn't block startup.
    {
        let qdrant = state.qdrant.clone();
        tokio::spawn(async move {
            backfill_document_id_indexes(&qdrant).await;
        });
    }

    // Start the background document-processing worker.
    worker::start(state.clone(), config.max_concurrent_ingests);

    // Benchmark FastEmbed models in the background (doesn't block startup).
    // Only the small ONNX models in `STARTUP_BENCHMARK_MODELS` are warmed
    // here -- loading every entry in `VALID_LOCAL_MODELS` (which now
    // includes Qwen3 0.6B, bge-m3, e5-large, etc.) would OOM-kill the
    // pod. Heavier candidates are benchmarked on demand via the admin
    // `POST /admin/embedding-benchmark` endpoint.
    let fastembed = state.fastembed.clone();
    tokio::spawn(async move {
        tracing::info!("running fastembed model benchmarks...");
        match fastembed
            .run_benchmarks(minerva_ingest::pipeline::STARTUP_BENCHMARK_MODELS)
            .await
        {
            Ok(results) => {
                tracing::info!("fastembed benchmarks complete ({} models)", results.len());
            }
            Err(e) => {
                tracing::warn!("fastembed benchmarks failed: {}", e);
            }
        }
    });

    let mut app = Router::new()
        .nest("/api", routes::api_router(state.clone()))
        .nest(
            "/lti",
            routes::lti::public_router().with_state(state.clone()),
        );

    if let Some(ref static_dir) = config.static_dir {
        let index = format!("{}/index.html", static_dir);
        app = app.fallback_service(ServeDir::new(static_dir).fallback(ServeFile::new(index)));
        tracing::info!("serving static files from {}", static_dir);
    } else if let Some(ref proxy_url) = config.dev_proxy {
        let proxy_url_log = proxy_url.clone();
        let proxy_url = proxy_url.clone();
        let client = state.http_client.clone();
        app = app.fallback(move |req: axum::extract::Request| {
            let proxy_url = proxy_url.clone();
            let client = client.clone();
            async move {
                let uri = req.uri().to_string();
                let url = format!("{}{}", proxy_url, uri);
                match client.get(&url).send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        let headers = resp.headers().clone();
                        let body = resp.bytes().await.unwrap_or_default();
                        let mut response = Response::builder().status(status);
                        for (k, v) in headers.iter() {
                            response = response.header(k, v);
                        }
                        response
                            .body(axum::body::Body::from(body))
                            .unwrap()
                            .into_response()
                    }
                    Err(_) => axum::http::StatusCode::BAD_GATEWAY.into_response(),
                }
            }
        });
        tracing::info!("dev proxy fallback to {}", proxy_url_log);
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

/// Walk every `course_*` collection and idempotently add the `document_id`
/// payload index. Existing indexes return an error from Qdrant which we
/// log and ignore -- the goal is to bring older deployments up to speed
/// without requiring a manual migration step.
async fn backfill_document_id_indexes(qdrant: &qdrant_client::Qdrant) {
    let collections = match qdrant.list_collections().await {
        Ok(resp) => resp.collections,
        Err(e) => {
            tracing::warn!(
                "qdrant: index backfill skipped, list_collections failed: {}",
                e
            );
            return;
        }
    };
    let course_collections: Vec<String> = collections
        .into_iter()
        .map(|c| c.name)
        .filter(|n| n.starts_with("course_"))
        .collect();
    tracing::info!(
        "qdrant: backfilling document_id index across {} course collection(s)",
        course_collections.len()
    );
    for name in course_collections {
        minerva_ingest::pipeline::ensure_document_id_index(qdrant, &name).await;
    }
}
