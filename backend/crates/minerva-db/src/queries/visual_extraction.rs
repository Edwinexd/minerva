//! Job queue for the visual extraction pipeline (slide OCR for Play lectures,
//! figure extraction for PDFs) and the Slurm workers that drain it.
//!
//! Lifecycle of a job, and who moves it:
//!
//! - `needs_source` -> `fetching`: GitHub Actions reserves a staging slot
//!   ([`reserve_fetch_slots`]), only for Play lectures.
//! - `fetching` -> `ready`: the staged video and cues arrive ([`mark_staged`]).
//!   PDFs are enqueued straight into `ready`.
//! - `ready` -> `leased`: a worker takes it ([`lease_next`]).
//! - `leased` -> `done` / back to `ready` / `failed`: the result is accepted
//!   ([`complete`]), or the attempt fails ([`fail_attempt`]), or its worker
//!   dies or the lease runs out ([`requeue_worker`], [`requeue_expired`]).
//!
//! Every transition is a guarded UPDATE on the expected status (and attempt,
//! where a worker is involved), so a stale actor changes nothing.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct FetchSlot {
    pub job_id: Uuid,
    pub document_id: Uuid,
    pub course_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: Uuid,
    pub document_id: Uuid,
    pub course_id: Uuid,
    pub kind: String,
    pub status: String,
    pub attempts: i32,
    pub staged_path: Option<String>,
    pub worker_id: Option<Uuid>,
    pub cues: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct WorkerRow {
    pub id: Uuid,
    pub slurm_job_id: Option<i64>,
    pub state: String,
    pub submitted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Enqueue every Play lecture and PDF that has no job yet. Play lectures are
/// `.url` stubs that are awaiting their transcript or already have a text
/// child (only Play stubs take either path); PDFs are ingested PDF documents.
/// Recent material gets a higher priority so new uploads overtake the
/// historical backlog. Returns how many jobs were created.
pub async fn enqueue_missing(
    db: &PgPool,
    pipeline_version: i32,
    recent_days: i32,
) -> Result<u64, sqlx::Error> {
    let lectures = sqlx::query!(
        r#"
        INSERT INTO visual_extraction_jobs
            (document_id, course_id, kind, status, priority, pipeline_version)
        SELECT d.id, d.course_id, 'play_lecture', 'needs_source',
               CASE WHEN d.created_at > NOW() - make_interval(days => $2) THEN 100 ELSE 0 END,
               $1
        FROM documents d
        WHERE d.mime_type = 'text/x-url'
          AND d.orphaned_at IS NULL
          AND (
              d.status = 'awaiting_transcript'
              OR EXISTS (
                  SELECT 1 FROM documents c
                  WHERE c.parent_document_id = d.id
                    AND c.orphaned_at IS NULL
                    AND c.mime_type = 'text/plain'
              )
          )
        ON CONFLICT (document_id) DO NOTHING
        "#,
        pipeline_version,
        recent_days,
    )
    .execute(db)
    .await?
    .rows_affected();

    let pdfs = sqlx::query!(
        r#"
        INSERT INTO visual_extraction_jobs
            (document_id, course_id, kind, status, priority, pipeline_version)
        SELECT d.id, d.course_id, 'pdf', 'ready',
               CASE WHEN d.created_at > NOW() - make_interval(days => $2) THEN 50 ELSE 0 END,
               $1
        FROM documents d
        WHERE d.mime_type = 'application/pdf'
          AND d.orphaned_at IS NULL
          AND d.status = 'ready'
        ON CONFLICT (document_id) DO NOTHING
        "#,
        pipeline_version,
        recent_days,
    )
    .execute(db)
    .await?
    .rows_affected();

    Ok(lectures + pdfs)
}

/// Hand out up to `limit` lectures to fetch, as many as fit the disk:
/// `free_bytes` (measured on the staging filesystem) minus `min_free_bytes`
/// minus what reserved slots will still write, at `estimate_bytes` each.
/// Staged videos are already on disk and so already out of `free_bytes`.
pub async fn reserve_fetch_slots(
    db: &PgPool,
    free_bytes: i64,
    min_free_bytes: i64,
    estimate_bytes: i64,
    limit: i64,
) -> Result<Vec<FetchSlot>, sqlx::Error> {
    let mut tx = db.begin().await?;
    // Serialise reservations so two concurrent callers cannot both see the
    // same free space.
    sqlx::query!("SELECT pg_advisory_xact_lock(hashtext('visual_extraction_staging'))")
        .execute(&mut *tx)
        .await?;
    let pending = sqlx::query_scalar!(
        r#"
        SELECT COALESCE(SUM(staged_bytes), 0)::BIGINT AS "pending!"
        FROM visual_extraction_jobs
        WHERE status = 'fetching'
        "#
    )
    .fetch_one(&mut *tx)
    .await?;
    let slots = ((free_bytes - min_free_bytes - pending) / estimate_bytes.max(1)).clamp(0, limit);
    if slots == 0 {
        tx.commit().await?;
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as!(
        FetchSlot,
        r#"
        UPDATE visual_extraction_jobs
        SET status = 'fetching', fetch_reserved_at = NOW(), staged_bytes = $1, updated_at = NOW()
        WHERE id IN (
            SELECT id FROM visual_extraction_jobs
            WHERE status = 'needs_source'
            ORDER BY priority DESC, created_at
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        RETURNING id AS job_id, document_id, course_id
        "#,
        estimate_bytes,
        slots,
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows)
}

/// Record a staged lecture video and its cues; the job becomes leasable.
pub async fn mark_staged(
    db: &PgPool,
    job_id: Uuid,
    staged_path: &str,
    staged_bytes: i64,
    cues: &serde_json::Value,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET status = 'ready', staged_path = $2, staged_bytes = $3, cues = $4,
            error_msg = NULL, updated_at = NOW()
        WHERE id = $1 AND status = 'fetching'
        "#,
        job_id,
        staged_path,
        staged_bytes,
        cues,
    )
    .execute(db)
    .await?
    .rows_affected();
    Ok(done > 0)
}

/// GitHub Actions could not get this lecture (no video, no transcript yet,
/// Play error). `retry` puts it back for a later run; otherwise it is
/// parked as `unavailable`.
pub async fn release_fetch(
    db: &PgPool,
    job_id: Uuid,
    error_msg: &str,
    retry: bool,
) -> Result<bool, sqlx::Error> {
    let status = if retry { "needs_source" } else { "unavailable" };
    let done = sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET status = $2, staged_bytes = NULL, fetch_reserved_at = NULL,
            error_msg = $3, updated_at = NOW()
        WHERE id = $1 AND status = 'fetching'
        "#,
        job_id,
        status,
        error_msg,
    )
    .execute(db)
    .await?
    .rows_affected();
    Ok(done > 0)
}

/// Reservations whose upload never arrived go back to `needs_source`.
pub async fn release_stale_fetches(db: &PgPool, older_than_secs: i64) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET status = 'needs_source', staged_bytes = NULL, fetch_reserved_at = NULL, updated_at = NOW()
        WHERE status = 'fetching'
          AND fetch_reserved_at < NOW() - make_interval(secs => $1)
        "#,
        older_than_secs as f64,
    )
    .execute(db)
    .await?
    .rows_affected())
}

