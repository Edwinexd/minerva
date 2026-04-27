//! Document-kind classification -- the "course knowledge graph V1".
//!
//! Pipeline:
//! 1. After text extraction, the worker calls
//!    [`document::CerebrasClassifier`] (which implements
//!    `minerva_ingest::classifier::Classifier`).
//! 2. The classifier asks gpt-oss-120b on Cerebras to label the doc as one
//!    of [`types::DocumentKind`], returning JSON via Cerebras structured
//!    outputs.
//! 3. Low-confidence or suspicious-flag results get re-run with
//!    `reasoning_effort: "high"`.
//! 4. The result is persisted by
//!    [`minerva_db::queries::documents::set_classification`] which is a
//!    no-op when the row is locked by a teacher.
//!
//! The chat path consumes this metadata via the Qdrant payload (each
//! point's `kind` field) plus the DB-side `doc_ids_with_kind` /
//! `unclassified_doc_ids` filters in `strategy::common`.

pub mod adversarial;
pub mod aegis;
pub mod document;
pub mod extraction_guard;
pub mod linker;
pub mod prompts;
pub mod types;

pub use document::CerebrasClassifier;
// Other exports (DocumentKind, ALL_KINDS, is_signal_only_kind) are reached
// via the `types` submodule directly from callers.
