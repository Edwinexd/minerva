use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct DocumentRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub status: String,
    pub chunk_count: Option<i32>,
    pub error_msg: Option<String>,
    pub uploaded_by: Uuid,
    pub displayable: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub processed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Origin URL for URL-sourced documents (preserved across the
    /// awaiting_transcript -> text/plain transition so dedup stays correct).
    pub source_url: Option<String>,
    /// Knowledge-graph classification (nullable until classifier has run).
    /// One of: lecture, reading, assignment_brief, sample_solution,
    /// lab_brief, exam, syllabus, unknown. CHECK constraint enforces the set.
    pub kind: Option<String>,
    /// Confidence in [0.0, 1.0] reported by the classifier.
    pub kind_confidence: Option<f32>,
    /// One-line rationale from the classifier (optional, for teacher review UI).
    pub kind_rationale: Option<String>,
    /// True when a teacher has set the kind manually. Auto-classification
    /// must skip these rows.
    pub kind_locked_by_teacher: bool,
    pub classified_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Mean-pooled embedding across the doc's chunks, under the
    /// course's configured embedding model. Used by the cross-doc
    /// linker for embedding-based candidate generation. NULL until
    /// the pipeline (or a lazy backfill in the linker) populates it.
    pub pooled_embedding: Option<Vec<f32>>,
}

// Note: sqlx::query_as! macros require literal SQL strings, so the column
// list is repeated below. When adding a new column, update every `SELECT …`
// and `RETURNING …` site in this file; the compile-time check will catch
// the row struct vs query column mismatch but won't catch a column missing
// from a SELECT that still happens to compile.

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
    uploaded_by: Uuid,
    source_url: Option<&str>,
) -> Result<DocumentRow, sqlx::Error> {
    sqlx::query_as!(
        DocumentRow,
        r#"INSERT INTO documents (id, course_id, filename, mime_type, size_bytes, uploaded_by, source_url)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at, source_url, kind, kind_confidence, kind_rationale, kind_locked_by_teacher, classified_at, pooled_embedding"#,
        id,
        course_id,
        filename,
        mime_type,
        size_bytes,
        uploaded_by,
        source_url,
    )
    .fetch_one(db)
    .await
}

pub async fn list_by_course(db: &PgPool, course_id: Uuid) -> Result<Vec<DocumentRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRow,
        "SELECT id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at, source_url, kind, kind_confidence, kind_rationale, kind_locked_by_teacher, classified_at, pooled_embedding FROM documents WHERE course_id = $1 ORDER BY created_at DESC",
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<DocumentRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRow,
        "SELECT id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at, source_url, kind, kind_confidence, kind_rationale, kind_locked_by_teacher, classified_at, pooled_embedding FROM documents WHERE id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

/// Look up a document in a course by its origin URL. Used for idempotency
/// in the URL-document creation flow.
pub async fn find_by_course_source_url(
    db: &PgPool,
    course_id: Uuid,
    source_url: &str,
) -> Result<Option<DocumentRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRow,
        "SELECT id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at, source_url, kind, kind_confidence, kind_rationale, kind_locked_by_teacher, classified_at, pooled_embedding FROM documents WHERE course_id = $1 AND source_url = $2",
        course_id,
        source_url,
    )
    .fetch_optional(db)
    .await
}

pub async fn update_displayable(
    db: &PgPool,
    id: Uuid,
    displayable: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE documents SET displayable = $1 WHERE id = $2",
        displayable,
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Returns the set of document IDs in a course that are NOT displayable.
pub async fn hidden_document_ids(
    db: &PgPool,
    course_id: Uuid,
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    let rows = sqlx::query_scalar!(
        "SELECT id FROM documents WHERE course_id = $1 AND displayable = FALSE",
        course_id,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|id| id.to_string()).collect())
}

pub async fn delete(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM documents WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Atomically claim up to `limit` pending documents for processing.
/// Uses `FOR UPDATE SKIP LOCKED` so multiple workers won't grab the same row.
pub async fn claim_pending(db: &PgPool, limit: i32) -> Result<Vec<DocumentRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRow,
        r#"UPDATE documents
        SET status = 'processing', processing_started_at = NOW()
        WHERE id IN (
            SELECT id FROM documents
            WHERE status = 'pending'
            ORDER BY created_at ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        RETURNING id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at, source_url, kind, kind_confidence, kind_rationale, kind_locked_by_teacher, classified_at, pooled_embedding"#,
        limit as i64,
    )
    .fetch_all(db)
    .await
}