pub async fn find_job(db: &PgPool, job_id: Uuid) -> Result<Option<JobRow>, sqlx::Error> {
    sqlx::query_as!(
        JobRow,
        r#"
        SELECT id, document_id, course_id, kind, status, attempts, staged_path, worker_id, cues
        FROM visual_extraction_jobs WHERE id = $1
        "#,
        job_id,
    )
    .fetch_optional(db)
    .await
}

/// Lease the most urgent ready job to `worker_id`. The attempt number in the
/// returned row is what the result URL is signed for.
pub async fn lease_next(
    db: &PgPool,
    worker_id: Uuid,
    lease_secs: i64,
) -> Result<Option<JobRow>, sqlx::Error> {
    sqlx::query_as!(
        JobRow,
        r#"
        UPDATE visual_extraction_jobs
        SET status = 'leased', worker_id = $1, attempts = attempts + 1,
            lease_expires_at = NOW() + make_interval(secs => $2), updated_at = NOW()
        WHERE id = (
            SELECT id FROM visual_extraction_jobs
            WHERE status = 'ready'
            ORDER BY priority DESC, created_at
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        RETURNING id, document_id, course_id, kind, status, attempts, staged_path, worker_id, cues
        "#,
        worker_id,
        lease_secs as f64,
    )
    .fetch_optional(db)
    .await
}

/// Mark a leased job done, inside the caller's ingest transaction so the
/// pages, figures and documents it wrote land together with the status.
/// Frees its staging bytes. False when the lease is no longer this attempt.
pub async fn complete(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    attempt: i32,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET status = 'done', worker_id = NULL, lease_expires_at = NULL,
            staged_path = NULL, staged_bytes = NULL, cues = NULL, error_msg = NULL,
            completed_at = NOW(), updated_at = NOW()
        WHERE id = $1 AND status = 'leased' AND attempts = $2
        "#,
        job_id,
        attempt,
    )
    .execute(&mut **tx)
    .await?
    .rows_affected();
    Ok(done > 0)
}

