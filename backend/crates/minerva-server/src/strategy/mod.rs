pub mod common;
pub mod extraction_guard;
pub mod flare;
pub mod parallel;
pub mod simple;

use axum::response::sse::Event;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::error::AppError;

/// Context passed to every generation strategy.
pub struct GenerationContext {
    pub course_name: String,
    pub custom_prompt: Option<String>,
    pub model: String,
    pub temperature: f64,
    pub max_chunks: i32,
    pub min_score: f32,
    pub course_id: Uuid,
    pub conversation_id: Uuid,
    pub user_id: Uuid,
    pub cerebras_api_key: String,
    /// Base URL for the Cerebras chat-completions endpoint. Production
    /// routes default this to `common::CEREBRAS_CHAT_COMPLETIONS_URL`;
    /// integration tests override it to point at a wiremock server.
    pub cerebras_base_url: String,
    pub openai_api_key: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub history: Vec<minerva_db::queries::conversations::MessageRow>,
    pub user_content: String,
    pub is_first_message: bool,
    /// Per-student-per-course daily token limit copied from `courses.daily_token_limit`.
    /// 0 = unlimited (no per-course cap configured). Used by FLARE as an input to
    /// the single-response fail-safe so one answer cannot burn more than 2x a
    /// student's daily allowance.
    pub daily_token_limit: i64,
    pub db: sqlx::PgPool,
    pub qdrant: std::sync::Arc<qdrant_client::Qdrant>,
    pub fastembed: std::sync::Arc<minerva_ingest::fastembed_embedder::FastEmbedder>,
    /// Resolved per-request from the `course_kg` feature flag. When
    /// FALSE, RAG behaviour reverts to the pre-KG baseline:
    ///
    ///   * adversarial chunk filter skipped
    ///   * `unclassified_doc_ids` lookup skipped (treated as empty)
    ///   * `partition_chunks` puts every chunk into context
    ///   * `build_system_prompt_with_signals` gets no signals (no
    ///     refusal addendum)
    ///
    /// Decided once at the chat-route entry and propagated through
    /// the strategy so each pass sees a stable view.
    pub kg_enabled: bool,
}

/// Run the appropriate strategy based on the strategy name.
pub async fn run_strategy(
    strategy: &str,
    ctx: GenerationContext,
    tx: mpsc::Sender<Result<Event, AppError>>,
) {
    match strategy {
        "flare" => flare::run(ctx, tx).await,
        "simple" => simple::run(ctx, tx).await,
        _ => parallel::run(ctx, tx).await, // "parallel" is the default
    }
}
