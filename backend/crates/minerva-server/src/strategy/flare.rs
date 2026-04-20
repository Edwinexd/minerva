use axum::response::sse::Event;
use futures::StreamExt;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use super::common;
use super::common::RagChunk;
use super::GenerationContext;
use crate::error::AppError;

/// Log-probability threshold below which a token is considered "low confidence".
/// -2.0 corresponds to roughly <13% probability. The FLARE paper uses token
/// probability thresholds; we use logprobs since that's what Cerebras returns.
const LOGPROB_THRESHOLD: f64 = -2.0;

/// Qdrant similarity threshold for FLARE retrieval results.
const SIMILARITY_THRESHOLD: f32 = 0.35;

/// Maximum number of retrieval-triggered restarts to prevent infinite loops.
const MAX_FLARE_RESTARTS: usize = 5;

/// Hard cap on outer-loop iterations, independent of whether retrieval added
/// new chunks. Guards against pathological cases where the model gets stuck
/// regenerating the same high-confidence content across continuation windows
/// (no low-confidence sentence ever fires, so MAX_FLARE_RESTARTS never trips).
/// At FLARE_MAX_TOKENS_PER_CHUNK tokens per iteration this bounds a single
/// FLARE response to at most MAX_FLARE_ITERATIONS * FLARE_MAX_TOKENS_PER_CHUNK
/// completion tokens.
const MAX_FLARE_ITERATIONS: usize = 8;

/// Max completion tokens per FLARE generation window.
/// Bounding each call ensures streams always terminate naturally so Cerebras
/// returns usage stats in the final [DONE] chunk. Token counts are then exact
/// additions across windows rather than estimates from dropped connections.
/// 1536 = 1024 + 512; wide enough that a structured answer (table + explanation)
/// usually finishes in one window, reducing continuation overhead.
const FLARE_MAX_TOKENS_PER_CHUNK: i32 = 1536;

/// Defence-in-depth cap on accumulated response size. Nominal ceiling with
/// MAX_FLARE_ITERATIONS * FLARE_MAX_TOKENS_PER_CHUNK * ~4 bytes/token is
/// ~49 KB; this leaves 2x headroom and prevents runaway memory growth if
/// constants are retuned or a model produces unusually byte-dense output.
const MAX_FLARE_FULL_TEXT_BYTES: usize = 100_000;

/// Idle timeout between consecutive SSE frames from Cerebras. Protects
/// against a silently-stalled TCP connection that never delivers [DONE].
/// Applied per `stream.next().await`, not as a total-request deadline.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Length (in chars) of a literal-text window that, if it appears verbatim in
/// already-generated content, signals the model has looped back and is
/// regenerating prior output. 150 chars of exact overlap is essentially
/// impossible by chance and tolerates short legitimate back-references
/// ("as mentioned above...") which are always much shorter.
const REPEAT_FINGERPRINT_LEN: usize = 150;

/// How often (in chars of new content) to run the streaming repeat check.
/// Trade-off: smaller = catches repeats sooner (less visible duplication to
/// the user) at cost of more `str::contains` scans against `full_text`.
/// 60 chars ≈ every ~15 tokens.
const REPEAT_CHECK_INTERVAL: usize = 60;

/// Fallback single-response token cap when a course has `daily_token_limit = 0`
/// (unlimited per student). Even "unlimited" courses shouldn't allow one
/// answer to burn six-figure token counts; this is a backstop.
const UNLIMITED_COURSE_RESPONSE_CAP: i64 = 200_000;

/// Multiplier applied to `courses.daily_token_limit` to derive the per-response
/// fail-safe cap. At 2x a student cannot burn more than two days of their
/// daily allowance in a single answer, even if daily-limit enforcement hasn't
/// kicked in yet (that check runs at request start; intra-response drift is
/// what this cap guards).
const DAILY_LIMIT_RESPONSE_MULTIPLIER: i64 = 2;

