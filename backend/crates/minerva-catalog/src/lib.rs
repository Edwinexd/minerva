//! Pure-data model catalog.
//!
//! The compile-time tables of which embedding / reranker models the
//! runtime knows how to load, plus the embedding-provider whitelist.
//! This crate has no dependencies on purpose: it is the single source
//! of truth shared by tiers that must NOT depend on one another. The
//! ingest pipeline reads it; the model engine (`minerva-embed-engine`)
//! reads it for boot warmup; the api/server reads it for catalog
//! validation and DB seeding. Keeping the tables here means none of
//! those tiers has to pull the others (or the heavy ONNX/candle engine)
//! just to know the list of valid model ids.

/// Embedding providers a course owner can pick between. `"local"`
/// dispatches to the in-cluster model engine; `"openai"` calls the
/// OpenAI embeddings HTTP API.
pub const VALID_EMBEDDING_PROVIDERS: &[&str] = &["openai", "local"];

/// Whitelist of local embedding models a course owner can pick. Each
/// entry is `(huggingface-style id, output dimension)`. The dimension is
/// authoritative: `ensure_collection` reads from this list when creating
/// the per-course Qdrant collection, so a wrong number here means
/// upserts will fail with a vector-size mismatch.
///
/// Sourced from three backends, all dispatched by
/// `fastembed_embedder::FastEmbedder`:
/// * fastembed-rs's `EmbeddingModel` enum (ONNX, the default path);
/// * the Qwen3 candle entry (`Qwen3TextEmbedding`, gated behind
///   fastembed's `qwen3` feature);
/// * "bring your own ONNX" via `UserDefinedEmbeddingModel` for HF repos
///   whose ONNX export works but isn't part of `EmbeddingModel` yet --
///   currently snowflake-arctic-embed-m-v2.0.
///
/// Adding a model here: also add a `parse_fast_model_name` arm (or a
/// `custom_model_spec` arm for the user-defined path) in
/// `fastembed_embedder.rs`, and consider whether it's small enough to
/// warm up at boot (`STARTUP_BENCHMARK_MODELS` below). If unsure, leave
/// it out of startup; admins can run `POST /api/admin/embedding-benchmark`
/// to benchmark on demand without OOMing the box.
pub const VALID_LOCAL_MODELS: &[(&str, u64)] = &[
    // English-only, original set kept for backwards compatibility with
    // courses that picked these before multilingual options existed.
    ("sentence-transformers/all-MiniLM-L6-v2", 384),
    ("BAAI/bge-small-en-v1.5", 384),
    ("BAAI/bge-base-en-v1.5", 768),
    ("nomic-ai/nomic-embed-text-v1.5", 768),
    // Multilingual (Swedish + English, matters for SU/DSV course mix).
    ("intfloat/multilingual-e5-small", 384),
    ("intfloat/multilingual-e5-base", 768),
    ("intfloat/multilingual-e5-large", 1024),
    ("BAAI/bge-m3", 1024),
    ("google/embeddinggemma-300m", 768),
    // Snowflake Arctic Embed M v2.0: multilingual (Swedish + English),
    // 768 dims, ~311 MB int8 ONNX. Not part of fastembed-rs's
    // `EmbeddingModel` enum; loaded via `UserDefinedEmbeddingModel` --
    // see the `Backend::Custom` branch in `fastembed_embedder.rs`.
    ("Snowflake/snowflake-arctic-embed-m-v2.0", 768),
    // English, top-of-MTEB-class upgrades.
    ("mixedbread-ai/mxbai-embed-large-v1", 1024),
    ("Alibaba-NLP/gte-large-en-v1.5", 1024),
    ("snowflake/snowflake-arctic-embed-l", 1024),
    // Qwen3 (candle backend). Dim 1024, multilingual.
    ("Qwen/Qwen3-Embedding-0.6B", 1024),
];

/// Models the server warms up + benchmarks at boot. Subset of
/// `VALID_LOCAL_MODELS`: small/fast ONNX models the pod can hold in RAM
/// simultaneously without touching the cache budget too hard.
/// Everything else gets benchmarked on demand via the admin endpoint
/// so a single boot doesn't try to load every candidate at once and
/// OOM-kill the pod.
///
/// Arctic-m-v2.0 is in the warm set despite its ~311 MB int8 footprint
/// because (a) it's the multilingual default we now recommend for new
/// SU/DSV courses and (b) on first benchmark its session takes 30-60 s
/// to materialize from the freshly-downloaded ONNX; warming at boot
/// shifts that cost off the first teacher's "Run benchmark" click.
///
/// `BAAI/bge-base-en-v1.5` is intentionally not warmed: it's English-
/// only and overlapping with bge-small-en (also warmed). Existing
/// courses on bge-base still work; teachers who want a benchmark can
/// trigger one from the admin page.
pub const STARTUP_BENCHMARK_MODELS: &[(&str, u64)] = &[
    ("sentence-transformers/all-MiniLM-L6-v2", 384),
    ("BAAI/bge-small-en-v1.5", 384),
    ("nomic-ai/nomic-embed-text-v1.5", 768),
    ("Snowflake/snowflake-arctic-embed-m-v2.0", 768),
];

/// OpenAI embedding model used when a course's provider is `"openai"`.
pub const OPENAI_EMBEDDING_MODEL: &str = "text-embedding-3-small";

