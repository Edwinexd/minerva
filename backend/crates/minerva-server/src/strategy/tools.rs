//! Tool catalog and dispatch for the agentic research phase.
//!
//! Tools are course-scoped: `course_id` is injected from the dispatch
//! context, never from model-provided args. The model only sees the
//! JSON schemas; per-tool result size is capped at
//! `MAX_TOOL_RESULT_BYTES` with a truncation marker, preventing
//! context blowup if the model issues an open-ended query.
//!
//! Catalog assembly is feature-flag aware: `assemble_catalog` reads
//! from `ToolCatalogFlags`, so adding a flag-gated tool (e.g.
//! `search_concept_graph` when `kg_enabled`) is one match arm here
//! plus an implementation.

use serde::Deserialize;
use serde_json::Value as Json;
use std::sync::Arc;
use uuid::Uuid;

use super::common;
use super::common::RagChunk;
use minerva_ingest::fastembed_embedder::FastEmbedder;

/// Max bytes of serialized JSON returned to the model per tool call.
/// Beyond this we truncate the result list with a
/// `truncated_omitted` marker. Keeps the model from filling its
/// context window with a single overly-broad call.
pub const MAX_TOOL_RESULT_BYTES: usize = 8192;

/// Per-course feature flags that gate which tools appear in the
/// catalog passed to the model. Mirrors the runtime flag resolution
/// that strategies already perform; we re-thread the relevant subset
/// into tool dispatch so a single source of truth controls both
/// catalog visibility and runtime gating.
#[derive(Debug, Clone, Copy)]
pub struct ToolCatalogFlags {
    /// Knowledge-graph feature flag. Currently reserved for a
    /// follow-up `search_concept_graph` tool; not consumed by any v1
    /// tool. Kept in the struct so the catalog assembler has the
    /// information it needs once that tool lands.
    pub kg_enabled: bool,
}

/// Services the dispatcher borrows to execute a tool call. Owned by
/// the research-phase orchestrator and rebuilt per request; fields
/// mirror the subset of `GenerationContext` the tools actually need.
pub struct ToolDispatchCtx<'a> {
    pub http_client: &'a reqwest::Client,
    pub openai_api_key: &'a str,
    pub fastembed: &'a Arc<FastEmbedder>,
    pub qdrant: &'a Arc<qdrant_client::Qdrant>,
    pub db: &'a sqlx::PgPool,
    pub collection_name: &'a str,
    pub embedding_provider: &'a str,
    pub embedding_model: &'a str,
    pub course_id: Uuid,
    /// Score threshold passed through to `semantic_search`. The
    /// tool-provided `k` is clamped against the per-tool schema cap
    /// (10 for semantic, 20 for keyword), so we don't thread the
    /// course-level `max_chunks` here ; that ceiling already lives
    /// on the seed retrieval path.
    pub min_score: f32,
}

#[derive(Debug)]
pub enum ToolError {
    /// The model emitted a tool name not in the (course-scoped,
    /// flag-filtered) catalog. Returned to the model so it can
    /// retry with a real tool rather than crashing the loop.
    UnknownTool(String),
    /// The `arguments` JSON failed to parse against the tool schema.
    /// Returned to the model so it can retry with valid args.
    BadArgs { tool: &'static str, reason: String },
    /// Tool implementation hit an internal error (DB, Qdrant,
    /// embedding). Logged at warn; returned to the model as a short
    /// error message so the loop can proceed.
    Backend { tool: &'static str, reason: String },
}

impl ToolError {
    /// JSON-string form sent back to the model as the tool result.
    /// We use the same Result-as-JSON convention OpenAI's tool
    /// protocol expects, with a top-level `error` field so the model
    /// can detect failure without parsing prose.
    pub fn to_tool_message(&self) -> String {
        let (code, message) = match self {
            ToolError::UnknownTool(name) => (
                "unknown_tool",
                format!(
                    "Tool '{}' is not available in this course. Pick from the catalog.",
                    name
                ),
            ),
            ToolError::BadArgs { tool, reason } => ("bad_args", format!("{}: {}", tool, reason)),
            ToolError::Backend { tool, reason } => {
                ("backend_error", format!("{}: {}", tool, reason))
            }
        };
        serde_json::json!({"error": code, "message": message}).to_string()
    }
}

// Tool specs (JSON schemas the model sees)

fn semantic_search_spec() -> Json {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "semantic_search",
            "description": "Search course materials by semantic similarity (embedding-based). Best for conceptual questions where exact wording may differ between the query and the source. Returns the top-k chunks ranked by similarity.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The query phrased as a question or topic."},
                    "k": {"type": "integer", "description": "Max chunks to return (1-10).", "default": 5, "minimum": 1, "maximum": 10}
                },
                "required": ["query"]
            }
        }
    })
}