/// FLARE strategy: Forward-Looking Active REtrieval augmented generation.
///
/// Uses Cerebras logprobs to detect low-confidence tokens. When the model
/// generates a sentence containing uncertain tokens, that sentence is used
/// as a retrieval query. If relevant chunks are found, generation restarts
/// from that point with enriched context.
pub async fn run(ctx: GenerationContext, tx: mpsc::Sender<Result<Event, AppError>>) {
    let started_at = std::time::Instant::now();
    let http_client = reqwest::Client::new();
    let collection_name = format!("course_{}", ctx.course_id);

    // Initial retrieval using user's question (per the paper)
    let initial_chunks = common::rag_lookup(
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

    let mut all_chunks: Vec<RagChunk> = initial_chunks;
    // Parallel hash set for O(1) dedup when merging mid-stream retrievals.
    // Cheaper than `Vec::contains` which does full-text equality on every chunk.
    let mut chunk_hashes: HashSet<u64> = all_chunks.iter().map(chunk_identity_hash).collect();
    let mut full_text = String::new();
    let mut total_prompt_tokens = 0i32;
    let mut total_completion_tokens = 0i32;
    let mut restarts = 0usize;
    let mut iterations = 0usize;

    // Per-response token fail-safe. If the course has `daily_token_limit = 0`
    // (unlimited), fall back to `UNLIMITED_COURSE_RESPONSE_CAP`; otherwise cap
    // at DAILY_LIMIT_RESPONSE_MULTIPLIER * course limit. A single answer
    // cannot burn more than this many total tokens (prompt + completion
    // across all FLARE iterations).
    let per_response_token_cap: i64 = if ctx.daily_token_limit > 0 {
        ctx.daily_token_limit
            .saturating_mul(DAILY_LIMIT_RESPONSE_MULTIPLIER)
    } else {
        UNLIMITED_COURSE_RESPONSE_CAP
    };

    tracing::info!(
        "flare: starting with {} initial chunks for conversation {} (per_response_cap={})",
        all_chunks.len(),
        ctx.conversation_id,
        per_response_token_cap,
    );

    loop {
        let tokens_so_far =
            (total_prompt_tokens as i64).saturating_add(total_completion_tokens as i64);
        if tokens_so_far >= per_response_token_cap {
            tracing::warn!(
                "flare: per-response token cap hit ({} >= {}, daily_limit={}), stopping at iteration {}",
                tokens_so_far,
                per_response_token_cap,
                ctx.daily_token_limit,
                iterations
            );
            break;
        }
        if iterations >= MAX_FLARE_ITERATIONS {
            tracing::warn!(
                "flare: hit hard iteration cap ({}), stopping (restarts={}, full_text_len={})",
                MAX_FLARE_ITERATIONS,
                restarts,
                full_text.len()
            );
            break;
        }
        if full_text.len() > MAX_FLARE_FULL_TEXT_BYTES {
            tracing::warn!(
                "flare: full_text exceeded byte cap ({} > {}), stopping (iteration={})",
                full_text.len(),
                MAX_FLARE_FULL_TEXT_BYTES,
                iterations
            );
            break;
        }
        iterations += 1;
        let iteration_start_text_len = full_text.len();

        let system = common::build_system_prompt(&ctx.course_name, &ctx.custom_prompt, &all_chunks);
        let mut messages = common::build_chat_messages(&system, &ctx.history);

        if !full_text.is_empty() {
            // Continuation framing. Context-supply is the hard problem here:
            // Cerebras doesn't expose OpenAI's `prefix: true` field (see
            // commit e17781b), so a bare trailing assistant message is
            // ambiguous. Empirically the model sometimes treats it as a
            // completed turn and generates a fresh response (visible to the
            // user as repeating headings / section restarts). To
            // disambiguate, we follow the partial with an explicit user-role
            // directive that says "continue the assistant turn above". This
            // gives the model an unambiguous "your turn is to continue what
            // I already see", rather than inferring intent from a trailing
            // assistant message alone.
            if let Some(sys_msg) = messages.first_mut() {
                if let Some(content) = sys_msg.get("content").and_then(|c| c.as_str()) {
                    let new_content = format!(
                        "{}\n\n## Continuation mode\n\
                        You previously began writing a response and were interrupted \
                        mid-output so additional course materials could be retrieved. \
                        Those materials now appear above. The next message labelled \
                        `assistant` contains your partial response verbatim. Your task \
                        is to RESUME that exact response from the character after its \
                        final character, as if the interruption never happened. \
                        Rules:\n\
                        - Do NOT restart, summarise, or repeat any heading, list item, \
                        table, code block, or sentence that already appears in the \
                        partial response.\n\
                        - Do NOT acknowledge the interruption or the retrieval; the \
                        student will not see that machinery.\n\
                        - Pick up inside whatever construct was in progress (mid-list, \
                        mid-table, mid-paragraph). If the partial ended inside an \
                        unclosed code fence or table row, close it properly before \
                        adding new content.",
                        content,
                    );
                    sys_msg["content"] = serde_json::Value::String(new_content);
                }
            }
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": full_text,
            }));
            // Explicit user directive disambiguates the trailing assistant
            // message for providers (like Cerebras) that lack a native
            // prefill/prefix flag. Empirically, without this the model
            // restarts the response and we see duplicated headings.
            messages.push(serde_json::json!({
                "role": "user",
                "content":
                    "Continue the assistant response above from exactly where it was \
                    cut off. Do NOT start with a new heading; do NOT repeat content \
                    already written; do NOT summarise what came before. Your very \
                    next token should be whatever naturally follows the last \
                    character of the assistant message.",
            }));
        }

        // Stream with logprobs, checking confidence per sentence
        let result = stream_with_logprobs(
            &http_client,
            &ctx.cerebras_api_key,
            &ctx.model,
            ctx.temperature,
            &messages,
            &tx,
            &mut full_text,
        )
        .await;

        let outcome = match result {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("flare: stream error: {}", e);
                let _ = tx
                    .send(Ok(Event::default().data(
                        serde_json::json!({"type": "error", "error": e}).to_string(),
                    )))
                    .await;
                break;
            }
        };

        total_prompt_tokens += outcome.prompt_tokens;
        total_completion_tokens += outcome.completion_tokens;

        // Post-iteration exact-heading repeat check. Cheap and very
        // precise: if the same heading (after whitespace/punctuation
        // normalization) appears in both the prior content and this
        // iteration's output, the model restarted a section. This is a
        // complement to streaming-time literal detection which catches
        // 150-char prose overlaps. No fuzzy matching -- paraphrased
        // headings are NOT flagged here; the primary defence against
        // paraphrased restarts is the revised continuation prompt.
        if iteration_start_text_len > 0 && full_text.len() > iteration_start_text_len {
            let prior = &full_text[..iteration_start_text_len];
            let new_content = &full_text[iteration_start_text_len..];
            if let Some(collision) = detect_exact_heading_repeat(prior, new_content) {
                tracing::warn!(
                    "flare: exact heading repeat detected (iteration {}): {:?}",
                    iterations,
                    truncate_for_log(&collision, 80)
                );
                break;
            }
        }

        match outcome.kind {
            StreamOutcome::Completed => break,
            StreamOutcome::RepeatDetected => {
                tracing::warn!(
                    "flare: aborting outer loop due to detected content repeat (iteration {})",
                    iterations
                );
                break;
            }
            StreamOutcome::HitLimit {
                low_confidence_sentence: None,
            } => {
                // Hit the token window but all sentences were confident; keep generating.
                tracing::debug!(
                    "flare: hit token limit with no low-confidence sentence, continuing"
                );
                continue;
            }
            StreamOutcome::HitLimit {
                low_confidence_sentence: Some(ref sentence),
            } => {
                if restarts >= MAX_FLARE_RESTARTS {
                    tracing::info!(
                        "flare: max restarts ({}) reached, stopping",
                        MAX_FLARE_RESTARTS
                    );
                    break;
                }

                tracing::info!(
                    "flare: low-confidence sentence detected, retrieving (restart {}/{})",
                    restarts + 1,
                    MAX_FLARE_RESTARTS
                );

                // Use the low-confidence sentence as a retrieval query.
                // Mid-stream FLARE retrieval keeps its own paper-derived floor
                // (SIMILARITY_THRESHOLD) but tightens to the course's configured
                // min_score when the teacher has set a stricter value.
                let flare_threshold = SIMILARITY_THRESHOLD.max(ctx.min_score);
                let new_chunks = flare_retrieve(
                    &http_client,
                    &ctx.openai_api_key,
                    &ctx.fastembed,
                    &ctx.qdrant,
                    &collection_name,
                    sentence,
                    ctx.max_chunks,
                    flare_threshold,
                    &ctx.embedding_provider,
                    &ctx.embedding_model,
                )
                .await;

                let mut added = false;
                for chunk in &new_chunks {
                    if all_chunks.len() >= ctx.max_chunks as usize {
                        break;
                    }
                    if chunk_hashes.insert(chunk_identity_hash(chunk)) {
                        all_chunks.push(chunk.clone());
                        added = true;
                    }
                }

                if added {
                    restarts += 1;
                    tracing::info!(
                        "flare: added new chunks, total now {}. Restarting from {} chars.",
                        all_chunks.len(),
                        full_text.len()
                    );
                } else {
                    tracing::debug!("flare: no new chunks found, continuing without restart");
                }
                continue;
            }
        }
    }

    let hidden = minerva_db::queries::documents::hidden_document_ids(&ctx.db, ctx.course_id)
        .await
        .unwrap_or_default();
    let chunks_json = if all_chunks.is_empty() {
        None
    } else {
        let client_chunks = common::chunks_for_client(&all_chunks, &hidden);
        serde_json::to_value(&client_chunks).ok()
    };

    common::finalize(
        &ctx,
        &tx,
        &full_text,
        chunks_json.as_ref(),
        total_prompt_tokens,
        total_completion_tokens,
        !all_chunks.is_empty(),
        started_at.elapsed().as_millis() as i64,
        1 + restarts as i32,
    )
    .await;
}

