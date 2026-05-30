//! Generates the tonic gRPC client + server stubs from the proto files
//! in `proto/`.
//!
//! Both client (for the api / worker) and server (for the minerva-
//! embedder / minerva-reranker binaries) live in this crate so they
//! consume the same generated types; the consumer crates pick which
//! half they import.
//!
//! Phase 1 ships `embedder.proto`; Phase 2 adds `reranker.proto`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(
            &["proto/embedder.proto", "proto/reranker.proto"],
            &["proto"],
        )?;
    println!("cargo:rerun-if-changed=proto/embedder.proto");
    println!("cargo:rerun-if-changed=proto/reranker.proto");
    Ok(())
}