/// A worker reported this attempt as failed. Back to `ready` until
/// `max_attempts`, then `failed` (and its staging bytes are freed).
/// Returns the staged path to delete when the job became terminal.
pub async fn fail_attempt(
    db: &PgPool,
    job_id: Uuid,
    attempt: i32,
    error_msg: &str,
    max_attempts: i32,
) -> Result<Option<Option<String>>, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs j
        SET status = CASE WHEN j.attempts >= $4 THEN 'failed' ELSE 'ready' END,
            worker_id = NULL, lease_expires_at = NULL, error_msg = $3, updated_at = NOW(),
            staged_bytes = CASE WHEN j.attempts >= $4 THEN NULL ELSE j.staged_bytes END,
            staged_path = CASE WHEN j.attempts >= $4 THEN NULL ELSE j.staged_path END
        FROM (SELECT staged_path AS old_path FROM visual_extraction_jobs WHERE id = $1) old
        WHERE j.id = $1 AND j.status = 'leased' AND j.attempts = $2
        RETURNING j.status, old.old_path
        "#,
        job_id,
        attempt,
        error_msg,
        max_attempts,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| {
        if r.status == "failed" {
            r.old_path
        } else {
            None
        }
    }))
}

/// Return every job leased to a dead worker to the queue (or to `failed`
/// once out of attempts). Returns staged paths of jobs that became terminal.
pub async fn requeue_worker(
    db: &PgPool,
    worker_id: Uuid,
    reason: &str,
    max_attempts: i32,
) -> Result<Vec<String>, sqlx::Error> {
    requeue_where(db, Some(worker_id), reason, max_attempts).await
}

/// Backstop for leases nobody reported on: expired leases go back to the
/// queue the same way.
pub async fn requeue_expired(db: &PgPool, max_attempts: i32) -> Result<Vec<String>, sqlx::Error> {
    requeue_where(db, None, "lease expired", max_attempts).await
}

async fn requeue_where(
    db: &PgPool,
    worker_id: Option<Uuid>,
    reason: &str,
    max_attempts: i32,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        WITH hit AS (
            SELECT id, staged_path AS old_path, attempts >= $3 AS terminal
            FROM visual_extraction_jobs
            WHERE status = 'leased'
              AND (($1::UUID IS NOT NULL AND worker_id = $1)
                   OR ($1::UUID IS NULL AND lease_expires_at < NOW()))
            FOR UPDATE
        )
        UPDATE visual_extraction_jobs j
        SET status = CASE WHEN hit.terminal THEN 'failed' ELSE 'ready' END,
            worker_id = NULL, lease_expires_at = NULL, error_msg = $2, updated_at = NOW(),
            staged_bytes = CASE WHEN hit.terminal THEN NULL ELSE j.staged_bytes END,
            staged_path = CASE WHEN hit.terminal THEN NULL ELSE j.staged_path END
        FROM hit
        WHERE j.id = hit.id
        RETURNING hit.terminal AS "terminal!", hit.old_path
        "#,
        worker_id,
        reason,
        max_attempts,
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| r.terminal)
        .filter_map(|r| r.old_path)
        .collect())
}