struct StreamWithLogprobsResult {
    prompt_tokens: i32,
    completion_tokens: i32,
    kind: StreamOutcome,
}

enum StreamOutcome {
    /// Model finished naturally (finish_reason: stop)
    Completed,
    /// Stream hit the FLARE_MAX_TOKENS_PER_CHUNK window limit.
    /// Contains the first low-confidence sentence seen during the window, if any.
    HitLimit {
        low_confidence_sentence: Option<String>,
    },
    /// Detected that the model is regenerating content that already appears
    /// earlier in `full_text`. Streaming was stopped early; the caller should
    /// break the outer loop rather than try to continue.
    RepeatDetected,
}

/// Stream from Cerebras with logprobs enabled, bounded by FLARE_MAX_TOKENS_PER_CHUNK.
///
/// The token cap ensures every call terminates naturally so Cerebras always
/// returns usage stats in the [DONE] chunk. The caller adds them up directly.
///
/// Buffers tokens into sentences and records the first low-confidence sentence
/// (if any) for use as a retrieval query by the caller.
async fn stream_with_logprobs(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    temperature: f64,
    messages: &[serde_json::Value],
    tx: &mpsc::Sender<Result<Event, AppError>>,
    full_text: &mut String,
) -> Result<StreamWithLogprobsResult, String> {
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "stream": true,
        "logprobs": true,
        "top_logprobs": 1,
        "max_tokens": FLARE_MAX_TOKENS_PER_CHUNK,
        "stream_options": { "include_usage": true },
    });

    let response = common::cerebras_request_with_retry(client, api_key, &body).await?;

    let mut stream = response.bytes_stream();
    // Raw TCP frames may split multi-byte UTF-8 codepoints; accumulate bytes
    // and only promote validated prefixes to the line buffer.
    let mut byte_carry: Vec<u8> = Vec::new();
    let mut sse_buffer = String::new();
    let mut sentence_buffer = String::new();
    let mut sentence_has_low_confidence = false;
    let mut first_low_confidence_sentence: Option<String> = None;
    let mut finish_reason: Option<String> = None; // secondary hint; token count is primary
    let mut prompt_tokens = 0i32;
    let mut completion_tokens = 0i32;

    // Streaming-time repeat detection state. `iteration_start_len` is the byte
    // offset in `full_text` at which this stream began; anything at that
    // offset or beyond is this iteration's output. We periodically check
    // whether new output contains a long literal substring that also appears
    // in the prior content (anything before `iteration_start_len`); if so the
    // model is regenerating and we abort early.
    let iteration_start_len = full_text.len();
    let mut chars_since_last_check: usize = 0;

    loop {
        let next = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
            Ok(n) => n,
            Err(_) => {
                return Err(format!(
                    "Cerebras stream idle timeout ({}s)",
                    STREAM_IDLE_TIMEOUT.as_secs()
                ));
            }
        };
        let chunk = match next {
            Some(Ok(c)) => c,
            Some(Err(e)) => return Err(format!("Stream error: {}", e)),
            None => break, // stream closed without [DONE]
        };
        byte_carry.extend_from_slice(&chunk);
        let valid_up_to = match std::str::from_utf8(&byte_carry) {
            Ok(_) => byte_carry.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            // Safe: validated above.
            let valid_str = std::str::from_utf8(&byte_carry[..valid_up_to])
                .expect("prefix was UTF-8 validated");
            sse_buffer.push_str(valid_str);
            byte_carry.drain(..valid_up_to);
        }

        while let Some(line_end) = sse_buffer.find('\n') {
            let line = sse_buffer[..line_end].trim().to_string();
            sse_buffer.drain(..=line_end);

            if line == "data: [DONE]" {
                // Detect window exhaustion by token count: more reliable than
                // parsing finish_reason strings which vary across API versions.
                let hit_limit = completion_tokens >= FLARE_MAX_TOKENS_PER_CHUNK
                    || finish_reason.as_deref() == Some("length");
                let kind = if hit_limit {
                    // Window-end flush: if no sentence boundary fired during this
                    // window (e.g. unclosed code fence, long markdown table) but
                    // the trailing unclosed sentence was low-confidence, still
                    // use it as a retrieval query. Otherwise the iteration is
                    // wasted on "hit limit, no query, continue" with no progress.
                    let sentence = first_low_confidence_sentence.or_else(|| {
                        if sentence_has_low_confidence && !sentence_buffer.is_empty() {
                            Some(sentence_buffer.clone())
                        } else {
                            None
                        }
                    });
                    StreamOutcome::HitLimit {
                        low_confidence_sentence: sentence,
                    }
                } else {
                    StreamOutcome::Completed
                };
                return Ok(StreamWithLogprobsResult {
                    prompt_tokens,
                    completion_tokens,
                    kind,
                });
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(err) = parsed.get("error") {
                        let msg = err["message"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string();
                        return Err(msg);
                    }

                    if let Some(choice) = parsed["choices"].get(0) {
                        // Track finish_reason from the final data chunk
                        if let Some(fr) = choice["finish_reason"].as_str() {
                            finish_reason = Some(fr.to_string());
                        }

                        if let Some(delta_content) = choice["delta"]["content"].as_str() {
                            full_text.push_str(delta_content);
                            sentence_buffer.push_str(delta_content);
                            chars_since_last_check += delta_content.len();

                            // Stream to client immediately
                            if tx
                                .send(Ok(Event::default().data(
                                    serde_json::json!({"type": "token", "token": delta_content})
                                        .to_string(),
                                )))
                                .await
                                .is_err()
                            {
                                return Err("client disconnected".to_string());
                            }

                            // Periodic repeat check: if recent new content
                            // duplicates text that was in full_text before
                            // this iteration started, the model has looped
                            // back and is regenerating. Abort immediately.
                            if chars_since_last_check >= REPEAT_CHECK_INTERVAL
                                && iteration_start_len > 0
                                && full_text.len() > iteration_start_len + REPEAT_FINGERPRINT_LEN
                            {
                                chars_since_last_check = 0;
                                let prior = &full_text[..iteration_start_len];
                                let new_content = &full_text[iteration_start_len..];
                                if detect_content_repeat(prior, new_content).is_some() {
                                    tracing::warn!(
                                        "flare: repeat detected during stream (new_content_len={}), aborting",
                                        new_content.len()
                                    );
                                    return Ok(StreamWithLogprobsResult {
                                        prompt_tokens,
                                        completion_tokens,
                                        kind: StreamOutcome::RepeatDetected,
                                    });
                                }
                            }

                            // Check logprob for this token
                            if let Some(logprobs) = choice.get("logprobs") {
                                if let Some(content_logprobs) = logprobs["content"].as_array() {
                                    for lp in content_logprobs {
                                        if let Some(logprob) = lp["logprob"].as_f64() {
                                            if logprob < LOGPROB_THRESHOLD {
                                                sentence_has_low_confidence = true;
                                            }
                                        }
                                    }
                                }
                            }

                            // Check for sentence boundary
                            if is_sentence_boundary(&sentence_buffer) {
                                if sentence_has_low_confidence
                                    && first_low_confidence_sentence.is_none()
                                {
                                    tracing::debug!(
                                        "flare: low-confidence sentence: {:?}",
                                        truncate_for_log(&sentence_buffer, 80)
                                    );
                                    first_low_confidence_sentence = Some(sentence_buffer.clone());
                                }
                                sentence_buffer.clear();
                                sentence_has_low_confidence = false;
                            }
                        }
                    }

                    // Extract usage from final chunk
                    if let Some(usage) = parsed.get("usage") {
                        if !usage.is_null() {
                            prompt_tokens = usage["prompt_tokens"].as_i64().unwrap_or(0) as i32;
                            completion_tokens =
                                usage["completion_tokens"].as_i64().unwrap_or(0) as i32;
                        }
                    }
                }
            }
        }
    }

    Ok(StreamWithLogprobsResult {
        prompt_tokens,
        completion_tokens,
        kind: StreamOutcome::Completed,
    })
}

