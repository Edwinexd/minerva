//! Edges in the course knowledge graph. Populated by the cross-doc
//! linking pass; consumed by the graph-view endpoint.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DocumentRelationRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub src_doc_id: Uuid,
    pub dst_doc_id: Uuid,
    /// One of: `solution_of`, `part_of_unit`. The DB CHECK enforces the
    /// closed set; new relations need a migration.
    pub relation: String,
    pub confidence: f32,
    pub rationale: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Insert or update a single edge. Idempotent on
/// `(src_doc_id, dst_doc_id, relation)` -- re-running the linker just
/// refreshes confidence/rationale rather than duplicating rows.
pub async fn upsert(
    db: &PgPool,
    course_id: Uuid,
    src_doc_id: Uuid,
    dst_doc_id: Uuid,
    relation: &str,
    confidence: f32,
    rationale: Option<&str>,
) -> Result<DocumentRelationRow, sqlx::Error> {
    sqlx::query_as!(
        DocumentRelationRow,
        r#"INSERT INTO document_relations
            (course_id, src_doc_id, dst_doc_id, relation, confidence, rationale)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (src_doc_id, dst_doc_id, relation)
        DO UPDATE SET
            confidence = EXCLUDED.confidence,
            rationale = EXCLUDED.rationale,
            updated_at = NOW()
        RETURNING id, course_id, src_doc_id, dst_doc_id, relation, confidence, rationale, created_at, updated_at"#,
        course_id,
        src_doc_id,
        dst_doc_id,
        relation,
        confidence,
        rationale,
    )
    .fetch_one(db)
    .await
}

pub async fn list_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<DocumentRelationRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRelationRow,
        "SELECT id, course_id, src_doc_id, dst_doc_id, relation, confidence, rationale, created_at, updated_at FROM document_relations WHERE course_id = $1 ORDER BY relation, created_at",
        course_id,
    )
    .fetch_all(db)
    .await
}

/// Delete every edge in a course. Used at the start of the linker
/// pass when we want a clean rebuild rather than incremental updates.
/// Cascades come from FK ON DELETE so a doc deletion already cleans
/// its edges; this is just for full re-link.
pub async fn delete_by_course(db: &PgPool, course_id: Uuid) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM document_relations WHERE course_id = $1",
        course_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Delete edges below a confidence threshold. Cheaper alternative to
/// a full rebuild when the linker is re-run with stricter tolerance.
#[allow(dead_code)]
pub async fn delete_below_confidence(
    db: &PgPool,
    course_id: Uuid,
    min_confidence: f32,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM document_relations WHERE course_id = $1 AND confidence < $2",
        course_id,
        min_confidence,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
