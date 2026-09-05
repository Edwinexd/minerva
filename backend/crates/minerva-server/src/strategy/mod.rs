pub mod common;
pub mod extraction_guard;
pub mod flare;
pub mod research_phase;
pub mod simple;
pub mod tool_use;
pub mod tools;
pub mod writeup;

#[cfg(test)]
pub(crate) mod test_support;

use axum::response::sse::Event;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::error::AppError;

#[derive(Clone)]
pub struct ChatRoute {
    pub model: String,
    pub provider: std::sync::Arc<dyn crate::llm::ChatProvider>,
}

/// What the extraction guard does to this turn's *visible* research
/// trace (thinking tokens, tool calls / results, sources panel).
///
/// Split from the guard's own verdict on purpose. Whether the guard
/// fired is a property of the turn and drives generation-side
/// behaviour (the writeup gets an empty research transcript) plus the
/// persisted `messages.thinking_hidden` audit bit; both are identical
/// no matter who is watching. What the *client* receives additionally
/// depends on the viewer, because a teacher is the audience the
/// evidence is being preserved for.
///
/// Before this existed the SSE layer suppressed for everyone while the
/// read-time gate in `routes::chat` exempted teachers, so a teacher
/// chatting in their own course saw the placeholder during generation
/// and the full trace the instant the post-stream refetch landed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThinkingDisclosure {
    /// Guard did not fire on this turn. Everything streams as normal.
    Open,
    /// Guard fired and the viewer is the student: no thinking tokens,
    /// tool events, or chunks reach the client, and a one-shot
    /// `thinking_hidden` event stands in for the disclosure.
    Withheld,
    /// Guard fired but the viewer teaches this course (or is an
    /// admin): the `thinking_hidden` marker is still emitted so the UI
    /// can label the turn as suppressed, and the trace streams anyway.
    Revealed,
}

impl ThinkingDisclosure {
    /// Did the guard fire this turn? Drives the persisted
    /// `messages.thinking_hidden` column and the client-side "this was
    /// hidden from the student" label.
    pub fn guarded(self) -> bool {
        !matches!(self, ThinkingDisclosure::Open)
    }

    /// Must this viewer be kept from the trace? Gates every
    /// `thinking_token` / `tool_call` / `tool_result` /
    /// `server_retrieval` emission and `chunks_used` on the `done`
    /// event.
    pub fn withheld(self) -> bool {
        matches!(self, ThinkingDisclosure::Withheld)
    }