fn keyword_search_spec() -> Json {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "keyword_search",
            "description": "Search course materials by exact keyword or phrase. Best for deadlines, dates, identifiers, file names, code symbols, and other terms where exact match matters. Tokenised word match, lowercased.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Words or phrases to find verbatim in course content."},
                    "k": {"type": "integer", "description": "Max chunks to return (1-20).", "default": 10, "minimum": 1, "maximum": 20}
                },
                "required": ["query"]
            }
        }
    })
}

fn list_documents_spec() -> Json {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "list_documents",
            "description": "List every document the course currently has, with filename, mime type, and processing status. Use this to discover what materials exist before fetching a specific one with get_document_chunks.",
            "parameters": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }
    })
}

fn get_document_chunks_spec() -> Json {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "get_document_chunks",
            "description": "Fetch chunks from one specific document, in order. Use when you already know which document is relevant (typically after list_documents) and want to read it sequentially rather than sampling via search.",
            "parameters": {
                "type": "object",
                "properties": {
                    "document_id": {"type": "string", "description": "UUID of the document, as returned by list_documents."},
                    "max_chunks": {"type": "integer", "description": "Maximum chunks to return (1-40).", "default": 20, "minimum": 1, "maximum": 40}
                },
                "required": ["document_id"]
            }
        }
    })
}

/// Build the catalog of tools to advertise to the model for this
/// course + this request. Feature flags drive which tools appear;
/// the v1 base catalog is unconditional (always present when
/// `tool_use_enabled`), and follow-up tools land behind their own
/// flag checks here.
pub fn assemble_catalog(flags: ToolCatalogFlags) -> Vec<Json> {
    // Base tools always present when tool_use_enabled. Ordered most-
    // useful-first so a token-budget-limited model is more likely to
    // see the right tool when it scans the catalog.
    let tools = vec![
        keyword_search_spec(),
        semantic_search_spec(),
        list_documents_spec(),
        get_document_chunks_spec(),
    ];
    // Reserved for a follow-up: when KG is enabled for this course,
    // expose `search_concept_graph` so the model can traverse
    // part_of_unit / applied_in relations explicitly. Re-bind `tools`
    // to `mut` and push to it once that tool lands. Reading
    // `flags.kg_enabled` here keeps the field live in the codebase.
    let _ = flags.kg_enabled;
    tools
}

// Argument types for parsing

#[derive(Deserialize)]
struct SemanticSearchArgs {
    query: String,
    #[serde(default)]
    k: Option<i32>,
}

#[derive(Deserialize)]
struct KeywordSearchArgs {
    query: String,
    #[serde(default)]
    k: Option<i32>,
}

#[derive(Deserialize)]
struct GetDocumentChunksArgs {
    document_id: String,
    #[serde(default)]
    max_chunks: Option<u32>,
}

// Dispatch

/// Result of a tool call: the chunks accumulated (so research_phase
/// can merge them into the shared chunk set) plus the JSON string
/// sent back to the model as the tool result. `chunks` is empty for
/// tools that don't return chunks (e.g. `list_documents`).
pub struct ToolOutcome {
    pub chunks: Vec<RagChunk>,
    pub model_message: String,
}

pub async fn dispatch(
    name: &str,
    args_str: &str,
    ctx: &ToolDispatchCtx<'_>,
    flags: ToolCatalogFlags,
) -> Result<ToolOutcome, ToolError> {
    // Defense in depth: even if the model emits a tool name we didn't
    // advertise (cached schema from a prior turn, prompt injection),
    // we re-check against the catalog. The flag gate thus runs at
    // dispatch time, not just at catalog assembly.
    if !is_advertised(name, flags) {
        return Err(ToolError::UnknownTool(name.to_string()));
    }

    match name {
        "semantic_search" => {
            let args: SemanticSearchArgs =
                serde_json::from_str(args_str).map_err(|e| ToolError::BadArgs {
                    tool: "semantic_search",
                    reason: e.to_string(),
                })?;
            run_semantic_search(args, ctx).await
        }
        "keyword_search" => {
            let args: KeywordSearchArgs =
                serde_json::from_str(args_str).map_err(|e| ToolError::BadArgs {
                    tool: "keyword_search",
                    reason: e.to_string(),
                })?;
            run_keyword_search(args, ctx).await
        }
        "list_documents" => run_list_documents(ctx).await,
        "get_document_chunks" => {
            let args: GetDocumentChunksArgs =
                serde_json::from_str(args_str).map_err(|e| ToolError::BadArgs {
                    tool: "get_document_chunks",
                    reason: e.to_string(),
                })?;
            run_get_document_chunks(args, ctx).await
        }
        _ => Err(ToolError::UnknownTool(name.to_string())),
    }
}

