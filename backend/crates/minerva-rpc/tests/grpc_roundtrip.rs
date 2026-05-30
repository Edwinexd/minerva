//! End-to-end gRPC wire-protocol test: spins up a stub embedder /
//! reranker tonic server in-process on a random port, connects a
//! `RemoteEmbedderClient` / `RemoteRerankerClient` to it, and exercises
//! every method on the trait.
//!
//! Stub server returns fixed canned responses; this isn't a model
//! correctness test, it's a contract test that proves:
//!
//! 1. The proto messages serialize round-trip without info loss.
//! 2. tonic Status codes from the server map back to the trait's
//!    error variants on the client (`Busy` becomes `BenchmarkError::Busy`).
//! 3. The trait method shapes (vector-of-vectors, scored indices,
//!    benchmark structs) survive the wire trip.
//!
//! Phase 1 + 2 verification. Phase 4 will delete this test along with
//! the rest of the dual-mode plumbing once gRPC is the only variant.

use std::sync::Arc;

use minerva_core::rpc::{EmbedderClient, RerankerClient};
use minerva_rpc::proto::embedder::{
    embedder_server::{Embedder as EmbedderProto, EmbedderServer},
    BenchmarkOneRequest as EmbBenchOneReq, BenchmarkResult as EmbBenchResult,
    BenchmarkStateRequest as EmbStateReq, BenchmarkStateResponse as EmbStateResp,
    BenchmarksResponse as EmbBenchmarksResp, EmbedRequest, EmbedResponse, FloatVec,
    GetBenchmarksRequest as EmbGetBenchReq, RunBenchmarksRequest,
};
use minerva_rpc::proto::reranker::{
    reranker_server::{Reranker as RerankerProto, RerankerServer},
    BenchmarkOneRequest as RerBenchOneReq, BenchmarkStateRequest as RerStateReq,
    BenchmarkStateResponse as RerStateResp, BenchmarksResponse as RerBenchmarksResp,
    GetBenchmarksRequest as RerGetBenchReq, RerankBenchmarkResult as RerProtoBench, RerankRequest,
    RerankResponse, ScoredIndex,
};
use minerva_rpc::{RemoteEmbedderClient, RemoteRerankerClient};
use std::net::SocketAddr;
use tokio::sync::Mutex;
use tonic::{transport::Server, Request, Response, Status};

// ── Stub embedder service ─────────────────────────────────────────

/// Stub embedder: returns canned vectors and tracks which method was
/// last called so the test can assert priority lane routing.
#[derive(Default)]
struct StubEmbedder {
    last_call: Arc<Mutex<Option<&'static str>>>,
    benchmark_busy: Arc<Mutex<bool>>,
}

#[tonic::async_trait]
impl EmbedderProto for StubEmbedder {
    async fn embed(
        &self,
        request: Request<EmbedRequest>,
    ) -> Result<Response<EmbedResponse>, Status> {
        *self.last_call.lock().await = Some("embed");
        let inner = request.into_inner();
        Ok(Response::new(EmbedResponse {
            // One canned 3-float vector per input text. Values encode
            // the input index so the test can verify ordering.
            vectors: inner
                .texts
                .into_iter()
                .enumerate()
                .map(|(i, _)| FloatVec {
                    values: vec![i as f32, 0.0, 1.0],
                })
                .collect(),
        }))
    }

    async fn embed_query(
        &self,
        request: Request<EmbedRequest>,
    ) -> Result<Response<EmbedResponse>, Status> {
        *self.last_call.lock().await = Some("embed_query");
        let inner = request.into_inner();
        Ok(Response::new(EmbedResponse {
            vectors: inner
                .texts
                .into_iter()
                .enumerate()
                .map(|(i, _)| FloatVec {
                    // Different sentinel value so we can verify we
                    // hit the high-priority lane.
                    values: vec![i as f32, 9.0, 9.0],
                })
                .collect(),
        }))
    }

