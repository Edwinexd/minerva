use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct UsageDailyRow {
    pub user_id: Uuid,
    pub course_id: Uuid,
    pub date: chrono::NaiveDate,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub embedding_tokens: i64,
    pub request_count: i32,
}

pub async fn record_usage(
    db: &PgPool,
    user_id: Uuid,
    course_id: Uuid,
    prompt_tokens: i64,
    completion_tokens: i64,
    embedding_tokens: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO usage_daily (user_id, course_id, date, prompt_tokens, completion_tokens, embedding_tokens, request_count)
        VALUES ($1, $2, CURRENT_DATE, $3, $4, $5, 1)
        ON CONFLICT (user_id, course_id, date)
        DO UPDATE SET
            prompt_tokens = usage_daily.prompt_tokens + $3,
            completion_tokens = usage_daily.completion_tokens + $4,
            embedding_tokens = usage_daily.embedding_tokens + $5,
            request_count = usage_daily.request_count + 1"#,
        user_id,
        course_id,
        prompt_tokens,
        completion_tokens,
        embedding_tokens,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get_course_usage(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<UsageDailyRow>, sqlx::Error> {
    sqlx::query_as!(
        UsageDailyRow,
        "SELECT user_id, course_id, date, prompt_tokens, completion_tokens, embedding_tokens, request_count FROM usage_daily WHERE course_id = $1 ORDER BY date DESC",
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn get_all_usage(db: &PgPool) -> Result<Vec<UsageDailyRow>, sqlx::Error> {
    sqlx::query_as!(
        UsageDailyRow,
        "SELECT user_id, course_id, date, prompt_tokens, completion_tokens, embedding_tokens, request_count FROM usage_daily ORDER BY date DESC",
    )
    .fetch_all(db)
    .await
}

pub async fn get_user_daily_tokens(
    db: &PgPool,
    user_id: Uuid,
    course_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"SELECT COALESCE(prompt_tokens + completion_tokens, 0) AS "total!" FROM usage_daily WHERE user_id = $1 AND course_id = $2 AND date = CURRENT_DATE"#,
        user_id,
        course_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.unwrap_or(0))
}

/// Deletes today's `usage_daily` rows for a user across every course,
/// effectively zeroing out both the per-student-per-course cap and the
/// contribution to any owner's aggregate cap for the rest of UTC today.
///
/// Destructive: today's audit trail for this user is lost. That is the
/// point; we use DELETE rather than a zero-update so the subsequent
/// `record_usage` upsert starts from a clean row. Historical rows
/// (`date < CURRENT_DATE`) are untouched. Returns the number of rows
/// deleted so the caller can surface "no usage today" to the UI.
pub async fn reset_user_daily_usage(db: &PgPool, user_id: Uuid) -> Result<u64, sqlx::Error> {
    let res = sqlx::query!(
        "DELETE FROM usage_daily WHERE user_id = $1 AND date = CURRENT_DATE",
        user_id,
    )
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}

/// Sum of (prompt_tokens + completion_tokens) today across every course
/// owned by `owner_id`. Used to enforce the per-owner aggregate cap so a
/// teacher's spend across all their courses stays under one budget.
pub async fn get_owner_daily_tokens(db: &PgPool, owner_id: Uuid) -> Result<i64, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"SELECT COALESCE(SUM(u.prompt_tokens + u.completion_tokens), 0)::bigint AS "total!"
           FROM usage_daily u
           JOIN courses c ON c.id = u.course_id
           WHERE c.owner_id = $1 AND u.date = CURRENT_DATE"#,
        owner_id,
    )
    .fetch_one(db)
    .await?;
    Ok(row)
}

#[derive(Debug)]
pub struct UsageSummaryRow {
    pub course_id: Uuid,
    pub total_prompt_tokens: Option<i64>,
    pub total_completion_tokens: Option<i64>,
    pub total_embedding_tokens: Option<i64>,
    pub total_requests: Option<i64>,
}

pub async fn get_course_summary(
    db: &PgPool,
    course_id: Uuid,
) -> Result<UsageSummaryRow, sqlx::Error> {
    sqlx::query_as!(
        UsageSummaryRow,
        r#"SELECT course_id,
            SUM(prompt_tokens)::bigint as total_prompt_tokens,
            SUM(completion_tokens)::bigint as total_completion_tokens,
            SUM(embedding_tokens)::bigint as total_embedding_tokens,
            SUM(request_count)::bigint as total_requests
        FROM usage_daily WHERE course_id = $1 GROUP BY course_id"#,
        course_id,
    )
    .fetch_optional(db)
    .await
    .map(|opt| {
        opt.unwrap_or(UsageSummaryRow {
            course_id,
            total_prompt_tokens: Some(0),
            total_completion_tokens: Some(0),
            total_embedding_tokens: Some(0),
            total_requests: Some(0),
        })
    })
}
