//! The per-document ingest pipeline: text extraction (pdf / html /
//! plain), chunking, the OpenAI-HTTP embedder, the `Classifier` trait,
//! and the Qdrant collection helpers. Engine-free: embedding /
//! reranking go through the `minerva_core::rpc` client traits, so the
//! worker can link this without pulling the heavy model engine.

pub mod chunker;
pub mod classifier;
pub mod embedder;
pub mod pdf;
pub mod pipeline;
