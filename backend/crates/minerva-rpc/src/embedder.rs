//! Embedder client trait + in-process impl.
//!
//! The trait is the surface every caller in api / worker code talks to;
//! the impl is what's wired into AppState. Phase 0 has one impl
//! ([`LocalEmbedderClient`]) that delegates to the existing
//! `FastEmbedder` in-process. Phase 1 adds a `RemoteEmbedderClient`
//! that speaks tonic gRPC to a separate pod.
//!
//! ## Why `Arc<dyn EmbedderClient>` (not generics)
//!
//! AppState is cloned into every route handler and worker task; making
//! it generic over the client type would bubble that generic parameter
//! up through every signature in `minerva-server`. Trait objects are
//! cheap (one vtable indirection per call, dwarfed by ONNX inference)
//! and keep the call-site swap mechanical.

use std::sync::Arc;

use async_trait::async_trait;
use minerva_core::rpc::{BenchmarkError, EmbedBenchmarkResult, EmbedderClient};
use minerva_ingest::fastembed_embedder::{BenchmarkResult as InnerBenchmarkResult, FastEmbedder};
use tokio::sync::Mutex;
use tonic::transport::{Channel, Endpoint};

use crate::proto::embedder::{
    embedder_client::EmbedderClient as ProtoClient, BenchmarkOneRequest,
    BenchmarkResult as ProtoBenchmarkResult, BenchmarkStateRequest, EmbedRequest,
    GetBenchmarksRequest, ModelEntry, RunBenchmarksRequest,
};

/// In-process impl: holds the live `FastEmbedder` and delegates every
/// method to it. This is the Phase 0 wiring; we are not yet a separate
/// service. Behaviour is byte-identical to calling the FastEmbedder
/// directly.
///
/// The struct is just a newtype around `Arc<FastEmbedder>` so the
/// cache, dispatcher tasks, and LRU all live exactly where they live
/// today.
pub struct LocalEmbedderClient {
    inner: Arc<FastEmbedder>,
}

impl LocalEmbedderClient {
    pub fn new(inner: Arc<FastEmbedder>) -> Self {
        Self { inner }
    }

    /// Borrow the underlying FastEmbedder. Useful for code paths that
    /// still need access to the cache directly during the cutover
    /// (e.g. boot-time `run_benchmarks` from startup hooks); deleted
    /// in Phase 4 when those move behind the trait too.
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
            Err(minerva_ingest::fastembed_embedder::BenchmarkError::Busy) => {
                Err(BenchmarkError::Busy)
            }
            Err(minerva_ingest::fastembed_embedder::BenchmarkError::Failed(s)) => {
                Err(BenchmarkError::Failed(s))
            }
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

/// Map the minerva-ingest internal struct to our protocol-neutral one.
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
