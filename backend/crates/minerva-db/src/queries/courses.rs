use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct CourseRow {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Uuid,
    pub context_ratio: f64,
    pub temperature: f64,
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_chunks: i32,
    pub min_score: f32,
    pub strategy: String,
    /// Orthogonal toggle: when TRUE, the model has access to a tool
    /// catalog during a "research" phase (visible as thinking) that
    /// precedes a clean single-pass writeup. Strategy (`simple` vs
    /// `flare`) only determines which retrieval signals run inside
    /// that research phase. Defaults to FALSE on existing courses so
    /// they keep their pre-tool-use behaviour.
    pub tool_use_enabled: bool,
    pub embedding_provider: String,
    pub embedding_model: String,
    /// Bumped on every embedding model/provider rotation. Used by the
    /// pipeline to pick a versioned Qdrant collection name; legacy
    /// version=1 maps to `course_{id}` for backward compatibility,
    /// version>=2 maps to `course_{id}_v{n}`. See
    /// `minerva-pipeline::pipeline::collection_name`.
    pub embedding_version: i32,
    /// Per-course cross-encoder re-ranker model id. Selected from the
    /// admin-managed `reranker_models` catalog; independent of the
    /// embedding model (changing it needs no re-embed). See
    /// `minerva_embed_engine::reranker`.
    pub reranker_model: String,
    /// Per-student-per-course daily AI spending cap in USD. 0 = unlimited.
    /// Spend is derived on read from token usage x each model's rate.
    pub daily_cost_limit_usd: Decimal,
    pub active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Free-text `VT2026` / `HT2025`. NULL for ad-hoc courses; populated
    /// by the Daisy auto-import phase for every imported offering. Drives
    /// the per-semester grouping on the My Courses page.
    pub semester_label: Option<String>,
    /// TRUE when the row was created by the Daisy auto-import phase;
    /// membership stays additive and owner is the env-var fallback
    /// until a real course-responsible is identified and present in
    /// Minerva.
    pub auto_managed: bool,
    /// Course code (e.g. `PROG2`). Stable identifier across name
    /// renames; populated by the Daisy auto-import from the
    /// `beteckning` field on the Daisy side. NULL on historical /
    /// ad-hoc courses without a Daisy linkage.
    pub course_code: Option<String>,
}

/// Every column the admin-tunable `system_defaults` registry covers
/// is passed in here as `Some(...)` from the route layer; the SQL
/// column DEFAULTs become a fallback ("nothing in `system_defaults`,
/// nothing the caller wants to set") for legacy callers like tests
/// that don't want to know about the registry. The route layer
/// (`routes::courses::create_course`) always supplies every field so
/// that admin edits in `/admin/defaults` flow straight into new
/// courses; existing courses are unaffected.
pub struct CreateCourse {
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Uuid,
    pub daily_cost_limit_usd: Decimal,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub context_ratio: Option<f64>,
    pub max_chunks: Option<i32>,
    pub min_score: Option<f32>,
    pub strategy: Option<String>,
    pub tool_use_enabled: Option<bool>,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    /// Re-ranker model id snapshotted from the `reranker_models` catalog
    /// default at create time. `None` falls through to the column DEFAULT.
    pub reranker_model: Option<String>,
    pub system_prompt: Option<String>,
    /// Required for new courses. The route layer validates the format
    /// (VT|HT + 4-digit year); pre-existing rows from before this
    /// column existed are nullable to keep historical data intact,
    /// but every new INSERT carries a value.
    pub semester_label: String,
}

pub struct UpdateCourse {
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_ratio: Option<f64>,
    pub temperature: Option<f64>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub max_chunks: Option<i32>,
    pub min_score: Option<f32>,
    pub strategy: Option<String>,
    pub tool_use_enabled: Option<bool>,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    /// Per-course re-ranker model change. `None` = no change (COALESCE
    /// no-op). No re-embed side effect, so unlike embedding rotation this
    /// just lands in the row.
    pub reranker_model: Option<String>,
    pub daily_cost_limit_usd: Option<Decimal>,
    /// Admin / owner backfill of the per-semester grouping label.
    /// Outer `Option` distinguishes "no change" (None) from "set
    /// to value" (Some). We don't expose a way to clear the label
    /// back to NULL through the API; once stamped it stays stamped.
    pub semester_label: Option<String>,
}

