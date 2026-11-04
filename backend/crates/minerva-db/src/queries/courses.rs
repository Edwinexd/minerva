//! The course subtype of a workspace: what only a course has (term,
//! Daisy course code, Daisy auto-management) plus the Daisy offering
//! upsert that creates courses. Generic workspace queries (config,
//! members, archive, merge) live in `queries::workspaces`; see migration
//! `20261104000002_workspaces.sql`.

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CourseRow {
    pub workspace_id: Uuid,
    /// Free-text `VT2026` / `HT2025`. NULL for ad-hoc courses; populated
    /// by the Daisy auto-import for every imported offering. Drives the
    /// per-semester grouping on the home page.
    pub semester_label: Option<String>,
    /// Daisy designation (e.g. `PROG2`). Stable across name renames;
    /// NULL on ad-hoc courses without a Daisy linkage.
    pub course_code: Option<String>,
    /// TRUE when the Daisy auto-import created the course; membership
    /// stays additive and the owner is the env-var fallback until a real
    /// course-responsible is identified and present in Minerva.
    pub auto_managed: bool,
}

/// Insert the subtype row for a workspace whose `kind` is `course`.
pub async fn insert<'e, E>(
    db: E,
    workspace_id: Uuid,
    semester_label: Option<&str>,
    course_code: Option<&str>,
    auto_managed: bool,
) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query!(
        r#"INSERT INTO courses (workspace_id, semester_label, course_code, auto_managed)
           VALUES ($1, $2, $3, $4)"#,
        workspace_id,
        semester_label,
        course_code,
        auto_managed,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn find(db: &PgPool, workspace_id: Uuid) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT workspace_id, semester_label, course_code, auto_managed FROM courses WHERE workspace_id = $1",
        workspace_id,
    )
    .fetch_optional(db)
    .await
}

/// Course facts for many workspaces at once, keyed by workspace id.
/// Non-course workspaces are simply absent.
pub async fn find_many(
    db: &PgPool,
    workspace_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, CourseRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        CourseRow,
        "SELECT workspace_id, semester_label, course_code, auto_managed FROM courses WHERE workspace_id = ANY($1)",
        workspace_ids,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.workspace_id, r)).collect())
}

