//! Remote (gRPC) reranker client.
//!
//! Same shape as [`crate::embedder`]: a tonic gRPC client over the
//! protocol-neutral `RerankerClient` trait. The in-process
//! `LocalRerankerClient` lives in `minerva-rpc-local`.

use async_trait::async_trait;
use minerva_core::rpc::{BenchmarkError, RerankBenchmarkResult, RerankerClient};
use tonic::transport::{Channel, Endpoint};

use crate::proto::reranker::{
    reranker_client::RerankerClient as ProtoClient, BenchmarkOneRequest, BenchmarkStateRequest,
    GetBenchmarksRequest, RerankBenchmarkResult as ProtoBenchmarkResult, RerankRequest,
};

/// Per-RPC client-side deadlines; see [`crate::embedder`] for the full
/// rationale (one blanket channel timeout can't fit both the hot rerank
/// path and a multi-minute cold benchmark load, so each RPC class gets
/// its own, enforced via `tokio::time::timeout`).
mod timeouts {
    use std::time::Duration;
    /// Interactive rerank of a chat turn's candidate pool.
    pub const HOT: Duration = Duration::from_secs(120);
    /// Cold cross-encoder load (download + ONNX session build + warmup).
    pub const BENCHMARK: Duration = Duration::from_secs(900);
    /// Cheap metadata reads.
    pub const META: Duration = Duration::from_secs(30);
}

fn from_proto_bench(r: ProtoBenchmarkResult) -> RerankBenchmarkResult {
    RerankBenchmarkResult {
        model: r.model,
        pairs_per_second: r.pairs_per_second,
        total_ms: r.total_ms,
        pairs: r.pairs as usize,
    }
}

/// gRPC variant: talks to a remote minerva-reranker pod. Same lifetime
/// and concurrency shape as [`crate::embedder::RemoteEmbedderClient`]:
/// the tonic client is `Clone` and multiplexes over one HTTP/2
/// connection, so each call clones it and there's no shared mutex for a
/// long benchmark to block the hot rerank path on.
pub struct RemoteRerankerClient {
    inner: ProtoClient<Channel>,
}

impl RemoteRerankerClient {
    pub async fn connect(url: String) -> Result<Self, String> {
        let endpoint = Endpoint::from_shared(url.clone())
            .map_err(|e| format!("invalid reranker url {url}: {e}"))?
            // No blanket per-RPC `.timeout()`; per-RPC deadlines are
            // applied at the call sites. Connection-level knobs stay.
            .connect_timeout(std::time::Duration::from_secs(10))
            .http2_keep_alive_interval(std::time::Duration::from_secs(30))
            .keep_alive_timeout(std::time::Duration::from_secs(20))
            .keep_alive_while_idle(true);
        let channel = endpoint.connect_lazy();
        Ok(Self {
            inner: ProtoClient::new(channel),
        })
    }
}

#[async_trait]
impl RerankerClient for RemoteRerankerClient {
    async fn rerank(
        &self,
        model_code: &str,
        query: String,
        documents: Vec<String>,
    ) -> Result<Vec<(usize, f32)>, String> {
        let req = RerankRequest {
            model_code: model_code.to_string(),
            query,
            documents,
        };
        let mut client = self.inner.clone();
        let resp = tokio::time::timeout(timeouts::HOT, client.rerank(req))
            .await
            .map_err(|_| format!("rerank RPC timed out after {}s", timeouts::HOT.as_secs()))?
            .map_err(|e| format!("rerank RPC failed: {e}"))?;
        Ok(resp
            .into_inner()
            .results
            .into_iter()
            .map(|s| (s.index as usize, s.score))
            .collect())
    }

    async fn benchmark_one(
        &self,
        model_code: &str,
    ) -> Result<RerankBenchmarkResult, BenchmarkError> {
        let req = BenchmarkOneRequest {
            model_code: model_code.to_string(),
        };
        let mut client = self.inner.clone();
        match tokio::time::timeout(timeouts::BENCHMARK, client.benchmark_one(req)).await {
            Err(_elapsed) => Err(BenchmarkError::Failed(format!(
                "reranker benchmark_one RPC timed out after {}s (model load + warmup exceeded the deadline)",
                timeouts::BENCHMARK.as_secs()
            ))),
            Ok(Ok(r)) => Ok(from_proto_bench(r.into_inner())),
            Ok(Err(s)) if s.code() == tonic::Code::FailedPrecondition => Err(BenchmarkError::Busy),
            Ok(Err(s)) => Err(BenchmarkError::Failed(format!(
                "reranker benchmark_one RPC failed: {s}"
            ))),
        }
    }

    async fn get_benchmarks(&self) -> Vec<RerankBenchmarkResult> {
        let mut client = self.inner.clone();
        match tokio::time::timeout(
            timeouts::META,
            client.get_benchmarks(GetBenchmarksRequest {}),
        )
        .await
        {
            Ok(Ok(r)) => r
                .into_inner()
                .results
                .into_iter()
                .map(from_proto_bench)
                .collect(),
            Ok(Err(e)) => {
                tracing::warn!("reranker get_benchmarks RPC failed: {e}");
                Vec::new()
            }
            Err(_elapsed) => {
                tracing::warn!(
                    "reranker get_benchmarks RPC timed out after {}s",
                    timeouts::META.as_secs()
                );
                Vec::new()
            }
        }
    }

    async fn is_benchmark_running(&self) -> bool {
        let mut client = self.inner.clone();
        match tokio::time::timeout(
            timeouts::META,
            client.is_benchmark_running(BenchmarkStateRequest {}),
        )
        .await
        {
            Ok(Ok(r)) => r.into_inner().running,
            Ok(Err(e)) => {
                tracing::warn!("reranker is_benchmark_running RPC failed: {e}");
                false
            }
            Err(_elapsed) => {
                tracing::warn!(
                    "reranker is_benchmark_running RPC timed out after {}s",
                    timeouts::META.as_secs()
                );
                false
            }
        }
    }
}
