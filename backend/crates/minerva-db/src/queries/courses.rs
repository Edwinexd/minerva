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
            r#"INSERT INTO courses (id, name, description, owner_id, daily_token_limit, embedding_model)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at"#,
            id,
            input.name,
            input.description,
            input.owner_id,
            input.daily_token_limit,
            model,
        )
        .fetch_one(db)
        .await
    } else {
        sqlx::query_as!(
            CourseRow,
            r#"INSERT INTO courses (id, name, description, owner_id, daily_token_limit)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at"#,
            id,
            input.name,
            input.description,
            input.owner_id,
            input.daily_token_limit,
        )
        .fetch_one(db)
        .await
    }
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at FROM courses WHERE id = $1 AND active = true",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_by_owner(db: &PgPool, owner_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at FROM courses WHERE owner_id = $1 AND active = true ORDER BY updated_at DESC",
        owner_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_by_member(db: &PgPool, user_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as!(
        CourseRow,
        r#"SELECT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.daily_token_limit, c.active, c.created_at, c.updated_at
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
        r#"SELECT DISTINCT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.daily_token_limit, c.active, c.created_at, c.updated_at
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
        r#"SELECT DISTINCT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.daily_token_limit, c.active, c.created_at, c.updated_at
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
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at FROM courses WHERE active = true ORDER BY updated_at DESC",
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
            updated_at = NOW()
        WHERE id = $1 AND active = true
        RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, daily_token_limit, active, created_at, updated_at"#,
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