    /// Build from the guard's per-turn verdict and the viewer's role.
    pub fn resolve(guard_flagged: bool, viewer_is_teacher: bool) -> Self {
        match (guard_flagged, viewer_is_teacher) {
            (false, _) => ThinkingDisclosure::Open,
            (true, false) => ThinkingDisclosure::Withheld,
            (true, true) => ThinkingDisclosure::Revealed,
        }
    }
}

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
    /// Resolved chat provider for `model`, drawn from the `AppState`
    /// `LlmRegistry` at the route handler (provider derived from
    /// `chat_models.provider`). Streaming strategies call
    /// `common::stream_chat_to_client(&ctx.provider, ...)`.
    pub provider: std::sync::Arc<dyn crate::llm::ChatProvider>,
    /// Enabled, configured alternatives, ordered with the admin default first.
    pub fallback_routes: Vec<ChatRoute>,
    /// OpenAI-compatible `(api_key, base_url)` for the bespoke FLARE /
    /// research-phase streaming loops, which parse tool-calls + logprobs
    /// inline against the raw transport. Both are sourced from
    /// `provider.openai_endpoint()` at the route handler so they stay in
    /// lockstep with the registry. Provider-agnostic in name; they hold
    /// whichever OpenAI-compatible chat provider `model` resolved to.
    /// Integration tests override the URL to point at a wiremock server.
    pub chat_api_key: String,
    pub chat_base_url: String,
    /// Resolved utility model (admin-selected via
    /// `chat_models.is_utility_default`) for the adversarial filter and
    /// extraction guard run inside the strategy. Carries the model id +
    /// a handle to its provider. Resolved once at the route handler.
    pub utility: crate::llm::UtilityModel,
    pub openai_api_key: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    /// Version stamp for the course's Qdrant collection; bumped when
    /// the teacher rotates embedding model. Strategies thread this
    /// into `pipeline::collection_name(course_id, version)` so query
    /// vectors land in the same collection ingest writes to.
    pub embedding_version: i32,
    pub history: Vec<minerva_db::queries::conversations::MessageRow>,
    /// `conversations.carryover_summary`: a recap of the thread this
    /// conversation was split off when the previous one hit its hard
    /// token ceiling. Present only on continuations. Rendered into the
    /// system prompt as inert reference text by
    /// `common::build_system_prompt_with_signals`, never as
    /// instructions, because it is model-written from student input.
    pub carryover: Option<String>,
    pub user_content: String,
    pub is_first_message: bool,
    /// Per-response token budget for the tool-use / FLARE fail-safe: the
    /// course's daily USD cap converted to a token count at the chosen
    /// model's blended rate (the route handler does the conversion). 0 =
    /// unlimited (no cap, or a free model). One answer cannot burn more
    /// than 2x this; see `tool_use::per_response_token_cap`.
    pub daily_token_budget: i64,
    /// The model's `(input, output)` USD-per-Mtok rates, resolved once at
    /// the route handler via `chat_models::rates_of`. Reused here so a chat
    /// turn does a single rate lookup: the route uses it to derive
    /// `daily_token_budget`, and `common::finalize` reuses it for the
    /// fleet-wide `chat_cost_microusd_total` metric instead of querying
    /// again. `None` = unknown price (NULL rate) or model absent from the
    /// catalog; `finalize` then bumps `chat_unpriced_calls_total`.
    pub billing_rates: Option<(rust_decimal::Decimal, rust_decimal::Decimal)>,
    pub db: sqlx::PgPool,
    pub qdrant: std::sync::Arc<qdrant_client::Qdrant>,
    pub fastembed: std::sync::Arc<dyn minerva_core::rpc::EmbedderClient>,
    /// Cross-encoder re-ranker client shared from `AppState`. Every RAG
    /// retrieval over-fetches a candidate pool from Qdrant and runs it
    /// through this before truncating to `max_chunks`; see
    /// `common::rag_lookup` / `common::rerank_chunks`.
    pub reranker: std::sync::Arc<dyn minerva_core::rpc::RerankerClient>,
    /// Per-course re-ranker model id (`courses.reranker_model`), chosen
    /// from the admin-managed `reranker_models` catalog. Passed to the
    /// `reranker` on every RAG lookup; independent of the embedding
    /// model (no re-embed on change).
    pub reranker_model: String,
    pub reranking_enabled: bool,
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
    /// Mirror of `courses.tool_use_enabled`. When TRUE, the strategy
    /// orchestrator splits generation into a hidden-thinking research
    /// phase (model uses `tools::catalog` and, for `flare`, the
    /// logprob signal) followed by a clean writeup phase. When FALSE,
    /// the legacy single-pass behaviour of `simple` / `flare` runs
    /// unchanged. Validated against `model_capabilities::validate_config`
    /// at config-save time so a runtime mismatch is impossible.
    pub tool_use_enabled: bool,
    /// Does the person on the other end of this stream teach the
    /// course (or administer the instance)? Only consumed by
    /// `ThinkingDisclosure::resolve` to decide whether a guarded
    /// turn's research trace is withheld or merely labelled. Never
    /// touches generation: the writeup still gets an empty research
    /// transcript and `messages.thinking_hidden` is still persisted on
    /// a guarded turn regardless of who asked. Always FALSE on the
    /// embed surface, whose tokens are student-scoped by construction.
    pub viewer_is_teacher: bool,
}

/// Run the appropriate strategy based on the strategy name.
///
/// `parallel` is retired (migration `20260519000001` remaps existing
/// rows to `simple`); any unknown strategy string falls through to
/// `simple` here so a stray DB value doesn't 5xx. The orthogonal
/// `tool_use_enabled` axis is read off `ctx` inside each strategy:
/// when FALSE they behave as they always have; when TRUE they split
/// into a research+writeup pair (see `research_phase`, `writeup`).
pub async fn run_strategy(
    strategy: &str,
    ctx: GenerationContext,
    tx: mpsc::Sender<Result<Event, AppError>>,
) {
    match strategy {
        "flare" => flare::run(ctx, tx).await,
        _ => simple::run(ctx, tx).await,
    }
}

#[cfg(test)]
mod tests {
    use super::ThinkingDisclosure;

    #[test]
    fn disclosure_separates_the_guard_verdict_from_the_viewer() {
        // Guard silent: nobody is gated, and nothing is persisted as
        // suppressed.
        for viewer_is_teacher in [false, true] {
            let d = ThinkingDisclosure::resolve(false, viewer_is_teacher);
            assert_eq!(d, ThinkingDisclosure::Open);
            assert!(!d.guarded());
            assert!(!d.withheld());
        }

        // Guard fired: the turn is marked suppressed for both, so the
        // persisted `thinking_hidden` column and the frontend's label
        // agree no matter who chatted. Only the student's trace is
        // actually withheld.
        let student = ThinkingDisclosure::resolve(true, false);
        assert_eq!(student, ThinkingDisclosure::Withheld);
        assert!(student.guarded());
        assert!(student.withheld());

        let teacher = ThinkingDisclosure::resolve(true, true);
        assert_eq!(teacher, ThinkingDisclosure::Revealed);
        assert!(teacher.guarded());
        assert!(!teacher.withheld());
    }
}
