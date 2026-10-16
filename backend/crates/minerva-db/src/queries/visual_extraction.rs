//! Job queue for the visual extraction pipeline (slide OCR for Play lectures,
//! figure extraction for PDFs) and the Slurm workers that drain it.
//!
//! Lifecycle of a job, and who moves it:
//!
//! - `needs_source` -> `fetching`: GitHub Actions reserves a staging slot
//!   ([`reserve_fetch_slots`]), only for Play lectures.
//! - `fetching` -> `ready`: Actions offers a URL/deadline/cues, or several
//!   candidate sources that `minerva-fetcher` probes for the slide track
//!   ([`offer_candidates`], [`choose_download`]); the fetcher downloads the
//!   URL and commits its claim ([`finish_download`]).
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
    /// Consecutive not-ready releases, so Actions can stop waiting for a
    /// transcript Play may never generate.
    pub not_ready_releases: i32,
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
/// minus what fetching slots will still write. New unknown slots reserve
/// `estimate_bytes` each; offered sources can reserve their header size.
/// Staged videos are already on disk and so already out of `free_bytes`.
pub async fn reserve_fetch_slots(
    db: &PgPool,
    free_bytes: i64,
    min_free_bytes: i64,
    estimate_bytes: i64,
    limit: i64,
    exclude_job_ids: &[Uuid],
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
        SET status = 'fetching', fetch_reserved_at = NOW(), staged_bytes = $1,
            error_msg = NULL, retry_after = NULL, updated_at = NOW()
        WHERE id IN (
            SELECT id FROM visual_extraction_jobs
            WHERE status = 'needs_source'
              AND (retry_after IS NULL OR retry_after <= NOW())
              AND NOT (id = ANY($3))
            ORDER BY priority DESC, created_at
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        RETURNING id AS job_id, document_id, course_id, not_ready_releases
        "#,
        estimate_bytes,
        slots,
        exclude_job_ids,
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows)
}

/// First wait after Play reports a lecture not ready; doubles per release.
pub const NOT_READY_BACKOFF_BASE_SECS: i64 = 3600;
/// Longest wait, so a lecture Play never serves is still checked weekly.
pub const NOT_READY_BACKOFF_MAX_SECS: i64 = 7 * 24 * 3600;