/// Check for a sentence/paragraph boundary in the buffer.
/// Avoids triggering inside markdown tables, code blocks, or lists.
fn is_sentence_boundary(text: &str) -> bool {
    if text.len() < 100 {
        return false;
    }

    // Don't trigger inside markdown tables (lines starting with |)
    if let Some(last_line) = text.lines().last() {
        let trimmed_line = last_line.trim();
        if trimmed_line.starts_with('|') || trimmed_line.starts_with("|-") {
            return false;
        }
    }

    // Don't trigger inside code blocks
    let fence_count = text.matches("```").count();
    if !fence_count.is_multiple_of(2) {
        return false; // Inside an unclosed code block
    }

    // Don't trigger inside markdown lists (last line starts with - or *)
    if let Some(last_line) = text.lines().last() {
        let trimmed_line = last_line.trim();
        if trimmed_line.starts_with("- ") || trimmed_line.starts_with("* ") {
            return false;
        }
    }

    // Paragraph break is always a good boundary
    if text.ends_with("\n\n") {
        return true;
    }

    // For long buffers, check if we ended a sentence cleanly
    if text.len() > 300 {
        let trimmed = text.trim_end();
        let last_char = trimmed.chars().last().unwrap_or(' ');
        if last_char == '.' || last_char == '?' || last_char == '!' {
            // Make sure we're not inside a table row or list
            if let Some(last_line) = trimmed.lines().last() {
                let ll = last_line.trim();
                if !ll.starts_with('|') && !ll.starts_with("- ") && !ll.starts_with("* ") {
                    return true;
                }
            }
        }
    }

    false
}