/// Done jobs from an older pipeline version go back into the queue, a few
/// at a time so a version bump does not flood the GPUs or staging.
pub async fn reenqueue_outdated(
    db: &PgPool,
    pipeline_version: i32,
    limit: i64,
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET status = CASE WHEN kind = 'pdf' THEN 'ready' ELSE 'needs_source' END,
            pipeline_version = $1, attempts = 0, error_msg = NULL, updated_at = NOW()
        WHERE id IN (
            SELECT id FROM visual_extraction_jobs
            WHERE status = 'done' AND pipeline_version < $1
            ORDER BY completed_at
            LIMIT $2
        )
        "#,
        pipeline_version,
        limit,
    )
    .execute(db)
    .await?
    .rows_affected())
}

/// Jobs per status, for the queue-depth gauge.
pub async fn count_by_status(db: &PgPool) -> Result<Vec<(String, i64)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT status, COUNT(*) AS "count!" FROM visual_extraction_jobs GROUP BY status"#
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.status, r.count)).collect())
}

pub async fn count_leasable(db: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM visual_extraction_jobs WHERE status = 'ready'"#
    )
    .fetch_one(db)
    .await
}

pub async fn insert_worker(
    db: &PgPool,
    id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO visual_extraction_workers (id, expires_at) VALUES ($1, $2)",
        id,
        expires_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn set_worker_slurm_job(
    db: &PgPool,
    id: Uuid,
    slurm_job_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE visual_extraction_workers SET slurm_job_id = $2, state = 'queued' WHERE id = $1",
        id,
        slurm_job_id,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Move a worker to `state`; terminal states stamp `finished_at`.
pub async fn set_worker_state(
    db: &PgPool,
    id: Uuid,
    state: &str,
    error_msg: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE visual_extraction_workers
        SET state = $2, error_msg = COALESCE($3, error_msg),
            finished_at = CASE WHEN $2 IN ('finished', 'failed') THEN NOW() ELSE finished_at END
        WHERE id = $1
        "#,
        id,
        state,
        error_msg,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// A worker called in; also proves it is running.
pub async fn touch_worker(db: &PgPool, id: Uuid) -> Result<Option<WorkerRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkerRow,
        r#"
        UPDATE visual_extraction_workers
        SET last_seen_at = NOW(),
            state = CASE WHEN state IN ('submitting', 'queued') THEN 'running' ELSE state END
        WHERE id = $1 AND state IN ('submitting', 'queued', 'running') AND expires_at > NOW()
        RETURNING id, slurm_job_id, state, submitted_at, expires_at
        "#,
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_active_workers(db: &PgPool) -> Result<Vec<WorkerRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkerRow,
        r#"
        SELECT id, slurm_job_id, state, submitted_at, expires_at
        FROM visual_extraction_workers
        WHERE state IN ('submitting', 'queued', 'running')
        ORDER BY submitted_at
        "#
    )
    .fetch_all(db)
    .await
}

pub struct NewPage<'a> {
    pub position: i32,
    pub page_number: Option<i32>,
    pub start_seconds: Option<f32>,
    pub end_seconds: Option<f32>,
    pub image_path: Option<&'a str>,
    pub blocks: &'a serde_json::Value,
    pub text: &'a str,
}

pub struct NewFigure<'a> {
    pub page_position: i32,
    pub bbox: &'a [f32],
    pub image_path: &'a str,
    pub caption: Option<&'a str>,
    pub context: &'a str,
    pub visual_model: &'a str,
    pub visual_vector: &'a [f32],
}

#[derive(Debug, Clone)]
pub struct UnindexedFigure {
    pub id: Uuid,
    pub document_id: Uuid,
    pub course_id: Uuid,
    pub context: String,
    pub visual_model: String,
    pub visual_vector: Vec<f32>,
}