pub async fn create(db: &PgPool, id: Uuid, input: &CreateCourse) -> Result<CourseRow, sqlx::Error> {
    // One unconditional INSERT, with `COALESCE($N, <literal>)` falling
    // through to the migration's column DEFAULT for any field the
    // caller passes as `None`. The route layer
    // (`routes::courses::create_course`) always supplies every field
    // from `system_defaults`, so the literals here only matter for
    // `dev_seed` + tests which knowingly pass `None`. The literals
    // mirror the values in the courses-table migrations; drifting them
    // would only affect those legacy callers. `semester_label` is the
    // one mandatory field beyond the original schema; the Daisy
    // auto-import + manual /courses POST both validate the format
    // before reaching this layer.
    sqlx::query_as!(
        CourseRow,
        r#"INSERT INTO courses (
            id, name, description, owner_id, daily_cost_limit_usd,
            model, temperature, context_ratio, max_chunks, min_score,
            strategy, tool_use_enabled, embedding_provider, embedding_model, system_prompt,
            semester_label, reranker_model
        ) VALUES (
            $1, $2, $3, $4, $5,
            COALESCE($6, 'gpt-oss-120b'),
            COALESCE($7::DOUBLE PRECISION, 0.3),
            COALESCE($8::DOUBLE PRECISION, 0.7),
            COALESCE($9::INTEGER, 10),
            COALESCE($10::REAL, 0.0),
            COALESCE($11, 'simple'),
            COALESCE($12::BOOLEAN, FALSE),
            COALESCE($13, 'local'),
            COALESCE($14, 'sentence-transformers/all-MiniLM-L6-v2'),
            $15,
            $16,
            COALESCE($17, 'jinaai/jina-reranker-v2-base-multilingual')
        )
        RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, active, created_at, updated_at, semester_label, auto_managed, course_code"#,
        id,
        input.name,
        input.description,
        input.owner_id,
        input.daily_cost_limit_usd,
        input.model.as_deref(),
        input.temperature,
        input.context_ratio,
        input.max_chunks,
        input.min_score,
        input.strategy.as_deref(),
        input.tool_use_enabled,
        input.embedding_provider.as_deref(),
        input.embedding_model.as_deref(),
        input.system_prompt.as_deref(),
        input.semester_label,
        input.reranker_model.as_deref(),
    )
    .fetch_one(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, active, created_at, updated_at, semester_label, auto_managed, course_code FROM courses WHERE id = $1 AND active = true",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_by_owner(db: &PgPool, owner_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, active, created_at, updated_at, semester_label, auto_managed, course_code FROM courses WHERE owner_id = $1 AND active = true ORDER BY updated_at DESC",
        owner_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_by_member(db: &PgPool, user_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        r#"SELECT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.reranker_model, c.daily_cost_limit_usd, c.active, c.created_at, c.updated_at, c.semester_label, c.auto_managed, c.course_code
        FROM courses c
        JOIN course_members cm ON cm.course_id = c.id
        WHERE cm.user_id = $1 AND c.active = true
        ORDER BY c.updated_at DESC"#,
        user_id,
    )
    .fetch_all(db)
    .await
}

/// Courses where the user is owner OR a teacher/ta member. Used for the
/// teacher dashboard so co-teachers (added via `/courses/:id/members` with
/// role=teacher) see the course even though they don't own it.
pub async fn list_for_teacher(db: &PgPool, user_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        r#"SELECT DISTINCT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.reranker_model, c.daily_cost_limit_usd, c.active, c.created_at, c.updated_at, c.semester_label, c.auto_managed, c.course_code
        FROM courses c
        LEFT JOIN course_members cm ON cm.course_id = c.id AND cm.user_id = $1
        WHERE c.active = true
          AND (c.owner_id = $1 OR cm.role IN ('teacher', 'ta'))
        ORDER BY c.updated_at DESC"#,
        user_id,
    )
    .fetch_all(db)
    .await
}

