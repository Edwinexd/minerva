//! In-process embedder client.
//!
//! Holds the live `FastEmbedder` and delegates every method to it.
//! Behaviour is byte-identical to calling the FastEmbedder directly.
//! The struct is just a newtype around `Arc<FastEmbedder>` so the
//! cache, dispatcher tasks, and LRU all live exactly where they live
//! today.

use std::sync::Arc;

use async_trait::async_trait;
use minerva_core::rpc::{BenchmarkError, EmbedBenchmarkResult, EmbedderClient};
use minerva_embed_engine::fastembed_embedder::{
    BenchmarkError as InnerBenchmarkError, BenchmarkResult as InnerBenchmarkResult, FastEmbedder,
};

/// In-process impl: holds the live `FastEmbedder` and delegates every
/// method to it.
pub struct LocalEmbedderClient {
    inner: Arc<FastEmbedder>,
}

impl LocalEmbedderClient {
    pub fn new(inner: Arc<FastEmbedder>) -> Self {
        Self { inner }
    }

    /// Borrow the underlying FastEmbedder. Useful for code paths that
    /// still need access to the cache directly (e.g. boot-time
    /// `run_benchmarks` from the model-server's startup hook).
    pub fn inner(&self) -> &Arc<FastEmbedder> {
        &self.inner
    }
}

#[async_trait]
impl EmbedderClient for LocalEmbedderClient {
    async fn embed(&self, model_name: &str, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        self.inner.embed(model_name, texts).await
    }

    async fn embed_query(
        &self,
        model_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        self.inner.embed_query(model_name, texts).await
    }

    async fn run_benchmarks(
        &self,
        models: &[(String, u64)],
    ) -> Result<Vec<EmbedBenchmarkResult>, String> {
        // FastEmbedder::run_benchmarks expects `&[(&str, u64)]`; build
        // a borrowed slice in-place. Lifetime stays tied to `models`
        // for the duration of the call.
        let borrowed: Vec<(&str, u64)> = models.iter().map(|(m, d)| (m.as_str(), *d)).collect();
        let inner = self.inner.run_benchmarks(&borrowed).await?;
        Ok(inner.into_iter().map(from_inner_bench).collect())
    }

    async fn benchmark_one(
        &self,
        model_name: &str,
        dimensions: u64,
    ) -> Result<EmbedBenchmarkResult, BenchmarkError> {
        match self.inner.benchmark_one(model_name, dimensions).await {
            Ok(r) => Ok(from_inner_bench(r)),
            Err(InnerBenchmarkError::Busy) => Err(BenchmarkError::Busy),
            Err(InnerBenchmarkError::Failed(s)) => Err(BenchmarkError::Failed(s)),
        }
    }

    async fn get_benchmarks(&self) -> Vec<EmbedBenchmarkResult> {
        self.inner
            .get_benchmarks()
            .await
            .into_iter()
            .map(from_inner_bench)
            .collect()
    }

    async fn is_benchmark_running(&self) -> bool {
        self.inner.is_benchmark_running().await
    }
}

/// Map the engine's internal struct to our protocol-neutral one.
/// Field-for-field copy; no information loss.
fn from_inner_bench(r: InnerBenchmarkResult) -> EmbedBenchmarkResult {
    EmbedBenchmarkResult {
        model: r.model,
        dimensions: r.dimensions,
        embeddings_per_second: r.embeddings_per_second,
        total_ms: r.total_ms,
        corpus_size: r.corpus_size,
    }
}