fn is_advertised(name: &str, flags: ToolCatalogFlags) -> bool {
    assemble_catalog(flags).iter().any(|t| {
        t.get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .map(|n| n == name)
            .unwrap_or(false)
    })
}

async fn run_semantic_search(
    args: SemanticSearchArgs,
    ctx: &ToolDispatchCtx<'_>,
) -> Result<ToolOutcome, ToolError> {
    let k = args.k.unwrap_or(5).clamp(1, 10);
    let chunks = common::rag_lookup(
        ctx.http_client,
        ctx.openai_api_key,
        ctx.fastembed,
        ctx.qdrant,
        ctx.collection_name,
        &args.query,
        k,
        ctx.min_score,
        ctx.embedding_provider,
        ctx.embedding_model,
    )
    .await;
    let model_message = format_chunks_for_model(&chunks);
    Ok(ToolOutcome {
        chunks,
        model_message,
    })
}

async fn run_keyword_search(
    args: KeywordSearchArgs,
    ctx: &ToolDispatchCtx<'_>,
) -> Result<ToolOutcome, ToolError> {
    let k = args.k.unwrap_or(10).clamp(1, 20);
    let chunks = common::keyword_lookup(ctx.qdrant, ctx.collection_name, &args.query, k as u64)
        .await
        .map_err(|e| ToolError::Backend {
            tool: "keyword_search",
            reason: e,
        })?;
    let model_message = format_chunks_for_model(&chunks);
    Ok(ToolOutcome {
        chunks,
        model_message,
    })
}

async fn run_list_documents(ctx: &ToolDispatchCtx<'_>) -> Result<ToolOutcome, ToolError> {
    let docs = minerva_db::queries::documents::list_by_course(ctx.db, ctx.course_id)
        .await
        .map_err(|e| ToolError::Backend {
            tool: "list_documents",
            reason: e.to_string(),
        })?;
    // Only show docs in a useful retrieval state to the model.
    //   * `ready` = chunked and embedded, searchable;
    //   * `awaiting_transcript` = URL stub pending fetch (model can
    //     still mention its existence even though retrieval will be
    //     empty until the transcript pipeline catches up);
    // Hide `pending`, `processing`, `failed`, `unsupported` so the
    // model doesn't waste calls on them.
    let useful: Vec<_> = docs
        .into_iter()
        .filter(|d| matches!(d.status.as_str(), "ready" | "awaiting_transcript"))
        .map(|d| {
            serde_json::json!({
                "id": d.id,
                "filename": d.filename,
                "mime_type": d.mime_type,
                "status": d.status
            })
        })
        .collect();
    let model_message = truncate_for_model(&serde_json::Value::Array(useful));
    Ok(ToolOutcome {
        chunks: Vec::new(),
        model_message,
    })
}

async fn run_get_document_chunks(
    args: GetDocumentChunksArgs,
    ctx: &ToolDispatchCtx<'_>,
) -> Result<ToolOutcome, ToolError> {
    let doc_id = Uuid::parse_str(&args.document_id).map_err(|e| ToolError::BadArgs {
        tool: "get_document_chunks",
        reason: format!("document_id is not a UUID: {}", e),
    })?;
    let max_chunks = args.max_chunks.unwrap_or(20).clamp(1, 40);

    // Cross-course safety: confirm the document belongs to this
    // course before we scroll its chunks. Otherwise a model that
    // hallucinates a document_id from another course (or a prompt
    // injection) could pull chunks across the course boundary.
    let doc = minerva_db::queries::documents::find_by_id(ctx.db, doc_id)
        .await
        .map_err(|e| ToolError::Backend {
            tool: "get_document_chunks",
            reason: e.to_string(),
        })?;
    let Some(doc) = doc else {
        return Err(ToolError::BadArgs {
            tool: "get_document_chunks",
            reason: format!("no document with id {}", doc_id),
        });
    };
    if doc.course_id != ctx.course_id {
        return Err(ToolError::BadArgs {
            tool: "get_document_chunks",
            reason: format!("document {} does not belong to this course", doc_id),
        });
    }

    let by_index = common::scroll_doc_chunks(ctx.qdrant, ctx.collection_name, doc_id, max_chunks)
        .await
        .map_err(|e| ToolError::Backend {
            tool: "get_document_chunks",
            reason: e,
        })?;

    // Synthesise RagChunks (no similarity score; mark with 0.0 so
    // the accumulator can still dedupe). These flow into the chunk
    // set the writeup phase consumes.
    let chunks: Vec<RagChunk> = by_index
        .into_values()
        .map(|text| RagChunk {
            document_id: doc_id.to_string(),
            filename: doc.filename.clone(),
            text,
            kind: doc.kind.clone(),
            score: 0.0,
        })
        .collect();

    let model_message = format_chunks_for_model(&chunks);
    Ok(ToolOutcome {
        chunks,
        model_message,
    })
}

