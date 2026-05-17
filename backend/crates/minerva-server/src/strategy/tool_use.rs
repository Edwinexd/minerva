//! Orchestration for `tool_use_enabled = TRUE` courses.
//!
//! Both `simple+tools` and `flare+tools` collapse to this same
//! pipeline; the only thing that differs is the `use_logprobs` flag
//! we hand to the research phase (and, indirectly, whether
//! per-token logprobs are requested on the Cerebras side and the
//! FLARE-style low-confidence detector fires).
//!
//! Pipeline:
//!
//! 1. Seed retrieval ; same path as the legacy strategies
//!    (`rag_lookup` + KG expansion + adversarial filter +
//!    partition).
//! 2. Extraction-guard pre-evaluation.
//! 3. Research phase (`research_phase::run`) emitting `thinking_*`
//!    SSE events to the client and accumulating chunks via tool
//!    calls.
//! 4. Writeup phase (`writeup::run`) emitting the user-facing
//!    `token` SSE stream.
//! 5. Extraction-guard post-intercept on the writeup.
//! 6. `common::finalize`.
//!
//! All shared with the legacy paths so behaviour deltas are
//! limited to steps 3 and 4.

use axum::response::sse::Event;
use tokio::sync::mpsc;

use super::common;
use super::research_phase::{self, ResearchConfig};
use super::tools::ToolCatalogFlags;
use super::writeup;
use super::GenerationContext;
use crate::error::AppError;

/// Per-response token cap mirroring `flare.rs`. Same multiplier
/// applied to `courses.daily_token_limit` so a tool-use answer
/// can't burn more than 2x a student's daily budget in one turn.
const UNLIMITED_COURSE_RESPONSE_CAP: i64 = 200_000;
const DAILY_LIMIT_RESPONSE_MULTIPLIER: i64 = 2;

fn per_response_token_cap(daily_token_limit: i64) -> i64 {
    if daily_token_limit > 0 {
        daily_token_limit.saturating_mul(DAILY_LIMIT_RESPONSE_MULTIPLIER)
    } else {
        UNLIMITED_COURSE_RESPONSE_CAP
    }
}

