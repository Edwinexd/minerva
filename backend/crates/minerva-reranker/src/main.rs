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

    let port: u16 = std::env::var("MINERVA_RERANKER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50052);
    let host = std::env::var("MINERVA_RERANKER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let addr = format!("{host}:{port}").parse()?;

    let reranker = Arc::new(FastReranker::new());

    let service = service::RerankerService::new(reranker);

    tracing::info!("minerva-reranker listening on {addr}");
    Server::builder()
        .add_service(RerankerServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
