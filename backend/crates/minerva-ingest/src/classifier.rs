//! Trait abstraction for document-kind classification.
//!
//! The actual classifier (calls Cerebras llama3.1-8b) lives in
//! `minerva-server`, which depends on this crate. We can't reach back
//! across that dependency edge, so the pipeline takes a `&dyn Classifier`
//! and the concrete impl is supplied by the worker at the top level.
//!
//! Tests can substitute a fake `Classifier` returning canned results.

use async_trait::async_trait;
use uuid::Uuid;

/// Result of a single classification call. Strings (not enums) so this
/// crate doesn't need to know the closed set of valid kinds; that's
/// validated at the DB CHECK constraint and at the route handlers.
#[derive(Debug, Clone)]
pub struct ClassifiedKind {
    pub kind: String,
    pub confidence: f32,
    pub rationale: Option<String>,
    /// Free-form tags surfaced by the classifier ("might_be_solution",
    /// "contains_worked_examples", …). The pipeline triggers a
    /// re-run-with-thinking when this is non-empty even if confidence
    /// is high.
    pub suspicious_flags: Vec<String>,
}

#[async_trait]
pub trait Classifier: Send + Sync {
    /// Classify a single document by its filename, mime type, and
    /// extracted text. Implementations should truncate `text` as
    /// needed; the pipeline passes the full string.
    ///
    /// `course_id` is passed through so implementations that talk
    /// to a paid LLM API can attribute their token spend to the
    /// owning course (`course_token_usage` table). Tests / no-op
    /// implementations may ignore it.
    async fn classify(
        &self,
        course_id: Uuid,
        filename: &str,
        mime_type: &str,
        text: &str,
    ) -> Result<ClassifiedKind, String>;
}
