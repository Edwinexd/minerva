//! RunPod async-job ledger. One row per submission, with idempotency-first
//! semantics so a worker crash between `INSERT` and `UPDATE runpod_job_id`
//! never leaves a billed job we can't reconcile. Schema:
//! `20260506000003_runpod_jobs.sql`.

use sqlx::PgPool;
use uuid::Uuid;

/// Ledger row mirroring `runpod_jobs` exactly.
#[derive(Debug, Clone)]
pub struct RunpodJobRow {
    pub id: Uuid,
    /// Our idempotency key, embedded in the RunPod input payload so we can
    /// find the job again if the submit-then-PATCH window crashes.
    pub client_request_id: String,
    /// RunPod's own job id; NULL while status='submitting', filled by PATCH.
    pub runpod_job_id: Option<String>,
    /// 'ocr_pdf' | 'ocr_image' | 'video_index'.
    pub task: String,
    pub document_id: Uuid,
    /// 'submitting' | 'in_queue' | 'in_progress' | 'completed' | 'failed'.
    pub status: String,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub retry_count: i32,
    pub gpu_seconds: Option<f32>,
    pub estimated_cost_usd: Option<f32>,
}

/// Pre-write a 'submitting' row before the network call to RunPod. If we
/// crash between this and `mark_in_queue`, the row is recoverable: list
/// recent RunPod jobs and match on `client_request_id` from the input payload.
pub async fn insert_submitting(
    db: &PgPool,
    client_request_id: &str,
    task: &str,
    document_id: Uuid,
) -> Result<RunpodJobRow, sqlx::Error> {
    sqlx::query_as!(
        RunpodJobRow,
        r#"INSERT INTO runpod_jobs
               (client_request_id, task, document_id, status)
           VALUES ($1, $2, $3, 'submitting')
           RETURNING id, client_request_id, runpod_job_id, task, document_id,
                     status, submitted_at, completed_at, output, error,
                     retry_count, gpu_seconds, estimated_cost_usd"#,
        client_request_id,
        task,
        document_id,
    )
    .fetch_one(db)
    .await
}

/// Promote a 'submitting' row to 'in_queue' after RunPod accepts the job.
pub async fn mark_in_queue(
    db: &PgPool,
    client_request_id: &str,
    runpod_job_id: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE runpod_jobs
           SET runpod_job_id = $1, status = 'in_queue'
           WHERE client_request_id = $2 AND status = 'submitting'"#,
        runpod_job_id,
        client_request_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Track an in-flight status update reported by RunPod. Skips rows already
/// in a terminal state so a stray late poll can't flip 'completed' back.
pub async fn update_runtime_status(
    db: &PgPool,
    runpod_job_id: &str,
    status: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE runpod_jobs
           SET status = $1
           WHERE runpod_job_id = $2 AND status NOT IN ('completed', 'failed')"#,
        status,
        runpod_job_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Persist a successful completion: store the full RunPod response payload
/// (timeline / pages / etc) and the GPU cost numbers used by the daily
/// budget circuit breaker.
pub async fn mark_completed(
    db: &PgPool,
    runpod_job_id: &str,
    output: &serde_json::Value,
    gpu_seconds: Option<f32>,
    estimated_cost_usd: Option<f32>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE runpod_jobs
           SET status = 'completed',
               completed_at = NOW(),
               output = $1,
               gpu_seconds = $2,
               estimated_cost_usd = $3
           WHERE runpod_job_id = $4 AND status NOT IN ('completed', 'failed')"#,
        output,
        gpu_seconds,
        estimated_cost_usd,
        runpod_job_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark a job failed and bump the retry counter. Caller decides whether to
/// dead-letter the underlying document based on `retry_count` after this
/// returns.
pub async fn mark_failed(
    db: &PgPool,
    runpod_job_id: &str,
    error: &str,
) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"UPDATE runpod_jobs
           SET status = 'failed', completed_at = NOW(), error = $1,
               retry_count = retry_count + 1
           WHERE runpod_job_id = $2
           RETURNING retry_count"#,
        error,
        runpod_job_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| r.retry_count).unwrap_or(0))
}

/// Same as `mark_failed` but keyed by client_request_id, used by the
/// reconciliation path when the row never got a runpod_job_id assigned.
pub async fn mark_failed_by_client_id(
    db: &PgPool,
    client_request_id: &str,
    error: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE runpod_jobs
           SET status = 'failed', completed_at = NOW(), error = $1,
               retry_count = retry_count + 1
           WHERE client_request_id = $2"#,
        error,
        client_request_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Active jobs the poll loop should hit RunPod for. Excludes 'submitting'
/// (those are reconciled separately) and terminal statuses.
pub async fn list_in_flight(db: &PgPool) -> Result<Vec<RunpodJobRow>, sqlx::Error> {
    sqlx::query_as!(
        RunpodJobRow,
        r#"SELECT id, client_request_id, runpod_job_id, task, document_id,
                  status, submitted_at, completed_at, output, error,
                  retry_count, gpu_seconds, estimated_cost_usd
           FROM runpod_jobs
           WHERE status IN ('in_queue', 'in_progress')
           ORDER BY submitted_at"#,
    )
    .fetch_all(db)
    .await
}

/// Rows that are stuck in 'submitting' for longer than `older_than_secs`
/// seconds. The caller (reconcile_orphans) lists RunPod's recent jobs
/// and matches on `client_request_id` to recover them.
pub async fn list_orphaned_submissions(
    db: &PgPool,
    older_than_secs: i64,
) -> Result<Vec<RunpodJobRow>, sqlx::Error> {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(older_than_secs);
    sqlx::query_as!(
        RunpodJobRow,
        r#"SELECT id, client_request_id, runpod_job_id, task, document_id,
                  status, submitted_at, completed_at, output, error,
                  retry_count, gpu_seconds, estimated_cost_usd
           FROM runpod_jobs
           WHERE status = 'submitting' AND submitted_at < $1"#,
        cutoff,
    )
    .fetch_all(db)
    .await
}

/// True if a doc already has an in-flight (non-terminal) RunPod job. The
/// submitter checks this before insert_submitting to avoid double-billing
/// the same doc on a tight retry loop.
pub async fn has_in_flight_for_document(
    db: &PgPool,
    document_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"SELECT EXISTS(
               SELECT 1 FROM runpod_jobs
               WHERE document_id = $1
                 AND status IN ('submitting', 'in_queue', 'in_progress')
           ) as "exists!""#,
        document_id,
    )
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// Sum of `estimated_cost_usd` across all jobs that completed in the last
/// 24 hours. Used by the daily budget circuit breaker.
pub async fn estimated_cost_last_24h(db: &PgPool) -> Result<f64, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"SELECT COALESCE(SUM(estimated_cost_usd), 0.0)::float8 AS "total!"
           FROM runpod_jobs
           WHERE completed_at IS NOT NULL
             AND completed_at > NOW() - INTERVAL '24 hours'"#,
    )
    .fetch_one(db)
    .await?;
    Ok(row)
}
