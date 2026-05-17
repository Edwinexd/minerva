//! Agentic research phase for `tool_use_enabled` courses.
//!
//! Runs *before* the writeup phase and is what makes the
//! `tools` checkbox different from the legacy single-pass strategies.
//! Visible to the user as a "thinking" stream: every model token,
//! every tool call, and every tool result is emitted as SSE so the
//! frontend can render a collapsible disclosure under the assistant
//! reply.
//!
//! Two retrieval signals drive context expansion during the loop:
//!
//! 1. **Tool calls** (always on when this module runs): the model
//!    explicitly calls `keyword_search`, `semantic_search`,
//!    `list_documents`, or `get_document_chunks`. Catalog assembled
//!    by `tools::assemble_catalog` from per-course feature flags;
//!    dispatch goes through `tools::dispatch`.
//! 2. **Logprobs** (only when `use_logprobs = true`, i.e. the course's
//!    strategy is `flare`): per-token logprobs are requested on every
//!    research turn. When a sentence boundary fires with one or more
//!    tokens below `logprob_threshold`, the loop synthesises a
//!    follow-up `user` message of the form *"I want to double-check
//!    X. Here are some additional course materials: …"* and appends
//!    the chunks `flare_retrieve` returned for that sentence. The
//!    model then continues with the augmented context. This is the
//!    `flare+tools` combination: the implicit uncertainty signal
//!    augments the explicit tool-calling channel without needing the
//!    restart-with-continuation-prompt machinery the legacy
//!    `flare.rs` path uses (which was needed only because that path
//!    produces user-visible answer text; the research phase produces
//!    thinking-stream text where mid-stream injection is fine).
//!
//! The loop terminates when the model emits `finish_reason="stop"`
//! with no pending tool calls and no FLARE-triggered injection
//! pending, or when any of the safety caps fire
//! (`max_research_turns`, `max_tool_calls`, per-response token cap,
//! a content-repeat detector, or a stream error).

use axum::response::sse::Event;
use futures::StreamExt;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc;

use super::common;
use super::common::{chunk_identity_hash, RagChunk};
use super::tools::{self, ToolCatalogFlags, ToolDispatchCtx};
use super::GenerationContext;
use crate::error::AppError;

/// Configuration knobs for one research-phase run. `simple+tools` and
/// `flare+tools` differ only by `use_logprobs`.
#[derive(Debug, Clone, Copy)]
pub struct ResearchConfig {
    /// When true, request per-token logprobs and run the
    /// low-confidence sentence detector (the FLARE signal). When
    /// false, the research loop is pure tool calling.
    pub use_logprobs: bool,
    /// Logprob below which a token is "low confidence" (FLARE paper
    /// uses a probability threshold; we use the equivalent log).
    /// Default mirrors the legacy `flare::LOGPROB_THRESHOLD`.
    pub logprob_threshold: f64,
    /// Hard cap on outer-loop iterations. Each iteration issues one
    /// Cerebras request. Multiple tool calls within a single
    /// iteration count as one turn (the model batched them).
    pub max_research_turns: usize,
    /// Hard cap on total tool-call executions across the whole
    /// research phase. Protects against a model that loops on
    /// `keyword_search` forever.
    pub max_tool_calls: usize,
    /// Hard cap on FLARE-driven retrieval injections. Only consulted
    /// when `use_logprobs = true`.
    pub max_flare_injections: usize,
    /// Score threshold passed to FLARE's qdrant lookup for the
    /// low-confidence sentence query.
    pub flare_similarity_threshold: f32,
    /// Wall-clock budget. Protects against a slow model or a stuck
    /// tool dispatch holding up the user-visible thinking stream.
    pub wall_clock_budget: Duration,
    /// Per-request `max_tokens` on each Cerebras call. Bounded so
    /// every call returns usage statistics in the [DONE] chunk and
    /// the token accounting stays exact.
    pub max_tokens_per_turn: i32,
    /// Idle timeout between consecutive SSE frames from Cerebras
    /// during one research turn. Same role as
    /// `flare::STREAM_IDLE_TIMEOUT`.
    pub stream_idle_timeout: Duration,
}

impl ResearchConfig {
    /// Defaults tuned to keep the user-visible thinking stream
    /// snappy without burning context. Tunables that match the
    /// legacy FLARE knobs share their values so behaviour is
    /// comparable.
    pub fn defaults(use_logprobs: bool) -> Self {
        Self {
            use_logprobs,
            logprob_threshold: -2.0,
            max_research_turns: 8,
            max_tool_calls: 12,
            max_flare_injections: 4,
            flare_similarity_threshold: 0.35,
            wall_clock_budget: Duration::from_secs(45),
            max_tokens_per_turn: 1536,
            stream_idle_timeout: Duration::from_secs(60),
        }
    }
}

