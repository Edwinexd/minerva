use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct UsageDailyRow {
    pub user_id: Uuid,
    pub course_id: Uuid,
    pub date: chrono::NaiveDate,
    /// The chat model these tokens were billed against. The daily
    /// aggregate is per-model so on-read cost (tokens x the model's
    /// current rate) stays correct even when a course switches model.
    pub model: String,
    /// Provider id for `model` (denormalized for per-provider reporting;
    /// the rate still comes from `chat_models` keyed by `model`).
    pub provider: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub embedding_tokens: i64,
    pub request_count: i32,
    /// Research-phase prompt-token share of `prompt_tokens` for the
    /// day. Lets the teacher usage view nest research/writeup under
    /// the prompt total (writeup = `prompt_tokens -
    /// research_prompt_tokens`). Backfills to 0 on rows that predate
    /// the column, matching the migration default.
    pub research_prompt_tokens: i64,
    /// Research-phase completion-token share of `completion_tokens`
    /// for the day. Writeup share is `completion_tokens -
    /// research_completion_tokens`.
    pub research_completion_tokens: i64,
}

#[allow(clippy::too_many_arguments)]
pub async fn record_usage(
    db: &PgPool,
    user_id: Uuid,
    course_id: Uuid,
    model: &str,
    provider: &str,
    prompt_tokens: i64,
    completion_tokens: i64,
    embedding_tokens: i64,
    research_prompt_tokens: i64,
    research_completion_tokens: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO usage_daily (user_id, course_id, date, model, provider, prompt_tokens, completion_tokens, embedding_tokens, research_prompt_tokens, research_completion_tokens, request_count)
        VALUES ($1, $2, CURRENT_DATE, $3, $4, $5, $6, $7, $8, $9, 1)
        ON CONFLICT (user_id, course_id, date, model)
        DO UPDATE SET
            prompt_tokens = usage_daily.prompt_tokens + $5,
            completion_tokens = usage_daily.completion_tokens + $6,
            embedding_tokens = usage_daily.embedding_tokens + $7,
            research_prompt_tokens = usage_daily.research_prompt_tokens + $8,
            research_completion_tokens = usage_daily.research_completion_tokens + $9,
            request_count = usage_daily.request_count + 1"#,
        user_id,
        course_id,
        model,
        provider,
        prompt_tokens,
        completion_tokens,
        embedding_tokens,
        research_prompt_tokens,
        research_completion_tokens,
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
        "SELECT user_id, course_id, date, model, provider, prompt_tokens, completion_tokens, embedding_tokens, request_count, research_prompt_tokens, research_completion_tokens FROM usage_daily WHERE course_id = $1 ORDER BY date DESC",
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn get_all_usage(db: &PgPool) -> Result<Vec<UsageDailyRow>, sqlx::Error> {
    sqlx::query_as!(
        UsageDailyRow,
        "SELECT user_id, course_id, date, model, provider, prompt_tokens, completion_tokens, embedding_tokens, request_count, research_prompt_tokens, research_completion_tokens FROM usage_daily ORDER BY date DESC",
    )
    .fetch_all(db)
    .await
}

pub async fn get_user_daily_tokens(
    db: &PgPool,
    user_id: Uuid,
    course_id: Uuid,
) -> Result<i64, sqlx::Error> {
    // SUM across the per-model rows for the day (a course can switch
    // model mid-day, splitting the aggregate into multiple rows).
    let row = sqlx::query_scalar!(
        r#"SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0)::bigint AS "total!"
           FROM usage_daily
           WHERE user_id = $1 AND course_id = $2 AND date = CURRENT_DATE"#,
        user_id,
        course_id,
    )
    .fetch_one(db)
    .await?;
    Ok(row)
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

/// Today's USD spend for one student in one course: tokens x each
/// model's current rate, summed across the per-model usage rows. Cost is
/// derived on read (the ledger stores tokens + model), so a later
/// re-price changes future enforcement without rewriting today's
/// already-recorded tokens. Unpriced models (NULL rate) contribute 0.
pub async fn get_user_daily_cost(
    db: &PgPool,
    user_id: Uuid,
    course_id: Uuid,
) -> Result<Decimal, sqlx::Error> {
    // LEFT JOIN + COALESCE so a usage row whose model is no longer in the
    // catalog stays in the sum (priced at $0) rather than silently
    // dropping the whole row and under-counting the cap.
    let cost = sqlx::query_scalar!(
        r#"SELECT COALESCE(SUM(
               (u.prompt_tokens * COALESCE(cm.input_usd_per_mtok, 0)
                + u.completion_tokens * COALESCE(cm.output_usd_per_mtok, 0)) / 1000000
           ), 0)::numeric AS "cost!"
           FROM usage_daily u
           LEFT JOIN chat_models cm ON cm.model = u.model
           WHERE u.user_id = $1 AND u.course_id = $2 AND u.date = CURRENT_DATE"#,
        user_id,
        course_id,
    )
    .fetch_one(db)
    .await?;
    Ok(cost)
}

