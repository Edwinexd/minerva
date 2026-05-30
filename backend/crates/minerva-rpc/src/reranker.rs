//! Reranker client trait + in-process impl.
//!
//! Same shape as [`crate::embedder`]: a protocol-neutral trait plus a
//! local wrapper that delegates to the existing `FastReranker`. Phase
//! 2 adds the remote gRPC variant.

use std::sync::Arc;

use async_trait::async_trait;
use minerva_core::rpc::{BenchmarkError, RerankBenchmarkResult, RerankerClient};
use minerva_ingest::reranker::{
    BenchmarkError as InnerBenchmarkError, FastReranker,
    RerankBenchmarkResult as InnerRerankBenchmarkResult,
};
use tokio::sync::Mutex;
use tonic::transport::{Channel, Endpoint};

use crate::proto::reranker::{
    reranker_client::RerankerClient as ProtoClient, BenchmarkOneRequest, BenchmarkStateRequest,
    GetBenchmarksRequest, RerankBenchmarkResult as ProtoBenchmarkResult, RerankRequest,
};

/// In-process impl. Delegates to the existing `FastReranker` cache
/// behind an `Arc`. Phase 0 wiring; deleted in Phase 4 once the gRPC
/// client is the only variant.
pub struct LocalRerankerClient {
    inner: Arc<FastReranker>,
}

impl LocalRerankerClient {
    pub fn new(inner: Arc<FastReranker>) -> Self {
        Self { inner }
    }

    /// Borrow the underlying FastReranker for code paths that haven't
    /// yet been routed through the trait. Deleted in Phase 4.
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

fn from_proto_bench(r: ProtoBenchmarkResult) -> RerankBenchmarkResult {
    RerankBenchmarkResult {
        model: r.model,
        pairs_per_second: r.pairs_per_second,
        total_ms: r.total_ms,
        pairs: r.pairs as usize,
    }
}

/// gRPC variant: talks to a remote minerva-reranker pod. Same lifetime
///   and shape as [`crate::embedder::RemoteEmbedderClient`]; see that
///   doc comment for the channel and mutex reasoning.
pub struct RemoteRerankerClient {
    inner: Mutex<ProtoClient<Channel>>,
}

impl RemoteRerankerClient {
    pub async fn connect(url: String) -> Result<Self, String> {
        let endpoint = Endpoint::from_shared(url.clone())
            .map_err(|e| format!("invalid reranker url {url}: {e}"))?
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .http2_keep_alive_interval(std::time::Duration::from_secs(30))
            .keep_alive_timeout(std::time::Duration::from_secs(20))
            .keep_alive_while_idle(true);
        let channel = endpoint.connect_lazy();
        let client = ProtoClient::new(channel);
        Ok(Self {
            inner: Mutex::new(client),
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
        let resp = {
            let mut guard = self.inner.lock().await;
            guard
                .rerank(req)
                .await
                .map_err(|e| format!("rerank RPC failed: {e}"))?
        };
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
        let resp = {
            let mut guard = self.inner.lock().await;
            guard.benchmark_one(req).await
        };
        match resp {
            Ok(r) => Ok(from_proto_bench(r.into_inner())),
            Err(s) if s.code() == tonic::Code::FailedPrecondition => Err(BenchmarkError::Busy),
            Err(s) => Err(BenchmarkError::Failed(format!(
                "reranker benchmark_one RPC failed: {s}"
            ))),
        }
    }

    async fn get_benchmarks(&self) -> Vec<RerankBenchmarkResult> {
        let resp = {
            let mut guard = self.inner.lock().await;
            guard.get_benchmarks(GetBenchmarksRequest {}).await
        };
        match resp {
            Ok(r) => r
                .into_inner()
                .results
                .into_iter()
                .map(from_proto_bench)
                .collect(),
            Err(e) => {
                tracing::warn!("reranker get_benchmarks RPC failed: {e}");
                Vec::new()
            }
        }
    }

    async fn is_benchmark_running(&self) -> bool {
        let resp = {
            let mut guard = self.inner.lock().await;
            guard.is_benchmark_running(BenchmarkStateRequest {}).await
        };
        match resp {
            Ok(r) => r.into_inner().running,
            Err(e) => {
                tracing::warn!("reranker is_benchmark_running RPC failed: {e}");
                false
            }
        }
    }
}
