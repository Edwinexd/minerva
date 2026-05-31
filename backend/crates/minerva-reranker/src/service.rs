//! gRPC service impl for the reranker. Same status-code mapping as the
//! embedder service: `FailedPrecondition` for Busy on the benchmark
//! path, `Internal` for anything else.

use std::sync::Arc;

use minerva_embed_engine::reranker::{BenchmarkError as InnerBenchmarkError, FastReranker};
use minerva_rpc::proto::reranker::{
    reranker_server::Reranker as ProtoService, BenchmarkOneRequest, BenchmarkStateRequest,
    BenchmarkStateResponse, BenchmarksResponse, GetBenchmarksRequest,
    RerankBenchmarkResult as ProtoBenchmarkResult, RerankRequest, RerankResponse, ScoredIndex,
};
use tonic::{Request, Response, Status};

pub struct RerankerService {
    reranker: Arc<FastReranker>,
}

impl RerankerService {
    pub fn new(reranker: Arc<FastReranker>) -> Self {
        Self { reranker }
    }
}

#[tonic::async_trait]
impl ProtoService for RerankerService {
    async fn rerank(
        &self,
        request: Request<RerankRequest>,
    ) -> Result<Response<RerankResponse>, Status> {
        let inner = request.into_inner();
        if inner.documents.is_empty() {
            return Ok(Response::new(RerankResponse { results: vec![] }));
        }
        match self
            .reranker
            .rerank(&inner.model_code, inner.query, inner.documents)
            .await
        {
            Ok(scored) => Ok(Response::new(RerankResponse {
                results: scored
                    .into_iter()
                    .map(|(i, s)| ScoredIndex {
                        index: i as u32,
                        score: s,
                    })
                    .collect(),
            })),
            Err(e) => Err(Status::internal(e)),
        }
    }

    async fn benchmark_one(
        &self,
        request: Request<BenchmarkOneRequest>,
    ) -> Result<Response<ProtoBenchmarkResult>, Status> {
        let inner = request.into_inner();
        match self.reranker.benchmark_one(&inner.model_code).await {
            Ok(r) => Ok(Response::new(ProtoBenchmarkResult {
                model: r.model,
                pairs_per_second: r.pairs_per_second,
                total_ms: r.total_ms,
                pairs: r.pairs as u64,
            })),
            Err(InnerBenchmarkError::Busy) => Err(Status::failed_precondition(
                "another benchmark is already running",
            )),
            Err(InnerBenchmarkError::Failed(e)) => Err(Status::internal(e)),
        }
    }

    async fn get_benchmarks(
        &self,
        _request: Request<GetBenchmarksRequest>,
    ) -> Result<Response<BenchmarksResponse>, Status> {
        let results = self.reranker.get_benchmarks().await;
        Ok(Response::new(BenchmarksResponse {
            results: results
                .into_iter()
                .map(|r| ProtoBenchmarkResult {
                    model: r.model,
                    pairs_per_second: r.pairs_per_second,
                    total_ms: r.total_ms,
                    pairs: r.pairs as u64,
                })
                .collect(),
        }))
    }

    async fn is_benchmark_running(
        &self,
        _request: Request<BenchmarkStateRequest>,
    ) -> Result<Response<BenchmarkStateResponse>, Status> {
        Ok(Response::new(BenchmarkStateResponse {
            running: self.reranker.is_benchmark_running().await,
        }))
    }
}