/// GitHub Actions could not get this lecture (no video, no transcript yet,
/// Play error). `retry` puts it back for a later run; otherwise it is parked
/// as `unavailable`. `not_ready` (Play has no transcript or video yet) also
/// holds it back for a wait that doubles with each such release, capped at
/// [`NOT_READY_BACKOFF_MAX_SECS`].
pub async fn release_fetch(
    db: &PgPool,
    job_id: Uuid,
    error_msg: &str,
    retry: bool,
    not_ready: bool,
) -> Result<bool, sqlx::Error> {
    let status = if retry { "needs_source" } else { "unavailable" };
    let not_ready = retry && not_ready;
    let done = sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET status = $2, staged_bytes = NULL, fetch_reserved_at = NULL,
            download_url = NULL, download_deadline = NULL, download_started_at = NULL,
            download_candidates = NULL, probe_started_at = NULL,
            download_size_bytes = NULL,
            retry_after = CASE WHEN $4 THEN NOW() + make_interval(secs => LEAST(
                $5::FLOAT8 * POWER(2, LEAST(not_ready_releases, 30)), $6::FLOAT8))
            END,
            not_ready_releases = not_ready_releases + CASE WHEN $4 THEN 1 ELSE 0 END,
            error_msg = $3, updated_at = NOW()
        WHERE id = $1 AND status = 'fetching' AND download_started_at IS NULL
        "#,
        job_id,
        status,
        error_msg,
        not_ready,
        NOT_READY_BACKOFF_BASE_SECS as f64,
        NOT_READY_BACKOFF_MAX_SECS as f64,
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
        SET status = 'needs_source', staged_bytes = NULL, fetch_reserved_at = NULL,
            download_url = NULL, download_deadline = NULL, download_started_at = NULL,
            download_candidates = NULL, probe_started_at = NULL,
            download_size_bytes = NULL,
            updated_at = NOW()
        WHERE status = 'fetching'
          AND ((download_started_at IS NULL AND (
                fetch_reserved_at < NOW() - make_interval(secs => $1)
                OR download_deadline <= NOW()))
              OR download_started_at < NOW() - INTERVAL '35 minutes')
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

/// Bounded aggregate labels only: no document, course, worker or bearer URL
/// becomes a metric label. Recent completions survive scheduler restarts.
pub struct JobMetrics {
    pub kind: String,
    pub status: String,
    pub count: i64,
    pub oldest_age_seconds: f64,
    pub completed_last_15m: i64,
    pub with_errors: i64,
    pub expired_leases: i64,
    /// `needs_source` lectures Play could not serve yet, held back until
    /// `retry_after`. Left out of the age and error figures: waiting on Play
    /// is not a stuck queue or a failure.
    pub waiting_on_play: i64,
}

pub async fn job_metrics(db: &PgPool) -> Result<Vec<JobMetrics>, sqlx::Error> {
    sqlx::query_as!(
        JobMetrics,
        r#"
        SELECT kind, status, COUNT(*) AS "count!",
               COALESCE(EXTRACT(EPOCH FROM NOW() - MIN(updated_at) FILTER (WHERE NOT waiting)), 0)::FLOAT8 AS "oldest_age_seconds!",
               COUNT(*) FILTER (WHERE status = 'done' AND completed_at >= NOW() - INTERVAL '15 minutes') AS "completed_last_15m!",
               COUNT(*) FILTER (WHERE error_msg IS NOT NULL AND NOT waiting) AS "with_errors!",
               COUNT(*) FILTER (WHERE status = 'leased' AND lease_expires_at <= NOW()) AS "expired_leases!",
               COUNT(*) FILTER (WHERE waiting) AS "waiting_on_play!"
        FROM (
            SELECT *, COALESCE(status = 'needs_source' AND retry_after > NOW(), FALSE) AS waiting
            FROM visual_extraction_jobs
        ) j
        GROUP BY kind, status
        "#
    )
    .fetch_all(db)
    .await
}

pub struct WorkerMetrics {
    pub state: String,
    pub count: i64,
    pub failed_last_15m: i64,
}

pub async fn worker_metrics(db: &PgPool) -> Result<Vec<WorkerMetrics>, sqlx::Error> {
    sqlx::query_as!(
        WorkerMetrics,
        r#"
        SELECT state, COUNT(*) AS "count!",
               COUNT(*) FILTER (WHERE state = 'failed' AND finished_at >= NOW() - INTERVAL '15 minutes') AS "failed_last_15m!"
        FROM visual_extraction_workers GROUP BY state
        "#
    )
    .fetch_all(db)
    .await
}

pub struct DownloadMetrics {
    pub reserved: i64,
    pub probing: i64,
    pub queued: i64,
    pub downloading: i64,
    pub expired: i64,
    pub earliest_deadline: f64,
    pub oldest_started_seconds: f64,
    pub staging_bytes: i64,
}

pub async fn download_metrics(db: &PgPool) -> Result<DownloadMetrics, sqlx::Error> {
    sqlx::query_as!(
        DownloadMetrics,
        r#"
        SELECT COUNT(*) FILTER (WHERE status = 'fetching' AND download_url IS NULL AND download_candidates IS NULL) AS "reserved!",
               COUNT(*) FILTER (WHERE status = 'fetching' AND download_candidates IS NOT NULL) AS "probing!",
               COUNT(*) FILTER (WHERE status = 'fetching' AND download_url IS NOT NULL AND download_started_at IS NULL) AS "queued!",
               COUNT(*) FILTER (WHERE status = 'fetching' AND download_started_at IS NOT NULL) AS "downloading!",
               COUNT(*) FILTER (WHERE status = 'fetching' AND download_deadline <= NOW()) AS "expired!",
               COALESCE(EXTRACT(EPOCH FROM MIN(download_deadline) FILTER (WHERE status = 'fetching')), 0)::FLOAT8 AS "earliest_deadline!",
               COALESCE(EXTRACT(EPOCH FROM NOW() - MIN(download_started_at) FILTER (WHERE status = 'fetching')), 0)::FLOAT8 AS "oldest_started_seconds!",
               COALESCE(SUM(staged_bytes), 0)::BIGINT AS "staging_bytes!"
        FROM visual_extraction_jobs
        "#
    )
    .fetch_one(db)
    .await
}

pub struct FigureMetrics {
    pub pending: i64,
    pub indexed: i64,
    pub people: i64,
    pub oldest_pending_seconds: f64,
}

/// Pending counts only figures the pipeline will still move: those of live
/// documents in active courses, which description skips otherwise, and
/// people crops awaiting removal from search, which runs regardless. A
/// replaced document's undescribed figures would otherwise age forever.
pub async fn figure_metrics(db: &PgPool) -> Result<FigureMetrics, sqlx::Error> {
    sqlx::query_as!(
        FigureMetrics,
        r#"
        WITH figures AS (
            SELECT f.indexed_at, f.shows_person, f.created_at,
                   f.indexed_at IS NULL
                       AND (f.shows_person IS TRUE OR (d.orphaned_at IS NULL AND c.active))
                       AS pending
            FROM document_figures f
            JOIN documents d ON d.id = f.document_id
            JOIN courses c ON c.id = f.course_id
        )
        SELECT COUNT(*) FILTER (WHERE pending) AS "pending!",
               COUNT(*) FILTER (WHERE indexed_at IS NOT NULL AND shows_person = FALSE) AS "indexed!",
               COUNT(*) FILTER (WHERE indexed_at IS NOT NULL AND shows_person) AS "people!",
               COALESCE(EXTRACT(EPOCH FROM NOW() - MIN(created_at) FILTER (WHERE pending)), 0)::FLOAT8 AS "oldest_pending_seconds!"
        FROM figures
        "#
    )
    .fetch_one(db)
    .await
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
    pub described_at: chrono::DateTime<chrono::Utc>,
    pub is_decorative: bool,
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
        SELECT f.id, f.document_id, f.course_id,
               fd.description AS "context!", f.visual_model,
               f.visual_vector AS "visual_vector!: Vec<f32>",
               fd.described_at AS "described_at!", fd.is_decorative AS "is_decorative!"
        FROM document_figures f
        JOIN figure_descriptions fd ON fd.figure_id = f.id
        WHERE f.indexed_at IS NULL AND fd.described_at IS NOT NULL
          AND f.shows_person = FALSE
        ORDER BY f.created_at
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(db)
    .await
}

pub struct UnscreenedFigure {
    pub id: Uuid,
    pub visual_vector: Vec<f32>,
}

/// Crops the people screen has not scored yet, oldest first.
pub async fn list_unscreened_figures(
    db: &PgPool,
    limit: i64,
) -> Result<Vec<UnscreenedFigure>, sqlx::Error> {
    sqlx::query_as!(
        UnscreenedFigure,
        r#"
        SELECT id, visual_vector AS "visual_vector!: Vec<f32>"
        FROM document_figures
        WHERE shows_person IS NULL
        ORDER BY created_at
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(db)
    .await
}

/// CLIP vectors of crops the description model reported as people although
/// the screen scored them below `threshold`: the screen's known misses.
pub async fn person_exemplars(db: &PgPool, threshold: f32) -> Result<Vec<Vec<f32>>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT visual_vector AS "visual_vector!: Vec<f32>"
        FROM document_figures
        WHERE shows_person AND people_score < $1
        ORDER BY created_at DESC
        LIMIT 1000
        "#,
        threshold,
    )
    .fetch_all(db)
    .await
}