/// Replace a document's pages and figures with a fresh OCR result, inside
/// the ingest transaction. Returns the ids of the figures it replaced, whose
/// Qdrant points the caller deletes after commit (by id, so the indexing
/// sweep's points for the new figures are never touched).
pub async fn replace_layout_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    document_id: Uuid,
    course_id: Uuid,
    pages: &[NewPage<'_>],
    figures: &[NewFigure<'_>],
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query!(
        "DELETE FROM document_visual_pages WHERE document_id = $1",
        document_id
    )
    .execute(&mut **tx)
    .await?;
    let replaced = sqlx::query_scalar!(
        "DELETE FROM document_figures WHERE document_id = $1 RETURNING id",
        document_id
    )
    .fetch_all(&mut **tx)
    .await?;
    for page in pages {
        sqlx::query!(
            r#"
            INSERT INTO document_visual_pages
                (document_id, position, page_number, start_seconds, end_seconds, image_path, blocks, text)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            document_id,
            page.position,
            page.page_number,
            page.start_seconds,
            page.end_seconds,
            page.image_path,
            page.blocks,
            page.text,
        )
        .execute(&mut **tx)
        .await?;
    }
    for figure in figures {
        sqlx::query!(
            r#"
            INSERT INTO document_figures
                (document_id, course_id, page_position, box, image_path, caption, context,
                 visual_model, visual_vector)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
            document_id,
            course_id,
            figure.page_position,
            figure.bbox,
            figure.image_path,
            figure.caption,
            figure.context,
            figure.visual_model,
            figure.visual_vector,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(replaced)
}

/// Figures whose vectors have not reached Qdrant yet, oldest first.
pub async fn list_unindexed_figures(
    db: &PgPool,
    limit: i64,
) -> Result<Vec<UnindexedFigure>, sqlx::Error> {
    sqlx::query_as!(
        UnindexedFigure,
        r#"
        SELECT id, document_id, course_id, context, visual_model, visual_vector AS "visual_vector!: Vec<f32>"
        FROM document_figures
        WHERE indexed_at IS NULL
        ORDER BY created_at
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(db)
    .await
}

pub async fn mark_figures_indexed(db: &PgPool, ids: &[Uuid]) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE document_figures SET indexed_at = NOW() WHERE id = ANY($1)",
        ids
    )
    .execute(db)
    .await?;
    Ok(())
}

pub struct CourseEmbeddingRow {
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_version: i32,
}

/// How a course embeds text, for indexing its figures' context. Archived
/// courses included: their figures are indexed like their chunks were.
pub async fn course_embedding(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Option<CourseEmbeddingRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseEmbeddingRow,
        "SELECT embedding_provider, embedding_model, embedding_version FROM courses WHERE id = $1",
        course_id,
    )
    .fetch_optional(db)
    .await
}

/// A figure as chat offers it: where it comes from and how to label it.
#[derive(Debug, Clone)]
pub struct ChatFigureRow {
    pub id: Uuid,
    pub document_id: Uuid,
    pub filename: String,
    pub caption: Option<String>,
    pub context: String,
    pub page_number: Option<i32>,
    pub start_seconds: Option<f32>,
}

/// Load figures for a chat turn, dropping any a student must not get:
/// figures of hidden or orphaned documents always, and with
/// `exclude_withheld_kinds` (the course's kind rules are on) figures of
/// solution, assessment, unknown and unclassified material. A lecture's
/// figures belong to its `.url` stub, which carries no kind of its own, so
/// the kind is read from its active text child. Returned in `ids` order.
pub async fn find_chat_figures(
    db: &PgPool,
    course_id: Uuid,
    ids: &[Uuid],
    exclude_withheld_kinds: bool,
) -> Result<Vec<ChatFigureRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        ChatFigureRow,
        r#"
        SELECT f.id, f.document_id, d.filename, f.caption, f.context,
               p.page_number, p.start_seconds
        FROM document_figures f
        JOIN documents d ON d.id = f.document_id
        LEFT JOIN document_visual_pages p
               ON p.document_id = f.document_id AND p.position = f.page_position
        LEFT JOIN documents child
               ON child.parent_document_id = d.id AND child.orphaned_at IS NULL
        WHERE f.id = ANY($2)
          AND f.course_id = $1
          AND d.displayable
          AND d.orphaned_at IS NULL
          AND (
              NOT $3
              OR COALESCE(child.kind, d.kind) NOT IN
                 ('assignment_brief', 'lab_brief', 'exam', 'sample_solution', 'unknown')
          )
          AND (NOT $3 OR COALESCE(child.kind, d.kind) IS NOT NULL)
        "#,
        course_id,
        ids,
        exclude_withheld_kinds,
    )
    .fetch_all(db)
    .await?;
    let mut by_id: std::collections::HashMap<Uuid, ChatFigureRow> =
        rows.into_iter().map(|r| (r.id, r)).collect();
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