/// What the research phase produced, consumed by the writeup phase.
#[derive(Debug)]
pub struct ResearchOutput {
    /// Consolidated chunk set: initial seed plus everything tools and
    /// FLARE pulled in. Deduped by `chunk_identity_hash`.
    pub chunks: Vec<RagChunk>,
    /// Concatenation of every thinking token the model emitted.
    /// The writeup phase reads `research_summary` rather than this;
    /// the raw transcript is preserved on the struct so a future
    /// tracing pipeline or a test assertion can inspect it without
    /// re-deriving from SSE events.
    #[allow(dead_code)]
    pub transcript: String,
    /// Compressed bullet-list summary of the research the model did:
    /// each tool call and its result-size. The writeup system prompt
    /// embeds this so the writeup model can reference what was done.
    pub research_summary: String,
    pub total_prompt_tokens: i32,
    pub total_completion_tokens: i32,
    pub turns: usize,
    pub tool_calls_executed: usize,
    pub flare_injections: usize,
    pub stop_reason: ResearchStopReason,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ResearchStopReason {
    Completed,
    HitMaxTurns,
    HitMaxToolCalls,
    HitMaxFlareInjections,
    TokenCapReached,
    WallClockBudgetExceeded,
    StreamError(String),
}

/// SSE event helpers. Defined here so the protocol contract lives next
/// to the only producer; `chat.rs` adds nothing new on its side.
async fn emit_thinking_token(tx: &mpsc::Sender<Result<Event, AppError>>, token: &str) {
    let _ = tx
        .send(Ok(Event::default().data(
            serde_json::json!({"type": "thinking_token", "token": token}).to_string(),
        )))
        .await;
}

async fn emit_tool_call(
    tx: &mpsc::Sender<Result<Event, AppError>>,
    name: &str,
    args: &serde_json::Value,
) {
    let _ = tx
        .send(Ok(Event::default().data(
            serde_json::json!({"type": "tool_call", "name": name, "args": args}).to_string(),
        )))
        .await;
}

async fn emit_tool_result(tx: &mpsc::Sender<Result<Event, AppError>>, name: &str, summary: &str) {
    let _ = tx
        .send(Ok(Event::default().data(
            serde_json::json!({
                "type": "tool_result",
                "name": name,
                "result_summary": summary,
            })
            .to_string(),
        )))
        .await;
}

async fn emit_thinking_done(tx: &mpsc::Sender<Result<Event, AppError>>) {
    let _ = tx
        .send(Ok(Event::default().data(
            serde_json::json!({"type": "thinking_done"}).to_string(),
        )))
        .await;
}

/// Entry point. Drives the research loop until it hits `stop` (or a
/// cap), then returns the accumulated chunks + transcript for the
/// writeup phase.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    ctx: &GenerationContext,
    config: ResearchConfig,
    catalog_flags: ToolCatalogFlags,
    initial_chunks: Vec<RagChunk>,
    per_response_token_cap: i64,
    tx: &mpsc::Sender<Result<Event, AppError>>,
) -> ResearchOutput {
    let started_at = std::time::Instant::now();
    let http_client = reqwest::Client::new();
    let collection_name =
        minerva_ingest::pipeline::collection_name(ctx.course_id, ctx.embedding_version);

    let dispatch_ctx = ToolDispatchCtx {
        http_client: &http_client,
        openai_api_key: &ctx.openai_api_key,
        fastembed: &ctx.fastembed,
        qdrant: &ctx.qdrant,
        db: &ctx.db,
        collection_name: &collection_name,
        embedding_provider: &ctx.embedding_provider,
        embedding_model: &ctx.embedding_model,
        course_id: ctx.course_id,
        min_score: ctx.min_score,
    };

    let catalog = tools::assemble_catalog(catalog_flags);

    // Chunk accumulator: starts with the caller-provided seed and
    // grows monotonically as tool calls and FLARE retrievals
    // contribute new chunks. Dedup is `chunk_identity_hash`-based to
    // avoid O(n) `Vec::contains` work on every push.
    let mut chunks = initial_chunks;
    let mut chunk_hashes: HashSet<u64> = chunks.iter().map(chunk_identity_hash).collect();

    // Conversation state. The system prompt is rebuilt every turn so
    // newly added chunks land in the next request. Assistant + tool
    // messages accumulate across turns; the initial user query is
    // the last entry in `history` from the chat route.
    let mut history_messages = common::build_chat_messages("", ctx.history.as_slice());
    // Drop the empty system placeholder; we rebuild per turn.
    if matches!(
        history_messages
            .first()
            .and_then(|m| m.get("role").and_then(|r| r.as_str())),
        Some("system")
    ) {
        history_messages.remove(0);
    }
    // Append the just-arrived user content (chat route puts it in
    // `user_content` rather than `history`).
    history_messages.push(serde_json::json!({
        "role": "user",
        "content": ctx.user_content.clone(),
    }));

    let mut transcript = String::new();
    let mut tool_log: Vec<ToolLogEntry> = Vec::new();
    let mut total_prompt_tokens: i32 = 0;
    let mut total_completion_tokens: i32 = 0;
    let mut turns: usize = 0;
    let mut tool_calls_executed: usize = 0;
    let mut flare_injections: usize = 0;
    let mut recent_tool_call_args: HashSet<u64> = HashSet::new();

    let stop_reason = loop {
        if turns >= config.max_research_turns {
            tracing::info!(
                "research: hit max_research_turns ({}), handing off to writeup",
                config.max_research_turns
            );
            break ResearchStopReason::HitMaxTurns;
        }
        if tool_calls_executed >= config.max_tool_calls {
            tracing::info!(
                "research: hit max_tool_calls ({}), handing off to writeup",
                config.max_tool_calls
            );
            break ResearchStopReason::HitMaxToolCalls;
        }
        let tokens_so_far =
            (total_prompt_tokens as i64).saturating_add(total_completion_tokens as i64);
        if tokens_so_far >= per_response_token_cap {
            tracing::warn!(
                "research: per-response token cap hit ({} >= {}), handing off to writeup",
                tokens_so_far,
                per_response_token_cap
            );
            break ResearchStopReason::TokenCapReached;
        }
        if started_at.elapsed() > config.wall_clock_budget {
            tracing::warn!(
                "research: wall-clock budget {}s exceeded after turn {}, handing off",
                config.wall_clock_budget.as_secs(),
                turns
            );
            break ResearchStopReason::WallClockBudgetExceeded;
        }
        turns += 1;

        let system = build_research_system_prompt(ctx, &chunks);
        let messages = compose_messages(&system, &history_messages);

        let outcome = match stream_research_turn(
            &http_client,
            ctx,
            &config,
            &messages,
            &catalog,
            tx,
            &mut transcript,
        )
        .await
        {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("research: stream error: {}", e);
                break ResearchStopReason::StreamError(e);
            }
        };

        total_prompt_tokens += outcome.prompt_tokens;
        total_completion_tokens += outcome.completion_tokens;

        // Persist the assistant turn in the conversation so the model
        // sees its own prior content on the next turn. Tool calls are
        // attached to the assistant message per OpenAI's protocol.
        let mut assistant_msg = serde_json::json!({"role": "assistant"});
        if !outcome.content.is_empty() {
            assistant_msg["content"] = serde_json::Value::String(outcome.content.clone());
        } else {
            // Cerebras (and OpenAI) accept content=null when the
            // turn is purely tool-calls; some libraries are stricter.
            // Use empty-string for the safer wire form.
            assistant_msg["content"] = serde_json::Value::String(String::new());
        }
        if !outcome.tool_calls.is_empty() {
            let calls: Vec<serde_json::Value> = outcome
                .tool_calls
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "type": "function",
                        "function": {"name": c.name, "arguments": c.arguments},
                    })
                })
                .collect();
            assistant_msg["tool_calls"] = serde_json::Value::Array(calls);
        }
        history_messages.push(assistant_msg);

        // Execute pending tool calls.
        if !outcome.tool_calls.is_empty() {
            for call in &outcome.tool_calls {
                if tool_calls_executed >= config.max_tool_calls {
                    break;
                }

                // Duplicate-call short-circuit: if this exact
                // (name, args) pair was executed earlier in the
                // research phase, return a "you already called
                // this" stub rather than re-running the tool. Costs
                // less than a real call and nudges the model to
                // either accept the existing result or refine.
                let dup_hash = hash_tool_call(&call.name, &call.arguments);
                let model_message = if !recent_tool_call_args.insert(dup_hash) {
                    serde_json::json!({
                        "error": "duplicate_call",
                        "message": format!(
                            "You already called {} with these arguments this turn. Use the prior result, or refine.",
                            call.name
                        ),
                    })
                    .to_string()
                } else {
                    let args_value: serde_json::Value =
                        serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                    emit_tool_call(tx, &call.name, &args_value).await;
                    match tools::dispatch(&call.name, &call.arguments, &dispatch_ctx, catalog_flags)
                        .await
                    {
                        Ok(outcome) => {
                            tool_log.push(ToolLogEntry {
                                name: call.name.clone(),
                                args: call.arguments.clone(),
                                result_size: outcome.model_message.len(),
                                chunks_returned: outcome.chunks.len(),
                            });
                            for c in outcome.chunks {
                                if chunk_hashes.insert(chunk_identity_hash(&c)) {
                                    chunks.push(c);
                                }
                            }
                            emit_tool_result(
                                tx,
                                &call.name,
                                &summarise_for_event(&outcome.model_message),
                            )
                            .await;
                            outcome.model_message
                        }
                        Err(err) => {
                            tracing::warn!("research: tool {} failed: {:?}", call.name, err);
                            let msg = err.to_tool_message();
                            emit_tool_result(tx, &call.name, &summarise_for_event(&msg)).await;
                            msg
                        }
                    }
                };

                history_messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": model_message,
                }));
                tool_calls_executed += 1;
            }
            // Tool calls drove this turn; loop back so the model can
            // react to the results. Do NOT also fire a FLARE
            // injection on the same turn ; let the next turn see the
            // tool results first.
            continue;
        }

        // No tool calls. Either the model finished (`stop`) or hit
        // its per-turn token cap (`length`). Inspect the FLARE
        // signal first: if a low-confidence sentence was captured
        // and we still have budget for an injection, do a server-
        // side retrieve and append a synthetic user message with
        // the chunks. Then continue the loop.
        if let Some(sentence) = outcome.low_confidence_sentence {
            if flare_injections < config.max_flare_injections {
                let new_chunks = common::rag_lookup(
                    &http_client,
                    &ctx.openai_api_key,
                    &ctx.fastembed,
                    &ctx.qdrant,
                    &collection_name,
                    &sentence,
                    ctx.max_chunks,
                    config.flare_similarity_threshold.max(ctx.min_score),
                    &ctx.embedding_provider,
                    &ctx.embedding_model,
                )
                .await;
                let mut added = 0usize;
                for c in &new_chunks {
                    if chunk_hashes.insert(chunk_identity_hash(c)) {
                        chunks.push(c.clone());
                        added += 1;
                    }
                }
                if added > 0 {
                    flare_injections += 1;
                    let bullets: String = new_chunks
                        .iter()
                        .take(ctx.max_chunks as usize)
                        .map(|c| {
                            format!(
                                "- [{}] {}",
                                c.filename,
                                c.text.chars().take(400).collect::<String>()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let injection = format!(
                        "Some additional course materials related to your last point follow. Use them to refine your thinking before continuing.\n\n{}",
                        bullets
                    );
                    history_messages.push(serde_json::json!({
                        "role": "user",
                        "content": injection,
                    }));
                    continue;
                }
                // Retrieval found nothing new; fall through to break.
            } else {
                tracing::info!(
                    "research: hit max_flare_injections ({}), not injecting again",
                    config.max_flare_injections
                );
                break ResearchStopReason::HitMaxFlareInjections;
            }
        }

        // Nothing left to do. Hand off to writeup.
        break ResearchStopReason::Completed;
    };

    emit_thinking_done(tx).await;

    let research_summary = render_summary(&tool_log, flare_injections);

    ResearchOutput {
        chunks,
        transcript,
        research_summary,
        total_prompt_tokens,
        total_completion_tokens,
        turns,
        tool_calls_executed,
        flare_injections,
        stop_reason,
    }
}

// Internals

#[derive(Debug)]
struct ToolLogEntry {
    name: String,
    args: String,
    result_size: usize,
    chunks_returned: usize,
}

#[derive(Debug)]
struct TurnOutcome {
    content: String,
    tool_calls: Vec<PendingToolCall>,
    /// First captured low-confidence sentence (only populated when
    /// `use_logprobs = true`). Read by the outer loop to decide
    /// whether to inject a FLARE retrieval.
    low_confidence_sentence: Option<String>,
    prompt_tokens: i32,
    completion_tokens: i32,
}

#[derive(Debug, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    /// JSON-string form, exactly as the model emitted. We pass this
    /// to `tools::dispatch` verbatim and also stash it on the
    /// assistant message we persist to history.
    arguments: String,
}