/// List documents awaiting external transcript processing.
/// These are play.dsv.su.se URL documents that the worker has triaged.
pub async fn list_awaiting_transcripts(db: &PgPool) -> Result<Vec<DocumentRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRow,
        "SELECT id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at, source_url, kind, kind_confidence, kind_rationale, kind_locked_by_teacher, classified_at, pooled_embedding FROM documents WHERE status = 'awaiting_transcript' ORDER BY created_at ASC",
    )
    .fetch_all(db)
    .await
}

/// Replace a document's bytes-on-disk metadata and reset it to `pending` so
/// the ingest worker re-chunks it. Caller is responsible for having already
/// cleared the old Qdrant chunks (otherwise stale vectors will coexist with
/// the new ones).
///
/// Also clears `classified_at` (only when the kind isn't teacher-locked).
/// Replacing a file's content makes the previous classification stale, so
/// the chat-time filter must treat the doc as unclassified until the
/// worker re-runs the classifier on the new bytes. Keeping a teacher's
/// manual lock honours the "manual override wins" contract.
pub async fn reset_for_resync(
    db: &PgPool,
    id: Uuid,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE documents
           SET filename = $1,
               mime_type = $2,
               size_bytes = $3,
               status = 'pending',
               error_msg = NULL,
               chunk_count = NULL,
               processed_at = NULL,
               processing_started_at = NULL,
               classified_at = CASE WHEN kind_locked_by_teacher THEN classified_at ELSE NULL END,
               kind = CASE WHEN kind_locked_by_teacher THEN kind ELSE NULL END,
               kind_confidence = CASE WHEN kind_locked_by_teacher THEN kind_confidence ELSE NULL END,
               kind_rationale = CASE WHEN kind_locked_by_teacher THEN kind_rationale ELSE NULL END,
               pooled_embedding = NULL
           WHERE id = $4"#,
        filename,
        mime_type,
        size_bytes,
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Update a document's filename, mime_type, size, and reset status to 'pending'.
/// Used when replacing a URL stub with actual transcript content.
pub async fn replace_with_transcript(
    db: &PgPool,
    id: Uuid,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE documents SET filename = $1, mime_type = $2, size_bytes = $3, status = 'pending', error_msg = NULL, chunk_count = NULL, processed_at = NULL WHERE id = $4 AND status = 'awaiting_transcript'",
        filename,
        mime_type,
        size_bytes,
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Reset documents stuck in 'processing' back to 'pending'.
/// Used on startup for crash recovery: any document still marked 'processing'
/// was interrupted by a server restart.
pub async fn reset_stale_processing(db: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE documents SET status = 'pending', processing_started_at = NULL WHERE status = 'processing'",
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Reset documents that have been stuck in 'processing' for longer than
/// `min_age_seconds`. Run periodically so a silently-panicked or wedged
/// processing task doesn't leave a document stuck until the next pod restart.
///
/// Documents being actively processed (started <= threshold ago) are left
/// alone. The threshold should comfortably exceed the worst-case processing
/// time for a single document.
pub async fn reset_stale_processing_older_than(
    db: &PgPool,
    min_age_seconds: i64,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE documents
        SET status = 'pending', processing_started_at = NULL
        WHERE status = 'processing'
          AND (processing_started_at IS NULL
               OR processing_started_at < NOW() - make_interval(secs => $1))"#,
        min_age_seconds as f64,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

// ── Classification helpers ─────────────────────────────────────────

/// Persist a new auto-classification result. No-ops when the row is locked
/// by a teacher; defense in depth on top of the application-layer check
/// in the worker. Returns true iff a row was actually updated.
pub async fn set_classification(
    db: &PgPool,
    doc_id: Uuid,
    kind: &str,
    confidence: f32,
    rationale: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE documents
        SET kind = $2,
            kind_confidence = $3,
            kind_rationale = $4,
            classified_at = NOW()
        WHERE id = $1
          AND kind_locked_by_teacher = FALSE"#,
        doc_id,
        kind,
        confidence,
        rationale,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Set a kind manually and lock it against future auto-classification.
/// Clears confidence/rationale (they're only meaningful for auto-classified
/// rows). The CHECK constraint will reject invalid `kind` values at the DB
/// level; we re-validate in the route handler too.
pub async fn set_kind_locked(db: &PgPool, doc_id: Uuid, kind: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE documents
        SET kind = $2,
            kind_confidence = NULL,
            kind_rationale = NULL,
            kind_locked_by_teacher = TRUE,
            classified_at = NOW()
        WHERE id = $1"#,
        doc_id,
        kind,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Clear the teacher lock so the next reclassification pass can overwrite
/// the kind. Does not change the current kind value; operator can call
/// reclassify afterwards if they want a fresh run.
pub async fn clear_kind_lock(db: &PgPool, doc_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE documents SET kind_locked_by_teacher = FALSE WHERE id = $1",
        doc_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// IDs of docs in a course whose `kind` is in the given list. Used by the
/// chat-time RAG filter to drop chunks that came from kinds we never want
/// pasted into the prompt context (assignment_brief / lab_brief / exam /
/// sample_solution as defense-in-depth in case stale vectors exist).
pub async fn doc_ids_with_kind(
    db: &PgPool,
    course_id: Uuid,
    kinds: &[&str],
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    let kinds_owned: Vec<String> = kinds.iter().map(|s| s.to_string()).collect();
    let rows = sqlx::query_scalar!(
        "SELECT id FROM documents WHERE course_id = $1 AND kind = ANY($2)",
        course_id,
        &kinds_owned,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|id| id.to_string()).collect())
}

/// IDs of docs in a course whose classification has not yet completed
/// (`classified_at IS NULL`). The chat-time filter excludes their chunks
/// from the prompt context; defensive: we'd rather give a slightly worse
/// answer for the ~30s after upload than risk leaking an unclassified
/// sample-solution into context.
pub async fn unclassified_doc_ids(
    db: &PgPool,
    course_id: Uuid,
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    let rows = sqlx::query_scalar!(
        "SELECT id FROM documents WHERE course_id = $1 AND classified_at IS NULL",
        course_id,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|id| id.to_string()).collect())
}

/// List docs that need (re)classification. `limit` caps batch size for
/// the admin backfill so we don't pull a huge installation's worth into
/// memory in one shot.
pub async fn list_needing_classification(
    db: &PgPool,
    limit: i64,
) -> Result<Vec<DocumentRow>, sqlx::Error> {
    sqlx::query_as!(
        DocumentRow,
        "SELECT id, course_id, filename, mime_type, size_bytes, status, chunk_count, error_msg, uploaded_by, displayable, created_at, processed_at, source_url, kind, kind_confidence, kind_rationale, kind_locked_by_teacher, classified_at, pooled_embedding FROM documents WHERE classified_at IS NULL AND kind_locked_by_teacher = FALSE AND status = 'ready' ORDER BY created_at ASC LIMIT $1",
        limit,
    )
    .fetch_all(db)
    .await
}

/// Aggregate classification counts across the whole installation.
/// Used by the admin "backfill" UI to show how much work is pending
/// before the operator clicks the button.
#[derive(Debug, Clone, Copy)]
pub struct ClassificationStats {
    /// Total `documents.status = 'ready'` rows. Other statuses
    /// (`pending`, `processing`, `unsupported`, `failed`,
    /// `awaiting_transcript`) aren't backfill candidates.
    pub total_ready: i64,
    /// Ready docs with a non-NULL kind, regardless of source
    /// (auto-classified or teacher-locked).
    pub classified: i64,
    /// Ready docs whose kind is NULL and aren't locked by a teacher
    /// (i.e. eligible backfill targets).
    pub unclassified: i64,
    /// Docs whose kind was set/locked by a teacher.
    pub locked_by_teacher: i64,
}

/// Persist a doc's mean-pooled embedding. Called from the pipeline
/// once all chunk embeddings are known. Idempotent; the linker may
/// also lazily fill this in for older docs.
pub async fn set_pooled_embedding(
    db: &PgPool,
    doc_id: Uuid,
    embedding: &[f32],
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE documents SET pooled_embedding = $2 WHERE id = $1",
        doc_id,
        embedding,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn classification_stats(db: &PgPool) -> Result<ClassificationStats, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE status = 'ready') AS "total_ready!",
            COUNT(*) FILTER (WHERE status = 'ready' AND kind IS NOT NULL) AS "classified!",
            COUNT(*) FILTER (WHERE status = 'ready' AND kind IS NULL AND kind_locked_by_teacher = FALSE) AS "unclassified!",
            COUNT(*) FILTER (WHERE kind_locked_by_teacher = TRUE) AS "locked_by_teacher!"
        FROM documents
        "#
    )
    .fetch_one(db)
    .await?;

    Ok(ClassificationStats {
        total_ready: row.total_ready,
        classified: row.classified,
        unclassified: row.unclassified,
        locked_by_teacher: row.locked_by_teacher,
    })
}
