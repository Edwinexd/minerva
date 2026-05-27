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
    /// `minerva-ingest::pipeline::collection_name`.
    pub embedding_version: i32,
    pub daily_token_limit: i64,
    pub active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Free-text `VT2026` / `HT2025`. NULL for ad-hoc courses; populated
    /// by the Daisy auto-import phase for every imported offering. Drives
    /// the per-semester grouping on the My Courses page.
    pub semester_label: Option<String>,
    /// Daisy momenttillfID (e.g. `7620`). Primary dedup key for the
    /// auto-import upsert; NULL on courses created manually.
    pub daisy_momenttillf_id: Option<String>,
    /// Public Daisy info URL for the offering (momentinfo.Momentinfo).
    pub daisy_info_url: Option<String>,
    /// External syllabus URL (utbildning.su.se planarkiv) sourced from
    /// the Daisy detail page.
    pub daisy_syllabus_url: Option<String>,
    /// Owning unit at DSV, e.g. `ACT`. Detail-page only.
    pub daisy_unit: Option<String>,
    /// Wall-clock of the last successful Daisy sync that touched this row.
    pub daisy_last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TRUE when the row was created by the Daisy auto-import phase;
    /// membership stays additive and owner is the env-var fallback
    /// until a real kursansvarig is identified and present in Minerva.
    pub auto_managed: bool,
}

pub struct CreateCourse {
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Uuid,
    pub daily_token_limit: i64,
    /// When `Some`, the new course is created with this embedding model
    /// instead of the SQL column DEFAULT. Used to honor the
    /// admin-managed `embedding_models.is_default` row; which lives
    /// in a separate table and so can't be wired through the ALTER
    /// COLUMN DEFAULT machinery. `None` keeps the original behaviour
    /// (column DEFAULT applies) so legacy callers and tests don't have
    /// to know about the table.
    pub embedding_model: Option<String>,
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
    pub daily_token_limit: Option<i64>,
    /// Admin / owner backfill of the per-semester grouping label.
    /// Outer `Option` distinguishes "no change" (None) from "set
    /// to value" (Some). We don't expose a way to clear the label
    /// back to NULL through the API; once stamped it stays stamped.
    pub semester_label: Option<String>,
}