/// Store screen results. A crop found to show a person loses any cached
/// description (which may name them) and is queued for removal from
/// search; one the description model already flagged keeps its verdict.
pub async fn record_screening(
    db: &PgPool,
    ids: &[Uuid],
    scores: &[f32],
    people: &[bool],
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        WITH screened AS (
            UPDATE document_figures f SET people_score = s.score, shows_person = s.person,
                indexed_at = CASE WHEN s.person THEN NULL ELSE f.indexed_at END
            FROM UNNEST($1::uuid[], $2::real[], $3::bool[]) AS s(id, score, person)
            WHERE f.id = s.id AND f.shows_person IS NULL
            RETURNING f.id, s.person
        )
        DELETE FROM figure_descriptions fd USING screened
        WHERE fd.figure_id = screened.id AND screened.person
        "#,
        ids,
        scores,
        people,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub struct PersonToRemove {
    pub id: Uuid,
    pub course_id: Uuid,
}

/// Person crops whose removal from both Qdrant collections is not yet
/// confirmed. They reuse `indexed_at` as "search is up to date".
pub async fn list_people_to_remove(
    db: &PgPool,
    limit: i64,
) -> Result<Vec<PersonToRemove>, sqlx::Error> {
    sqlx::query_as!(
        PersonToRemove,
        r#"
        SELECT id, course_id FROM document_figures
        WHERE indexed_at IS NULL AND shows_person
        ORDER BY created_at
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(db)
    .await
}

pub async fn mark_people_removed(db: &PgPool, ids: &[Uuid]) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE document_figures SET indexed_at = NOW() WHERE id = ANY($1) AND shows_person",
        ids,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn mark_figures_indexed(
    db: &PgPool,
    ids: &[Uuid],
    described_at: &[chrono::DateTime<chrono::Utc>],
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE document_figures f SET indexed_at = NOW()
           FROM UNNEST($1::uuid[], $2::timestamptz[]) AS expected(id, described_at),
                figure_descriptions fd
           WHERE f.id = expected.id AND fd.figure_id = f.id
             AND fd.described_at = expected.described_at
             AND f.shows_person = FALSE"#,
        ids,
        described_at,
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
        SELECT f.id, f.document_id, d.filename,
               COALESCE(fd.caption, f.caption) AS caption,
               fd.description AS "context!",
               p.page_number, p.start_seconds
        FROM document_figures f
        JOIN figure_descriptions fd ON fd.figure_id = f.id
        JOIN documents d ON d.id = f.document_id
        LEFT JOIN document_visual_pages p
               ON p.document_id = f.document_id AND p.position = f.page_position
        LEFT JOIN documents child
               ON child.parent_document_id = d.id AND child.orphaned_at IS NULL
        WHERE f.id = ANY($2)
          AND f.course_id = $1
          AND fd.described_at IS NOT NULL AND fd.is_decorative = FALSE
          AND f.shows_person = FALSE
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
        SELECT mf.message_id, f.id AS figure_id, d.filename,
               COALESCE(fd.caption, f.caption) AS caption,
               p.page_number, p.start_seconds
        FROM message_figures mf
        JOIN messages m ON m.id = mf.message_id
        JOIN document_figures f ON f.id = mf.figure_id
        LEFT JOIN figure_descriptions fd ON fd.figure_id = f.id
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
        r#"SELECT EXISTS (
            SELECT 1 FROM document_figures f
            JOIN figure_descriptions fd ON fd.figure_id = f.id
            WHERE f.course_id = $1 AND f.indexed_at IS NOT NULL
              AND fd.described_at IS NOT NULL AND fd.is_decorative = FALSE
              AND f.shows_person = FALSE
        ) AS "exists!""#,
        course_id,
    )
    .fetch_one(db)
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferOutcome {
    Offered,
    /// No longer a fetching reservation awaiting its offer (released,
    /// expired, or already offered).
    NotFetching,
    /// The announced size needs more than the reservation and the staging
    /// window has no room for the difference right now.
    NoSpace,
}

/// Lock the staging window and check that `id` awaits its offer and that
/// `size_bytes` fits: within its reservation, or growing it by what the
/// window still has (`budget_bytes`, free disk minus the guard). `None`
/// means go ahead, holding the lock until the caller's transaction ends.
async fn check_room(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    size_bytes: Option<i64>,
    budget_bytes: i64,
) -> Result<Option<OfferOutcome>, sqlx::Error> {
    sqlx::query!("SELECT pg_advisory_xact_lock(hashtext('visual_extraction_staging'))")
        .execute(&mut **tx)
        .await?;
    let row = sqlx::query!(
        r#"
        SELECT staged_bytes,
            (SELECT COALESCE(SUM(staged_bytes), 0) FROM visual_extraction_jobs
             WHERE status = 'fetching')::BIGINT AS "pending!"
        FROM visual_extraction_jobs
        WHERE id = $1 AND status = 'fetching' AND download_url IS NULL
          AND download_candidates IS NULL AND download_started_at IS NULL
        FOR UPDATE
        "#,
        id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(Some(OfferOutcome::NotFetching));
    };
    let reserved = row.staged_bytes.unwrap_or(0);
    if size_bytes
        .is_some_and(|size| size > reserved && row.pending - reserved + size > budget_bytes)
    {
        return Ok(Some(OfferOutcome::NoSpace));
    }
    Ok(None)
}

/// Small authenticated handoff from Actions. Do not expose bearer URLs in
/// logs. A size replaces the reservation, shrinking it or growing it into
/// free staging space; None keeps the placeholder.
pub async fn offer_download(
    db: &PgPool,
    id: Uuid,
    url: &str,
    deadline: DateTime<Utc>,
    cues: &serde_json::Value,
    size_bytes: Option<i64>,
    budget_bytes: i64,
) -> Result<OfferOutcome, sqlx::Error> {
    let mut tx = db.begin().await?;
    if let Some(outcome) = check_room(&mut tx, id, size_bytes, budget_bytes).await? {
        return Ok(outcome);
    }
    let offered = sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET download_url = $2, download_deadline = $3, cues = $4,
            download_size_bytes = $5, staged_bytes = COALESCE($5, staged_bytes),
            not_ready_releases = 0, updated_at = NOW()
        WHERE id = $1 AND $3 > NOW()
        "#,
        id,
        url,
        deadline,
        cues,
        size_bytes,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    tx.commit().await?;
    Ok(if offered {
        OfferOutcome::Offered
    } else {
        OfferOutcome::NotFetching
    })
}

/// Several sources, none chosen yet. `reserve_bytes` is the largest
/// candidate's size (None keeps the placeholder), so whichever source wins
/// fits what was reserved.
pub async fn offer_candidates(
    db: &PgPool,
    id: Uuid,
    candidates: &serde_json::Value,
    deadline: DateTime<Utc>,
    cues: &serde_json::Value,
    reserve_bytes: Option<i64>,
    budget_bytes: i64,
) -> Result<OfferOutcome, sqlx::Error> {
    let mut tx = db.begin().await?;
    if let Some(outcome) = check_room(&mut tx, id, reserve_bytes, budget_bytes).await? {
        return Ok(outcome);
    }
    let offered = sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET download_candidates = $2, download_deadline = $3, cues = $4,
            staged_bytes = COALESCE($5, staged_bytes), not_ready_releases = 0,
            updated_at = NOW()
        WHERE id = $1 AND $3 > NOW()
        "#,
        id,
        candidates,
        deadline,
        cues,
        reserve_bytes,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    tx.commit().await?;
    Ok(if offered {
        OfferOutcome::Offered
    } else {
        OfferOutcome::NotFetching
    })
}

pub struct ProbeClaim {
    pub id: Uuid,
    pub candidates: serde_json::Value,
    pub deadline: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
}

/// Claim the earliest-expiring lecture whose source is still undecided.
/// A claim older than `stale_secs` belonged to a probe that died and is
/// taken over.
pub async fn claim_probe(db: &PgPool, stale_secs: i64) -> Result<Option<ProbeClaim>, sqlx::Error> {
    sqlx::query_as!(
        ProbeClaim,
        r#"
        UPDATE visual_extraction_jobs
        SET probe_started_at = NOW(), updated_at = NOW()
        WHERE id = (
            SELECT id FROM visual_extraction_jobs
            WHERE status = 'fetching' AND download_candidates IS NOT NULL
                AND download_deadline > NOW()
                AND (probe_started_at IS NULL
                    OR probe_started_at < NOW() - make_interval(secs => $1))
            ORDER BY download_deadline, id LIMIT 1 FOR UPDATE SKIP LOCKED
        )
        RETURNING id, download_candidates AS "candidates!", download_deadline AS "deadline!",
            probe_started_at AS "started_at!"
        "#,
        stale_secs as f64,
    )
    .fetch_optional(db)
    .await
}

/// Promote the probed slide source to the download queue and shrink the
/// reservation to its size. Only the current probe claim can choose.
pub async fn choose_download(
    db: &PgPool,
    claim: &ProbeClaim,
    url: &str,
    size_bytes: Option<i64>,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET download_url = $3, download_size_bytes = $4,
            staged_bytes = COALESCE($4, staged_bytes),
            download_candidates = NULL, probe_started_at = NULL, updated_at = NOW()
        WHERE id = $1 AND status = 'fetching' AND probe_started_at = $2
          AND ($4::BIGINT IS NULL OR $4 <= staged_bytes)
        "#,
        claim.id,
        claim.started_at,
        url,
        size_bytes,
    )
    .execute(db)
    .await?
    .rows_affected()
        == 1)
}