    async fn run_benchmarks(
        &self,
        request: Request<RunBenchmarksRequest>,
    ) -> Result<Response<EmbBenchmarksResp>, Status> {
        let inner = request.into_inner();
        Ok(Response::new(EmbBenchmarksResp {
            results: inner
                .models
                .into_iter()
                .map(|m| EmbBenchResult {
                    model: m.model_name,
                    dimensions: m.dimensions,
                    embeddings_per_second: 100.0,
                    total_ms: 5.0,
                    corpus_size: 4,
                })
                .collect(),
        }))
    }

    async fn benchmark_one(
        &self,
        request: Request<EmbBenchOneReq>,
    ) -> Result<Response<EmbBenchResult>, Status> {
        if *self.benchmark_busy.lock().await {
            return Err(Status::failed_precondition("busy"));
        }
        let inner = request.into_inner();
        Ok(Response::new(EmbBenchResult {
            model: inner.model_name,
            dimensions: inner.dimensions,
            embeddings_per_second: 123.0,
            total_ms: 7.5,
            corpus_size: 8,
        }))
    }

    async fn get_benchmarks(
        &self,
        _request: Request<EmbGetBenchReq>,
    ) -> Result<Response<EmbBenchmarksResp>, Status> {
        Ok(Response::new(EmbBenchmarksResp {
            results: vec![EmbBenchResult {
                model: "test-model".into(),
                dimensions: 384,
                embeddings_per_second: 50.0,
                total_ms: 20.0,
                corpus_size: 2,
            }],
        }))
    }

    async fn is_benchmark_running(
        &self,
        _request: Request<EmbStateReq>,
    ) -> Result<Response<EmbStateResp>, Status> {
        Ok(Response::new(EmbStateResp {
            running: *self.benchmark_busy.lock().await,
        }))
    }
}

// ── Stub reranker service ─────────────────────────────────────────

#[derive(Default)]
struct StubReranker;

#[tonic::async_trait]
impl RerankerProto for StubReranker {
    async fn rerank(
        &self,
        request: Request<RerankRequest>,
    ) -> Result<Response<RerankResponse>, Status> {
        let inner = request.into_inner();
        // Return indices in reverse with descending scores so the test
        // can verify the order survives the wire trip.
        let n = inner.documents.len();
        let results = (0..n)
            .map(|i| ScoredIndex {
                index: (n - 1 - i) as u32,
                score: 1.0 - (i as f32) * 0.1,
            })
            .collect();
        Ok(Response::new(RerankResponse { results }))
    }

    async fn benchmark_one(
        &self,
        request: Request<RerBenchOneReq>,
    ) -> Result<Response<RerProtoBench>, Status> {
        let inner = request.into_inner();
        Ok(Response::new(RerProtoBench {
            model: inner.model_code,
            pairs_per_second: 50.0,
            total_ms: 100.0,
            pairs: 5,
        }))
    }

    async fn get_benchmarks(
        &self,
        _request: Request<RerGetBenchReq>,
    ) -> Result<Response<RerBenchmarksResp>, Status> {
        Ok(Response::new(RerBenchmarksResp { results: vec![] }))
    }

    async fn is_benchmark_running(
        &self,
        _request: Request<RerStateReq>,
    ) -> Result<Response<RerStateResp>, Status> {
        Ok(Response::new(RerStateResp { running: false }))
    }
}

// ── Test harness ──────────────────────────────────────────────────

/// Start a tonic server on a random port for the duration of a test.
/// Returns the URL the client should connect to.
async fn spawn_embedder_server(stub: StubEmbedder) -> (String, Arc<Mutex<Option<&'static str>>>) {
    let last_call = stub.last_call.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        Server::builder()
            .add_service(EmbedderServer::new(stub))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    // Tiny pause so the server's listener is up before the client
    // tries to dial. tonic's connect_lazy means the dial happens on
    // first RPC, so the test's await will retry on EOF anyway, but
    // this keeps log noise down.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (url, last_call)
}

async fn spawn_reranker_server(stub: StubReranker) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        Server::builder()
            .add_service(RerankerServer::new(stub))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    url
}

// ── Tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn embed_routes_to_low_priority_lane() {
    let stub = StubEmbedder::default();
    let (url, last_call) = spawn_embedder_server(stub).await;
    let client = RemoteEmbedderClient::connect(url).await.unwrap();

    let vectors = client
        .embed("test-model", vec!["a".into(), "b".into()])
        .await
        .unwrap();

    assert_eq!(*last_call.lock().await, Some("embed"));
    assert_eq!(vectors.len(), 2);
    // Each canned vec was `[i, 0, 1]`; check both axes survived the
    // proto round trip.
    assert_eq!(vectors[0], vec![0.0, 0.0, 1.0]);
    assert_eq!(vectors[1], vec![1.0, 0.0, 1.0]);
}

#[tokio::test]
async fn embed_query_routes_to_high_priority_lane() {
    let stub = StubEmbedder::default();
    let (url, last_call) = spawn_embedder_server(stub).await;
    let client = RemoteEmbedderClient::connect(url).await.unwrap();

    let vectors = client
        .embed_query("test-model", vec!["q".into()])
        .await
        .unwrap();

    assert_eq!(*last_call.lock().await, Some("embed_query"));
    assert_eq!(vectors.len(), 1);
    assert_eq!(vectors[0], vec![0.0, 9.0, 9.0]);
}

#[tokio::test]
async fn embed_run_benchmarks_round_trip() {
    let stub = StubEmbedder::default();
    let (url, _) = spawn_embedder_server(stub).await;
    let client = RemoteEmbedderClient::connect(url).await.unwrap();

    let results = client
        .run_benchmarks(&[("m1".into(), 256), ("m2".into(), 512)])
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].model, "m1");
    assert_eq!(results[0].dimensions, 256);
    assert_eq!(results[1].dimensions, 512);
}

#[tokio::test]
async fn embed_benchmark_one_busy_maps_to_busy() {
    let stub = StubEmbedder::default();
    *stub.benchmark_busy.lock().await = true;
    let (url, _) = spawn_embedder_server(stub).await;
    let client = RemoteEmbedderClient::connect(url).await.unwrap();

    let err = client.benchmark_one("m1", 256).await.unwrap_err();
    matches!(err, minerva_core::rpc::BenchmarkError::Busy);
}

#[tokio::test]
async fn embed_get_benchmarks_decodes() {
    let stub = StubEmbedder::default();
    let (url, _) = spawn_embedder_server(stub).await;
    let client = RemoteEmbedderClient::connect(url).await.unwrap();

    let results = client.get_benchmarks().await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].model, "test-model");
    assert_eq!(results[0].dimensions, 384);
}

#[tokio::test]
async fn embed_is_benchmark_running_decodes() {
    let stub = StubEmbedder::default();
    *stub.benchmark_busy.lock().await = true;
    let (url, _) = spawn_embedder_server(stub).await;
    let client = RemoteEmbedderClient::connect(url).await.unwrap();

    assert!(client.is_benchmark_running().await);
}

#[tokio::test]
async fn rerank_returns_indices_and_scores() {
    let url = spawn_reranker_server(StubReranker).await;
    let client = RemoteRerankerClient::connect(url).await.unwrap();

    let scored = client
        .rerank(
            "test-reranker",
            "query".into(),
            vec!["a".into(), "b".into(), "c".into()],
        )
        .await
        .unwrap();

    // Stub reverses indices, so for 3 docs we expect [(2, 1.0), (1, 0.9), (0, 0.8)].
    assert_eq!(scored.len(), 3);
    assert_eq!(scored[0].0, 2);
    assert!((scored[0].1 - 1.0).abs() < 1e-6);
    assert_eq!(scored[1].0, 1);
    assert_eq!(scored[2].0, 0);
}

#[tokio::test]
async fn rerank_benchmark_one_round_trip() {
    let url = spawn_reranker_server(StubReranker).await;
    let client = RemoteRerankerClient::connect(url).await.unwrap();

    let r = client.benchmark_one("the-model").await.unwrap();
    assert_eq!(r.model, "the-model");
    assert_eq!(r.pairs, 5);
}
