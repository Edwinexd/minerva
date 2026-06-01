//! Standalone gRPC server fronting the FastReranker cache.
//!
//! Phase 2 of the microservices split. Same architecture as
//! `minerva-embedder` (see that binary's main.rs for the lifecycle
//! notes); wired up when api / worker have `MINERVA_RERANKER_URL` set.

use std::sync::Arc;

use minerva_embed_engine::reranker::FastReranker;
use minerva_rpc::proto::reranker::reranker_server::RerankerServer;
use tonic::transport::Server;

mod service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .json()
        .init();

    // Prometheus exporter + the shared memprobe. The reranker pod owns the
    // FastReranker cross-encoder cache (the 3Gi pod from the OOM-fix
    // commit), so its RSS gauges matter as much as the embedder's.
    minerva_metrics::init("minerva-reranker");
    minerva_metrics::spawn_memprobe("minerva-reranker");

    let port: u16 = std::env::var("MINERVA_RERANKER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50052);
    let host = std::env::var("MINERVA_RERANKER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let addr = format!("{host}:{port}").parse()?;

    let reranker = Arc::new(FastReranker::new());

    // Boot warmup: load + benchmark the default cross-encoder so the
    // reranker pod is warm by the time the first chat reranks, instead of
    // making that student pay the cold download + ONNX session build (and
    // leaving every reranker_* metric empty until then). Mirrors the
    // embedder's boot benchmark. Background-spawned so the gRPC listener
    // accepts connections immediately; a rerank that arrives before warmup
    // finishes just blocks on the same load.
    let warmup = reranker.clone();
    tokio::spawn(async move {
        let model = minerva_embed_engine::reranker::DEFAULT_RERANK_MODEL;
        tracing::info!("running reranker boot warmup for {model}...");
        match warmup.benchmark_one(model).await {
            Ok(r) => tracing::info!(
                "reranker boot warmup complete: {} at {:.1} pairs/sec",
                r.model,
                r.pairs_per_second
            ),
            Err(e) => tracing::warn!("reranker boot warmup failed: {e}"),
        }
    });

    let service = service::RerankerService::new(reranker);

    tracing::info!("minerva-reranker listening on {addr}");
    Server::builder()
        .add_service(RerankerServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
