//! Per-pair LLM decision cache for the cross-doc linker.
//!
//! The linker calls gpt-oss-120b once per candidate pair to label
//! the relation. Without a cache, every relink (triggered by every
//! ingest) re-asks the model about every pair, even when nothing
//! has changed. This table records every decision the model has
//! returned; INCLUDING `none`; keyed by the unordered pair
//! (`a_doc_id < b_doc_id`), with snapshot timestamps for both
//! endpoints. The linker checks the cache before each call and
//! only invokes the LLM when at least one endpoint has been
//! re-classified since the cached decision was made.

use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DecisionRow {
    pub a_doc_id: Uuid,
    pub b_doc_id: Uuid,
    pub decided_at: chrono::DateTime<chrono::Utc>,
    pub a_classified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub b_classified_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `None` means the model said "none" / no relation; still
    /// recorded so we don't re-ask. `Some(_)` mirrors the
    /// `document_relations.relation` enum.
    pub relation: Option<String>,
    pub confidence: Option<f32>,
}

/// Load every decision ever recorded for this course into a
/// HashMap keyed by the unordered pair (always `min < max`). The
/// linker uses this to short-circuit LLM calls.
pub async fn list_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<HashMap<(Uuid, Uuid), DecisionRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        DecisionRow,
        r#"SELECT a_doc_id, b_doc_id, decided_at, a_classified_at, b_classified_at,
                  relation, confidence
           FROM linker_decisions
           WHERE course_id = $1"#,
        course_id,
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ((r.a_doc_id, r.b_doc_id), r))
        .collect())
}

/// Record a decision (or update the cached one) for a pair. The
/// caller is responsible for normalising the (a, b) order so
/// `a_doc_id < b_doc_id`; this lets the table's PK + CHECK
/// constraint catch ordering bugs at write time.
#[allow(clippy::too_many_arguments)]
pub async fn upsert(
    db: &PgPool,
    course_id: Uuid,
    a_doc_id: Uuid,
    b_doc_id: Uuid,
    a_classified_at: Option<chrono::DateTime<chrono::Utc>>,
    b_classified_at: Option<chrono::DateTime<chrono::Utc>>,
    relation: Option<&str>,
    confidence: Option<f32>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO linker_decisions
            (course_id, a_doc_id, b_doc_id, decided_at,
             a_classified_at, b_classified_at, relation, confidence)
        VALUES ($1, $2, $3, NOW(), $4, $5, $6, $7)
        ON CONFLICT (a_doc_id, b_doc_id) DO UPDATE
            SET decided_at = NOW(),
                a_classified_at = EXCLUDED.a_classified_at,
                b_classified_at = EXCLUDED.b_classified_at,
                relation = EXCLUDED.relation,
                confidence = EXCLUDED.confidence"#,
        course_id,
        a_doc_id,
        b_doc_id,
        a_classified_at,
        b_classified_at,
        relation,
        confidence,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Number of cached decisions whose endpoints have moved past the
/// snapshot timestamps; i.e. how many pairs the linker would have
/// to re-evaluate on the next relink. Surfaced to the graph viewer
/// so the teacher sees "linking pending" while ingest catches up.
pub async fn stale_decisions_for_course(db: &PgPool, course_id: Uuid) -> Result<i64, sqlx::Error> {
    let n: Option<i64> = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!"
           FROM linker_decisions ld
           JOIN documents da ON da.id = ld.a_doc_id
           JOIN documents db ON db.id = ld.b_doc_id
           WHERE ld.course_id = $1
             AND (
                 da.classified_at IS DISTINCT FROM ld.a_classified_at
              OR db.classified_at IS DISTINCT FROM ld.b_classified_at
             )"#,
        course_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(n.unwrap_or(0))
}