/// Stricter variant of `list_for_teacher` that excludes TA memberships.
/// Used by the site-integration provisioning flow: only course owners and
/// full teachers should be able to mint an API key, matching the
/// course-settings UI which restricts key creation to owners/admins.
pub async fn list_for_teacher_strict(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        r#"SELECT DISTINCT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.reranker_model, c.daily_cost_limit_usd, c.active, c.created_at, c.updated_at, c.semester_label, c.auto_managed, c.course_code
        FROM courses c
        LEFT JOIN course_members cm ON cm.course_id = c.id AND cm.user_id = $1
        WHERE c.active = true
          AND (c.owner_id = $1 OR cm.role = 'teacher')
        ORDER BY c.updated_at DESC"#,
        user_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_all(db: &PgPool) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, active, created_at, updated_at, semester_label, auto_managed, course_code FROM courses WHERE active = true ORDER BY updated_at DESC",
    )
    .fetch_all(db)
    .await
}

pub async fn update(
    db: &PgPool,
    id: Uuid,
    input: &UpdateCourse,
) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        r#"UPDATE courses SET
            name = COALESCE($2, name),
            description = COALESCE($3, description),
            context_ratio = COALESCE($4, context_ratio),
            temperature = COALESCE($5, temperature),
            model = COALESCE($6, model),
            system_prompt = COALESCE($7, system_prompt),
            max_chunks = COALESCE($8, max_chunks),
            strategy = COALESCE($9, strategy),
            tool_use_enabled = COALESCE($10, tool_use_enabled),
            daily_cost_limit_usd = COALESCE($11, daily_cost_limit_usd),
            embedding_provider = COALESCE($12, embedding_provider),
            embedding_model = COALESCE($13, embedding_model),
            min_score = COALESCE($14, min_score),
            semester_label = COALESCE($15, semester_label),
            reranker_model = COALESCE($16, reranker_model),
            updated_at = NOW()
        WHERE id = $1 AND active = true
        RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, active, created_at, updated_at, semester_label, auto_managed, course_code"#,
        id,
        input.name,
        input.description,
        input.context_ratio,
        input.temperature,
        input.model,
        input.system_prompt,
        input.max_chunks,
        input.strategy,
        input.tool_use_enabled,
        input.daily_cost_limit_usd,
        input.embedding_provider,
        input.embedding_model,
        input.min_score,
        input.semester_label,
        input.reranker_model,
    )
    .fetch_optional(db)
    .await
}

/// Snapshot of the columns the rotation path mutates atomically. The
/// caller hands us the new provider/model (already validated up the
/// stack); we bump `embedding_version` and re-queue every document in
/// the course for re-ingestion under the new model.
pub struct RotateEmbeddingOutcome {
    /// New `embedding_version` after the bump. The runtime composes
    /// the fresh Qdrant collection name from this.
    pub new_version: i32,
    /// Number of documents flipped back to `pending` so the worker
    /// will re-chunk + re-embed them. Useful for the API response so
    /// the UI can surface a "re-queued N documents" toast.
    pub requeued_documents: i64,
}