pub async fn create(db: &PgPool, id: Uuid, input: &CreateCourse) -> Result<CourseRow, sqlx::Error> {
    // `COALESCE($6, embedding_model_default)` would be nicer but
    // postgres doesn't expose a column DEFAULT in expressions. Instead
    // we let `NULL::TEXT` fall through to the SQL DEFAULT via a
    // conditional INSERT: when the caller supplies `Some`, we override;
    // when `None`, we omit the column entirely so the DEFAULT kicks in.
    // Branching here keeps the row construction in one statement and
    // dodges a second UPDATE that would also bump `updated_at`.
    if let Some(model) = input.embedding_model.as_deref() {
        sqlx::query_as!(
            CourseRow,
            r#"INSERT INTO courses (id, name, description, owner_id, daily_token_limit, embedding_model, semester_label)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at, semester_label, daisy_momenttillf_id, daisy_info_url, daisy_syllabus_url, daisy_unit, daisy_last_synced_at, auto_managed"#,
            id,
            input.name,
            input.description,
            input.owner_id,
            input.daily_token_limit,
            model,
            input.semester_label,
        )
        .fetch_one(db)
        .await
    } else {
        sqlx::query_as!(
            CourseRow,
            r#"INSERT INTO courses (id, name, description, owner_id, daily_token_limit, semester_label)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at, semester_label, daisy_momenttillf_id, daisy_info_url, daisy_syllabus_url, daisy_unit, daisy_last_synced_at, auto_managed"#,
            id,
            input.name,
            input.description,
            input.owner_id,
            input.daily_token_limit,
            input.semester_label,
        )
        .fetch_one(db)
        .await
    }
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at, semester_label, daisy_momenttillf_id, daisy_info_url, daisy_syllabus_url, daisy_unit, daisy_last_synced_at, auto_managed FROM courses WHERE id = $1 AND active = true",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_by_owner(db: &PgPool, owner_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at, semester_label, daisy_momenttillf_id, daisy_info_url, daisy_syllabus_url, daisy_unit, daisy_last_synced_at, auto_managed FROM courses WHERE owner_id = $1 AND active = true ORDER BY updated_at DESC",
        owner_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_by_member(db: &PgPool, user_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        r#"SELECT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.daily_token_limit, c.active, c.created_at, c.updated_at, c.semester_label, c.daisy_momenttillf_id, c.daisy_info_url, c.daisy_syllabus_url, c.daisy_unit, c.daisy_last_synced_at, c.auto_managed
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
        r#"SELECT DISTINCT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.daily_token_limit, c.active, c.created_at, c.updated_at, c.semester_label, c.daisy_momenttillf_id, c.daisy_info_url, c.daisy_syllabus_url, c.daisy_unit, c.daisy_last_synced_at, c.auto_managed
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
        r#"SELECT DISTINCT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.daily_token_limit, c.active, c.created_at, c.updated_at, c.semester_label, c.daisy_momenttillf_id, c.daisy_info_url, c.daisy_syllabus_url, c.daisy_unit, c.daisy_last_synced_at, c.auto_managed
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
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at, semester_label, daisy_momenttillf_id, daisy_info_url, daisy_syllabus_url, daisy_unit, daisy_last_synced_at, auto_managed FROM courses WHERE active = true ORDER BY updated_at DESC",
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
            daily_token_limit = COALESCE($11, daily_token_limit),
            embedding_provider = COALESCE($12, embedding_provider),
            embedding_model = COALESCE($13, embedding_model),
            min_score = COALESCE($14, min_score),
            semester_label = COALESCE($15, semester_label),
            updated_at = NOW()
        WHERE id = $1 AND active = true
        RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at, semester_label, daisy_momenttillf_id, daisy_info_url, daisy_syllabus_url, daisy_unit, daisy_last_synced_at, auto_managed"#,
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
        input.daily_token_limit,
        input.embedding_provider,
        input.embedding_model,
        input.min_score,
        input.semester_label,
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
    /// Fallback owner applied only on INSERT. Subsequent syncs leave
    /// `owner_id` untouched here; see `swap_owner_from_fallback`.
    pub fallback_owner_id: Uuid,
    /// Per-student daily token cap stamped on INSERT. Same default the
    /// manual course-creation endpoint uses.
    pub daily_token_limit: i64,
    /// Resolved admin-default embedding model, or `None` to fall
    /// through to the courses-table column DEFAULT.
    pub embedding_model: Option<&'a str>,
}

pub struct DaisyUpsertOutcome {
    pub course: CourseRow,
    /// TRUE on a fresh INSERT, FALSE when an existing row was refreshed.
    /// Used by the import endpoint to decide whether to also auto-create
    /// the matching `play_designations` row.
    pub created: bool,
}

/// Idempotently upsert a course by Daisy momenttillfID.
///
/// INSERT path: stamps `auto_managed = TRUE`, owner = fallback,
/// `daisy_last_synced_at = NOW()`. UPDATE path: refreshes `name`,
/// `semester_label`, and the `daisy_*` metadata columns; bumps
/// `daisy_last_synced_at`. Never touches `owner_id`, `description`,
/// the model/strategy fields, or any teacher-tunable column.
///
/// The `(xmax = 0)` trick distinguishes INSERT from UPDATE: postgres
/// sets `xmax = 0` for a freshly inserted row and the conflict-target
/// xact id otherwise. Wrapping it in CAST() avoids the macro's bool
/// inference complaining about the system column.
pub async fn upsert_from_daisy(
    db: &PgPool,
    input: &DaisyCourseInput<'_>,
) -> Result<DaisyUpsertOutcome, sqlx::Error> {
    // Two-step: do the conflict-aware INSERT, then re-fetch the row.
    // We can't fold both into one `RETURNING` because sqlx generates a
    // distinct anonymous record type per `query!` site, and the
    // embedding_model branch must omit a column entirely (postgres
    // won't expose column DEFAULT in an INSERT expression). The
    // second SELECT is cheap and lets us keep `CourseRow` as the
    // single source of truth for the column list.
    let inserted_id_and_flag = if let Some(model) = input.embedding_model {
        let row = sqlx::query!(
            r#"INSERT INTO courses (
                id, name, owner_id, daily_token_limit, embedding_model,
                semester_label, daisy_momenttillf_id, daisy_info_url,
                daisy_syllabus_url, daisy_unit, daisy_last_synced_at,
                auto_managed
            ) VALUES (
                gen_random_uuid(), $1, $2, $3, $4,
                $5, $6, $7, $8, $9, NOW(), TRUE
            )
            ON CONFLICT (daisy_momenttillf_id)
                WHERE daisy_momenttillf_id IS NOT NULL
            DO UPDATE SET
                name = EXCLUDED.name,
                semester_label = COALESCE(EXCLUDED.semester_label, courses.semester_label),
                daisy_info_url = COALESCE(EXCLUDED.daisy_info_url, courses.daisy_info_url),
                daisy_syllabus_url = COALESCE(EXCLUDED.daisy_syllabus_url, courses.daisy_syllabus_url),
                daisy_unit = COALESCE(EXCLUDED.daisy_unit, courses.daisy_unit),
                daisy_last_synced_at = NOW(),
                updated_at = NOW()
            RETURNING id, (xmax = 0) AS "inserted!: bool""#,
            input.name,
            input.fallback_owner_id,
            input.daily_token_limit,
            model,
            input.semester_label,
            input.momenttillf_id,
            input.info_url,
            input.syllabus_url,
            input.unit,
        )
        .fetch_one(db)
        .await?;
        (row.id, row.inserted)
    } else {
        let row = sqlx::query!(
            r#"INSERT INTO courses (
                id, name, owner_id, daily_token_limit,
                semester_label, daisy_momenttillf_id, daisy_info_url,
                daisy_syllabus_url, daisy_unit, daisy_last_synced_at,
                auto_managed
            ) VALUES (
                gen_random_uuid(), $1, $2, $3,
                $4, $5, $6, $7, $8, NOW(), TRUE
            )
            ON CONFLICT (daisy_momenttillf_id)
                WHERE daisy_momenttillf_id IS NOT NULL
            DO UPDATE SET
                name = EXCLUDED.name,
                semester_label = COALESCE(EXCLUDED.semester_label, courses.semester_label),
                daisy_info_url = COALESCE(EXCLUDED.daisy_info_url, courses.daisy_info_url),
                daisy_syllabus_url = COALESCE(EXCLUDED.daisy_syllabus_url, courses.daisy_syllabus_url),
                daisy_unit = COALESCE(EXCLUDED.daisy_unit, courses.daisy_unit),
                daisy_last_synced_at = NOW(),
                updated_at = NOW()
            RETURNING id, (xmax = 0) AS "inserted!: bool""#,
            input.name,
            input.fallback_owner_id,
            input.daily_token_limit,
            input.semester_label,
            input.momenttillf_id,
            input.info_url,
            input.syllabus_url,
            input.unit,
        )
        .fetch_one(db)
        .await?;
        (row.id, row.inserted)
    };

    let (course_id, created) = inserted_id_and_flag;
    let course = find_by_id(db, course_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(DaisyUpsertOutcome { course, created })
}