/// Run the full research+writeup pipeline. Branched into by
/// `simple::run` and `flare::run` when `ctx.tool_use_enabled` is
/// true.
pub async fn run(
    ctx: GenerationContext,
    use_logprobs: bool,
    tx: mpsc::Sender<Result<Event, AppError>>,
) {
    let started_at = std::time::Instant::now();
    let http_client = reqwest::Client::new();
    let collection_name =
        minerva_ingest::pipeline::collection_name(ctx.course_id, ctx.embedding_version);

    // 1. Seed retrieval (identical to simple/parallel/flare's preamble).
    let raw_chunks = common::rag_lookup(
        &http_client,
        &ctx.openai_api_key,
        &ctx.fastembed,
        &ctx.qdrant,
        &collection_name,
        &ctx.user_content,
        ctx.max_chunks,
        ctx.min_score,
        &ctx.embedding_provider,
        &ctx.embedding_model,
    )
    .await;

    let unclassified = if ctx.kg_enabled {
        minerva_db::queries::documents::unclassified_doc_ids(&ctx.db, ctx.course_id)
            .await
            .unwrap_or_default()
    } else {
        std::collections::HashSet::new()
    };
    let mut rag = common::partition_chunks(raw_chunks, &unclassified, ctx.kg_enabled);

    if ctx.kg_enabled {
        rag.context = crate::classification::adversarial::filter_solution_chunks(
            &http_client,
            &ctx.cerebras_api_key,
            &ctx.db,
            ctx.course_id,
            rag.context,
        )
        .await;
    }

    if ctx.kg_enabled {
        let extra = common::expand_context_via_graph(
            &ctx.db,
            &ctx.qdrant,
            &ctx.fastembed,
            &http_client,
            &ctx.openai_api_key,
            ctx.course_id,
            &collection_name,
            &ctx.embedding_provider,
            &ctx.embedding_model,
            &ctx.user_content,
            &rag.context,
        )
        .await;
        rag.context.extend(extra);
    }

    // 2. Extraction-guard pre-eval against the seed view. The
    //    research phase can drag more chunks in later via tool
    //    calls; the guard's intent classifier only sees the seed
    //    partition, which is also what the legacy paths feed it.
    let guard_decision = super::extraction_guard::evaluate_for_turn(
        &ctx.db,
        &http_client,
        &ctx.cerebras_api_key,
        ctx.course_id,
        ctx.conversation_id,
        &ctx.history,
        &ctx.user_content,
        &rag.signals,
        &rag.context,
    )
    .await;

    // 3. Research phase. Seeds the chunk accumulator with the
    //    initial partition's `context` (signals are excluded from
    //    LLM context by definition; tool calls can still surface
    //    relevant content if needed).
    let cap = per_response_token_cap(ctx.daily_token_limit);
    let catalog_flags = ToolCatalogFlags {
        kg_enabled: ctx.kg_enabled,
    };
    let config = ResearchConfig::defaults(use_logprobs);
    let research =
        research_phase::run(&ctx, config, catalog_flags, rag.context.clone(), cap, &tx).await;

    tracing::info!(
        "tool_use: research finished for conv {}: turns={}, tool_calls={}, flare_injections={}, stop={:?}, chunks={}",
        ctx.conversation_id,
        research.turns,
        research.tool_calls_executed,
        research.flare_injections,
        research.stop_reason,
        research.chunks.len(),
    );

    // 4. Writeup phase. Single clean streaming pass; tokens flow
    //    to the client as `{"type":"token", ...}` (same shape as
    //    the legacy strategies).
    let writeup_output = match writeup::run(
        &ctx,
        &research.chunks,
        // The research agent's actual narrative (its bullet-point
        // findings) is what the writeup model should lean on. The
        // tool log is metadata about HOW those findings were
        // produced and is still surfaced so the writeup model can
        // see what was searched.
        &research.transcript,
        &research.research_summary,
        &tx,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            let msg = format!("{}", e);
            let _ = tx
                .send(Ok(Event::default().data(
                    serde_json::json!({"type": "error", "error": msg}).to_string(),
                )))
                .await;
            return;
        }
    };

    // 5. Post-generation extraction-guard intercept. Operates on
    //    a clean single-pass writeup (much better signal than the
    //    legacy FLARE path's multi-restart full_text).
    let final_text = super::extraction_guard::intercept_reply(
        &ctx.db,
        &http_client,
        &ctx.cerebras_api_key,
        ctx.course_id,
        ctx.conversation_id,
        &guard_decision,
        &ctx.user_content,
        &writeup_output.full_text,
        &tx,
    )
    .await;

    // 6. Surface the consolidated chunk set to the client. Mirrors
    //    the legacy strategies' sources panel ; students see every
    //    document that informed the answer, both seed-RAG and the
    //    ones the model pulled in via tool calls.
    let hidden = minerva_db::queries::documents::hidden_document_ids(&ctx.db, ctx.course_id)
        .await
        .unwrap_or_default();
    let mut displayed = research.chunks.clone();
    displayed.extend(rag.signals.iter().cloned());
    let client_chunks = common::chunks_for_client(&displayed, &hidden);
    let chunks_json = serde_json::to_value(&client_chunks).ok();

    let total_prompt = research.total_prompt_tokens + writeup_output.prompt_tokens;
    let total_completion = research.total_completion_tokens + writeup_output.completion_tokens;

    // Persist the research-phase artefacts alongside the assistant
    // message so the frontend's "Thinking" disclosure survives a
    // page refresh. We persist even when the research is trivial
    // (no tool calls, no FLARE injections, model just chatted): the
    // disclosure stays collapsed by default and shows duration, so
    // boring research is unobtrusive but still discoverable.
    let thinking_transcript = if research.transcript.is_empty() {
        None
    } else {
        Some(research.transcript.as_str())
    };
    let tool_events_json = if research.tool_events.is_empty() {
        None
    } else {
        serde_json::to_value(&research.tool_events).ok()
    };

    common::finalize(
        &ctx,
        &tx,
        &final_text,
        chunks_json.as_ref(),
        total_prompt,
        total_completion,
        !research.chunks.is_empty() || !rag.signals.is_empty(),
        started_at.elapsed().as_millis() as i64,
        // `iterations` field in finalize counts FLARE outer-loop
        // iterations; for the tool-use path we report
        // research turns + 1 (the writeup) so it's comparable.
        (research.turns as i32) + 1,
        thinking_transcript,
        tool_events_json.as_ref(),
        Some(research.duration_ms.clamp(0, i32::MAX as i64) as i32),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_course_cap_floor() {
        assert_eq!(per_response_token_cap(0), UNLIMITED_COURSE_RESPONSE_CAP);
    }

    #[test]
    fn per_response_cap_doubles_daily_limit() {
        assert_eq!(per_response_token_cap(50_000), 100_000);
    }

    #[test]
    fn per_response_cap_saturates_on_overflow() {
        // i64::MAX * 2 would overflow; saturating_mul holds the
        // value at i64::MAX rather than wrapping to a tiny number.
        assert_eq!(per_response_token_cap(i64::MAX), i64::MAX);
    }
}
