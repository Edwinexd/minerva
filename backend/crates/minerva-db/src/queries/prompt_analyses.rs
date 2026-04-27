//! Aegis prompt-coaching analyses. One row per user message that
//! the analyzer LLM scored. See migration
//! `20260427000003_aegis_prompt_analyses.sql` for the schema.
//!
//! Two layers like every other queries module:
//!
//!   1. `insert` -- the analyzer writes one row after a successful
//!      LLM call. Idempotent under `(message_id)` UNIQUE: a retry
//!      after a transient failure replaces the previous row rather
//!      than inserting a duplicate.
//!   2. `list_for_conversation` -- the chat detail route pulls
//!      every analysis for a conversation in one query so the
//!      Feedback panel's history list and the latest-prompt summary
//!      come from the same payload.
//!
//! The analyzer is best-effort. None of the call sites here block
//! the chat hot path; a failed insert is logged at warn and dropped.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PromptAnalysisRow {
    pub id: Uuid,
    pub message_id: Uuid,
    pub overall_score: i32,
    pub clarity_score: i32,
    pub context_score: i32,
    pub constraints_score: i32,
    pub reasoning_demand_score: i32,
    pub critical_thinking_score: i32,
    pub structural_clarity_label: String,
    pub structural_clarity_feedback: String,
    pub terminology_label: String,
    pub terminology_feedback: String,
    pub missing_constraint_label: String,
    pub missing_constraint_feedback: String,
    pub model_used: String,
    pub created_at: DateTime<Utc>,
}

/// All scores + textual feedback the caller wants to persist for
/// one user message. Carrying a struct here (vs ten positional
/// parameters) keeps the call site at the analyzer readable and
/// makes it harder to swap two adjacent integer fields.
#[derive(Debug, Clone)]
pub struct PromptAnalysisInsert<'a> {
    pub message_id: Uuid,
    pub overall_score: i32,
    pub clarity_score: i32,
    pub context_score: i32,
    pub constraints_score: i32,
    pub reasoning_demand_score: i32,
    pub critical_thinking_score: i32,
    pub structural_clarity_label: &'a str,
    pub structural_clarity_feedback: &'a str,
    pub terminology_label: &'a str,
    pub terminology_feedback: &'a str,
    pub missing_constraint_label: &'a str,
    pub missing_constraint_feedback: &'a str,
    pub model_used: &'a str,
}

/// Insert (or replace, on retry) one analysis row. ON CONFLICT
/// (message_id) DO UPDATE so a transient analyzer retry doesn't
/// trip the UNIQUE constraint -- the latest verdict for a turn is
/// what the panel shows.
pub async fn insert(
    db: &PgPool,
    a: PromptAnalysisInsert<'_>,
) -> Result<PromptAnalysisRow, sqlx::Error> {
    sqlx::query_as!(
        PromptAnalysisRow,
        r#"
        INSERT INTO prompt_analyses (
            message_id, overall_score, clarity_score, context_score,
            constraints_score, reasoning_demand_score, critical_thinking_score,
            structural_clarity_label, structural_clarity_feedback,
            terminology_label, terminology_feedback,
            missing_constraint_label, missing_constraint_feedback,
            model_used
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        ON CONFLICT (message_id) DO UPDATE SET
            overall_score = EXCLUDED.overall_score,
            clarity_score = EXCLUDED.clarity_score,
            context_score = EXCLUDED.context_score,
            constraints_score = EXCLUDED.constraints_score,
            reasoning_demand_score = EXCLUDED.reasoning_demand_score,
            critical_thinking_score = EXCLUDED.critical_thinking_score,
            structural_clarity_label = EXCLUDED.structural_clarity_label,
            structural_clarity_feedback = EXCLUDED.structural_clarity_feedback,
            terminology_label = EXCLUDED.terminology_label,
            terminology_feedback = EXCLUDED.terminology_feedback,
            missing_constraint_label = EXCLUDED.missing_constraint_label,
            missing_constraint_feedback = EXCLUDED.missing_constraint_feedback,
            model_used = EXCLUDED.model_used
        RETURNING
            id, message_id, overall_score, clarity_score, context_score,
            constraints_score, reasoning_demand_score, critical_thinking_score,
            structural_clarity_label, structural_clarity_feedback,
            terminology_label, terminology_feedback,
            missing_constraint_label, missing_constraint_feedback,
            model_used, created_at
        "#,
        a.message_id,
        a.overall_score,
        a.clarity_score,
        a.context_score,
        a.constraints_score,
        a.reasoning_demand_score,
        a.critical_thinking_score,
        a.structural_clarity_label,
        a.structural_clarity_feedback,
        a.terminology_label,
        a.terminology_feedback,
        a.missing_constraint_label,
        a.missing_constraint_feedback,
        a.model_used,
    )
    .fetch_one(db)
    .await
}

/// All analyses for messages in `conversation_id`, oldest first.
/// Joined through `messages` so we can scope by conversation
/// without storing a redundant conversation_id column on the
/// analyses table.
pub async fn list_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<PromptAnalysisRow>, sqlx::Error> {
    sqlx::query_as!(
        PromptAnalysisRow,
        r#"
        SELECT
            a.id, a.message_id, a.overall_score, a.clarity_score, a.context_score,
            a.constraints_score, a.reasoning_demand_score, a.critical_thinking_score,
            a.structural_clarity_label, a.structural_clarity_feedback,
            a.terminology_label, a.terminology_feedback,
            a.missing_constraint_label, a.missing_constraint_feedback,
            a.model_used, a.created_at
        FROM prompt_analyses a
        JOIN messages m ON m.id = a.message_id
        WHERE m.conversation_id = $1
        ORDER BY m.created_at ASC, a.created_at ASC
        "#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}