/// One chat / utility LLM model the runtime knows how to talk to, keyed
/// by the provider's own model id. Mirrors the embedding/reranker
/// catalog pattern: this slice is "code exists for these"; the admin
/// *policy* layer (enable / default / per-model price) lives in the
/// `chat_models` DB table.
///
/// Prices are deliberately NOT in code: they are admin-entered (and
/// scrape-assisted) per deployment, since the same model id costs
/// different amounts across providers and over time. New rows seed with
/// price NULL (unusable until priced); the one exception is the seeded
/// `gpt-oss-120b` row, whose Cerebras rates are pinned in the
/// `chat_models` migration.
#[derive(Debug, Clone, Copy)]
pub struct ChatModelSeed {
    pub model: &'static str,
    /// Registry provider id (`cerebras`, `openai`, `anthropic`, `groq`).
    pub provider: &'static str,
    pub display_name: &'static str,
    pub supports_logprobs: bool,
    pub supports_tool_use: bool,
}

/// Compile-time catalog of chat-model ids the runtime can route to.
/// Seeded into `chat_models` at startup (`seed_if_missing`); new entries
/// land `enabled = FALSE` with price NULL so they never auto-appear or
/// bill until an admin enables and prices them.
pub const VALID_CHAT_MODELS: &[ChatModelSeed] = &[
    ChatModelSeed {
        model: "gpt-oss-120b",
        provider: "cerebras",
        display_name: "GPT-OSS 120B (Cerebras)",
        supports_logprobs: true,
        supports_tool_use: true,
    },
    ChatModelSeed {
        model: "gpt-4o-mini",
        provider: "openai",
        display_name: "GPT-4o mini",
        supports_logprobs: true,
        supports_tool_use: true,
    },
    ChatModelSeed {
        model: "gpt-4o",
        provider: "openai",
        display_name: "GPT-4o",
        supports_logprobs: true,
        supports_tool_use: true,
    },
    // Anthropic (and any other provider) models are added per-deployment.
    // The Anthropic provider + capability gating are wired in the
    // registry; a deployment seeds its chosen model ids here. Models with
    // no per-token logprobs set `supports_logprobs: false`, which gates
    // them out of the FLARE strategy at config-save time.
];

/// Default cross-encoder. Multilingual (Swedish + English), the lightest
/// multilingual model in fastembed's reranker catalog. Mirrored by the
/// `courses.reranker_model` column DEFAULT and the `reranker_models`
/// seed; kept here so validation / fallbacks have a single source.
pub const DEFAULT_RERANK_MODEL: &str = "jinaai/jina-reranker-v2-base-multilingual";

/// Compile-time catalog of re-ranker model ids the runtime can load.
///
/// Policy (which of these a teacher may actually pick, and which is the
/// default for new courses) lives in the `reranker_models` DB table;
/// this slice is just "code exists for these". Mirrors
/// `VALID_LOCAL_MODELS` for embeddings. Each id must be a
/// `model_code` fastembed's `RerankerModel` understands (asserted in
/// tests).
pub const VALID_RERANKER_MODELS: &[&str] = &[
    // Multilingual (Swedish + English). Default; lightest multilingual.
    "jinaai/jina-reranker-v2-base-multilingual",
    // Multilingual, higher quality but heavier (fp32, ~568M + external
    // data file). Off by default; admin can enable when RAM allows.
    "rozgo/bge-reranker-v2-m3",
    // English / Chinese. Useful for English-only courses.
    "BAAI/bge-reranker-base",
    // English, very small / fast.
    "jinaai/jina-reranker-v1-turbo-en",
];

/// Per-model query-side prefix for asymmetric retrieval models.
///
/// Some embedding models are trained with distinct prompt templates for
/// queries vs documents (`query: ...` for the search side, `passage: ...`
/// for the indexed side). Calling them without those prefixes works but
/// gives up some recall.
///
/// We only apply the *query-side* prefix here, and only for models that
/// have always been query-prefixed in production. The document side is
/// never touched because:
/// 1. Existing Qdrant collections were built without prefixes. Switching
///    document prefixing on at ingest would silently mismatch every
///    chunk currently in storage; a rebuild is the only correct fix and
///    that's an explicit migration, not an automatic one.
/// 2. For arctic-m-v2.0 specifically, the model card *only* prescribes a
///    query prefix; documents stay bare by design.
///
/// Multilingual-e5-* is intentionally not in this list: its training
/// regime expects both `query:` and `passage:` prefixes, so prefixing
/// only the query side against bare-embedded documents would be an
/// asymmetric mismatch that's likely to *hurt* retrieval more than the
/// missing prefix already does. Fixing E5 properly requires a per-course
/// re-embed and is out of scope here.
///
/// Lives here (no-dep catalog) rather than in the model engine so the
/// api can prefix a query before sending it to the remote embedder
/// without linking the engine.
pub fn query_prefix_for_model(model: &str) -> Option<&'static str> {
    match model {
        "Snowflake/snowflake-arctic-embed-m-v2.0" => Some("query: "),
        _ => None,
    }
}

/// Apply the query-side prefix for `model` (if any) to `query` and
/// return an owned `String`. Cheap when no prefix is registered (one
/// move, no allocation). Used by the retrieval call sites in
/// `strategy::common`; the embed pipeline doesn't go through this.
pub fn format_query_for_model(model: &str, query: &str) -> String {
    match query_prefix_for_model(model) {
        Some(prefix) => format!("{prefix}{query}"),
        None => query.to_string(),
    }
}