/// A failed probe returns the lecture for a fresh authorization later, or
/// with `retry` false parks it as `unavailable`.
pub async fn fail_probe(
    db: &PgPool,
    claim: &ProbeClaim,
    error_msg: &str,
    retry: bool,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET status = CASE WHEN $4 THEN 'needs_source' ELSE 'unavailable' END,
            staged_bytes = NULL, fetch_reserved_at = NULL,
            download_url = NULL, download_deadline = NULL, download_started_at = NULL,
            download_candidates = NULL, probe_started_at = NULL,
            download_size_bytes = NULL,
            error_msg = $3, updated_at = NOW()
        WHERE id = $1 AND status = 'fetching' AND probe_started_at = $2
        "#,
        claim.id,
        claim.started_at,
        error_msg,
        retry,
    )
    .execute(db)
    .await?
    .rows_affected()
        == 1)
}

/// At fetcher startup. It runs as a single `Recreate` replica, so any
/// probe or download claim belongs to a process that is gone. Unclaim
/// them so they are picked up at once instead of after the stale timeout.
pub async fn reset_fetch_claims(db: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query!(
        r#"
        UPDATE visual_extraction_jobs
        SET probe_started_at = NULL, download_started_at = NULL, updated_at = NOW()
        WHERE status = 'fetching'
          AND (probe_started_at IS NOT NULL OR download_started_at IS NOT NULL)
        "#
    )
    .execute(db)
    .await?
    .rows_affected())
}

