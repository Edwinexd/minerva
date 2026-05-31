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
//!    (`rag_lookup` + adversarial filter + partition).
//! 2. Build prelim retrieval records + KG expansion. Records held
//!    in memory ; no SSE emit yet.
//! 3. Extraction-guard pre-evaluation against the seed + KG-
//!    expanded RAG view.
//! 4. Emit the prelim retrieval records over SSE, gated by the
//!    guard's per-turn `flagged_this_turn` signal.
//! 5. Research phase (`research_phase::run`) emitting `thinking_*`
//!    SSE events to the client and accumulating chunks via tool
//!    calls.
//! 6. Writeup phase (`writeup::run`) emitting the user-facing
//!    `token` SSE stream.
//! 7. Extraction-guard post-intercept on the writeup.
//! 8. Surface chunk set + `common::finalize`.
//!
//! All shared with the legacy paths so behaviour deltas are
//! limited to steps 3 and 4.

use axum::response::sse::Event;
use tokio::sync::mpsc;

use super::common;
use super::common::RagChunk;
use super::research_phase::{self, ResearchConfig, ToolEventRecord};
use super::tools::ToolCatalogFlags;
use super::writeup;
use super::GenerationContext;
use crate::error::AppError;

/// Build a `ToolEventRecord` for a server-initiated retrieval (seed
/// RAG, KG expansion). These don't go through the model-visible
/// tool catalog, but from the user's perspective they're still
/// retrievals and should show up in the "Thinking" disclosure
/// alongside the model-initiated tool calls.
///
/// This is a pure builder ; it does NOT emit any SSE events. Live
/// emit is deferred to `emit_server_retrieval` below, called AFTER
/// the extraction-guard decision lands so we know whether to gate
/// the user-visible retrieval events for this turn. (Building the
/// records first lets the guard see the full KG-expanded RAG view
/// while still being able to suppress prelim emits when the guard
/// trips.) The persisted record is always returned so the teacher
/// dashboard's audit trail keeps the seed/KG retrievals regardless
/// of suppression.
fn build_server_retrieval_record(
    name: &str,
    args: serde_json::Value,
    chunks: &[RagChunk],
) -> ToolEventRecord {
    let result_payload: Vec<serde_json::Value> = chunks
        .iter()
        .map(|c| serde_json::json!({"filename": c.filename, "text": c.text}))
        .collect();
    let result_value = serde_json::Value::Array(result_payload);
    let summary = if chunks.is_empty() {
        "0 chunks".to_string()
    } else if chunks.len() == 1 {
        "1 chunk".to_string()
    } else {
        format!("{} chunks", chunks.len())
    };
    ToolEventRecord {
        name: name.to_string(),
        args,
        result_summary: summary,
        result: result_value,
    }
}

