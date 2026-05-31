//! Local model engine.
//!
//! Owns the two in-process model caches and the shared memory budget:
//! * [`fastembed_embedder::FastEmbedder`] : the embedding model LRU
//!   (ONNX via fastembed, candle for Qwen3), plus `compute_budget_bytes`
//!   and the boot-benchmark machinery.
//! * [`reranker::FastReranker`] : the cross-encoder re-ranker cache.
//! * [`mem_budget::MemBudget`] : the pod-wide MiB-permit pool for fat
//!   background allocations outside the fastembed cache.
//!
//! This crate is intentionally the sole home for the heavy ONNX /
//! candle / hf-hub dependencies. The model-server binaries
//! (`minerva-embedder`, `minerva-reranker`) link it directly; the
//! api / worker / scheduler reach the same functionality over gRPC and
//! never compile it.

pub mod fastembed_embedder;
pub mod mem_budget;
pub mod reranker;
