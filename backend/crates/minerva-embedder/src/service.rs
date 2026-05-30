//! gRPC service impl: receives `Embedder.*` RPCs and dispatches into
//! the in-process `FastEmbedder`.
//!
//! Errors from the embedder are surfaced as `tonic::Status` with the
//! appropriate code so `RemoteEmbedderClient` can map them back to
//! `Result<_, String>` and `BenchmarkError::{Busy, Failed}` on the
//! caller side. The mapping mirrors what tonic uses by convention:
//!
//! - `InvalidArgument` for "we don't know this model name"
//! - `FailedPrecondition` for "another benchmark is already running"
//! - `Internal` for everything else
use std::sync::Arc;

use minerva_ingest::fastembed_embedder::FastEmbedder;
use minerva_rpc::proto::embedder::{
    embedder_server::Embedder as ProtoService, BenchmarkOneRequest, BenchmarkResult,
    BenchmarkStateRequest, BenchmarkStateResponse, BenchmarksResponse, EmbedRequest, EmbedResponse,
    FloatVec, GetBenchmarksRequest, RunBenchmarksRequest,
};
use tonic::{Request, Response, Status};

pub struct EmbedderService {
    embedder: Arc<FastEmbedder>,
}

impl EmbedderService {
    pub fn new(embedder: Arc<FastEmbedder>) -> Self {
        Self { embedder }
    }

    /// Common shape for the two embed RPCs. Only difference is which
    /// priority lane we land in; both go through the same dispatcher.
    async fn do_embed(
        &self,
        req: EmbedRequest,
        high_priority: bool,
    ) -> Result<Response<EmbedResponse>, Status> {
        if req.texts.is_empty() {
            return Ok(Response::new(EmbedResponse { vectors: vec![] }));
        }
        let result = if high_priority {
            self.embedder.embed_query(&req.model_name, req.texts).await
        } else {
            self.embedder.embed(&req.model_name, req.texts).await
        };
        match result {
            Ok(vectors) => Ok(Response::new(EmbedResponse {
                vectors: vectors
                    .into_iter()
                    .map(|values| FloatVec { values })
                    .collect(),
            })),
            // FastEmbedder's error is a plain String; treat
            // "unknown" / "not implemented" classes as InvalidArgument
            // so misconfigured callers see a 4xx-shaped error.
            Err(e) if e.to_lowercase().contains("unknown") => Err(Status::invalid_argument(e)),
            Err(e) => Err(Status::internal(e)),
        }
    }
}

#[tonic::async_trait]
impl ProtoService for EmbedderService {
    async fn embed(
        &self,
        request: Request<EmbedRequest>,
    ) -> Result<Response<EmbedResponse>, Status> {
        self.do_embed(request.into_inner(), false).await
    }

    async fn embed_query(
        &self,
        request: Request<EmbedRequest>,
    ) -> Result<Response<EmbedResponse>, Status> {
        self.do_embed(request.into_inner(), true).await
    }

    async fn run_benchmarks(
        &self,
        request: Request<RunBenchmarksRequest>,
    ) -> Result<Response<BenchmarksResponse>, Status> {
        let inner = request.into_inner();
        // Borrow `model_name` from each ModelEntry; FastEmbedder's
        // run_benchmarks takes `&[(&str, u64)]`.
        let borrowed: Vec<(&str, u64)> = inner
            .models
            .iter()
            .map(|m| (m.model_name.as_str(), m.dimensions))
            .collect();
        let results = self
            .embedder
            .run_benchmarks(&borrowed)
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(BenchmarksResponse {
            results: results.into_iter().map(to_proto_bench).collect(),
        }))
    }

    async fn benchmark_one(
        &self,
        request: Request<BenchmarkOneRequest>,
    ) -> Result<Response<BenchmarkResult>, Status> {
        let inner = request.into_inner();
        match self
            .embedder
            .benchmark_one(&inner.model_name, inner.dimensions)
            .await
        {
            Ok(r) => Ok(Response::new(to_proto_bench(r))),
            Err(minerva_ingest::fastembed_embedder::BenchmarkError::Busy) => Err(
                Status::failed_precondition("another benchmark is already running"),
            ),
            Err(minerva_ingest::fastembed_embedder::BenchmarkError::Failed(e)) => {
                Err(Status::internal(e))
            }
        }
    }

    async fn get_benchmarks(
        &self,
        _request: Request<GetBenchmarksRequest>,
    ) -> Result<Response<BenchmarksResponse>, Status> {
        let results = self.embedder.get_benchmarks().await;
        Ok(Response::new(BenchmarksResponse {
            results: results.into_iter().map(to_proto_bench).collect(),
        }))
    }

    async fn is_benchmark_running(
        &self,
        _request: Request<BenchmarkStateRequest>,
    ) -> Result<Response<BenchmarkStateResponse>, Status> {
        Ok(Response::new(BenchmarkStateResponse {
            running: self.embedder.is_benchmark_running().await,
        }))
    }
}

fn to_proto_bench(r: minerva_ingest::fastembed_embedder::BenchmarkResult) -> BenchmarkResult {
    BenchmarkResult {
        model: r.model,
        dimensions: r.dimensions,
        embeddings_per_second: r.embeddings_per_second,
        total_ms: r.total_ms,
        corpus_size: r.corpus_size as u64,
    }
}