/// Emit the SSE `tool_call` / `tool_result` pair for a previously-
/// built server-retrieval record. Called after the extraction guard
/// has run so we can gate the emit on the per-turn signal. The
/// record itself (returned by `build_server_retrieval_record`) is
/// always persisted regardless of whether we emit live.
async fn emit_server_retrieval(
    tx: &tokio::sync::mpsc::Sender<Result<axum::response::sse::Event, AppError>>,
    record: &ToolEventRecord,
) {
    research_phase::emit_tool_call(tx, &record.name, &record.args).await;
    research_phase::emit_tool_result(tx, &record.name, &record.result_summary, &record.result)
        .await;
}

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
        minerva_pipeline::pipeline::collection_name(ctx.course_id, ctx.embedding_version);

    // Drop chunks from orphaned docs before any partition; see
    // `simple.rs` for the rationale. Computed once per turn and
    // re-used for the model's tool-driven retrievals below.
    let orphaned = minerva_db::queries::documents::orphaned_doc_ids(&ctx.db, ctx.course_id)
        .await
        .unwrap_or_default();

    // 1. Seed retrieval (identical to simple/parallel/flare's preamble).
    let raw_chunks = common::rag_lookup(
        &http_client,
        &ctx.openai_api_key,
        &ctx.fastembed,
        &ctx.reranker,
        &ctx.reranker_model,
        &ctx.qdrant,
        &collection_name,
        &ctx.user_content,
        ctx.max_chunks,
        ctx.min_score,
        &ctx.embedding_provider,
        &ctx.embedding_model,
        &orphaned,
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

    // 2. Build the prelim retrieval records (seed + optional KG
    //    expansion) BEFORE the guard evaluation and BEFORE emitting
    //    anything to the SSE channel. Deferring the live emit lets
    //    the guard see the full KG-expanded RAG view for its
    //    proximity classifier (the proximity check reads rag_context
    //    for lecture/reading chunks and maps them via `applied_in`
    //    to assignment doc ids; if KG expansion isn't in yet, those
    //    KG-derived assignment partners are silently invisible to
    //    the sliding-window check and slow multi-turn extractions
    //    slip through). The records themselves are always persisted
    //    onto `tool_events` for the teacher dashboard regardless of
    //    whether we end up emitting live to the student.
    let mut prelim_events: Vec<ToolEventRecord> = Vec::new();
    prelim_events.push(build_server_retrieval_record(
        "initial_retrieve",
        serde_json::json!({
            "query": ctx.user_content.clone(),
            "trigger": "user_question",
        }),
        &rag.context,
    ));

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
            &orphaned,
        )
        .await;
        // Only surface the KG-expansion event when it actually
        // added something ; an empty extras list means the seed
        // chunks already covered the KG neighbourhood and there's
        // nothing useful to disclose.
        if !extra.is_empty() {
            prelim_events.push(build_server_retrieval_record(
                "kg_expand",
                serde_json::json!({
                    "trigger": "knowledge_graph_neighbours",
                    "seeded_from_chunks": rag.context.len(),
                }),
                &extra,
            ));
        }
        rag.context.extend(extra);
    }

    // 3. Extraction-guard pre-eval against the full seed + KG-
    //    expanded view. The proximity classifier reads `rag_context`
    //    for lecture / reading kinds and maps them via `applied_in`
    //    to assignment doc ids ; running this AFTER KG expansion
    //    ensures KG-derived assignment partners enter the sliding-
    //    window `assignments_near` set. (The intent classifier is
    //    unaffected ; it reads recent user messages, not RAG.)
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

    // Should we hide the live thinking stream + sources panel for
    // this turn? We key on `flagged_this_turn` (per-turn intent OR
    // proximity signal), NOT the sticky `constraint_active`. The
    // sticky bit keeps the writeup-time output check armed across
    // benign follow-up turns (which is correct ; the student may
    // still be drifting toward an extraction), but using it for
    // *live* suppression would force every innocent question after
    // a single past paste-extract into the "[Reasoning hidden by
    // integrity guard]" placeholder + empty sources panel ; UX cost
    // and an unintended information disclosure that "this
    // conversation was flagged".
    //
    // When true: the research phase runs server-side as normal (so
    // the teacher dashboard's audit trail is preserved) but
    // suppresses the user-visible `thinking_token` / `tool_call` /
    // `tool_result` SSE stream and emits a one-shot
    // `thinking_hidden` event in its place; the frontend renders a
    // placeholder for the disclosure.
    let suppress_thinking = guard_decision
        .as_ref()
        .map(|g| g.flagged_this_turn)
        .unwrap_or(false);

    // 4. Emit the prelim retrieval records live (gated by the
    //    guard's per-turn signal). Done AFTER guard eval so we
    //    never leak chunks the retriever pulled on a guarded turn ;
    //    on a flagged turn the seed RAG is keyed off the student's
    //    pasted assignment text and the chunks may contain the
    //    assignment_brief itself or a TA-uploaded solution PDF.
    if !suppress_thinking {
        for record in &prelim_events {
            emit_server_retrieval(&tx, record).await;
        }
    }

    // 5. Research phase. Seeds the chunk accumulator with the
    //    initial partition's `context` (signals are excluded from
    //    LLM context by definition; tool calls can still surface
    //    relevant content if needed).
    let cap = per_response_token_cap(ctx.daily_token_limit);
    let catalog_flags = ToolCatalogFlags {
        kg_enabled: ctx.kg_enabled,
    };
    let config = ResearchConfig::defaults(use_logprobs);
    let mut research = research_phase::run(
        &ctx,
        config,
        catalog_flags,
        rag.context.clone(),
        cap,
        &orphaned,
        suppress_thinking,
        &tx,
    )
    .await;

    // Prepend the server-initiated retrievals (seed RAG, optional
    // KG expansion) so the persisted message and the in-progress
    // disclosure both render them at the top of the tool-event
    // list, chronologically before any model-initiated calls.
    let prelim_count = prelim_events.len();
    if !prelim_events.is_empty() {
        let mut combined = std::mem::take(&mut prelim_events);
        combined.extend(std::mem::take(&mut research.tool_events));
        research.tool_events = combined;
    }

    tracing::info!(
        "tool_use: research finished for conv {}: turns={}, tool_calls={}, flare_injections={}, stop={:?}, chunks={}",
        ctx.conversation_id,
        research.turns,
        research.tool_calls_executed,
        research.flare_injections,
        research.stop_reason,
        research.chunks.len(),
    );

    // 6. Writeup phase. Single clean streaming pass; tokens flow
    //    to the client as `{"type":"token", ...}` (same shape as
    //    the legacy strategies).
    //
    // On a guarded turn (`suppress_thinking` true) we feed the
    // writeup an EMPTY research transcript instead of
    // `research.transcript`. The transcript is the research agent's
    // bullet-point handoff, which `build_writeup_system_prompt`
    // tells the writeup model to "treat as established facts and
    // build on directly"; if the research model wrote the assignment
    // solution into those bullets (which is exactly what an
    // extraction-flagged turn invites), passing it through hands the
    // writeup model a launder-this-into-pedagogical-prose blueprint
    // and the post-generation output check is the only thing
    // standing between that and the student. Cutting the feed kills
    // the pathway at its source ; the writeup composes from the raw
    // citable chunks (still present) plus the tool-call summary
    // (metadata only, no model prose). The empty-transcript branch
    // in `build_writeup_system_prompt` already drops the
    // "Research agent findings" section cleanly when this is "".
    let writeup_transcript = if suppress_thinking {
        ""
    } else {
        &research.transcript
    };
    let writeup_output = match writeup::run(
        &ctx,
        &research.chunks,
        writeup_transcript,
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

    // 7. Post-generation extraction-guard intercept. Operates on
    //    a clean single-pass writeup (much better signal than the
    //    legacy FLARE path's multi-restart full_text). Keys off the
    //    sticky `constraint_active` (not `flagged_this_turn`), so a
    //    student drifting toward an extraction on a turn that
    //    didn't itself fire is still caught at the output stage.
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

    // 8. Surface the consolidated chunk set to the client. Mirrors
    //    the legacy strategies' sources panel ; students see every
    //    document that informed the answer, both seed-RAG and the
    //    ones the model pulled in via tool calls.
    //
    // Always built and persisted ; `messages.chunks_used` is part
    // of the teacher dashboard's audit trail and they need it even
    // on a guarded turn (to see what the retriever pulled when the
    // guard fired). Suppression for the student happens at the SSE
    // layer (finalize omits `chunks_used` from the `done` event
    // when `thinking_hidden`) and at read time in chat.rs /
    // embed.rs (owner viewers get null on GET, teachers see the
    // full set). Same shape as the thinking_transcript handling.
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

    // retrieval_count: real count of retrievals the user can see
    // in the disclosure. Mirrors `tool_events.len()` ; one row per
    // retrieval regardless of who triggered it (server seed, KG
    // expansion, model tool call, FLARE injection).
    let retrieval_count =
        (prelim_count + research.tool_calls_executed + research.flare_injections) as i32;

    // Split of the message's token total into research vs writeup.
    // Tracked separately for prompt and completion so the
    // per-message footer and the daily-usage dashboards can render
    // research / writeup as honest subsets of the prompt and
    // completion totals (writeup_prompt = `total_prompt -
    // research_prompt`, writeup_completion = `total_completion -
    // research_completion`). `i32` cap is fine: the per-message
    // token budget (200K default) is well under i32::MAX.
    let research_prompt_tokens = research.total_prompt_tokens;
    let research_completion_tokens = research.total_completion_tokens;

    common::finalize(
        &ctx,
        &tx,
        &final_text,
        chunks_json.as_ref(),
        total_prompt,
        total_completion,
        !research.chunks.is_empty() || !rag.signals.is_empty(),
        started_at.elapsed().as_millis() as i64,
        retrieval_count,
        thinking_transcript,
        tool_events_json.as_ref(),
        Some(research.duration_ms.clamp(0, i32::MAX as i64) as i32),
        Some(research_prompt_tokens),
        Some(research_completion_tokens),
        // `thinking_hidden` is the persisted record of whether the
        // guard was active for this turn; the read-time gate on
        // `get_conversation` uses it to blank the disclosure for the
        // owner even on refresh long after the SSE stream closed.
        // True iff we suppressed the live thinking stream above.
        suppress_thinking,
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