/// Atomically rotate a course's embedding model:
///
/// 1. `UPDATE courses SET embedding_provider, embedding_model,
///    embedding_version = embedding_version + 1`. The bumped version
///    triggers a fresh Qdrant collection on the next ingest, leaving
///    the previous collection intact (lazy migration; old vectors
///    aren't deleted, just orphaned, so a teacher who rotates by
///    mistake can be rolled back manually).
/// 2. Re-queue every document in the course: `status = 'pending'`,
///    clear `chunk_count`, `processed_at`, `error_msg`,
///    `pooled_embedding`, and `processing_started_at`. Classification
///    state (`kind`, `kind_confidence`, `kind_rationale`,
///    `classified_at`, `kind_locked_by_teacher`) is preserved; the
///    embedding model has no bearing on document kind.
///
/// Both steps run in a single transaction so partial-rotate states
/// (version bumped but documents not re-queued, or vice versa) cannot
/// be observed by the worker between writes.
pub async fn rotate_embedding(
    db: &PgPool,
    id: Uuid,
    new_provider: &str,
    new_model: &str,
) -> Result<RotateEmbeddingOutcome, sqlx::Error> {
    let mut tx = db.begin().await?;

    let new_version = sqlx::query_scalar!(
        r#"UPDATE courses
           SET embedding_provider = $2,
               embedding_model = $3,
               embedding_version = embedding_version + 1,
               updated_at = NOW()
           WHERE id = $1 AND active = true
           RETURNING embedding_version"#,
        id,
        new_provider,
        new_model,
    )
    .fetch_one(&mut *tx)
    .await?;

    // Re-queue every document for re-ingestion under the new model.
    // We deliberately re-queue ALL statuses (ready / failed /
    // unsupported / awaiting_transcript / processing) because:
    //   * `ready` docs need fresh embeddings.
    //   * `failed` docs deserve another shot under the new model.
    //   * `unsupported` docs stay unsupported (text extractor doesn't
    //     change with embedding model); the worker re-checks the
    //     extension and flips them back to `unsupported` quickly.
    //   * `awaiting_transcript` docs are URL stubs the transcript job
    //     will refill; a status flip here doesn't lose the URL.
    //   * `processing` docs were claimed by the previous-model
    //     worker; resetting them is safe because the upsert went
    //     into the OLD collection; which we no longer point at.
    let requeued_documents = sqlx::query_scalar!(
        r#"WITH updated AS (
               UPDATE documents
               SET status = 'pending',
                   chunk_count = NULL,
                   processed_at = NULL,
                   processing_started_at = NULL,
                   error_msg = NULL,
                   pooled_embedding = NULL
               WHERE course_id = $1
               RETURNING 1
           )
           SELECT COUNT(*) FROM updated"#,
        id,
    )
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);

    tx.commit().await?;

    Ok(RotateEmbeddingOutcome {
        new_version,
        requeued_documents,
    })
}

