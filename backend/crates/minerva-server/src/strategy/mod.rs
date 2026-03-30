pub mod common;
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
    pub course_id: Uuid,
    pub conversation_id: Uuid,
    pub user_id: Uuid,
    pub cerebras_api_key: String,
    pub openai_api_key: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub history: Vec<minerva_db::queries::conversations::MessageRow>,
    pub user_content: String,
    pub is_first_message: bool,
    pub db: sqlx::PgPool,
    pub qdrant: std::sync::Arc<qdrant_client::Qdrant>,
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
