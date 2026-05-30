//! In-process and remote client impls for the embedder and reranker
//! model servers, plus tonic-generated gRPC bindings.
//!
//! The traits + protocol-neutral result types live in
//! `minerva_core::rpc`; this crate provides the actual transports.
//!
//! - [`LocalEmbedderClient`] / [`LocalRerankerClient`]: in-process
//!   wrappers (Phase 0 default).
//! - [`embedder::RemoteEmbedderClient`]: tonic gRPC client (Phase 1).
//!   Wired up when `MINERVA_EMBEDDER_URL` is set on the api/worker.
//! - [`proto::embedder`]: generated client + server types from
//!   `proto/embedder.proto`. The minerva-embedder binary imports the
//!   server half; this crate's `RemoteEmbedderClient` uses the client
//!   half. See `docs/microservices-split.md` for the full plan.

pub mod embedder;
pub mod reranker;

pub use embedder::{LocalEmbedderClient, RemoteEmbedderClient};
pub use reranker::{LocalRerankerClient, RemoteRerankerClient};

// Re-export the trait + result types for convenience so consumers can
// `use minerva_rpc::EmbedderClient` instead of reaching into core.
pub use minerva_core::rpc::{
    BenchmarkError, EmbedBenchmarkResult, EmbedderClient, RerankBenchmarkResult, RerankerClient,
};

/// tonic-generated client + server types. One module per `.proto` file
/// to match the package paths declared inside them. Consumers
/// (binaries / clients) import only the half they need.
pub mod proto {
    pub mod embedder {
        // Mirrors `package minerva.embedder.v1;` in embedder.proto.
        tonic::include_proto!("minerva.embedder.v1");
    }
    pub mod reranker {
        // Mirrors `package minerva.reranker.v1;` in reranker.proto.
        tonic::include_proto!("minerva.reranker.v1");
    }
}