/// Search Qdrant with a similarity threshold for FLARE retrieval.
#[allow(clippy::too_many_arguments)]
async fn flare_retrieve(
    client: &reqwest::Client,
    openai_key: &str,
    fastembed: &Arc<minerva_ingest::fastembed_embedder::FastEmbedder>,
    qdrant: &Arc<qdrant_client::Qdrant>,
    collection_name: &str,
    query: &str,
    max_chunks: i32,
    score_threshold: f32,
    embedding_provider: &str,
    embedding_model: &str,
) -> Vec<RagChunk> {
    match common::embedding_search(
        client,
        openai_key,
        fastembed,
        qdrant,
        collection_name,
        query,
        max_chunks as u64,
        Some(score_threshold),
        embedding_provider,
        embedding_model,
    )
    .await
    {
        Ok(points) => points
            .iter()
            .filter_map(|point| {
                let chunk = common::scored_point_to_rag_chunk(point)?;
                tracing::debug!(
                    "flare: retrieved chunk from '{}' score {:.3}",
                    chunk.filename,
                    point.score
                );
                Some(chunk)
            })
            .collect(),
        Err(e) => {
            tracing::warn!("flare: {}", e);
            Vec::new()
        }
    }
}

fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Compute a stable identity hash for a chunk so we can dedupe across
/// mid-stream retrievals in O(1) instead of `Vec::contains` (O(n) with full
/// text equality). (document_id, text) uniquely identifies a chunk within
/// a course since the same document is split into non-overlapping texts.
fn chunk_identity_hash(c: &RagChunk) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    c.document_id.hash(&mut h);
    c.text.hash(&mut h);
    h.finish()
}

/// Extract markdown headings from `text`, normalized for comparison.
/// Returned strings are lowercased, punctuation-stripped, and whitespace-
/// collapsed. Lines that look like headings (leading `#`, or bold-wrapped
/// standalone lines like `**Title**`) are included.
fn extract_normalized_headings(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let title = if let Some(stripped) = trimmed.strip_prefix('#') {
                // Strip any further # chars and whitespace
                stripped.trim_start_matches('#').trim().to_string()
            } else if trimmed.starts_with("**") && trimmed.ends_with("**") && trimmed.len() > 4 {
                // Bold-wrapped standalone line, often used as pseudo-heading
                trimmed[2..trimmed.len() - 2].trim().to_string()
            } else {
                return None;
            };
            if title.is_empty() {
                return None;
            }
            let normalized: String = title
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { ' ' })
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if normalized.is_empty() {
                None
            } else {
                Some(normalized)
            }
        })
        .collect()
}

/// Detect exact (post-normalization) heading repeats between `prior` and
/// `new_content`. Returns the offending heading if found, else None.
///
/// Exact-match only: avoids the false-positive risk of fuzzy/paraphrase
/// matching, which would otherwise cut off legitimate responses with
/// similar-but-distinct section titles ("Build Process" vs "Build System").
/// The primary defence against paraphrased restarts is the revised
/// continuation prompt, not this detector.
fn detect_exact_heading_repeat(prior: &str, new_content: &str) -> Option<String> {
    let prior_headings: HashSet<String> = extract_normalized_headings(prior).into_iter().collect();
    if prior_headings.is_empty() {
        return None;
    }
    for h in extract_normalized_headings(new_content) {
        // Require at least 2 words so trivial "Summary" / "Overview" / "Notes"
        // pseudo-headings don't falsely trigger.
        if h.split_whitespace().count() < 2 {
            continue;
        }
        if prior_headings.contains(&h) {
            return Some(h);
        }
    }
    None
}

