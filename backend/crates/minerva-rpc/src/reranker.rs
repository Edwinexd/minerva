//! Remote (gRPC) reranker client.
//!
//! Same shape as [`crate::embedder`]: a tonic gRPC client over the
//! protocol-neutral `RerankerClient` trait. The in-process
//! `LocalRerankerClient` lives in `minerva-rpc-local`.

use async_trait::async_trait;
use minerva_core::rpc::{BenchmarkError, RerankBenchmarkResult, RerankerClient};
use tokio::sync::Mutex;
use tonic::transport::{Channel, Endpoint};

use crate::proto::reranker::{
    reranker_client::RerankerClient as ProtoClient, BenchmarkOneRequest, BenchmarkStateRequest,
    GetBenchmarksRequest, RerankBenchmarkResult as ProtoBenchmarkResult, RerankRequest,
};

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
