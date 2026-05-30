//! Remote (gRPC) embedder client.
//!
//! The trait + protocol-neutral result types live in
//! `minerva_core::rpc`; the in-process `LocalEmbedderClient` lives in
//! `minerva-rpc-local`. This module is the tonic gRPC client that the
//! api / worker / scheduler use to reach a separate `minerva-embedder`
//! pod, so depending on it never pulls the heavy model engine.
//!
//! ## Why `Arc<dyn EmbedderClient>` (not generics)
//!
//! AppState is cloned into every route handler and worker task; making
//! it generic over the client type would bubble that generic parameter
//! up through every signature in `minerva-server`. Trait objects are
//! cheap (one vtable indirection per call, dwarfed by ONNX inference)
//! and keep the call-site swap mechanical.

use async_trait::async_trait;
use minerva_core::rpc::{BenchmarkError, EmbedBenchmarkResult, EmbedderClient};
use tokio::sync::Mutex;
use tonic::transport::{Channel, Endpoint};

use crate::proto::embedder::{
    embedder_client::EmbedderClient as ProtoClient, BenchmarkOneRequest,
    BenchmarkResult as ProtoBenchmarkResult, BenchmarkStateRequest, EmbedRequest,
    GetBenchmarksRequest, ModelEntry, RunBenchmarksRequest,
};

fn from_proto_bench(r: ProtoBenchmarkResult) -> EmbedBenchmarkResult {
    EmbedBenchmarkResult {
        model: r.model,
        dimensions: r.dimensions,
        embeddings_per_second: r.embeddings_per_second,
        total_ms: r.total_ms,
        corpus_size: r.corpus_size as usize,
    }
}

/// gRPC variant: talks to a remote minerva-embedder pod over HTTP/2.
///
/// Wired up by `AppState::new` when `MINERVA_EMBEDDER_URL` is set;
/// otherwise the api / worker stay on the local in-process variant.
///
/// ## Connection lifetime
///
/// One `tonic::transport::Channel` per client. Channels are cheap
/// `Arc`-clones internally and multiplex over a single HTTP/2
/// connection, so we wrap a single `ProtoClient<Channel>` behind a
/// mutex for the methods that need `&mut self`. tonic codegen takes
/// `&mut self` on the RPC methods even though the underlying channel
/// is `Send + Sync`; the mutex is short-lived (we drop it before
/// awaiting the response stream).
pub struct RemoteEmbedderClient {
    inner: Mutex<ProtoClient<Channel>>,
}

impl RemoteEmbedderClient {
    /// Connect to the embedder service at `url` (e.g.
    /// `http://minerva-embedder.minerva.svc.cluster.local:50051`).
    /// Returns a client ready for use; the channel itself is lazy and
    /// only opens TCP on the first RPC.
    pub async fn connect(url: String) -> Result<Self, String> {
        let endpoint = Endpoint::from_shared(url.clone())
            .map_err(|e| format!("invalid embedder url {url}: {e}"))?
            // Generous timeouts; embedder can do long benchmark runs
            // (~30s for the heavier models in the startup set). Idle
            // connections get pinged via HTTP/2 keepalives so a
            // dropped TCP doesn't surface as the next request 504-ing.
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
impl EmbedderClient for RemoteEmbedderClient {
    async fn embed(&self, model_name: &str, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        let req = EmbedRequest {
            model_name: model_name.to_string(),
            texts,
        };
        let resp = {
            let mut guard = self.inner.lock().await;
            guard
                .embed(req)
                .await
                .map_err(|e| format!("embed RPC failed: {e}"))?
        };
        Ok(resp
            .into_inner()
            .vectors
            .into_iter()
            .map(|v| v.values)
            .collect())
    }

    async fn embed_query(
        &self,
        model_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        let req = EmbedRequest {
            model_name: model_name.to_string(),
            texts,
        };
        let resp = {
            let mut guard = self.inner.lock().await;
            guard
                .embed_query(req)
                .await
                .map_err(|e| format!("embed_query RPC failed: {e}"))?
        };
        Ok(resp
            .into_inner()
            .vectors
            .into_iter()
            .map(|v| v.values)
            .collect())
    }

    async fn run_benchmarks(
        &self,
        models: &[(String, u64)],
    ) -> Result<Vec<EmbedBenchmarkResult>, String> {
        let req = RunBenchmarksRequest {
            models: models
                .iter()
                .map(|(m, d)| ModelEntry {
                    model_name: m.clone(),
                    dimensions: *d,
                })
                .collect(),
        };
        let resp = {
            let mut guard = self.inner.lock().await;
            guard
                .run_benchmarks(req)
                .await
                .map_err(|e| format!("run_benchmarks RPC failed: {e}"))?
        };
        Ok(resp
            .into_inner()
            .results
            .into_iter()
            .map(from_proto_bench)
            .collect())
    }

    async fn benchmark_one(
        &self,
        model_name: &str,
        dimensions: u64,
    ) -> Result<EmbedBenchmarkResult, BenchmarkError> {
        let req = BenchmarkOneRequest {
            model_name: model_name.to_string(),
            dimensions,
        };
        let resp = {
            let mut guard = self.inner.lock().await;
            guard.benchmark_one(req).await
        };
        match resp {
            Ok(r) => Ok(from_proto_bench(r.into_inner())),
            Err(s) if s.code() == tonic::Code::FailedPrecondition => Err(BenchmarkError::Busy),
            Err(s) => Err(BenchmarkError::Failed(format!(
                "benchmark_one RPC failed: {s}"
            ))),
        }
    }

    async fn get_benchmarks(&self) -> Vec<EmbedBenchmarkResult> {
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
                // Match the local variant's signature (no Result), but
                // a network failure shouldn't poison the admin page;
                // log and return empty so the UI degrades gracefully.
                tracing::warn!("embedder get_benchmarks RPC failed: {e}");
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
                tracing::warn!("embedder is_benchmark_running RPC failed: {e}");
                false
            }
        }
    }
}