/// Detect whether the tail of `new_content` substantially duplicates text
/// that already appears in `prior`.
///
/// Checks the last `REPEAT_FINGERPRINT_LEN` bytes of `new_content` against
/// `prior` using `str::contains`. Since the streaming loop calls this every
/// `REPEAT_CHECK_INTERVAL` characters of new content, any 150-char repeat
/// will naturally appear in the tail window at one of those checks. The
/// post-iteration caller sees the completed stream; if an early-iteration
/// repeat slipped past the streaming check, the paired
/// `detect_exact_heading_repeat` catches section restarts.
///
/// At 150 chars of exact overlap false positives are negligible: legitimate
/// back-references ("as mentioned above") are far shorter, and the model
/// would have to coincidentally regenerate a 150-char sequence byte-for-byte.
fn detect_content_repeat(prior: &str, new_content: &str) -> Option<usize> {
    if prior.is_empty() || new_content.len() < REPEAT_FINGERPRINT_LEN {
        return None;
    }
    // Tail window: last REPEAT_FINGERPRINT_LEN bytes, rolled back to a char
    // boundary so we never slice into a multi-byte codepoint.
    let mut start = new_content.len() - REPEAT_FINGERPRINT_LEN;
    while start > 0 && !new_content.is_char_boundary(start) {
        start -= 1;
    }
    let fp = &new_content[start..];
    let trimmed = fp.trim();
    // Reject windows that are mostly whitespace / markdown syntax ("###",
    // "---", "|---|") to avoid trivially matching prior boilerplate.
    if trimmed.len() < REPEAT_FINGERPRINT_LEN / 2 {
        return None;
    }
    if prior.contains(trimmed) {
        Some(start)
    } else {
        None
    }
}

// ── Unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_chunk(doc: &str, name: &str, text: &str) -> RagChunk {
        RagChunk {
            document_id: doc.to_string(),
            filename: name.to_string(),
            text: text.to_string(),
        }
    }

    // ── chunk_identity_hash ────────────────────────────────────

    #[test]
    fn chunk_hash_same_content_same_hash() {
        let a = mk_chunk("doc1", "foo.pdf", "Hello world");
        let b = mk_chunk("doc1", "foo.pdf", "Hello world");
        assert_eq!(chunk_identity_hash(&a), chunk_identity_hash(&b));
    }

    #[test]
    fn chunk_hash_different_doc_different_hash() {
        let a = mk_chunk("doc1", "foo.pdf", "Hello world");
        let b = mk_chunk("doc2", "foo.pdf", "Hello world");
        assert_ne!(chunk_identity_hash(&a), chunk_identity_hash(&b));
    }

    #[test]
    fn chunk_hash_different_text_different_hash() {
        let a = mk_chunk("doc1", "foo.pdf", "Hello world");
        let b = mk_chunk("doc1", "foo.pdf", "Hello earth");
        assert_ne!(chunk_identity_hash(&a), chunk_identity_hash(&b));
    }

    #[test]
    fn chunk_hash_filename_does_not_matter() {
        // Filename is metadata only; identity is (doc_id, text).
        let a = mk_chunk("doc1", "foo.pdf", "Hello world");
        let b = mk_chunk("doc1", "renamed.pdf", "Hello world");
        assert_eq!(chunk_identity_hash(&a), chunk_identity_hash(&b));
    }

    // ── detect_content_repeat ──────────────────────────────────

    #[test]
    fn content_repeat_empty_prior_none() {
        let r = detect_content_repeat("", &"abc".repeat(100));
        assert!(r.is_none());
    }

    #[test]
    fn content_repeat_too_short_none() {
        // new_content shorter than REPEAT_FINGERPRINT_LEN
        let prior = "The course uses a single, two-module Maven project for practical work.";
        assert!(detect_content_repeat(prior, "short new").is_none());
    }

    #[test]
    fn content_repeat_exact_match_detected() {
        // The detector checks only the TAIL of new_content, because the
        // streaming loop calls it every REPEAT_CHECK_INTERVAL chars, so a
        // repeat will be in the tail window at some point. We simulate that
        // by making the new_content END with the repeated phrase.
        let phrase = "The course uses a single, two-module Maven project as the practical work for the whole term, which is unusual for introductory programming classes but makes module-system concepts concrete early.";
        assert!(
            phrase.len() > REPEAT_FINGERPRINT_LEN,
            "test phrase too short: {} <= {}",
            phrase.len(),
            REPEAT_FINGERPRINT_LEN
        );
        let prior = format!("Introduction section here. {} End of intro.", phrase);
        let new = format!("Continuing from earlier... {}", phrase);
        assert!(detect_content_repeat(&prior, &new).is_some());
    }

    #[test]
    fn content_repeat_head_only_not_detected_until_reaches_tail() {
        // If the repeat is at the HEAD of new_content but the content has
        // kept growing past it, the tail no longer contains the repeat and
        // we return None. This is expected: the streaming loop would have
        // caught this at an earlier check when the repeat WAS the tail.
        let repeat = "The course uses a single, two-module Maven project as the practical work for the whole term, which is unusual for introductory programming classes but reinforces module-system concepts concretely.";
        assert!(
            repeat.len() > REPEAT_FINGERPRINT_LEN,
            "repeat len {}",
            repeat.len()
        );
        let prior = format!("Intro. {}", repeat);
        // new_content has the repeat at the head, then ~800 chars of fresh
        // distinct text. The tail 150-byte window doesn't overlap the repeat.
        let fresh_tail: String = "fresh unrelated sentence number one hundred. ".repeat(20);
        let new = format!("{} {}", repeat, fresh_tail);
        assert!(detect_content_repeat(&prior, &new).is_none());
    }

    #[test]
    fn content_repeat_catches_growth_that_becomes_tail() {
        // Simulate the streaming pattern: we start a fresh stream, at each
        // check the detector sees a slightly longer `new_content`. The
        // detector should fire the moment the duplicate is in the tail.
        let repeat = "The course uses a single, two-module Maven project as the practical work for the whole term, which is unusual for introductory programming classes but reinforces module-system concepts concretely.";
        assert!(
            repeat.len() > REPEAT_FINGERPRINT_LEN,
            "repeat len {}",
            repeat.len()
        );
        let prior = format!("Intro. {}", repeat);
        // At the point where `new_content` ends with the repeat phrase,
        // the tail contains it, so detection should fire.
        let new = format!("some garbage text prefix here {}", repeat);
        assert!(detect_content_repeat(&prior, &new).is_some());
    }

    #[test]
    fn content_repeat_handles_multibyte_tail_boundary() {
        // Ensure that when the tail byte offset falls inside a multi-byte
        // codepoint, we roll back to a char boundary without panicking.
        // Swedish characters å/ä/ö are 2 bytes each in UTF-8.
        let phrase = "Kursen använder ett enda, tvåmodulers Maven-projekt som praktiskt arbete under terminen, vilket är ovanligt för grundkurser men gör modulsystem-begreppen konkreta.";
        assert!(phrase.len() > REPEAT_FINGERPRINT_LEN);
        let prior = format!("Inledning. {} Slut.", phrase);
        let new = format!("Fortsätter... {}", phrase);
        // Must not panic even though boundaries are awkward in UTF-8.
        let _ = detect_content_repeat(&prior, &new);
    }

    #[test]
    fn content_repeat_no_false_positive_on_distinct_prose() {
        let prior = "Rust is a systems programming language designed around memory safety without runtime garbage collection, performance that rivals C and C++, and strong guarantees about concurrency via its ownership and borrow system.";
        let new = "Python is a dynamically typed general-purpose language prized for its readability, broad standard library, and deep scientific-computing ecosystem including NumPy, SciPy, and the Jupyter notebook environment.";
        assert!(prior.len() > REPEAT_FINGERPRINT_LEN, "test prior too short");
        assert!(new.len() > REPEAT_FINGERPRINT_LEN, "test new too short");
        assert!(detect_content_repeat(prior, new).is_none());
    }

    #[test]
    fn content_repeat_whitespace_window_rejected() {
        // A window made up mostly of whitespace shouldn't count as a match,
        // even if the equivalent whitespace appears in prior.
        let prior = "\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n";
        let new = prior;
        assert!(detect_content_repeat(prior, new).is_none());
    }

    // ── extract_normalized_headings ────────────────────────────

    #[test]
    fn extract_headings_recognizes_hash_levels() {
        let text = "# H1 Title\n\n## H2 Title\n\n### H3 Title\n\nPlain paragraph.";
        let out = extract_normalized_headings(text);
        assert_eq!(out, vec!["h1 title", "h2 title", "h3 title"]);
    }

    #[test]
    fn extract_headings_recognizes_bold_pseudo_heading() {
        let text = "**PROG2 VT26 – Semester project**\n\nBody content.";
        let out = extract_normalized_headings(text);
        // The en-dash is punctuation, collapsed to space.
        assert_eq!(out, vec!["prog2 vt26 semester project"]);
    }

    #[test]
    fn extract_headings_ignores_inline_bold() {
        // Bold inside a paragraph is not a heading.
        let text = "This sentence has **bold text** mid-paragraph.";
        assert!(extract_normalized_headings(text).is_empty());
    }

    #[test]
    fn extract_headings_strips_punctuation_and_lowercases() {
        let text = "## 1. High-Level Goal!";
        let out = extract_normalized_headings(text);
        assert_eq!(out, vec!["1 high level goal"]);
    }

    // ── detect_exact_heading_repeat ────────────────────────────

    #[test]
    fn heading_repeat_no_prior_headings_none() {
        assert!(
            detect_exact_heading_repeat("plain paragraph, no headings", "## New Heading Here")
                .is_none()
        );
    }

    #[test]
    fn heading_repeat_exact_match_detected() {
        let prior = "## Project Overview\n\nSome content.";
        let new = "## Project Overview\n\nContent again.";
        assert!(detect_exact_heading_repeat(prior, new).is_some());
    }

    #[test]
    fn heading_repeat_case_and_punctuation_insensitive() {
        let prior = "## Project: Overview!\n\nFoo.";
        let new = "## project overview\n\nBar.";
        assert!(detect_exact_heading_repeat(prior, new).is_some());
    }

    #[test]
    fn heading_repeat_paraphrase_not_flagged() {
        // Exact-match-only detector: paraphrased variants are NOT flagged,
        // by design. This keeps the false-positive rate near zero -- the
        // continuation prompt is the primary defence against paraphrased
        // restarts, not this detector.
        let prior = "### 1. Project structure you will receive\n\nFoo.";
        let new = "### Project structure you will receive shortly\n\nBar.";
        assert!(detect_exact_heading_repeat(prior, new).is_none());
    }

    #[test]
    fn heading_repeat_weakly_related_not_flagged() {
        let prior = "## Common Pitfalls\n\nFoo.";
        let new = "## Build System\n\nBar.";
        assert!(detect_exact_heading_repeat(prior, new).is_none());
    }

    #[test]
    fn heading_repeat_single_word_generic_heading_not_flagged() {
        // Even if "Summary" appears twice verbatim, single-word generic
        // headings are ignored to avoid chopping off responses with multiple
        // legitimate "## Summary" sections.
        let prior = "## Summary\n\nFoo.";
        let new = "## Summary\n\nBar.";
        assert!(detect_exact_heading_repeat(prior, new).is_none());
    }

    #[test]
    fn heading_repeat_bold_pseudo_heading_exact_match() {
        let prior = "**PROG2 VT26 – Semester project**\n\nBody.";
        let new = "Some continuation...\n\n**PROG2 VT26 – Semester project**\n\nMore body.";
        assert!(detect_exact_heading_repeat(prior, new).is_some());
    }

    // ── is_sentence_boundary ──────────────────────────────────

    #[test]
    fn sentence_boundary_short_text_false() {
        assert!(!is_sentence_boundary("Short."));
    }

    #[test]
    fn sentence_boundary_paragraph_break_true() {
        let t = "A".repeat(120) + "\n\n";
        assert!(is_sentence_boundary(&t));
    }

    #[test]
    fn sentence_boundary_inside_code_fence_false() {
        // Unclosed fence (odd count of ```) ⇒ inside code.
        let t = "Some prose here. ```rust\nlet x = 1;\nlet y = 2;\n".to_string()
            + &"more content ".repeat(30);
        assert!(!is_sentence_boundary(&t));
    }

    #[test]
    fn sentence_boundary_inside_table_false() {
        // Long text ending on a table row -- should not fire.
        let t = "Intro paragraph here. ".to_string() + &"| col1 | col2 |\n".repeat(30) + "| next |";
        assert!(!is_sentence_boundary(&t));
    }

    #[test]
    fn sentence_boundary_after_period_true() {
        let mut t = "The quick brown fox jumps over the lazy dog. ".repeat(8);
        t.push_str("Another sentence ends here.");
        assert!(t.len() > 300);
        assert!(is_sentence_boundary(&t));
    }

    #[test]
    fn sentence_boundary_inside_list_false() {
        let mut t = String::from("Paragraph here that is long enough to pass the first gate. ");
        t.push_str(&"Filler text ".repeat(30));
        t.push_str("\n- list item that does not end with terminal punct");
        assert!(!is_sentence_boundary(&t));
    }

    // ── Per-response token cap math ────────────────────────────

    #[test]
    fn response_cap_uses_2x_course_limit_when_set() {
        // Simulate the exact computation used by the outer loop.
        let daily: i64 = 100_000;
        let cap = if daily > 0 {
            daily.saturating_mul(DAILY_LIMIT_RESPONSE_MULTIPLIER)
        } else {
            UNLIMITED_COURSE_RESPONSE_CAP
        };
        assert_eq!(cap, 200_000);
    }

    #[test]
    fn response_cap_falls_back_for_unlimited_course() {
        let daily: i64 = 0;
        let cap = if daily > 0 {
            daily.saturating_mul(DAILY_LIMIT_RESPONSE_MULTIPLIER)
        } else {
            UNLIMITED_COURSE_RESPONSE_CAP
        };
        assert_eq!(cap, UNLIMITED_COURSE_RESPONSE_CAP);
    }

    #[test]
    fn response_cap_saturates_on_huge_daily_limit() {
        // Pathological config: admin sets daily_token_limit to i64::MAX. We
        // must not overflow when multiplying by the response multiplier.
        let daily = i64::MAX;
        let cap = daily.saturating_mul(DAILY_LIMIT_RESPONSE_MULTIPLIER);
        assert_eq!(cap, i64::MAX);
    }

    // ── Streamed-token / full_text invariant ───────────────────
    //
    // The user asked: "make sure any token we send to the client as
    // 'complete' is actually in the next round(s) of requests to cerebras".
    // The implementation guarantee is that `full_text.push_str(delta)` runs
    // BEFORE `tx.send(delta)` in stream_with_logprobs, so every delta sent
    // to the client is already in full_text at that moment. `full_text` is
    // then serialized as the assistant message on the next Cerebras request.
    // We cannot directly unit test the real HTTP flow without a mock server,
    // but we can assert the state-ordering contract with a small harness.

    /// Simulate one delta arrival exactly as the streaming loop does: append
    /// to `full_text` first, then "send" (record) to the client channel.
    fn simulate_delta_arrival(full_text: &mut String, sent: &mut Vec<String>, delta: &str) {
        // Mirrors the sequence in stream_with_logprobs:
        full_text.push_str(delta);
        // Simulated tx.send -- runs AFTER push_str.
        sent.push(delta.to_string());
    }

    #[test]
    fn invariant_every_streamed_token_is_in_full_text() {
        let deltas = ["Hello ", "world. ", "This is FLARE. ", "åäö multibyte."];
        let mut full_text = String::new();
        let mut sent: Vec<String> = Vec::new();
        for d in deltas {
            simulate_delta_arrival(&mut full_text, &mut sent, d);
            // At every step, the client-facing sequence must be a prefix of
            // full_text. If it ever weren't, we'd have sent tokens we can't
            // also supply on the next round.
            let reassembled = sent.concat();
            assert!(
                full_text.starts_with(&reassembled),
                "streamed-to-client prefix must be present in full_text at every step"
            );
        }
        // End state: all streamed tokens exactly equal full_text.
        assert_eq!(sent.concat(), full_text);
    }

    #[test]
    fn invariant_full_text_round_trips_as_assistant_message() {
        // The exact pattern used in the outer loop when building the
        // continuation request. We check full_text survives serialization
        // into the JSON body without mutation, so Cerebras on the next
        // request sees exactly what the client saw.
        let full_text = String::from("Hello, this is FLARE. åäö -- a complete assistant partial.");
        let msg = serde_json::json!({
            "role": "assistant",
            "content": full_text,
        });
        assert_eq!(msg["role"], "assistant");
        assert_eq!(msg["content"].as_str(), Some(full_text.as_str()));
        // Critically: full_text is still owned and usable (the json! macro
        // borrows and clones into Value::String). This is the property the
        // outer loop depends on.
        assert!(!full_text.is_empty());
    }
}
