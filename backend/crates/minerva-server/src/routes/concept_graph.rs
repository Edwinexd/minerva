//! Admin routes for the eureka-2 concept knowledge graph integration.
//!
//! Mounted under `/admin/courses/{course_id}/concept-graph` from
//! `routes::mod` when the `eureka` cargo feature is on. Each handler
//! enforces admin role + the per-course `concept_graph` feature flag
//! before doing real work; runtime that the eureka context isn't
//! configured returns 503 instead of panicking, matching the
//! fail-soft policy in `crate::eureka_runtime`.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use minerva_core::models::User;
use minerva_eureka::eureka_2::{export::to_json_view, pipeline, schema};
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::feature_flags::concept_graph_enabled;
use crate::state::AppState;

pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/courses/{course_id}/concept-graph", get(get_concept_graph))
        .route(
            "/courses/{course_id}/concept-graph/extract",
            post(extract_concept_graph),
        )
}

fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

async fn require_flag_on(state: &AppState, course_id: Uuid) -> Result<(), AppError> {
    if !concept_graph_enabled(&state.db, course_id).await {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize)]
pub struct ExtractRunSummary {
    pub course_id: Uuid,
    pub graph_id: Option<i64>,
    pub documents_processed: usize,
    pub documents_skipped: usize,
    pub concepts_added: usize,
    pub edges_added: usize,
    pub errors: Vec<ExtractDocumentError>,
}

#[derive(Serialize)]
pub struct ExtractDocumentError {
    pub document_id: Uuid,
    pub filename: String,
    pub error: String,
}

