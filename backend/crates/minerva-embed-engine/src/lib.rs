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
//! Both caches share one budgeted-LRU implementation: the generic
//! [`model_cache::ModelCache`] (LRU + budget + warmup-before-measure +
//! sync eviction + the pre-load admission gate) driven by per-cache
//! [`model_cache::ModelLoader`] impls, with the leaf memory primitives
//! (RSS read, cgroup budget, glibc trim, per-model estimate table) in
//! [`mem`]. The embedder and reranker no longer carry two copies of the
//! eviction logic.
//!
//! This crate is intentionally the sole home for the heavy ONNX /
//! candle / hf-hub dependencies. The model-server binaries
//! (`minerva-embedder`, `minerva-reranker`) link it directly; the
//! api / worker / scheduler reach the same functionality over gRPC and
//! never compile it.

pub mod fastembed_embedder;
mod mem;
pub mod mem_budget;
mod model_cache;
pub mod reranker;