fn build_research_system_prompt(ctx: &GenerationContext, chunks: &[RagChunk]) -> String {
    let base = common::build_system_prompt(&ctx.course_name, &ctx.custom_prompt, chunks);
    format!(
        "{base}\n\n## Research phase\n\
        You are in a hidden research phase before composing the student's reply. \
        Think out loud, work through the problem, and call tools (keyword_search, \
        semantic_search, list_documents, get_document_chunks) whenever you need \
        information you don't already have. The student will NOT see this phase; \
        they will see only the final reply you will compose afterwards. Stop \
        calling tools when you have enough context, and just emit a short summary \
        of what you found so the writeup phase can act on it. Do NOT write the \
        student-facing reply here.",
    )
}

fn compose_messages(system_prompt: &str, history: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut out = Vec::with_capacity(history.len() + 1);
    out.push(serde_json::json!({"role": "system", "content": system_prompt}));
    out.extend_from_slice(history);
    out
}

/// Hash `(name, normalized_args)` for duplicate-tool-call detection.
fn hash_tool_call(name: &str, args: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    args.hash(&mut h);
    h.finish()
}

fn summarise_for_event(model_message: &str) -> String {
    // Try to parse the message as JSON and extract a useful one-liner.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(model_message) {
        if let Some(err) = value.get("error").and_then(|e| e.as_str()) {
            return format!("error: {}", err);
        }
        if let Some(arr) = value.as_array() {
            return format!("{} results", arr.len());
        }
        if let Some(obj) = value.as_object() {
            if let Some(arr) = obj.get("results").and_then(|r| r.as_array()) {
                let dropped = obj
                    .get("truncated_omitted")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0);
                if dropped > 0 {
                    return format!("{} results ({} more omitted)", arr.len(), dropped);
                }
                return format!("{} results", arr.len());
            }
        }
    }
    let trimmed: String = model_message.chars().take(120).collect();
    if model_message.len() > trimmed.len() {
        format!("{}…", trimmed)
    } else {
        trimmed
    }
}

