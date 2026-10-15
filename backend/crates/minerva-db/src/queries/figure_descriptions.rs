//! Post-extraction description cache and durable queue, independent of OCR.
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct DescriptionJob {
    pub figure_id: Uuid,
    pub course_id: Uuid,
    pub document_id: Uuid,
    pub image_path: String,
    pub context: String,
    pub lease: Uuid,
}

/// Bounded backfill, also invalidating only this cache when model/version changes.
pub async fn enqueue(
    db: &PgPool,
    model: &str,
    version: i32,
    limit: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO figure_descriptions (figure_id, model, version)
        SELECT f.id, $1, $2 FROM document_figures f
        JOIN documents d ON d.id = f.document_id
        JOIN courses c ON c.id = f.course_id
        LEFT JOIN figure_descriptions fd ON fd.figure_id = f.id
        WHERE d.orphaned_at IS NULL AND c.active
          AND (fd.figure_id IS NULL OR fd.model <> $1 OR fd.version <> $2)
          AND (fd.lease_expires_at IS NULL OR fd.lease_expires_at < NOW())
        ORDER BY f.created_at, f.id LIMIT $3
        ON CONFLICT (figure_id) DO UPDATE SET
            model = EXCLUDED.model, version = EXCLUDED.version,
            description = NULL, caption = NULL, is_decorative = NULL,
            described_at = NULL, attempts = 0, retry_at = NOW(),
            lease = NULL, lease_expires_at = NULL, error_msg = NULL
        WHERE figure_descriptions.lease_expires_at IS NULL
           OR figure_descriptions.lease_expires_at < NOW()
        "#,
        model,
        version,
        limit,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Claim just one crop; concurrent schedulers cannot describe the same attempt.
/// A restart releases work by expiry. Five failures stop retries until a
/// model/version change resets the job.
pub async fn claim(
    db: &PgPool,
    model: &str,
    version: i32,
) -> Result<Option<DescriptionJob>, sqlx::Error> {
    sqlx::query_as!(
        DescriptionJob,
        r#"
        WITH candidate AS (
            SELECT fd.figure_id FROM figure_descriptions fd
            JOIN document_figures f ON f.id = fd.figure_id
            JOIN documents d ON d.id = f.document_id
            JOIN courses c ON c.id = f.course_id
            WHERE fd.model = $1 AND fd.version = $2 AND fd.described_at IS NULL
              AND fd.attempts < 5 AND fd.retry_at <= NOW()
              AND (fd.lease_expires_at IS NULL OR fd.lease_expires_at < NOW())
              AND d.orphaned_at IS NULL AND c.active
            ORDER BY fd.retry_at, fd.created_at, fd.figure_id
            FOR UPDATE OF fd SKIP LOCKED LIMIT 1
        ), claimed AS (
            UPDATE figure_descriptions fd SET attempts = attempts + 1,
                lease = gen_random_uuid(), lease_expires_at = NOW() + INTERVAL '20 minutes'
            FROM candidate WHERE fd.figure_id = candidate.figure_id
            RETURNING fd.figure_id, fd.lease
        )
        SELECT f.id AS "figure_id!", f.course_id, f.document_id, f.image_path,
               f.context, claimed.lease AS "lease!"
        FROM claimed JOIN document_figures f ON f.id = claimed.figure_id
        "#,
        model,
        version,
    )
    .fetch_optional(db)
    .await
}

/// Lease check also rejects late results after replacement/model changes.
/// Reset indexing in the same transaction so the new description is embedded.
pub async fn complete(
    db: &PgPool,
    job: &DescriptionJob,
    description: &str,
    caption: &str,
    decorative: bool,
) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let updated = sqlx::query!(
        r#"UPDATE figure_descriptions SET description = $3, caption = $4,
            is_decorative = $5, described_at = NOW(), lease = NULL,
            lease_expires_at = NULL, error_msg = NULL
           WHERE figure_id = $1 AND lease = $2 AND lease_expires_at > NOW()"#,
        job.figure_id,
        job.lease,
        description,
        caption,
        decorative,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if updated {
        sqlx::query!(
            "UPDATE document_figures SET indexed_at = NULL WHERE id = $1",
            job.figure_id
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(updated)
}

pub async fn fail(db: &PgPool, job: &DescriptionJob, error: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE figure_descriptions SET lease = NULL, lease_expires_at = NULL,
            error_msg = $3,
            retry_at = NOW() + make_interval(secs => LEAST(3600, 60 * power(2, attempts - 1))::double precision)
           WHERE figure_id = $1 AND lease = $2"#,
        job.figure_id, job.lease, error,
    ).execute(db).await?;
    Ok(())
}

/// Owner spending caps defer work until the next day without consuming retries.
pub async fn defer_for_budget(db: &PgPool, job: &DescriptionJob) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE figure_descriptions SET lease = NULL, lease_expires_at = NULL,
            attempts = GREATEST(0, attempts - 1),
            retry_at = CURRENT_DATE + INTERVAL '1 day', error_msg = 'Owner daily spending cap reached'
           WHERE figure_id = $1 AND lease = $2"#,
        job.figure_id, job.lease,
    ).execute(db).await?;
    Ok(())
}

/// Flex capacity shortages are not a bad crop or a failed model result.
pub async fn defer_for_capacity(db: &PgPool, job: &DescriptionJob) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE figure_descriptions SET lease = NULL, lease_expires_at = NULL,
            attempts = GREATEST(0, attempts - 1), retry_at = NOW() + INTERVAL '5 minutes',
            error_msg = 'Provider temporarily unavailable'
           WHERE figure_id = $1 AND lease = $2"#,
        job.figure_id,
        job.lease,
    )
    .execute(db)
    .await?;
    Ok(())
}