/// Today's total USD spend across every course owned by `owner_id`:
/// student chat spend (`usage_daily`) PLUS pipeline / classification
/// spend (`course_token_usage`), each computed as tokens x the model's
/// current rate. This is what the per-owner cap enforces against.
pub async fn get_owner_daily_cost(db: &PgPool, owner_id: Uuid) -> Result<Decimal, sqlx::Error> {
    let cost = sqlx::query_scalar!(
        r#"
        WITH chat AS (
            SELECT COALESCE(SUM(
                (u.prompt_tokens * COALESCE(cm.input_usd_per_mtok, 0)
                 + u.completion_tokens * COALESCE(cm.output_usd_per_mtok, 0)) / 1000000
            ), 0) AS c
            FROM usage_daily u
            JOIN courses co ON co.id = u.course_id
            LEFT JOIN chat_models cm ON cm.model = u.model
            WHERE co.owner_id = $1 AND u.date = CURRENT_DATE
        ),
        pipeline AS (
            SELECT COALESCE(SUM(
                (ctu.prompt_tokens * COALESCE(cm.input_usd_per_mtok, 0)
                 + ctu.completion_tokens * COALESCE(cm.output_usd_per_mtok, 0)) / 1000000
            ), 0) AS c
            FROM course_token_usage ctu
            JOIN courses co ON co.id = ctu.course_id
            LEFT JOIN chat_models cm ON cm.model = ctu.model
            WHERE co.owner_id = $1 AND ctu.created_at >= CURRENT_DATE
        )
        SELECT (chat.c + pipeline.c)::numeric AS "cost!"
        FROM chat, pipeline
        "#,
        owner_id,
    )
    .fetch_one(db)
    .await?;
    Ok(cost)
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

/// One (course, day, model) slice of student chat spend across every
/// course owned by `owner_id`, for the teacher-facing spend view.
///
/// Pre-aggregated in SQL rather than returning raw `usage_daily` rows:
/// the ledger is per (student, course, day, model), so a 200-student
/// course over a month is thousands of rows the caller would only fold
/// back together anyway.
#[derive(Debug)]
pub struct OwnerChatUsageRow {
    pub course_id: Uuid,
    pub course_name: String,
    pub course_code: Option<String>,
    pub course_semester_label: Option<String>,
    pub course_active: bool,
    pub date: chrono::NaiveDate,
    pub model: String,
    pub provider: String,
    /// Whether this slice is part of today's spend, i.e. what the daily
    /// owner cap is enforced against. Computed by the DB (`= CURRENT_DATE`)
    /// so the caller never has to guess the server's date boundary.
    pub is_today: bool,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub request_count: i64,
    pub cost_usd: Decimal,
}

/// Window is `days` back from the DB's CURRENT_DATE, so the bound uses
/// the same date boundary as cap enforcement instead of the caller's
/// clock.
pub async fn get_owner_chat_usage(
    db: &PgPool,
    owner_id: Uuid,
    days: i32,
) -> Result<Vec<OwnerChatUsageRow>, sqlx::Error> {
    sqlx::query_as!(
        OwnerChatUsageRow,
        r#"SELECT c.id AS "course_id!", c.name AS "course_name!", c.course_code,
                  c.semester_label AS "course_semester_label", c.active AS "course_active!",
                  u.date AS "date!",
                  u.model AS "model!", u.provider AS "provider!",
                  (u.date = CURRENT_DATE) AS "is_today!",
                  SUM(u.prompt_tokens)::bigint AS "prompt_tokens!",
                  SUM(u.completion_tokens)::bigint AS "completion_tokens!",
                  SUM(u.request_count)::bigint AS "request_count!",
                  COALESCE(SUM(
                      (u.prompt_tokens * COALESCE(cm.input_usd_per_mtok, 0)
                       + u.completion_tokens * COALESCE(cm.output_usd_per_mtok, 0)) / 1000000
                  ), 0)::numeric AS "cost_usd!"
             FROM usage_daily u
             JOIN courses c ON c.id = u.course_id
             LEFT JOIN chat_models cm ON cm.model = u.model
            WHERE c.owner_id = $1 AND u.date >= CURRENT_DATE - $2::int
            GROUP BY c.id, u.date, u.model, u.provider
            ORDER BY u.date DESC"#,
        owner_id,
        days,
    )
    .fetch_all(db)
    .await
}

/// Same window and shape as [`get_owner_chat_usage`], for the pipeline /
/// classification spend the owner cap also counts (`course_token_usage`).
/// That spend has no student behind it, so it is kept as a separate axis
/// rather than folded into the chat rows.
#[derive(Debug)]
pub struct OwnerPipelineUsageRow {
    pub course_id: Uuid,
    pub course_name: String,
    pub course_code: Option<String>,
    pub course_semester_label: Option<String>,
    pub course_active: bool,
    pub date: chrono::NaiveDate,
    pub model: String,
    /// `chat_models.provider`, or `unknown` for a model no longer in the
    /// catalog (the row still counts, priced at 0, same as the cap query).
    pub provider: String,
    pub is_today: bool,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cost_usd: Decimal,
}

pub async fn get_owner_pipeline_usage(
    db: &PgPool,
    owner_id: Uuid,
    days: i32,
) -> Result<Vec<OwnerPipelineUsageRow>, sqlx::Error> {
    sqlx::query_as!(
        OwnerPipelineUsageRow,
        r#"SELECT c.id AS "course_id!", c.name AS "course_name!", c.course_code,
                  c.semester_label AS "course_semester_label", c.active AS "course_active!",
                  ctu.created_at::date AS "date!", ctu.model AS "model!",
                  COALESCE(cm.provider, 'unknown') AS "provider!",
                  (ctu.created_at >= CURRENT_DATE) AS "is_today!",
                  SUM(ctu.prompt_tokens)::bigint AS "prompt_tokens!",
                  SUM(ctu.completion_tokens)::bigint AS "completion_tokens!",
                  COALESCE(SUM(
                      (ctu.prompt_tokens * COALESCE(cm.input_usd_per_mtok, 0)
                       + ctu.completion_tokens * COALESCE(cm.output_usd_per_mtok, 0)) / 1000000
                  ), 0)::numeric AS "cost_usd!"
             FROM course_token_usage ctu
             JOIN courses c ON c.id = ctu.course_id
             LEFT JOIN chat_models cm ON cm.model = ctu.model
            WHERE c.owner_id = $1
              AND ctu.created_at >= (CURRENT_DATE - $2::int)::timestamptz
            GROUP BY c.id, ctu.created_at::date, ctu.model,
                     COALESCE(cm.provider, 'unknown'),
                     (ctu.created_at >= CURRENT_DATE)
            ORDER BY ctu.created_at::date DESC"#,
        owner_id,
        days,
    )
    .fetch_all(db)
    .await
}

#[derive(Debug)]
pub struct OwnerCourseStudentsRow {
    pub course_id: Uuid,
    pub active_students: i64,
}

/// Distinct students who sent at least one chat turn in each owned
/// course over the window. Separate from [`get_owner_chat_usage`]
/// because a distinct count cannot be re-derived from rows already
/// grouped by day and model.
pub async fn get_owner_active_students(
    db: &PgPool,
    owner_id: Uuid,
    days: i32,
) -> Result<Vec<OwnerCourseStudentsRow>, sqlx::Error> {
    sqlx::query_as!(
        OwnerCourseStudentsRow,
        r#"SELECT u.course_id AS "course_id!",
                  COUNT(DISTINCT u.user_id)::bigint AS "active_students!"
             FROM usage_daily u
             JOIN courses c ON c.id = u.course_id
            WHERE c.owner_id = $1 AND u.date >= CURRENT_DATE - $2::int
            GROUP BY u.course_id"#,
        owner_id,
        days,
    )
    .fetch_all(db)
    .await
}