/// Find a course by its Daisy momenttillfID. None when the offering
/// hasn't been imported yet.
pub async fn find_by_daisy_momenttillf_id(
    db: &PgPool,
    momenttillf_id: &str,
) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        r#"SELECT id, name, description, owner_id, context_ratio, temperature,
           model, system_prompt, max_chunks, min_score, strategy,
           tool_use_enabled, embedding_provider, embedding_model,
           embedding_version, daily_token_limit, active, created_at,
           updated_at, semester_label, daisy_momenttillf_id,
           daisy_info_url, daisy_syllabus_url, daisy_unit,
           daisy_last_synced_at, auto_managed
        FROM courses
        WHERE daisy_momenttillf_id = $1 AND active = true"#,
        momenttillf_id,
    )
    .fetch_optional(db)
    .await
}

/// Atomic owner swap: only succeeds when the course currently sits on
/// `fallback_owner_id`. Returns TRUE iff a row was updated. Used by
/// the auth-middleware drain to promote the first real kursansvarig
/// to owner; subsequent calls become no-ops once a real human owns
/// the course.
pub async fn swap_owner_from_fallback(
    db: &PgPool,
    course_id: Uuid,
    fallback_owner_id: Uuid,
    new_owner_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE courses
           SET owner_id = $3, updated_at = NOW()
           WHERE id = $1
             AND owner_id = $2
             AND auto_managed = TRUE
             AND active = true"#,
        course_id,
        fallback_owner_id,
        new_owner_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Bump `daisy_last_synced_at` without mutating anything else. Used
/// after the per-course upsert finishes its membership additions, so
/// the timestamp reflects the full sync rather than just the row write.
pub async fn touch_daisy_synced(db: &PgPool, course_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE courses SET daisy_last_synced_at = NOW() WHERE id = $1",
        course_id,
    )
    .execute(db)
    .await?;
    Ok(())
}
