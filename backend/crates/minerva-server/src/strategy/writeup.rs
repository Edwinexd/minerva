//! Writeup phase for `tool_use_enabled` courses.
//!
//! Runs after the research phase (`research_phase::run`) and emits
//! the user-facing answer as a single clean streaming pass. No
//! tools, no logprobs, no restart machinery; the heavy lifting is
//! done. The consolidated chunk set from research plus a compressed
//! summary of what was searched both flow in via the system prompt.
//!
//! Emits `{"type":"token", ...}` SSE frames identical in shape to
//! the legacy single-pass strategies, so the frontend's chat
//! renderer can stay agnostic about whether tool use was enabled
//! for this course.

use axum::response::sse::Event;
use tokio::sync::mpsc;

use super::common;
use super::common::RagChunk;
use super::GenerationContext;
use crate::error::AppError;

/// Result of the writeup phase. Mirrors what
/// `common::stream_cerebras_to_client` returns plus the final text
/// for downstream consumers (extraction-guard intercept, message
/// persistence).
#[derive(Debug)]
pub struct WriteupOutput {
    pub full_text: String,
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
}

/// Build the writeup system prompt: the standard course system
/// prompt seeded with the consolidated chunk set, plus a "Prior
/// research" section that tells the model what tools fired and
/// what they returned. Helps the model anchor on the research
/// without us having to re-feed raw tool results.
///
/// Takes primitives rather than a `GenerationContext` so unit
/// tests can exercise the prompt shape without standing up a
/// sqlx pool or qdrant client.
pub fn build_writeup_system_prompt(
    course_name: &str,
    custom_prompt: &Option<String>,
    chunks: &[RagChunk],
    research_summary: &str,
) -> String {
    let base = common::build_system_prompt(course_name, custom_prompt, chunks);
    format!(
        "{base}\n\n## Prior research (server-side)\n\
        Before this turn you ran a hidden research phase, using tools to\n\
        gather the materials now present in `Course materials` above.\n\
        Your prior calls were:\n\n{summary}\n\n\
        Compose the student's reply now. Use the gathered materials.\n\
        Do NOT call any tools; do NOT describe your research process to\n\
        the student; just write the reply.",
        base = base,
        summary = research_summary,
    )
}

/// Run the writeup phase. Forwards every content token to the SSE
/// channel as `{"type":"token", ...}`, then returns the full
/// accumulated text and usage counts.
pub async fn run(
    ctx: &GenerationContext,
    chunks: &[RagChunk],
    research_summary: &str,
    tx: &mpsc::Sender<Result<Event, AppError>>,
) -> Result<WriteupOutput, AppError> {
    let http_client = reqwest::Client::new();
    let system = build_writeup_system_prompt(
        &ctx.course_name,
        &ctx.custom_prompt,
        chunks,
        research_summary,
    );
    let messages = compose_messages(&system, ctx);

    let mut full_text = String::new();
    let (prompt_tokens, completion_tokens) = common::stream_cerebras_to_client(
        &http_client,
        &ctx.cerebras_api_key,
        &ctx.model,
        ctx.temperature,
        &messages,
        tx,
        &mut full_text,
    )
    .await
    .map_err(AppError::Internal)?;

    Ok(WriteupOutput {
        full_text,
        prompt_tokens,
        completion_tokens,
    })
}

/// Build the writeup-phase message list. Mirrors what
/// `common::build_chat_messages` does (system + history) and then
/// appends the user's just-arrived message; the research phase
/// doesn't bake the user's content into anything writeup needs
/// to see, so we re-derive from the canonical inputs.
fn compose_messages(system_prompt: &str, ctx: &GenerationContext) -> Vec<serde_json::Value> {
    let mut messages = common::build_chat_messages(system_prompt, ctx.history.as_slice());
    messages.push(serde_json::json!({
        "role": "user",
        "content": ctx.user_content.clone(),
    }));
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_embeds_research_summary() {
        let chunks: Vec<RagChunk> = Vec::new();
        let summary = "- keyword_search({\"query\":\"deadline\"}) -> 2 chunks";
        let prompt = build_writeup_system_prompt("Test Course", &None, &chunks, summary);
        assert!(prompt.contains("Prior research"));
        assert!(prompt.contains("keyword_search"));
    }

    #[test]
    fn prompt_instructs_model_to_skip_tool_calls() {
        let chunks: Vec<RagChunk> = Vec::new();
        let prompt = build_writeup_system_prompt("Test Course", &None, &chunks, "no calls");
        assert!(prompt.contains("Do NOT call any tools"));
    }
}
