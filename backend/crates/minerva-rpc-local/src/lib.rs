//! In-process client implementations over the local model engine.
//!
//! The traits + protocol-neutral result types live in
//! `minerva_core::rpc`; the remote gRPC transports live in
//! `minerva-rpc`. This crate provides the in-process variants that wrap
//! a live `FastEmbedder` / `FastReranker` from `minerva-embed-engine`,
//! used by the model-server binaries and the single-process local-dev
//! build.

pub mod embedder;
pub mod reranker;

pub use embedder::LocalEmbedderClient;
pub use reranker::LocalRerankerClient;
