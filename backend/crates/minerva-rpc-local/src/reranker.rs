//! In-process reranker client.
//!
//! Same shape as [`crate::embedder`]: delegates to the existing
//! `FastReranker` cache behind an `Arc`.

use std::sync::Arc;

use async_trait::async_trait;
use minerva_core::rpc::{BenchmarkError, RerankBenchmarkResult, RerankerClient};
use minerva_embed_engine::reranker::{
    BenchmarkError as InnerBenchmarkError, FastReranker,
    RerankBenchmarkResult as InnerRerankBenchmarkResult,
};

/// In-process impl. Delegates to the existing `FastReranker` cache
/// behind an `Arc`.
pub struct LocalRerankerClient {
    inner: Arc<FastReranker>,
}

impl LocalRerankerClient {
    pub fn new(inner: Arc<FastReranker>) -> Self {
        Self { inner }
    }

    /// Borrow the underlying FastReranker for code paths that need the
    /// cache directly (e.g. the model-server's startup hook).
    pub fn inner(&self) -> &Arc<FastReranker> {
        &self.inner
    }
}

#[async_trait]
impl RerankerClient for LocalRerankerClient {
    async fn rerank(
        &self,
        model_code: &str,
        query: String,
        documents: Vec<String>,
    ) -> Result<Vec<(usize, f32)>, String> {
        // `FastReranker::rerank` already returns `Result<_, String>`
        // (it doesn't expose the BenchmarkError busy/failed distinction
        // on the non-benchmark path); pass through unchanged.
        self.inner.rerank(model_code, query, documents).await
    }

    async fn benchmark_one(
        &self,
        model_code: &str,
    ) -> Result<RerankBenchmarkResult, BenchmarkError> {
        match self.inner.benchmark_one(model_code).await {
            Ok(r) => Ok(from_inner_bench(r)),
            Err(InnerBenchmarkError::Busy) => Err(BenchmarkError::Busy),
            Err(InnerBenchmarkError::Failed(s)) => Err(BenchmarkError::Failed(s)),
        }
    }

    async fn get_benchmarks(&self) -> Vec<RerankBenchmarkResult> {
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

fn from_inner_bench(r: InnerRerankBenchmarkResult) -> RerankBenchmarkResult {
    RerankBenchmarkResult {
        model: r.model,
        pairs_per_second: r.pairs_per_second,
        total_ms: r.total_ms,
        pairs: r.pairs,
    }
}