pub async fn archive(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE courses SET active = false, updated_at = NOW() WHERE id = $1 AND active = true",
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[derive(Debug)]
pub struct MemberRow {
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub eppn: Option<String>,
    pub display_name: Option<String>,
}

pub async fn add_member(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO course_members (course_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT (course_id, user_id) DO UPDATE SET role = $3",
        course_id,
        user_id,
        role,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn remove_member(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM course_members WHERE course_id = $1 AND user_id = $2",
        course_id,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_members(db: &PgPool, course_id: Uuid) -> Result<Vec<MemberRow>, sqlx::Error> {
    sqlx::query_as!(
        MemberRow,
        r#"SELECT cm.course_id, cm.user_id, cm.role, cm.added_at, u.eppn AS "eppn?", u.display_name
        FROM course_members cm
        JOIN users u ON u.id = cm.user_id
        WHERE cm.course_id = $1
        ORDER BY cm.added_at"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn is_member(db: &PgPool, course_id: Uuid, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT 1 FROM course_members WHERE course_id = $1 AND user_id = $2",
        course_id,
        user_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

pub async fn is_course_teacher(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT 1 FROM course_members WHERE course_id = $1 AND user_id = $2 AND role IN ('teacher', 'ta')",
        course_id,
        user_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Strict teacher check: `role = 'teacher'` only (TAs excluded). Use this for
/// operations TAs must not perform (LTI, API keys, invites, play designations).
pub async fn is_course_teacher_strict(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT 1 FROM course_members WHERE course_id = $1 AND user_id = $2 AND role = 'teacher'",
        course_id,
        user_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Returns the viewer's course_member role, or None if not a member.
pub async fn get_member_role(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT role FROM course_members WHERE course_id = $1 AND user_id = $2",
        course_id,
        user_id,
    )
    .fetch_optional(db)
    .await
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
    pub course: CourseRow,
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
                       last_synced_at = NOW()
                   WHERE momenttillf_id = $1"#,
                input.momenttillf_id,
                input.beteckning,
                input.name,
                input.semester_label,
                input.info_url,
                input.syllabus_url,
                input.unit,
            )
            .execute(&mut *tx)
            .await?;
            (course_id, false)
        }
        None => {
            let new_id = Uuid::new_v4();
            sqlx::query!(
                r#"INSERT INTO courses (
                    id, name, owner_id, daily_cost_limit_usd,
                    model, temperature, context_ratio, max_chunks, min_score,
                    strategy, tool_use_enabled, embedding_provider, embedding_model,
                    system_prompt, semester_label, auto_managed, course_code, reranker_model
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
                    $14, $15, TRUE, $16,
                    COALESCE($17, 'jinaai/jina-reranker-v2-base-multilingual')
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
                input.semester_label,
                input.beteckning,
                input.reranker_model,
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                r#"INSERT INTO course_daisy_offerings
                       (momenttillf_id, course_id, course_code, name,
                        semester_label, info_url, syllabus_url, unit, last_synced_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())"#,
                input.momenttillf_id,
                new_id,
                input.beteckning,
                input.name,
                input.semester_label,
                input.info_url,
                input.syllabus_url,
                input.unit,
            )
            .execute(&mut *tx)
            .await?;
            (new_id, true)
        }
    };

    tx.commit().await?;

    let course = find_by_id(db, course_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(DaisyUpsertOutcome { course, created })
}

/// Find the (active) course a Daisy offering is linked to. None when
/// the offering hasn't been imported yet, or its course was archived.
pub async fn find_by_daisy_momenttillf_id(
    db: &PgPool,
    momenttillf_id: &str,
) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        r#"SELECT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature,
               c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy,
               c.tool_use_enabled, c.embedding_provider, c.embedding_model,
               c.embedding_version, c.reranker_model, c.daily_cost_limit_usd, c.active, c.created_at,
               c.updated_at, c.semester_label, c.auto_managed, c.course_code
            FROM courses c
            JOIN course_daisy_offerings o ON o.course_id = c.id
            WHERE o.momenttillf_id = $1 AND c.active = true"#,
        momenttillf_id,
    )
    .fetch_optional(db)
    .await
}

/// Every course including archived ones (active first, then by
/// recency). Admin-only listing surface; the regular `list_all` hides
/// archived courses so they don't leak into teacher/student views.
pub async fn list_all_including_archived(db: &PgPool) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, active, created_at, updated_at, semester_label, auto_managed, course_code FROM courses ORDER BY active DESC, updated_at DESC",
    )
    .fetch_all(db)
    .await
}

/// Restore a soft-archived course (the inverse of `archive`). No-op if
/// the course is already active.
pub async fn unarchive(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE courses SET active = true, updated_at = NOW() WHERE id = $1 AND active = false",
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Per-table tallies returned to the admin UI after a merge.
pub struct MergeOutcome {
    pub documents_moved: i64,
    pub documents_orphaned: i64,
    pub documents_requeued: i64,
    pub conversations_moved: i64,
    pub members_merged: i64,
    pub offerings_moved: i64,
}

/// Merge `source_id` into `survivor_id` in one transaction, then
/// archive the source. Everything the source owns is re-pointed at the
/// survivor; conflicts resolve survivor-first.
///
/// The caller (route layer) MUST relocate the source's document bytes
/// on disk into the survivor's directory before this commits, because
/// the worker reconstructs file paths from `course_id`.
///
/// Policies:
///   * Documents: source docs that duplicate an active survivor doc (by
///     content hash, source identity, or source_url) are soft-orphaned
///     (and their source_url cleared, since that unique index ignores
///     orphaned_at) so they don't collide on move; everything else
///     moves. Moved, non-orphaned, non-URL-stub docs are re-queued to
///     `pending` so the worker re-embeds them into the survivor's
///     Qdrant collection. URL parent stubs stay as they are.
///   * Members: union, keeping the higher role (teacher > ta > student).
///   * usage_daily: summed per (user, day).
///   * Course-scoped config carrying a uniqueness constraint
///     (play designations, canvas connections, role suggestions,
///     feature flags, relink queue, suggested questions): survivor wins;
///     non-colliding source rows move, the rest are dropped.
///   * Everything else with a plain course_id (conversations, api keys,
///     signed urls, LTI links, KG caches, token usage, Daisy offerings):
///     re-pointed at the survivor.
pub async fn merge_courses(
    db: &PgPool,
    survivor_id: Uuid,
    source_id: Uuid,
) -> Result<MergeOutcome, sqlx::Error> {
    let mut tx = db.begin().await?;

    // Documents: neutralise duplicates against the survivor first so
    // the bulk move below can't trip a per-course unique index.
    let documents_orphaned = sqlx::query_scalar!(
        r#"WITH colliding AS (
               SELECT s.id
               FROM documents s
               WHERE s.course_id = $1
                 AND s.orphaned_at IS NULL
                 AND (
                   (s.parent_document_id IS NULL AND s.content_hash IS NOT NULL AND EXISTS (
                       SELECT 1 FROM documents t
                       WHERE t.course_id = $2 AND t.parent_document_id IS NULL
                         AND t.orphaned_at IS NULL AND t.content_hash = s.content_hash))
                   OR (s.source_ref IS NOT NULL AND EXISTS (
                       SELECT 1 FROM documents t
                       WHERE t.course_id = $2 AND t.orphaned_at IS NULL
                         AND t.source_ref = s.source_ref
                         AND t.source_system IS NOT DISTINCT FROM s.source_system))
                   OR (s.source_url IS NOT NULL AND EXISTS (
                       SELECT 1 FROM documents t
                       WHERE t.course_id = $2 AND t.source_url = s.source_url))
                 )
           ),
           updated AS (
               UPDATE documents
               SET orphaned_at = NOW(), source_url = NULL
               WHERE orphaned_at IS NULL
                 AND (id IN (SELECT id FROM colliding)
                      OR parent_document_id IN (SELECT id FROM colliding))
               RETURNING 1
           )
           SELECT COUNT(*) FROM updated"#,
        source_id,
        survivor_id,
    )
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);

    // Re-point every source document at the survivor.
    let moved_ids: Vec<Uuid> = sqlx::query_scalar!(
        "UPDATE documents SET course_id = $1 WHERE course_id = $2 RETURNING id",
        survivor_id,
        source_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let documents_moved = moved_ids.len() as i64;

    // Re-queue moved docs that carry embeddable bytes (skip orphaned
    // duplicates and URL parent stubs) so the worker re-embeds them into
    // the survivor's collection.
    let documents_requeued = if moved_ids.is_empty() {
        0
    } else {
        sqlx::query_scalar!(
            r#"WITH requeued AS (
                   UPDATE documents
                   SET status = 'pending',
                       chunk_count = NULL,
                       processed_at = NULL,
                       processing_started_at = NULL,
                       error_msg = NULL,
                       pooled_embedding = NULL
                   WHERE id = ANY($1)
                     AND orphaned_at IS NULL
                     AND mime_type <> 'text/x-url'
                   RETURNING 1
               )
               SELECT COUNT(*) FROM requeued"#,
            &moved_ids,
        )
        .fetch_one(&mut *tx)
        .await?
        .unwrap_or(0)
    };

    // Conversations (messages / reviews / flags / analyses / notes
    // cascade via conversation_id, so they ride along).
    let conversations_moved = sqlx::query_scalar!(
        r#"WITH m AS (
               UPDATE conversations SET course_id = $1 WHERE course_id = $2 RETURNING 1
           )
           SELECT COUNT(*) FROM m"#,
        survivor_id,
        source_id,
    )
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);

    // Members: union, higher role wins.
    let members_merged = sqlx::query_scalar!(
        r#"WITH ins AS (
               INSERT INTO course_members (course_id, user_id, role, added_at)
               SELECT $1, user_id, role, added_at FROM course_members WHERE course_id = $2
               ON CONFLICT (course_id, user_id) DO UPDATE SET role = CASE
                   WHEN (CASE EXCLUDED.role WHEN 'teacher' THEN 3 WHEN 'ta' THEN 2 ELSE 1 END)
                      > (CASE course_members.role WHEN 'teacher' THEN 3 WHEN 'ta' THEN 2 ELSE 1 END)
                   THEN EXCLUDED.role ELSE course_members.role END
               RETURNING 1
           )
           SELECT COUNT(*) FROM ins"#,
        survivor_id,
        source_id,
    )
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);
    sqlx::query!("DELETE FROM course_members WHERE course_id = $1", source_id)
        .execute(&mut *tx)
        .await?;

    // usage_daily: sum per (user, day) so spend caps stay accurate.
    sqlx::query!(
        r#"INSERT INTO usage_daily
               (user_id, course_id, date, prompt_tokens, completion_tokens,
                embedding_tokens, research_prompt_tokens, research_completion_tokens, request_count)
           SELECT user_id, $1, date, prompt_tokens, completion_tokens,
                  embedding_tokens, research_prompt_tokens, research_completion_tokens, request_count
           FROM usage_daily WHERE course_id = $2
           ON CONFLICT (user_id, course_id, date) DO UPDATE SET
               prompt_tokens = usage_daily.prompt_tokens + EXCLUDED.prompt_tokens,
               completion_tokens = usage_daily.completion_tokens + EXCLUDED.completion_tokens,
               embedding_tokens = usage_daily.embedding_tokens + EXCLUDED.embedding_tokens,
               research_prompt_tokens = usage_daily.research_prompt_tokens + EXCLUDED.research_prompt_tokens,
               research_completion_tokens = usage_daily.research_completion_tokens + EXCLUDED.research_completion_tokens,
               request_count = usage_daily.request_count + EXCLUDED.request_count"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM usage_daily WHERE course_id = $1", source_id)
        .execute(&mut *tx)
        .await?;

    // Survivor-wins course-scoped config: move non-colliding rows, drop
    // the rest.
    sqlx::query!(
        r#"UPDATE play_designations p SET course_id = $1
           WHERE p.course_id = $2
             AND NOT EXISTS (SELECT 1 FROM play_designations q
                             WHERE q.course_id = $1 AND q.designation = p.designation)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM play_designations WHERE course_id = $1",
        source_id
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        r#"UPDATE canvas_connections c SET course_id = $1
           WHERE c.course_id = $2
             AND NOT EXISTS (SELECT 1 FROM canvas_connections q
                             WHERE q.course_id = $1 AND q.canvas_course_id = c.canvas_course_id)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM canvas_connections WHERE course_id = $1",
        source_id
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        r#"UPDATE course_member_role_suggestions s SET course_id = $1
           WHERE s.course_id = $2
             AND NOT EXISTS (SELECT 1 FROM course_member_role_suggestions q
                             WHERE q.course_id = $1 AND q.user_id = s.user_id
                               AND q.suggested_role = s.suggested_role)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM course_member_role_suggestions WHERE course_id = $1",
        source_id,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        r#"UPDATE feature_flags f SET course_id = $1
           WHERE f.course_id = $2
             AND NOT EXISTS (SELECT 1 FROM feature_flags q
                             WHERE q.course_id = $1 AND q.flag = f.flag)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM feature_flags WHERE course_id = $1", source_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query!(
        r#"UPDATE relink_queue r SET course_id = $1
           WHERE r.course_id = $2
             AND NOT EXISTS (SELECT 1 FROM relink_queue q WHERE q.course_id = $1)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM relink_queue WHERE course_id = $1", source_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query!(
        r#"UPDATE course_suggested_questions c SET course_id = $1
           WHERE c.course_id = $2
             AND NOT EXISTS (SELECT 1 FROM course_suggested_questions q WHERE q.course_id = $1)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM course_suggested_questions WHERE course_id = $1",
        source_id,
    )
    .execute(&mut *tx)
    .await?;

    // Plain reassign (no course-scoped uniqueness to worry about).
    sqlx::query!(
        "UPDATE api_keys SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE signed_urls SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE lti_registrations SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE lti_course_bindings SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE lti_nrps_contexts SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE document_relations SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE rejected_edge_pairs SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE linker_decisions SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE course_token_usage SET course_id = $1 WHERE course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE daisy_pending_imports SET existing_course_id = $1 WHERE existing_course_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;

    // Daisy offerings: both the source's and survivor's now feed the
    // survivor, so future nightly syncs keep both in lockstep.
    let offerings_moved = sqlx::query_scalar!(
        r#"WITH m AS (
               UPDATE course_daisy_offerings SET course_id = $1 WHERE course_id = $2 RETURNING 1
           )
           SELECT COUNT(*) FROM m"#,
        survivor_id,
        source_id,
    )
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);

    // Finally archive the now-empty source.
    sqlx::query!(
        "UPDATE courses SET active = false, updated_at = NOW() WHERE id = $1",
        source_id,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(MergeOutcome {
        documents_moved,
        documents_orphaned,
        documents_requeued,
        conversations_moved,
        members_merged,
        offerings_moved,
    })
}
