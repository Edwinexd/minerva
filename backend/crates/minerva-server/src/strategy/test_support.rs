//! Engine-free fake clients for strategy unit tests.
//!
//! The tests that use these exercise guard / short-circuit / SSE-parsing
//! paths that never actually call the model, so the test build does not
//! need to link `minerva-embed-engine`. Using these fakes (instead of a
//! real `LocalEmbedderClient` over a `FastEmbedder`) is what keeps
//! `cargo test` on the api crate engine-free.

use async_trait::async_trait;
use minerva_core::rpc::{
    BenchmarkError, EmbedBenchmarkResult, EmbedderClient, RerankBenchmarkResult, RerankerClient,
};

/// Embedder client that errors on every call. For strategy tests that
/// need an `Arc<dyn EmbedderClient>` in context but never reach an
/// embedding call.
pub(crate) struct NoopEmbedderClient;

#[async_trait]
impl EmbedderClient for NoopEmbedderClient {
    async fn embed(&self, _model: &str, _texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        Err("noop embedder (test fake)".to_string())
    }
    async fn embed_query(
        &self,
        _model: &str,
        _texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        Err("noop embedder (test fake)".to_string())
    }
    async fn run_benchmarks(
        &self,
        _models: &[(String, u64)],
    ) -> Result<Vec<EmbedBenchmarkResult>, String> {
        Ok(Vec::new())
    }
    async fn benchmark_one(
        &self,
        _model: &str,
        _dimensions: u64,
    ) -> Result<EmbedBenchmarkResult, BenchmarkError> {
        Err(BenchmarkError::Failed(
            "noop embedder (test fake)".to_string(),
        ))
    }
    async fn get_benchmarks(&self) -> Vec<EmbedBenchmarkResult> {
        Vec::new()
    }
    async fn is_benchmark_running(&self) -> bool {
        false
    }
}

/// Reranker client that errors on every call. For strategy tests whose
/// rerank paths short-circuit before touching the model.
pub(crate) struct NoopRerankerClient;

#[async_trait]
impl RerankerClient for NoopRerankerClient {
    async fn rerank(
        &self,
        _model_code: &str,
        _query: String,
        _documents: Vec<String>,
    ) -> Result<Vec<(usize, f32)>, String> {
        Err("noop reranker (test fake)".to_string())
    }
    async fn benchmark_one(
        &self,
        _model_code: &str,
    ) -> Result<RerankBenchmarkResult, BenchmarkError> {
        Err(BenchmarkError::Failed(
            "noop reranker (test fake)".to_string(),
        ))
    }
    async fn get_benchmarks(&self) -> Vec<RerankBenchmarkResult> {
        Vec::new()
    }
    async fn is_benchmark_running(&self) -> bool {
        false
    }
}
