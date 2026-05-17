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
/// research" section that carries BOTH the research agent's
/// synthesised narrative (`research_transcript`) AND a compressed
/// log of which tools fired (`tool_log`). Without the transcript
/// the writeup model would have to re-derive everything from raw
/// chunks; the research agent's bullets are exactly the kind of
/// distilled signal that lets writeup just compose tone and
/// pedagogy on top.
///
/// Takes primitives rather than a `GenerationContext` so unit
/// tests can exercise the prompt shape without standing up a
/// sqlx pool or qdrant client.
pub fn build_writeup_system_prompt(
    course_name: &str,
    custom_prompt: &Option<String>,
    chunks: &[RagChunk],
    research_transcript: &str,
    tool_log: &str,
) -> String {
    let base = common::build_system_prompt(course_name, custom_prompt, chunks);
    // Inline the transcript section only when there's something to
    // show; on legacy paths or trivial turns it can be empty.
    let transcript_section = if research_transcript.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n\n### Research agent findings (your own prior notes)\n\
            You already analysed this question in a hidden research phase. \
            Treat the bullets below as established facts and build on them \
            directly; do NOT re-derive them from the raw `Course materials` \
            section.\n\n{}",
            research_transcript
        )
    };
    format!(
        "{base}\n\n## Prior research (server-side)\n\
        Before this turn you ran a hidden research phase to gather context. \
        The materials it produced are in `Course materials` above. The tools \
        you called were:\n\n{tool_log}{transcript_section}\n\n\
        Compose the student-facing reply now. Use the findings above directly \
        ; lean on them, paraphrase as needed, structure for pedagogy. Do NOT \
        call any tools; do NOT describe your research process to the student; \
        do NOT say things like \"based on my research\"; just answer.",
        base = base,
        tool_log = tool_log,
        transcript_section = transcript_section,
    )
}

/// Run the writeup phase. Forwards every content token to the SSE
/// channel as `{"type":"token", ...}`, then returns the full
/// accumulated text and usage counts.
pub async fn run(
    ctx: &GenerationContext,
    chunks: &[RagChunk],
    research_transcript: &str,
    tool_log: &str,
    tx: &mpsc::Sender<Result<Event, AppError>>,
) -> Result<WriteupOutput, AppError> {
    let http_client = reqwest::Client::new();
    let system = build_writeup_system_prompt(
        &ctx.course_name,
        &ctx.custom_prompt,
        chunks,
        research_transcript,
        tool_log,
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
    fn prompt_embeds_tool_log() {
        let chunks: Vec<RagChunk> = Vec::new();
        let tool_log = "- keyword_search({\"query\":\"deadline\"}) -> 2 chunks";
        let prompt = build_writeup_system_prompt("Test Course", &None, &chunks, "", tool_log);
        assert!(prompt.contains("Prior research"));
        assert!(prompt.contains("keyword_search"));
    }

    #[test]
    fn prompt_embeds_research_transcript_when_nonempty() {
        let chunks: Vec<RagChunk> = Vec::new();
        let transcript = "- Deadline for assignment 2 is November 15.";
        let prompt = build_writeup_system_prompt(
            "Test Course",
            &None,
            &chunks,
            transcript,
            "(no tool calls)",
        );
        assert!(prompt.contains("Research agent findings"));
        assert!(prompt.contains("November 15"));
    }

    #[test]
    fn prompt_skips_research_findings_when_transcript_empty() {
        let chunks: Vec<RagChunk> = Vec::new();
        let prompt = build_writeup_system_prompt(
            "Test Course",
            &None,
            &chunks,
            "",
            "- keyword_search(...) -> 0 chunks",
        );
        assert!(!prompt.contains("Research agent findings"));
    }

    #[test]
    fn prompt_instructs_model_to_skip_tool_calls() {
        let chunks: Vec<RagChunk> = Vec::new();
        let prompt = build_writeup_system_prompt("Test Course", &None, &chunks, "", "no calls");
        assert!(prompt.contains("Do NOT call any tools"));
    }
}
