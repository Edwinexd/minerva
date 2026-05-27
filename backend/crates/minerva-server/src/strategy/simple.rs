use axum::response::sse::Event;
use tokio::sync::mpsc;

use super::common;
use super::GenerationContext;
use crate::error::AppError;

/// Simple strategy: traditional RAG.
/// 1. Embed query
/// 2. Search Qdrant
/// 3. Build prompt with context
/// 4. Stream from Cerebras
///
/// When `ctx.tool_use_enabled` is TRUE, hands off to
/// `tool_use::run(use_logprobs = false)` instead: the model gets a
/// tool-using research phase before a clean writeup. The path
/// below runs only when tool use is disabled.
pub async fn run(ctx: GenerationContext, tx: mpsc::Sender<Result<Event, AppError>>) {
    if ctx.tool_use_enabled {
        super::tool_use::run(ctx, false, tx).await;
        return;
    }
    let started_at = std::time::Instant::now();
    let http_client = reqwest::Client::new();
    let collection_name =
        minerva_ingest::pipeline::collection_name(ctx.course_id, ctx.embedding_version);

    // Orphan filter runs first and unconditionally: an orphaned doc
    // (Moodle activity deleted, page edited, etc.) must never appear in
    // a new turn regardless of KG status. Soft-orphan instead of hard
    // delete because chat history's `chunks_used` cites doc ids we
    // still need to resolve client-side. Computed once and threaded
    // into every RAG primitive that runs this turn (rag_lookup +
    // keyword_lookup + tool dispatch); see `common::rag_lookup`.
    let orphaned = minerva_db::queries::documents::orphaned_doc_ids(&ctx.db, ctx.course_id)
        .await
        .unwrap_or_default();

    // RAG lookup (blocks before streaming starts). Orphaned chunks
    // are filtered inside.
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
        &orphaned,
    )
    .await;

    // Kind-aware partition: assignment_brief / lab_brief / exam matches
    // become `signals` (the model gets a refusal addendum but never the
    // chunk text); sample_solution leftovers are dropped defensively;
    // unclassified docs are held back for this turn. All gated on
    // `kg_enabled`; KG-disabled courses bypass the partition and the
    // adversarial filter entirely.
    let unclassified = if ctx.kg_enabled {
        minerva_db::queries::documents::unclassified_doc_ids(&ctx.db, ctx.course_id)
            .await
            .unwrap_or_default()
    } else {
        std::collections::HashSet::new()
    };
    let mut rag = common::partition_chunks(raw_chunks, &unclassified, ctx.kg_enabled);

    // Adversarial pre-retrieval check: drop any per-chunk worked
    // solutions that slipped through the doc-level classifier.
    // Fails open on timeout (see classification::adversarial).
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

    // Graph-aware enrichment: same logic as parallel.rs; pull
    // representative chunks from each top hit's KG partners so
    // the model sees siblings (part_of_unit) and applied
    // exercises (applied_in dst) the embedding search would
    // otherwise miss.
    if ctx.kg_enabled {
        let collection_name =
            minerva_ingest::pipeline::collection_name(ctx.course_id, ctx.embedding_version);
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
        rag.context.extend(extra);
    }

    // Extraction guard evaluation: runs intent classifier + multi-
    // turn proximity check, decides whether this turn's generation
    // needs post-output interception. None when the feature flag
    // is off; Some(_) otherwise. Doesn't gate generation; we
    // always stream, then maybe intercept at the end.
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

    // Per-turn extraction signal. The simple strategy doesn't have
    // a thinking stream to suppress (no research phase emits), but
    // `chunks_used` still needs gating on flagged turns: the seed
    // RAG is keyed off the student's pasted assignment text and
    // may include the assignment_brief itself or a TA-uploaded
    // solution PDF. Without this, the SSE `done` event ships those
    // chunks live (common::finalize gate keys off `thinking_hidden`)
    // and the persisted column stays false so the read-time gate
    // in chat.rs / embed.rs serves them on refresh too. Tracks
    // `flagged_this_turn` (per-turn intent OR proximity), NOT the
    // sticky `constraint_active`, to avoid blanking sources on
    // every benign follow-up after a single past trip.
    let suppress_thinking = guard_decision
        .as_ref()
        .map(|g| g.flagged_this_turn)
        .unwrap_or(false);

    let hidden = minerva_db::queries::documents::hidden_document_ids(&ctx.db, ctx.course_id)
        .await
        .unwrap_or_default();
    // Sources surfaced to the client include both context + signals; a
    // student should see *that* an assignment matched even though its
    // text is withheld from the model.
    let displayed = rag.all();
    let client_chunks = common::chunks_for_client(&displayed, &hidden);
    let chunks_json = serde_json::to_value(&client_chunks).ok();

    let system = common::build_system_prompt_with_signals(
        &ctx.course_name,
        &ctx.custom_prompt,
        &rag.context,
        &rag.signals,
    );
    let messages = common::build_chat_messages(&system, &ctx.history);

    let mut full_text = String::new();
    let (prompt_tokens, completion_tokens) = match common::stream_cerebras_to_client(
        &http_client,
        &ctx.cerebras_api_key,
        &ctx.model,
        ctx.temperature,
        &messages,
        &tx,
        &mut full_text,
    )
    .await
    {
        Ok(usage) => usage,
        Err(e) => {
            let _ = tx
                .send(Ok(Event::default().data(
                    serde_json::json!({"type": "error", "error": e}).to_string(),
                )))
                .await;
            return;
        }
    };

    // Post-generation extraction-guard intercept: when the
    // constraint is active for this turn, run the output-side
    // solution check. Trips -> Socratic rewrite, conversation_flag
    // logged, `rewrite` SSE event sent so the frontend swaps the
    // streamed message. Returns the text that should land in the
    // DB (original or rewrite).
    let final_text = super::extraction_guard::intercept_reply(
        &ctx.db,
        &http_client,
        &ctx.cerebras_api_key,
        ctx.course_id,
        ctx.conversation_id,
        &guard_decision,
        &ctx.user_content,
        &full_text,
        &tx,
    )
    .await;

    common::finalize(
        &ctx,
        &tx,
        &final_text,
        chunks_json.as_ref(),
        prompt_tokens,
        completion_tokens,
        true,
        started_at.elapsed().as_millis() as i64,
        1,
        // Legacy single-pass path; no research transcript, no tool
        // events, no thinking duration, no research-phase token
        // split (both research_prompt and research_completion are
        // None, which the daily aggregate treats as 0).
        None,
        None,
        None,
        None,
        None,
        // No live thinking stream to gate on this path, but the
        // SSE `done` event still ships `chunks_used` and the
        // persisted column drives the read-time owner-suppression
        // gate in chat.rs / embed.rs. On a flagged turn the seed
        // RAG can include the assignment_brief or a TA-uploaded
        // solution PDF, so we mirror the tool_use path's gate:
        // persist thinking_hidden=true when the guard fired
        // per-turn, blanking `chunks_used` in both the SSE done
        // event (via common::finalize) and on GET (via the read-
        // time gate).
        suppress_thinking,
    )
    .await;
}
