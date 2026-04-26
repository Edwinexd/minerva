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

// ── Graph-aware retrieval helpers ──────────────────────────────────

/// For each source doc id, the set of "complementary" doc ids the
/// chat path should consider pulling additional context from. Used
/// by `strategy::common::expand_context_via_graph` to enrich the
/// prompt with material the embedding search alone might have
/// missed.
///
/// Two relation kinds qualify:
///   * `part_of_unit`: undirected, both sides are siblings in the
///     same course unit. The student's question matched one; its
///     unit partners (the lecture's section summary, the
///     accompanying reading, etc.) are reasonable additional
///     context.
///   * `applied_in` (src side only): the source doc is theoretical
///     content (lecture / reading) and the dst doc applies it in
///     practice. When a student asks about the lecture, the
///     applying tutorial is useful background; the reverse
///     direction (practice -> theory) is intentionally NOT pulled
///     so an assignment query doesn't drag the lecture's full
///     content into context unsolicited.
///
/// Rejected edges (`document_relations.rejected_by_teacher`) are
/// excluded -- a teacher veto on an edge means it shouldn't be
/// part of the inference signal either.
///
/// Returned map keys are normalised: each input doc id appears as
/// a key whether or not it has partners (an empty Vec means no
/// partners).
pub async fn unit_partners_for_docs(
    db: &PgPool,
    course_id: Uuid,
    source_doc_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<Uuid>>, sqlx::Error> {
    if source_doc_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = sqlx::query!(
        r#"SELECT src_doc_id, dst_doc_id, relation
           FROM document_relations
           WHERE course_id = $1
             AND rejected_by_teacher = FALSE
             AND (
                 (relation = 'part_of_unit'
                     AND (src_doc_id = ANY($2) OR dst_doc_id = ANY($2)))
                 OR
                 (relation = 'applied_in' AND src_doc_id = ANY($2))
             )"#,
        course_id,
        source_doc_ids,
    )
    .fetch_all(db)
    .await?;

    let mut out: std::collections::HashMap<Uuid, Vec<Uuid>> = std::collections::HashMap::new();
    for src in source_doc_ids {
        out.entry(*src).or_default();
    }
    let source_set: std::collections::HashSet<Uuid> = source_doc_ids.iter().copied().collect();
    for r in rows {
        let (key, partner) = match r.relation.as_str() {
            "part_of_unit" => {
                // Undirected: figure out which side is the source
                // (might be either), surface the OTHER as partner.
                if source_set.contains(&r.src_doc_id) {
                    (r.src_doc_id, r.dst_doc_id)
                } else {
                    (r.dst_doc_id, r.src_doc_id)
                }
            }
            "applied_in" => {
                // Filtered by SQL to src-side only: source is
                // theoretical, dst is the practice doc.
                (r.src_doc_id, r.dst_doc_id)
            }
            _ => continue,
        };
        // Don't list a doc as its own partner.
        if partner != key {
            out.entry(key).or_default().push(partner);
        }
    }
    Ok(out)
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