/// Admin / owner backfill of the term label. We don't expose a way to
/// clear it back to NULL through the API; once stamped it stays stamped.
pub async fn set_semester_label(
    db: &PgPool,
    workspace_id: Uuid,
    semester_label: &str,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query!(
        "UPDATE courses SET semester_label = $2 WHERE workspace_id = $1",
        workspace_id,
        semester_label,
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Input bag for `upsert_from_daisy`. Mirrors the subset of
/// `dsv_wrapper.DaisyCourse` fields the Minerva backend cares about;
/// the python sync script flattens the dsv-wrapper model into this
/// shape before posting to `/api/service/daisy-courses`.
///
/// Every admin-tunable course-AI knob is carried here as
/// `Option<&'a str>` / `Option<numeric>` so the route layer can wire
/// `system_defaults` reads through to the INSERT exactly the way
/// the manual `POST /courses` endpoint does. `None` falls through
/// to the migration's literal default; the route layer should only
/// pass `None` from legacy / test paths.
pub struct DaisyCourseInput<'a> {
    /// Daisy momenttillfID. Dedup key for the upsert.
    pub momenttillf_id: &'a str,
    /// Course code, e.g. `PROG2`. Used as the Minerva course name's
    /// short prefix and as the play designation `beteckning`.
    pub beteckning: &'a str,
    /// Human course name (Swedish).
    pub name: &'a str,
    pub semester_label: Option<&'a str>,
    pub info_url: Option<&'a str>,
    pub syllabus_url: Option<&'a str>,
    pub unit: Option<&'a str>,
    /// First and last day of the offering's period, when Daisy has one.
    pub start_date: Option<chrono::NaiveDate>,
    pub end_date: Option<chrono::NaiveDate>,
    /// Owner stamped on INSERT only. Subsequent syncs leave the
    /// course's `owner_id` untouched (admin reassignments win
    /// permanently). The route layer resolves this via
    /// `users::find_or_create_by_eppn` from the kursansvarig Daisy
    /// surfaced, so it's always a real human, never a placeholder.
    pub owner_id: Uuid,
    /// Per-student daily token cap, sourced from
    /// `system_defaults::course_daily_cost_limit_usd` by the route layer.
    /// Applied on INSERT only.
    pub daily_cost_limit_usd: Decimal,
    /// Per-conversation token ceilings, sourced from
    /// `system_defaults::course_conversation_{soft,hard}_token_limit`.
    /// Applied on INSERT only, like every other AI default here: a
    /// later Daisy sync must not silently undo a teacher's override.
    pub conversation_soft_token_limit: Option<i64>,
    pub conversation_hard_token_limit: Option<i64>,
    /// Admin-default chat model (`system_defaults::course_model`).
    pub model: Option<&'a str>,
    pub temperature: Option<f64>,
    pub context_ratio: Option<f64>,
    pub max_chunks: Option<i32>,
    pub min_score: Option<f32>,
    pub strategy: Option<&'a str>,
    pub tool_use_enabled: Option<bool>,
    pub embedding_provider: Option<&'a str>,
    pub embedding_model: Option<&'a str>,
    /// Re-ranker model id snapshotted from the catalog default on INSERT.
    pub reranker_model: Option<&'a str>,
    pub system_prompt: Option<&'a str>,
}

pub struct DaisyUpsertOutcome {
    pub course: super::workspaces::WorkspaceRow,
    /// TRUE on a fresh INSERT, FALSE when an existing row was refreshed.
    /// Used by the import endpoint to decide whether to also auto-create
    /// the matching `play_designations` row.
    pub created: bool,
}

/// Idempotently link a Daisy offering to a Minerva course.
///
/// Matches on `course_daisy_offerings.momenttillf_id`:
///   * Existing offering -> refresh that offering's metadata and
///     `last_synced_at`. The course it points at is left untouched:
///     `name` / `course_code` / `semester_label` are course-level and
///     may have been set by an admin or by a merge that combined
///     several offerings, so no single offering gets to overwrite them.
///   * New offering -> create a fresh course (`auto_managed = TRUE`,
///     owner + admin-default AI knobs stamped on INSERT, same literals
///     as `create()`) plus the offering row pointing at it.
///
/// Runs in one transaction so a half-linked state (course inserted but
/// offering missing, or vice versa) can never be observed.
pub async fn upsert_from_daisy(
    db: &PgPool,
    input: &DaisyCourseInput<'_>,
) -> Result<DaisyUpsertOutcome, sqlx::Error> {
    let mut tx = db.begin().await?;

    let existing_course_id: Option<Uuid> = sqlx::query_scalar!(
        "SELECT course_id FROM course_daisy_offerings WHERE momenttillf_id = $1",
        input.momenttillf_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    let (course_id, created) = match existing_course_id {
        Some(course_id) => {
            sqlx::query!(
                r#"UPDATE course_daisy_offerings SET
                       course_code = $2,
                       name = $3,
                       semester_label = $4,
                       info_url = $5,
                       syllabus_url = $6,
                       unit = $7,
                       start_date = $8,
                       end_date = $9,
                       last_synced_at = NOW()
                   WHERE momenttillf_id = $1"#,
                input.momenttillf_id,
                input.beteckning,
                input.name,
                input.semester_label,
                input.info_url,
                input.syllabus_url,
                input.unit,
                input.start_date,
                input.end_date,
            )
            .execute(&mut *tx)
            .await?;
            (course_id, false)
        }
        None => {
            let new_id = Uuid::new_v4();
            sqlx::query!(
                r#"INSERT INTO workspaces (
                    id, name, owner_id, daily_cost_limit_usd,
                    model, temperature, context_ratio, max_chunks, min_score,
                    strategy, tool_use_enabled, embedding_provider, embedding_model,
                    system_prompt, kind, reranker_model,
                    conversation_soft_token_limit, conversation_hard_token_limit
                ) VALUES (
                    $1, $2, $3, $4,
                    COALESCE($5, 'gpt-oss-120b'),
                    COALESCE($6::DOUBLE PRECISION, 0.3),
                    COALESCE($7::DOUBLE PRECISION, 0.7),
                    COALESCE($8::INTEGER, 10),
                    COALESCE($9::REAL, 0.0),
                    COALESCE($10, 'simple'),
                    COALESCE($11::BOOLEAN, FALSE),
                    COALESCE($12, 'local'),
                    COALESCE($13, 'sentence-transformers/all-MiniLM-L6-v2'),
                    $14, 'course',
                    COALESCE($15, 'jinaai/jina-reranker-v2-base-multilingual'),
                    COALESCE($16::BIGINT, 300000),
                    COALESCE($17::BIGINT, 1000000)
                )"#,
                new_id,
                input.name,
                input.owner_id,
                input.daily_cost_limit_usd,
                input.model,
                input.temperature,
                input.context_ratio,
                input.max_chunks,
                input.min_score,
                input.strategy,
                input.tool_use_enabled,
                input.embedding_provider,
                input.embedding_model,
                input.system_prompt,
                input.reranker_model,
                input.conversation_soft_token_limit,
                input.conversation_hard_token_limit,
            )
            .execute(&mut *tx)
            .await?;
            insert(
                &mut *tx,
                new_id,
                input.semester_label,
                Some(input.beteckning),
                true,
            )
            .await?;

            sqlx::query!(
                r#"INSERT INTO course_daisy_offerings
                       (momenttillf_id, course_id, course_code, name,
                        semester_label, info_url, syllabus_url, unit,
                        start_date, end_date, last_synced_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())"#,
                input.momenttillf_id,
                new_id,
                input.beteckning,
                input.name,
                input.semester_label,
                input.info_url,
                input.syllabus_url,
                input.unit,
                input.start_date,
                input.end_date,
            )
            .execute(&mut *tx)
            .await?;
            (new_id, true)
        }
    };

    tx.commit().await?;

    let course = super::workspaces::find_by_id(db, course_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(DaisyUpsertOutcome { course, created })
}

/// Find the (active) course a Daisy offering is linked to. None when
/// the offering hasn't been imported yet, or its course was archived.
pub async fn find_by_daisy_momenttillf_id(
    db: &PgPool,
    momenttillf_id: &str,
) -> Result<Option<super::workspaces::WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        super::workspaces::WorkspaceRow,
        r#"SELECT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature,
               c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy,
               c.tool_use_enabled, c.embedding_provider, c.embedding_model,
               c.embedding_version, c.reranker_model, c.daily_cost_limit_usd, c.conversation_soft_token_limit, c.conversation_hard_token_limit, c.active, c.created_at,
               c.updated_at, c.kind
            FROM workspaces c
            JOIN course_daisy_offerings o ON o.course_id = c.id
            WHERE o.momenttillf_id = $1 AND c.active = true"#,
        momenttillf_id,
    )
    .fetch_optional(db)
    .await
}