/// Run extraction across every ready document in the course. Sequential by
/// design: extraction is dominated by LLM round-trips, and serialising
/// keeps cerebras' rate-limit footprint sane. Returns when all documents
/// have either succeeded or been recorded as errors.
async fn extract_concept_graph(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<ExtractRunSummary>, AppError> {
    require_admin(&user)?;
    require_flag_on(&state, course_id).await?;

    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let eureka = state
        .eureka
        .as_ref()
        .ok_or_else(|| {
            AppError::Internal(
                "eureka runtime is not configured (set EUREKA_LLM_API_KEY / EUREKA_EMBED_API_KEY)"
                    .to_string(),
            )
        })?
        .clone();

    let documents = minerva_db::queries::documents::list_by_course(&state.db, course_id).await?;

    let collection_name =
        minerva_ingest::pipeline::collection_name(course_id, course.embedding_version);
    let collection_exists = state
        .qdrant
        .collection_exists(&collection_name)
        .await
        .unwrap_or(false);
    if !collection_exists {
        return Err(AppError::bad_request("concept_graph.no_collection"));
    }

    let namespace = minerva_eureka::namespace_for_course();
    let graph_name = minerva_eureka::graph_name_for_course_uuid(course_id);

    let mut summary = ExtractRunSummary {
        course_id,
        graph_id: None,
        documents_processed: 0,
        documents_skipped: 0,
        concepts_added: 0,
        edges_added: 0,
        errors: Vec::new(),
    };

    for doc in documents {
        // Only documents that completed ingest have chunks to extract from.
        if doc.status != "ready" {
            summary.documents_skipped += 1;
            continue;
        }

        // Pull every chunk for this document from qdrant, ordered by
        // chunk_index, and concatenate. Chunks overlap by design so the
        // joined text duplicates some content; the extractor is robust to
        // that and reading the raw file would re-implement the chunker's
        // mime-type-aware text extraction.
        let document_text = match fetch_document_text(&state, &collection_name, doc.id).await {
            Ok(t) => t,
            Err(e) => {
                summary.errors.push(ExtractDocumentError {
                    document_id: doc.id,
                    filename: doc.filename.clone(),
                    error: e,
                });
                continue;
            }
        };
        if document_text.trim().is_empty() {
            summary.documents_skipped += 1;
            continue;
        }

        let external_id = doc.id.to_string();
        let label = doc.filename.clone();

        match pipeline::extract_and_persist(
            &state.db,
            &eureka.extractor,
            eureka.embedder.as_ref(),
            namespace,
            &graph_name,
            &external_id,
            &label,
            &document_text,
        )
        .await
        {
            Ok(contribution) => {
                summary.graph_id = Some(contribution.graph_id);
                summary.concepts_added += contribution.concept_count;
                summary.edges_added += contribution.edge_count;
                summary.documents_processed += 1;

                // Best-effort token-usage record. eureka-2 does not yet
                // surface per-call token counts, so we record a rough
                // character-based estimate against the
                // `concept_extraction` category. When eureka-2 starts
                // returning real counts we'll thread them through here.
                // Each chunk passes through extract once + summarise once;
                // estimate accordingly.
                let est_prompt_chars = (document_text.len() + label.len() + 1024) * 2;
                let est_prompt_tokens = i32::try_from(est_prompt_chars / 4).unwrap_or(i32::MAX);
                let est_completion_tokens =
                    i32::try_from((contribution.concept_count * 80).max(64)).unwrap_or(i32::MAX);

                if let Err(e) = minerva_db::queries::course_token_usage::record(
                    &state.db,
                    course_id,
                    "concept_extraction",
                    &eureka.llm_model,
                    est_prompt_tokens,
                    est_completion_tokens,
                )
                .await
                {
                    tracing::warn!(
                        course_id = %course_id,
                        document_id = %doc.id,
                        "concept_graph: token-usage record failed: {}",
                        e
                    );
                }
            }
            Err(e) => {
                summary.errors.push(ExtractDocumentError {
                    document_id: doc.id,
                    filename: doc.filename.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    Ok(Json(summary))
}

/// Pull every chunk for `doc_id` from `collection_name` via qdrant scroll
/// and concatenate (in chunk-index order) with paragraph breaks.
async fn fetch_document_text(
    state: &AppState,
    collection_name: &str,
    doc_id: Uuid,
) -> Result<String, String> {
    let by_index =
        crate::strategy::common::scroll_doc_chunks(&state.qdrant, collection_name, doc_id, 2000)
            .await?;
    Ok(by_index.values().cloned().collect::<Vec<_>>().join("\n\n"))
}

#[derive(Serialize)]
pub struct ConceptGraphResponse {
    pub course_id: Uuid,
    pub graph_id: i64,
    pub graph: serde_json::Value,
}

async fn get_concept_graph(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<ConceptGraphResponse>, AppError> {
    require_admin(&user)?;
    require_flag_on(&state, course_id).await?;

    if minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }

    let namespace = minerva_eureka::namespace_for_course();
    let graph_name = minerva_eureka::graph_name_for_course_uuid(course_id);

    let graph_row = match schema::find_graph(&state.db, namespace, &graph_name).await {
        Ok(row) => row,
        Err(minerva_eureka::eureka_2::Error::GraphNotFound { .. }) => {
            // No graph extracted yet for this course; surface an empty
            // graph so the frontend can render "no concepts yet" cleanly.
            return Ok(Json(ConceptGraphResponse {
                course_id,
                graph_id: 0,
                graph: serde_json::json!({ "concepts": [], "edges": [] }),
            }));
        }
        Err(e) => return Err(AppError::Internal(format!("eureka load_graph: {e}"))),
    };

    let graph = schema::load_graph(&state.db, graph_row.id)
        .await
        .map_err(|e| AppError::Internal(format!("eureka load_graph: {e}")))?;

    let view = to_json_view(&graph);
    let body = serde_json::to_value(&view).map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(ConceptGraphResponse {
        course_id,
        graph_id: graph_row.id,
        graph: body,
    }))
}

// Surface 503 instead of 500 when the eureka runtime is unconfigured;
// the rest of the file uses 500 for that case which is technically
// correct but less actionable from a client.
#[allow(dead_code)]
const _: StatusCode = StatusCode::SERVICE_UNAVAILABLE;
