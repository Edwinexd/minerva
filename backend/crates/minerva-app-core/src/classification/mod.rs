//! Document-kind classification; the "course knowledge graph V1".
//!
//! Pipeline:
//! 1. After text extraction, the worker calls
//!    [`document::LlmClassifier`] (which implements
//!    `minerva_pipeline::classifier::Classifier`).
//! 2. The classifier asks the admin-selected utility model to label the
//!    doc as one of [`types::DocumentKind`], returning JSON via the
//!    provider's structured-output rails.
//! 3. Low-confidence and suspicious-flag results flow through to the
//!    teacher dashboard; we previously did a high-effort retry on
//!    gpt-oss-120b but a temperature-0 re-call would just return the
//!    same JSON, so the retry was dropped.
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
pub mod topic_switch;
pub mod types;

pub use document::LlmClassifier;

/// Output cap for the short-verdict classifiers (adversarial filter,
/// topic switch), reasoning included. gpt-oss is a reasoning model and
/// counts its reasoning against the cap, so a cap sized for the verdict
/// alone runs out mid-reasoning: the reply comes back empty or as
/// truncated JSON while the prompt is still billed. In prod the old caps
/// did that on every adversarial call (4 tokens) and on 229 of the first
/// 300 topic-switch calls (64 tokens).
pub(crate) const VERDICT_MAX_TOKENS: usize = 256;
// Other exports (DocumentKind, ALL_KINDS, is_signal_only_kind) are reached
// via the `types` submodule directly from callers.
