//! Cross-service client traits for the embedder and reranker model
//! servers.
//!
//! Lives in `minerva-core` (not `minerva-rpc`) so any crate can take a
//! `&dyn EmbedderClient` without pulling in the in-process model
//! cache or the tonic-generated code. Both the local wrapper (in
//! `minerva-rpc`) and the future remote gRPC client implement these
//! traits.
//!
//! See `docs/ARCHITECTURE.md` for the service split design.

use async_trait::async_trait;
use serde::Serialize;

/// Prefix every "come back later" error from a model server carries.
///
/// Model-server errors cross the gRPC boundary as strings, so the marker
/// lives here, in the crate both the server side (`minerva-embed-engine`)
/// and the calling side (`minerva-app-core`'s worker) already depend on,
/// rather than as two literals that can drift apart.
///
/// It encodes the difference between "this document is broken" and "the
/// pod was busy". Ingest used to collapse both into `status = 'failed'`,
/// which is terminal, so one embedder restart burned 5032 perfectly good
/// documents and recovering them took a hand-written UPDATE against prod.
pub const DEFER_PREFIX: &str = "deferred";

/// True if `err` is a model-server deferral rather than a real failure.
///
/// Substring, not prefix: the marker is generated inside the model
/// server and then wrapped on its way out, so by the time the worker
/// sees it the text reads
/// `embed RPC failed: status: Internal, message: "deferred: ..."`.
/// A `starts_with` check silently missed every one of those and let 152
/// documents be failed terminally when they should have been requeued.
pub fn is_deferral(err: &str) -> bool {
    err.contains(DEFER_PREFIX)
}

/// Result of benchmarking one embedding model on the fixed benchmark
/// corpus. Field-for-field mirror of
/// `minerva_embed_engine::fastembed_embedder::BenchmarkResult` so the local
/// wrapper translates without loss; the Phase 1 gRPC proto maps to the
/// same shape.
#[derive(Clone, Debug, Serialize)]
pub struct EmbedBenchmarkResult {
    pub model: String,
    pub dimensions: u64,
    pub embeddings_per_second: f64,
    pub total_ms: f64,
    pub corpus_size: usize,
}

/// Result of benchmarking one reranker model on the fixed
/// (query, passage) corpus.
#[derive(Clone, Debug, Serialize)]
pub struct RerankBenchmarkResult {
    pub model: String,
    pub pairs_per_second: f64,
    pub total_ms: f64,
    pub pairs: usize,
}

/// Error shape shared by both embedder and reranker benchmark RPCs.
///
/// `Busy` is the soft case (another admin-triggered benchmark is in
/// flight); the route layer maps it to `admin.benchmark_busy` so the
/// frontend can render a friendly toast. `Failed` carries an
/// already-rendered message string; the future remote RPC layer
/// surfaces it as a tonic `Internal` status.
#[derive(Debug)]
pub enum BenchmarkError {
    Busy,
    Failed(String),
}

impl std::fmt::Display for BenchmarkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BenchmarkError::Busy => write!(f, "another benchmark is already running"),
            BenchmarkError::Failed(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for BenchmarkError {}

/// Surface used by every code path that needs to embed text.
///
/// Mirrors `FastEmbedder`'s public surface 1:1 so the call-site swap
/// is mechanical. The lane choice (`embed` vs `embed_query`) carries
/// through every transport: Phase 1's gRPC contract encodes the same
/// distinction as separate RPC methods.
#[async_trait]
pub trait EmbedderClient: Send + Sync {
    /// Low-priority lane. Used by the ingest worker for batches of
    /// document chunks. Concurrent `embed_query` callers preempt at
    /// `EMBED_BATCH_SIZE` boundaries inside the dispatcher.
    async fn embed(&self, model_name: &str, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String>;

    /// High-priority lane. Used by chat-side RAG retrieval (query
    /// embed). Each batch jumps ahead of any pending ingest batches at
    /// the dispatcher.
    async fn embed_query(
        &self,
        model_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String>;

    /// Run the boot-time benchmark set sequentially. Each entry is
    /// `(model_name, dimensions)`. Results are also persisted into the
    /// embedder's own in-memory benchmark store.
    async fn run_benchmarks(
        &self,
        models: &[(String, u64)],
    ) -> Result<Vec<EmbedBenchmarkResult>, String>;

    /// Run one admin-triggered benchmark. Returns `BenchmarkError::Busy`
    /// if another admin benchmark is already in flight.
    async fn benchmark_one(
        &self,
        model_name: &str,
        dimensions: u64,
    ) -> Result<EmbedBenchmarkResult, BenchmarkError>;

    /// Snapshot of the in-memory benchmark store (last result per
    /// model). The admin system page renders this.
    async fn get_benchmarks(&self) -> Vec<EmbedBenchmarkResult>;

    /// True if an admin-triggered benchmark is currently in flight.
    /// Drives the "Run benchmark" button's busy state in the admin UI.
    async fn is_benchmark_running(&self) -> bool;
}

/// Surface for cross-encoder rerank calls.
///
/// Reranking is only invoked from the chat path today; we still front
/// it with a client trait so the Phase 2 swap to gRPC is mechanical.
#[async_trait]
pub trait RerankerClient: Send + Sync {
    /// Score every (query, document) pair, return indices + scores
    /// sorted best-first. Errors are surfaced to the caller; the
    /// chat-path caller (`rerank_chunks`) fails open and falls back
    /// to the original embedding order.
    async fn rerank(
        &self,
        model_code: &str,
        query: String,
        documents: Vec<String>,
    ) -> Result<Vec<(usize, f32)>, String>;

    /// Admin-triggered benchmark on the fixed (query, passage) set.
    /// Returns `BenchmarkError::Busy` if a benchmark is already in
    /// flight on this reranker instance.
    async fn benchmark_one(
        &self,
        model_code: &str,
    ) -> Result<RerankBenchmarkResult, BenchmarkError>;

    /// Snapshot of the in-memory benchmark store (latest result per
    /// model). Same shape as the embedder client's `get_benchmarks`.
    async fn get_benchmarks(&self) -> Vec<RerankBenchmarkResult>;

    /// True if an admin-triggered benchmark is currently in flight.
    async fn is_benchmark_running(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferral_is_detected_through_the_grpc_wrapping() {
        // Exactly what the worker sees in prod: the marker is nested
        // inside tonic's status text, not at the start.
        assert!(is_deferral(
            r#"embed RPC failed: status: Internal, message: "deferred: fastembed cache is full serving interactive traffic""#
        ));
        // Bare form, straight from the cache.
        assert!(is_deferral("deferred: fastembed cache is full"));
        // A real failure must not be mistaken for one.
        assert!(!is_deferral("text extraction failed: empty text"));
        assert!(!is_deferral("processing task panicked"));
    }
}