pub struct Download {
    pub id: Uuid,
    pub url: String,
    pub deadline: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub expected_bytes: Option<i64>,
}

/// Atomically claim the earliest expiry; multiple scheduler instances cannot
/// download the same reservation. Crashed claims are recovered by queue upkeep.
pub async fn claim_download(db: &PgPool) -> Result<Option<Download>, sqlx::Error> {
    sqlx::query_as!(
        Download,
        r#"
        UPDATE visual_extraction_jobs
        SET download_started_at = NOW(), updated_at = NOW()
        WHERE id = (
            SELECT id FROM visual_extraction_jobs
            WHERE status = 'fetching' AND download_url IS NOT NULL
                AND download_deadline > NOW() AND download_started_at IS NULL
            ORDER BY download_deadline, id LIMIT 1 FOR UPDATE SKIP LOCKED
        )
        RETURNING id, download_url AS "url!", download_deadline AS "deadline!",
            download_started_at AS "started_at!", download_size_bytes AS expected_bytes
        "#
    )
    .fetch_optional(db)
    .await
}

/// Only the current claim can publish or fail. A stale downloader cannot
/// overwrite a refreshed URL or a replacement downloader's state.
pub async fn finish_download(
    db: &PgPool,
    claim: &Download,
    path: Option<&str>,
    bytes: i64,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query!(r#"
        UPDATE visual_extraction_jobs
        SET status = CASE WHEN $3::TEXT IS NULL THEN 'needs_source' ELSE 'ready' END,
            staged_path = $3, staged_bytes = CASE WHEN $3::TEXT IS NULL THEN NULL ELSE $4::BIGINT END,
            error_msg = CASE WHEN $3::TEXT IS NULL THEN 'Play download failed or expired' ELSE NULL END,
            download_url = NULL, download_deadline = NULL, download_started_at = NULL,
            download_candidates = NULL, probe_started_at = NULL,
            download_size_bytes = NULL,
            fetch_reserved_at = NULL, updated_at = NOW()
        WHERE id = $1 AND status = 'fetching' AND download_started_at = $2
        "#, claim.id, claim.started_at, path, bytes).execute(db).await?.rows_affected() == 1)
}

/// Used to remove abandoned staging files after a downloader process dies.
pub async fn retained_staging_paths(db: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(r#"
        SELECT staged_path AS "path!" FROM visual_extraction_jobs WHERE staged_path IS NOT NULL
        UNION ALL
        SELECT id::TEXT || '.' || (EXTRACT(EPOCH FROM download_started_at) * 1000000)::BIGINT::TEXT || '.mp4'
        FROM visual_extraction_jobs WHERE download_started_at IS NOT NULL
        "#).fetch_all(db).await
}