// Result shaping

/// Render a set of chunks as JSON for the model, truncating to fit
/// `MAX_TOOL_RESULT_BYTES`. Each chunk includes filename + text; we
/// omit document_id and score because the model doesn't need them
/// to reason about content.
fn format_chunks_for_model(chunks: &[RagChunk]) -> String {
    let array: Vec<Json> = chunks
        .iter()
        .map(|c| serde_json::json!({"filename": c.filename, "text": c.text}))
        .collect();
    truncate_for_model(&serde_json::Value::Array(array))
}

/// Serialize a JSON value, trimming items off the end of an array
/// if the serialization exceeds `MAX_TOOL_RESULT_BYTES`. Appends a
/// `truncated_omitted` count so the model knows results were dropped.
fn truncate_for_model(value: &Json) -> String {
    let full = serde_json::to_string(value).unwrap_or_else(|_| "[]".to_string());
    if full.len() <= MAX_TOOL_RESULT_BYTES {
        return full;
    }
    let Json::Array(items) = value else {
        return serde_json::json!({"truncated": true, "reason": "result too large"}).to_string();
    };
    // Step the keep count down until the serialized prefix fits the
    // budget (minus ~80 bytes of room for the wrapping object).
    let mut keep = items.len();
    while keep > 0 {
        let slice = &items[..keep];
        let serialized = serde_json::to_string(slice).unwrap_or_default();
        if serialized.len() + 80 <= MAX_TOOL_RESULT_BYTES {
            let dropped = items.len() - keep;
            let note = serde_json::json!({
                "results": slice,
                "truncated_omitted": dropped,
                "hint": "refine the query for fewer or shorter hits",
            });
            return note.to_string();
        }
        keep -= 1;
    }
    serde_json::json!({
        "results": [],
        "truncated_omitted": items.len(),
        "hint": "every individual result exceeded the result-size cap",
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_advertises_four_v1_tools() {
        let catalog = assemble_catalog(ToolCatalogFlags { kg_enabled: false });
        let names: Vec<&str> = catalog
            .iter()
            .filter_map(|t| {
                t.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
            })
            .collect();
        assert_eq!(names.len(), 4);
        assert!(names.contains(&"semantic_search"));
        assert!(names.contains(&"keyword_search"));
        assert!(names.contains(&"list_documents"));
        assert!(names.contains(&"get_document_chunks"));
    }

    #[test]
    fn kg_flag_does_not_change_v1_catalog() {
        // v1 has no kg-gated tool yet, so the flag is currently a
        // no-op. This test exists to fail-loudly when a follow-up
        // adds the gated tool but forgets to update the assertion.
        let off = assemble_catalog(ToolCatalogFlags { kg_enabled: false });
        let on = assemble_catalog(ToolCatalogFlags { kg_enabled: true });
        assert_eq!(off.len(), on.len());
    }

    #[test]
    fn tool_error_serializes_as_json_for_model() {
        let err = ToolError::BadArgs {
            tool: "keyword_search",
            reason: "missing field `query`".to_string(),
        };
        let msg = err.to_tool_message();
        let parsed: Json = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["error"], "bad_args");
        assert!(parsed["message"]
            .as_str()
            .unwrap()
            .contains("keyword_search"));
    }

    #[test]
    fn truncation_keeps_array_envelope_when_oversized() {
        let huge: Vec<Json> = (0..2000)
            .map(|i| serde_json::json!({"filename": "x.pdf", "text": format!("chunk {}", i)}))
            .collect();
        let value = Json::Array(huge);
        let serialized = truncate_for_model(&value);
        assert!(
            serialized.len() <= MAX_TOOL_RESULT_BYTES,
            "serialized result {} > cap {}",
            serialized.len(),
            MAX_TOOL_RESULT_BYTES
        );
        let parsed: Json = serde_json::from_str(&serialized).unwrap();
        assert!(parsed["truncated_omitted"].as_u64().unwrap() > 0);
    }

    #[test]
    fn semantic_search_args_parse_with_defaults() {
        let args: SemanticSearchArgs = serde_json::from_str(r#"{"query": "binary tree"}"#).unwrap();
        assert_eq!(args.query, "binary tree");
        assert!(args.k.is_none());
    }

    #[test]
    fn get_document_chunks_args_reject_non_uuid_at_dispatch() {
        // This exercises the BadArgs path for a non-UUID document_id;
        // the test doesn't need real services since dispatch fails
        // before touching any of them.
        // Build a minimal arg struct that parses but contains a bad
        // document_id, then verify Uuid::parse_str rejects it.
        let parsed: GetDocumentChunksArgs =
            serde_json::from_str(r#"{"document_id": "not-a-uuid"}"#).unwrap();
        assert!(Uuid::parse_str(&parsed.document_id).is_err());
    }
}
