//! Standalone gRPC server fronting the FastEmbedder model cache.
//!
//! Phase 1 of the microservices split (see
//! `docs/microservices-split.md`). The api / worker pods call into
//! this binary via `minerva_rpc::RemoteEmbedderClient` when
//! `MINERVA_EMBEDDER_URL` is set; otherwise they keep using the
//! in-process `LocalEmbedderClient` (zero-behaviour-change fallback).
//!
//! Architecture:
//!
//! - One [`tonic::transport::Server`] listening on `MINERVA_EMBEDDER_PORT`
//!   (default 50051), no TLS (internal-only ClusterIP traffic, gated
//!   by k8s NetworkPolicy).
//! - One process-wide [`FastEmbedder`] cache owned by the service
//!   impl; both Embed and EmbedQuery RPCs share it, preserving the
//!   biased high/low priority lanes.
//! - Boot-time benchmark warmup mirrors the api binary's startup
//!   tokio::spawn so the embedder pod is warm-cache by the time
//!   traffic arrives.

use std::sync::Arc;

use minerva_embed_engine::fastembed_embedder::FastEmbedder;
use minerva_rpc::proto::embedder::embedder_server::EmbedderServer;
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

    let port: u16 = std::env::var("MINERVA_EMBEDDER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50051);
    let host = std::env::var("MINERVA_EMBEDDER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let addr = format!("{host}:{port}").parse()?;

    let embedder = Arc::new(FastEmbedder::new());

    // Boot benchmark warmup; mirrors `main.rs`'s tokio::spawn in
    // minerva-server. Runs in the background so the listener can
    // accept connections immediately (a chat embed will block on the
    // model load if it arrives before warmup completes; same as today
    // in the monolith).
    let warmup = embedder.clone();
    tokio::spawn(async move {
        tracing::info!("running fastembed boot benchmark set...");
        match warmup
            .run_benchmarks(minerva_catalog::STARTUP_BENCHMARK_MODELS)
            .await
        {
            Ok(results) => tracing::info!(
                "fastembed boot benchmarks complete ({} models)",
                results.len()
            ),
            Err(e) => tracing::warn!("fastembed boot benchmarks failed: {e}"),
        }
    });

    let service = service::EmbedderService::new(embedder);

    tracing::info!("minerva-embedder listening on {addr}");
    Server::builder()
        .add_service(EmbedderServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
