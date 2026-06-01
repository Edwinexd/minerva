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
use tonic::transport::{Channel, Endpoint};

use crate::proto::embedder::{
    embedder_client::EmbedderClient as ProtoClient, BenchmarkOneRequest,
    BenchmarkResult as ProtoBenchmarkResult, BenchmarkStateRequest, EmbedRequest,
    GetBenchmarksRequest, ModelEntry, RunBenchmarksRequest,
};

/// Per-RPC client-side deadlines. These replace the single blanket
/// `Endpoint::timeout`, which applied the *same* deadline to every RPC
/// on the channel: a benchmark that cold-loads a multi-GB model
/// (download + ONNX session build + warmup, minutes) was being cancelled
/// by the 120s hot-path deadline and surfacing to the operator as a bare
/// 500, while the embedder kept working server-side. We enforce these
/// client-side with `tokio::time::timeout` (a dropped tonic call future
/// cancels the request), so each RPC class gets a deadline that fits it.
mod timeouts {
    use std::time::Duration;
    /// Interactive / ingest embeds. Long enough for a big batch, short
    /// enough that a genuinely stuck call fails fast.
    pub const HOT: Duration = Duration::from_secs(120);
    /// Benchmarks load (and on first use download) a model, then warm it
    /// before timing. A cold multi-GB model can take several minutes;
    /// this is generous because a benchmark is a rare, deliberate,
    /// operator-initiated action.
    pub const BENCHMARK: Duration = Duration::from_secs(900);
    /// Cheap metadata reads (in-memory state on the server).
    pub const META: Duration = Duration::from_secs(30);
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
/// ## Connection lifetime + concurrency
///
/// One `tonic::transport::Channel`, held inside a `ProtoClient<Channel>`.
/// Channels are cheap `Arc`-clones internally and multiplex over a single
/// HTTP/2 connection, and the tonic-generated client is `Clone`, so each
/// call clones the client and issues its RPC on that clone. There is no
/// shared mutex: a long-running benchmark RPC therefore can't block the
/// hot `embed` path behind a lock (the previous design held a `Mutex`
/// across the whole RPC await, so a multi-minute benchmark serialized
/// every embed on the pod).
pub struct RemoteEmbedderClient {
    inner: ProtoClient<Channel>,
}

impl RemoteEmbedderClient {
    /// Connect to the embedder service at `url` (e.g.
    /// `http://minerva-embedder.minerva.svc.cluster.local:50051`).
    /// Returns a client ready for use; the channel itself is lazy and
    /// only opens TCP on the first RPC.
    pub async fn connect(url: String) -> Result<Self, String> {
        let endpoint = Endpoint::from_shared(url.clone())
            .map_err(|e| format!("invalid embedder url {url}: {e}"))?
            // No blanket per-RPC `.timeout()` here: it would cap the long
            // benchmark RPC at the hot-path deadline. Per-RPC deadlines
            // are applied at the call sites via `tokio::time::timeout`.
            // Connection-level knobs stay: bound the initial connect, and
            // keepalive-ping idle connections so a dropped TCP surfaces
            // promptly instead of as the next request hanging.
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
impl EmbedderClient for RemoteEmbedderClient {
    async fn embed(&self, model_name: &str, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        let req = EmbedRequest {
            model_name: model_name.to_string(),
            texts,
        };
        let mut client = self.inner.clone();
        let resp = tokio::time::timeout(timeouts::HOT, client.embed(req))
            .await
            .map_err(|_| format!("embed RPC timed out after {}s", timeouts::HOT.as_secs()))?
            .map_err(|e| format!("embed RPC failed: {e}"))?;
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
        let mut client = self.inner.clone();
        let resp = tokio::time::timeout(timeouts::HOT, client.embed_query(req))
            .await
            .map_err(|_| {
                format!(
                    "embed_query RPC timed out after {}s",
                    timeouts::HOT.as_secs()
                )
            })?
            .map_err(|e| format!("embed_query RPC failed: {e}"))?;
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
        let mut client = self.inner.clone();
        let resp = tokio::time::timeout(timeouts::BENCHMARK, client.run_benchmarks(req))
            .await
            .map_err(|_| {
                format!(
                    "run_benchmarks RPC timed out after {}s",
                    timeouts::BENCHMARK.as_secs()
                )
            })?
            .map_err(|e| format!("run_benchmarks RPC failed: {e}"))?;
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
        let mut client = self.inner.clone();
        match tokio::time::timeout(timeouts::BENCHMARK, client.benchmark_one(req)).await {
            Err(_elapsed) => Err(BenchmarkError::Failed(format!(
                "benchmark_one RPC timed out after {}s (model load + warmup exceeded the deadline)",
                timeouts::BENCHMARK.as_secs()
            ))),
            Ok(Ok(r)) => Ok(from_proto_bench(r.into_inner())),
            Ok(Err(s)) if s.code() == tonic::Code::FailedPrecondition => Err(BenchmarkError::Busy),
            Ok(Err(s)) => Err(BenchmarkError::Failed(format!(
                "benchmark_one RPC failed: {s}"
            ))),
        }
    }

    async fn get_benchmarks(&self) -> Vec<EmbedBenchmarkResult> {
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
                // Match the local variant's signature (no Result), but
                // a network failure shouldn't poison the admin page;
                // log and return empty so the UI degrades gracefully.
                tracing::warn!("embedder get_benchmarks RPC failed: {e}");
                Vec::new()
            }
            Err(_elapsed) => {
                tracing::warn!(
                    "embedder get_benchmarks RPC timed out after {}s",
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
                tracing::warn!("embedder is_benchmark_running RPC failed: {e}");
                false
            }
            Err(_elapsed) => {
                tracing::warn!(
                    "embedder is_benchmark_running RPC timed out after {}s",
                    timeouts::META.as_secs()
                );
                false
            }
        }
    }
}
