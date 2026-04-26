//! Edges in the course knowledge graph. Populated by the cross-doc
//! linking pass; consumed by the graph-view endpoint.
//!
//! Two related state slices live here:
//!
//! 1. `document_relations` -- the live edge set the linker rewrites on
//!    every pass. Each row also carries a `rejected_by_teacher` flag
//!    so the graph viewer can hide an edge a teacher vetoed without
//!    losing the audit trail.
//!
//! 2. `rejected_edge_pairs` -- a separate veto list keyed by the
//!    (src, dst, relation) triple. The linker reads this BEFORE
//!    proposing edges so a rejected pair never re-appears in the
//!    output, even if the model would otherwise re-suggest it. We
//!    keep this in its own table because `delete_by_course` blows
//!    away `document_relations` on every relink and we don't want
//!    teacher vetoes to be collateral damage.

use sqlx::PgPool;
use std::collections::HashSet;
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
    pub rejected_by_teacher: bool,
    pub rejected_at: Option<chrono::DateTime<chrono::Utc>>,
    pub rejected_by: Option<Uuid>,
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
        RETURNING id, course_id, src_doc_id, dst_doc_id, relation, confidence, rationale, rejected_by_teacher, rejected_at, rejected_by, created_at, updated_at"#,
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

/// Fetch a single edge by id. Used by the per-edge-reject route.
pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<DocumentRelationRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRelationRow,
        "SELECT id, course_id, src_doc_id, dst_doc_id, relation, confidence, rationale, rejected_by_teacher, rejected_at, rejected_by, created_at, updated_at FROM document_relations WHERE id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<DocumentRelationRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRelationRow,
        "SELECT id, course_id, src_doc_id, dst_doc_id, relation, confidence, rationale, rejected_by_teacher, rejected_at, rejected_by, created_at, updated_at FROM document_relations WHERE course_id = $1 ORDER BY relation, created_at",
        course_id,
    )
    .fetch_all(db)
    .await
}

/// Delete every edge in a course. Used by the (full-rebuild) admin
/// path. Cascades come from FK ON DELETE so a doc deletion already
/// cleans its edges; this is just for full re-link.
///
/// Note: this does NOT touch `rejected_edge_pairs`. Teacher vetoes
/// must survive a relink -- otherwise the next pass would re-propose
/// every rejected edge.
#[allow(dead_code)]
pub async fn delete_by_course(db: &PgPool, course_id: Uuid) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM document_relations WHERE course_id = $1",
        course_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Per-pair cleanup the linker uses when a freshly-evaluated pair
/// gets a different relation than its prior decision (or "none"
/// where there used to be a relation). Drops any existing edge for
/// the unordered pair so the new decision can upsert without
/// colliding with a stale row pointing in the wrong direction.
pub async fn delete_relations_for_pair(db: &PgPool, a: Uuid, b: Uuid) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM document_relations
         WHERE (src_doc_id = $1 AND dst_doc_id = $2)
            OR (src_doc_id = $2 AND dst_doc_id = $1)",
        a,
        b,
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

// ── Per-edge teacher rejection ─────────────────────────────────────

/// Mark a stored edge as rejected by a teacher. Also writes the
/// veto into `rejected_edge_pairs` so the next linker pass skips
/// the pair entirely. Returns true iff the edge existed.
pub async fn reject_edge(
    db: &PgPool,
    edge_id: Uuid,
    rejected_by: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;

    let edge = sqlx::query!(
        r#"UPDATE document_relations
           SET rejected_by_teacher = TRUE,
               rejected_at = NOW(),
               rejected_by = $2,
               updated_at = NOW()
           WHERE id = $1
           RETURNING course_id, src_doc_id, dst_doc_id, relation"#,
        edge_id,
        rejected_by,
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(edge) = edge else {
        tx.rollback().await?;
        return Ok(false);
    };

    sqlx::query!(
        r#"INSERT INTO rejected_edge_pairs
            (course_id, src_doc_id, dst_doc_id, relation, rejected_by)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (src_doc_id, dst_doc_id, relation) DO UPDATE
            SET rejected_at = NOW(),
                rejected_by = EXCLUDED.rejected_by"#,
        edge.course_id,
        edge.src_doc_id,
        edge.dst_doc_id,
        edge.relation,
        rejected_by,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

/// Undo a teacher rejection: clear the flag on `document_relations`
/// AND remove the veto from `rejected_edge_pairs` so the next linker
/// pass can re-propose the pair if the model still likes it.
pub async fn unreject_edge(db: &PgPool, edge_id: Uuid) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;

    let edge = sqlx::query!(
        r#"UPDATE document_relations
           SET rejected_by_teacher = FALSE,
               rejected_at = NULL,
               rejected_by = NULL,
               updated_at = NOW()
           WHERE id = $1
           RETURNING src_doc_id, dst_doc_id, relation"#,
        edge_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(edge) = edge else {
        tx.rollback().await?;
        return Ok(false);
    };

    sqlx::query!(
        "DELETE FROM rejected_edge_pairs WHERE src_doc_id = $1 AND dst_doc_id = $2 AND relation = $3",
        edge.src_doc_id,
        edge.dst_doc_id,
        edge.relation,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct RejectedPairKey {
    pub src_doc_id: Uuid,
    pub dst_doc_id: Uuid,
    pub relation: String,
}

/// Load every teacher-rejected pair in a course. The linker calls this
/// once before generating candidates and skips any pair that comes back.
/// Returns a HashSet for O(1) membership tests.
pub async fn rejected_pairs_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<HashSet<RejectedPairKey>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT src_doc_id, dst_doc_id, relation
           FROM rejected_edge_pairs
           WHERE course_id = $1"#,
        course_id,
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RejectedPairKey {
            src_doc_id: r.src_doc_id,
            dst_doc_id: r.dst_doc_id,
            relation: r.relation,
        })
        .collect())
}