fn render_summary(tool_log: &[ToolLogEntry], flare_injections: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    for entry in tool_log {
        let args_short: String = entry.args.chars().take(80).collect();
        lines.push(format!(
            "- {}({}) -> {} chunks (result size {} bytes)",
            entry.name, args_short, entry.chunks_returned, entry.result_size
        ));
    }
    if flare_injections > 0 {
        lines.push(format!(
            "- (server-side) FLARE-driven retrieval x{} on low-confidence sentences",
            flare_injections
        ));
    }
    if lines.is_empty() {
        "No tool calls or retrievals fired during research.".to_string()
    } else {
        lines.join("\n")
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_research_turn(
    http_client: &reqwest::Client,
    ctx: &GenerationContext,
    config: &ResearchConfig,
    messages: &[serde_json::Value],
    catalog: &[serde_json::Value],
    tx: &mpsc::Sender<Result<Event, AppError>>,
    transcript: &mut String,
) -> Result<TurnOutcome, String> {
    let mut body = serde_json::json!({
        "model": ctx.model,
        "messages": messages,
        "temperature": ctx.temperature,
        "stream": true,
        "max_tokens": config.max_tokens_per_turn,
        "tools": catalog,
        "tool_choice": "auto",
        "stream_options": { "include_usage": true },
    });
    if config.use_logprobs {
        body["logprobs"] = serde_json::Value::Bool(true);
        body["top_logprobs"] = serde_json::Value::Number(1.into());
    }

    let response = common::cerebras_request_with_retry_to(
        http_client,
        &ctx.cerebras_base_url,
        &ctx.cerebras_api_key,
        &body,
    )
    .await?;

    let mut stream = response.bytes_stream();
    let mut byte_carry: Vec<u8> = Vec::new();
    let mut sse_buffer = String::new();

    let mut content = String::new();
    let mut sentence_buffer = String::new();
    let mut sentence_has_low_confidence = false;
    let mut first_low_confidence_sentence: Option<String> = None;
    let mut prompt_tokens = 0i32;
    let mut completion_tokens = 0i32;

    // Tool-call accumulator. Cerebras (like OpenAI) streams tool_calls
    // as a sequence of deltas, each keyed by `index`. We assemble them
    // here so the caller gets a clean `Vec<PendingToolCall>`.
    let mut tool_call_acc: Vec<PartialToolCall> = Vec::new();

    loop {
        let next = match tokio::time::timeout(config.stream_idle_timeout, stream.next()).await {
            Ok(n) => n,
            Err(_) => {
                return Err(format!(
                    "Cerebras stream idle timeout ({}s)",
                    config.stream_idle_timeout.as_secs()
                ));
            }
        };
        let chunk = match next {
            Some(Ok(c)) => c,
            Some(Err(e)) => return Err(format!("Stream error: {}", e)),
            None => break,
        };
        byte_carry.extend_from_slice(&chunk);
        let valid_up_to = match std::str::from_utf8(&byte_carry) {
            Ok(_) => byte_carry.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            let valid_str = std::str::from_utf8(&byte_carry[..valid_up_to])
                .expect("prefix was UTF-8 validated");
            sse_buffer.push_str(valid_str);
            byte_carry.drain(..valid_up_to);
        }

        while let Some(line_end) = sse_buffer.find('\n') {
            let line = sse_buffer[..line_end].trim().to_string();
            sse_buffer.drain(..=line_end);

            if line == "data: [DONE]" {
                let tool_calls: Vec<PendingToolCall> = tool_call_acc
                    .into_iter()
                    .filter_map(|p| p.finish())
                    .collect();
                return Ok(TurnOutcome {
                    content,
                    tool_calls,
                    low_confidence_sentence: first_low_confidence_sentence,
                    prompt_tokens,
                    completion_tokens,
                });
            }
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if let Some(err) = parsed.get("error") {
                let msg = err["message"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_string();
                return Err(msg);
            }
            if let Some(usage) = parsed.get("usage") {
                if !usage.is_null() {
                    prompt_tokens = usage["prompt_tokens"].as_i64().unwrap_or(0) as i32;
                    completion_tokens = usage["completion_tokens"].as_i64().unwrap_or(0) as i32;
                }
            }
            let Some(choice) = parsed["choices"].get(0) else {
                continue;
            };

            // Content delta: forward to client, accumulate, FLARE check.
            if let Some(delta_content) = choice["delta"]["content"].as_str() {
                if !delta_content.is_empty() {
                    content.push_str(delta_content);
                    transcript.push_str(delta_content);
                    sentence_buffer.push_str(delta_content);
                    emit_thinking_token(tx, delta_content).await;

                    if config.use_logprobs {
                        if let Some(logprobs_content) = choice["logprobs"]["content"].as_array() {
                            for lp in logprobs_content {
                                if let Some(logprob) = lp["logprob"].as_f64() {
                                    if logprob < config.logprob_threshold {
                                        sentence_has_low_confidence = true;
                                    }
                                }
                            }
                        }
                        if is_sentence_boundary(&sentence_buffer) {
                            if sentence_has_low_confidence
                                && first_low_confidence_sentence.is_none()
                            {
                                first_low_confidence_sentence = Some(sentence_buffer.clone());
                            }
                            sentence_buffer.clear();
                            sentence_has_low_confidence = false;
                        }
                    }
                }
            }

            // Tool-call delta: accumulate per index.
            if let Some(tool_call_deltas) = choice["delta"]["tool_calls"].as_array() {
                for tcd in tool_call_deltas {
                    let idx = tcd["index"].as_u64().unwrap_or(0) as usize;
                    while tool_call_acc.len() <= idx {
                        tool_call_acc.push(PartialToolCall::default());
                    }
                    let p = &mut tool_call_acc[idx];
                    if let Some(id) = tcd["id"].as_str() {
                        p.id.push_str(id);
                    }
                    if let Some(name) = tcd["function"]["name"].as_str() {
                        p.name.push_str(name);
                    }
                    if let Some(args) = tcd["function"]["arguments"].as_str() {
                        p.arguments.push_str(args);
                    }
                }
            }
        }
    }

    // Stream closed without [DONE] (rare); return what we have so the
    // outer loop can treat it as a natural turn end.
    let tool_calls: Vec<PendingToolCall> = tool_call_acc
        .into_iter()
        .filter_map(|p| p.finish())
        .collect();
    Ok(TurnOutcome {
        content,
        tool_calls,
        low_confidence_sentence: first_low_confidence_sentence,
        prompt_tokens,
        completion_tokens,
    })
}

#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl PartialToolCall {
    fn finish(self) -> Option<PendingToolCall> {
        if self.id.is_empty() || self.name.is_empty() {
            return None;
        }
        Some(PendingToolCall {
            id: self.id,
            name: self.name,
            arguments: if self.arguments.is_empty() {
                "{}".to_string()
            } else {
                self.arguments
            },
        })
    }
}

/// Same sentence-boundary heuristic as the legacy FLARE path: trigger
/// at paragraph breaks or end-of-sentence punctuation, but suppress
/// inside code fences, tables, and lists where the model is mid-
/// construct. Duplicated here (rather than promoted from `flare.rs`)
/// to keep the tools-off legacy path's helpers private and self-
/// contained; this copy may diverge in the future if the research
/// phase needs different tuning.
fn is_sentence_boundary(text: &str) -> bool {
    if text.len() < 100 {
        return false;
    }
    if let Some(last_line) = text.lines().last() {
        let trimmed_line = last_line.trim();
        if trimmed_line.starts_with('|') || trimmed_line.starts_with("|-") {
            return false;
        }
    }
    let fence_count = text.matches("```").count();
    if !fence_count.is_multiple_of(2) {
        return false;
    }
    if let Some(last_line) = text.lines().last() {
        let trimmed_line = last_line.trim();
        if trimmed_line.starts_with("- ") || trimmed_line.starts_with("* ") {
            return false;
        }
    }
    if text.ends_with("\n\n") {
        return true;
    }
    if text.len() > 300 {
        let trimmed = text.trim_end();
        let last_char = trimmed.chars().last().unwrap_or(' ');
        if last_char == '.' || last_char == '?' || last_char == '!' {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_legacy_flare_knobs() {
        let cfg = ResearchConfig::defaults(true);
        assert!(cfg.use_logprobs);
        assert!((cfg.logprob_threshold - (-2.0)).abs() < f64::EPSILON);
        assert_eq!(cfg.max_research_turns, 8);
        assert_eq!(cfg.max_tokens_per_turn, 1536);
    }

    #[test]
    fn sentence_boundary_fires_on_paragraph_break() {
        let body = "a".repeat(120) + "\n\n";
        assert!(is_sentence_boundary(&body));
    }

    #[test]
    fn sentence_boundary_suppressed_inside_code_fence() {
        let mut body = String::from("```rust\n");
        body.push_str(&"a".repeat(400));
        body.push('.');
        assert!(!is_sentence_boundary(&body));
    }

    #[test]
    fn partial_tool_call_yields_none_if_no_name() {
        let p = PartialToolCall {
            id: "abc".to_string(),
            name: String::new(),
            arguments: "{}".to_string(),
        };
        assert!(p.finish().is_none());
    }

    #[test]
    fn partial_tool_call_fills_default_args_when_empty() {
        let p = PartialToolCall {
            id: "x".to_string(),
            name: "list_documents".to_string(),
            arguments: String::new(),
        };
        let finished = p.finish().unwrap();
        assert_eq!(finished.arguments, "{}");
    }

    #[test]
    fn summarise_for_event_counts_array_results() {
        let msg = r#"[{"filename":"a.pdf"},{"filename":"b.pdf"}]"#;
        assert_eq!(summarise_for_event(msg), "2 results");
    }

    #[test]
    fn summarise_for_event_surfaces_truncation_count() {
        let msg = r#"{"results":[{"a":1}],"truncated_omitted":5}"#;
        assert_eq!(summarise_for_event(msg), "1 results (5 more omitted)");
    }

    #[test]
    fn render_summary_reports_no_calls() {
        assert!(render_summary(&[], 0).contains("No tool calls"));
    }

    #[test]
    fn hash_tool_call_dedups_same_args() {
        let h1 = hash_tool_call("keyword_search", r#"{"query":"deadline"}"#);
        let h2 = hash_tool_call("keyword_search", r#"{"query":"deadline"}"#);
        let h3 = hash_tool_call("keyword_search", r#"{"query":"other"}"#);
        let h4 = hash_tool_call("semantic_search", r#"{"query":"deadline"}"#);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_ne!(h1, h4);
    }

    #[test]
    fn render_summary_lists_each_tool_invocation() {
        let log = vec![
            ToolLogEntry {
                name: "keyword_search".to_string(),
                args: r#"{"query":"deadline"}"#.to_string(),
                result_size: 240,
                chunks_returned: 3,
            },
            ToolLogEntry {
                name: "list_documents".to_string(),
                args: "{}".to_string(),
                result_size: 510,
                chunks_returned: 0,
            },
        ];
        let summary = render_summary(&log, 2);
        assert!(summary.contains("keyword_search"));
        assert!(summary.contains("list_documents"));
        assert!(summary.contains("FLARE-driven retrieval x2"));
    }
}

#[cfg(test)]
mod stream_integration_tests {
    //! Wiremock-driven coverage for `stream_research_turn`: drives a
    //! scripted Cerebras SSE response through the parser and asserts
    //! it correctly separates `content` deltas, `tool_calls` deltas,
    //! and the FLARE logprob signal.
    //!
    //! Tests don't exercise `run` end-to-end (which would need a
    //! qdrant + sqlx stack) ; the orchestration layer is covered by
    //! unit tests on the helper functions, and the parser is the
    //! piece most likely to regress when Cerebras tweaks its SSE
    //! shape.

    use super::*;
    use minerva_db::queries::conversations::MessageRow;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fake_ctx_with_url(base_url: String) -> GenerationContext {
        GenerationContext {
            course_name: "Test Course".to_string(),
            custom_prompt: None,
            model: "qwen-3-235b-a22b-instruct-2507".to_string(),
            temperature: 0.3,
            max_chunks: 10,
            min_score: 0.0,
            course_id: uuid::Uuid::nil(),
            conversation_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            cerebras_api_key: "test-key".to_string(),
            cerebras_base_url: base_url,
            openai_api_key: String::new(),
            embedding_provider: "local".to_string(),
            embedding_model: "test".to_string(),
            embedding_version: 1,
            history: Vec::<MessageRow>::new(),
            user_content: "Hello".to_string(),
            is_first_message: true,
            daily_token_limit: 0,
            db: sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect_lazy("postgres://nobody:nobody@127.0.0.1:65535/none")
                .expect("lazy pool"),
            qdrant: std::sync::Arc::new(
                qdrant_client::Qdrant::from_url("http://127.0.0.1:65535")
                    .build()
                    .unwrap(),
            ),
            fastembed: std::sync::Arc::new(minerva_ingest::fastembed_embedder::FastEmbedder::new()),
            kg_enabled: false,
            tool_use_enabled: true,
        }
    }

    #[tokio::test]
    async fn stream_parses_interleaved_content_tool_call_and_logprob() {
        // Build a scripted SSE body that mixes:
        //   1. an assistant role-only opener (no payload)
        //   2. a content delta with a low logprob (should arm FLARE)
        //   3. a tool_calls delta (id, name, arguments split across)
        //   4. a usage chunk closing with finish_reason=tool_calls
        let mut body = String::new();
        body.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n");
        // Long content to clear the 100-char sentence-boundary floor.
        let long = "a".repeat(120);
        let content_delta = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"content": format!("{long}.\n\n")},
                "logprobs": {"content": [{"token": "a", "logprob": -3.5}]},
                "finish_reason": null,
            }]
        });
        body.push_str(&format!("data: {}\n\n", content_delta));
        // Tool call split across two deltas (id+name first, args after) to
        // exercise the per-index accumulator.
        let tc_open = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "call_abc",
                    "type": "function",
                    "function": {"name": "keyword_search", "arguments": ""},
                }]},
            }]
        });
        body.push_str(&format!("data: {}\n\n", tc_open));
        let tc_args = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"tool_calls": [{
                    "index": 0,
                    "function": {"arguments": "{\"query\":\"deadline\"}"},
                }]},
            }]
        });
        body.push_str(&format!("data: {}\n\n", tc_args));
        let usage = serde_json::json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150},
        });
        body.push_str(&format!("data: {}\n\n", usage));
        body.push_str("data: [DONE]\n\n");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;

        let ctx = fake_ctx_with_url(format!("{}/chat/completions", server.uri()));
        let config = ResearchConfig::defaults(true);
        let http = reqwest::Client::new();
        let (tx, mut rx) = mpsc::channel::<Result<Event, AppError>>(1024);
        let mut transcript = String::new();
        let catalog: Vec<serde_json::Value> = Vec::new();
        let messages = vec![serde_json::json!({"role": "user", "content": "?"})];

        let outcome = stream_research_turn(
            &http,
            &ctx,
            &config,
            &messages,
            &catalog,
            &tx,
            &mut transcript,
        )
        .await
        .expect("stream should succeed");

        // Drop tx so rx.recv returns None and we can drain.
        drop(tx);
        let mut events = 0;
        while rx.recv().await.is_some() {
            events += 1;
        }

        assert!(outcome.content.starts_with(&"a".repeat(120)));
        assert_eq!(outcome.tool_calls.len(), 1);
        assert_eq!(outcome.tool_calls[0].id, "call_abc");
        assert_eq!(outcome.tool_calls[0].name, "keyword_search");
        assert_eq!(outcome.tool_calls[0].arguments, "{\"query\":\"deadline\"}");
        assert!(outcome.low_confidence_sentence.is_some());
        assert_eq!(outcome.prompt_tokens, 100);
        assert_eq!(outcome.completion_tokens, 50);
        // We should have emitted at least one thinking_token event
        // (the long content delta).
        assert!(events >= 1, "expected at least one SSE thinking_token");
    }

    #[tokio::test]
    async fn stream_with_logprobs_off_does_not_arm_flare_signal() {
        // Same scripted response but ResearchConfig.use_logprobs=false.
        // The low-confidence sentence should NOT be captured regardless
        // of what the mock emits.
        let mut body = String::new();
        let long = "a".repeat(120);
        let delta = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"content": format!("{long}.\n\n")},
                "logprobs": {"content": [{"token": "a", "logprob": -10.0}]},
                "finish_reason": null,
            }]
        });
        body.push_str(&format!("data: {}\n\n", delta));
        let usage = serde_json::json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
        });
        body.push_str(&format!("data: {}\n\n", usage));
        body.push_str("data: [DONE]\n\n");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;

        let ctx = fake_ctx_with_url(format!("{}/chat/completions", server.uri()));
        let config = ResearchConfig::defaults(false); // logprobs OFF
        let http = reqwest::Client::new();
        let (tx, _rx) = mpsc::channel(1024);
        let mut transcript = String::new();
        let catalog: Vec<serde_json::Value> = Vec::new();
        let messages = vec![serde_json::json!({"role": "user", "content": "?"})];

        let outcome = stream_research_turn(
            &http,
            &ctx,
            &config,
            &messages,
            &catalog,
            &tx,
            &mut transcript,
        )
        .await
        .expect("stream should succeed");

        assert!(outcome.low_confidence_sentence.is_none());
        assert!(outcome.tool_calls.is_empty());
    }
}