pub async fn insert_message_figures(
    db: &PgPool,
    message_id: Uuid,
    figure_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    for (position, figure_id) in figure_ids.iter().enumerate() {
        sqlx::query!(
            "INSERT INTO message_figures (message_id, position, figure_id) VALUES ($1, $2, $3)",
            message_id,
            position as i32,
            figure_id,
        )
        .execute(db)
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct MessageFigureRow {
    pub message_id: Uuid,
    pub figure_id: Uuid,
    pub filename: String,
    pub caption: Option<String>,
    pub page_number: Option<i32>,
    pub start_seconds: Option<f32>,
}

/// The figures of every reply in a conversation, in each reply's order.
pub async fn list_conversation_figures(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<MessageFigureRow>, sqlx::Error> {
    sqlx::query_as!(
        MessageFigureRow,
        r#"
        SELECT mf.message_id, f.id AS figure_id, d.filename, f.caption,
               p.page_number, p.start_seconds
        FROM message_figures mf
        JOIN messages m ON m.id = mf.message_id
        JOIN document_figures f ON f.id = mf.figure_id
        JOIN documents d ON d.id = f.document_id
        LEFT JOIN document_visual_pages p
               ON p.document_id = f.document_id AND p.position = f.page_position
        WHERE m.conversation_id = $1
        ORDER BY mf.message_id, mf.position
        "#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}

/// Where a figure's image is on disk, for serving it.
pub async fn figure_image(
    db: &PgPool,
    figure_id: Uuid,
) -> Result<Option<(Uuid, Uuid, String)>, sqlx::Error> {
    Ok(sqlx::query!(
        "SELECT course_id, document_id, image_path FROM document_figures WHERE id = $1",
        figure_id,
    )
    .fetch_optional(db)
    .await?
    .map(|r| (r.course_id, r.document_id, r.image_path)))
}

/// A document's OCR text, page by page under `## Page N` headings, for the
/// ingest worker to chunk instead of the extractor's text. None until the
/// document has been through visual extraction, or when OCR read nothing.
pub async fn ocr_text(db: &PgPool, document_id: Uuid) -> Result<Option<String>, sqlx::Error> {
    let pages = sqlx::query!(
        r#"
        SELECT position, page_number, text
        FROM document_visual_pages
        WHERE document_id = $1
        ORDER BY position
        "#,
        document_id,
    )
    .fetch_all(db)
    .await?;
    let sections: Vec<String> = pages
        .into_iter()
        .filter(|p| !p.text.trim().is_empty())
        .map(|p| {
            format!(
                "## Page {}\n\n{}",
                p.page_number.unwrap_or(p.position + 1),
                p.text.trim()
            )
        })
        .collect();
    Ok((!sections.is_empty()).then(|| sections.join("\n\n")))
}

/// Send an ingested document back through the ingest worker so it re-chunks
/// from its fresh OCR text. Leaves a document that is mid-ingest alone; it
/// will read the OCR text when it gets there.
pub async fn requeue_for_reingest(db: &PgPool, document_id: Uuid) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query!(
        r#"
        UPDATE documents
        SET status = 'pending', error_msg = NULL, processing_started_at = NULL,
            ingest_attempts = 0, retry_after = NULL
        WHERE id = $1 AND status IN ('ready', 'failed')
        "#,
        document_id,
    )
    .execute(db)
    .await?
    .rows_affected();
    Ok(updated > 0)
}

/// Whether any of a course's figures have reached Qdrant. Chat skips figure
/// retrieval (and so never loads the CLIP query encoder) until one has.
pub async fn course_has_indexed_figures(db: &PgPool, course_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM document_figures WHERE course_id = $1 AND indexed_at IS NOT NULL) AS "exists!""#,
        course_id,
    )
    .fetch_one(db)
    .await
}
