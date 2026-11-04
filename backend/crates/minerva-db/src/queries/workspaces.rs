use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct WorkspaceRow {
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
    /// Cumulative billed tokens at which a conversation starts nudging
    /// the student to continue in a fresh one. 0 = never nudge.
    pub conversation_soft_token_limit: i64,
    /// Cumulative billed tokens at which a conversation stops accepting
    /// new messages entirely. 0 = no ceiling. Enforced in
    /// `routes::chat::send_message`; see the `20260905000001` migration
    /// for why the limit is per-conversation and denominated in tokens.
    pub conversation_hard_token_limit: i64,
    pub active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Which subtype row this workspace has: `course` (a row in
    /// `courses`) or `credit_transfer`. Parsed by
    /// `minerva_core::models::WorkspaceKind`. Course-only facts (term,
    /// course code, Daisy management) live in `queries::courses`.
    pub kind: String,
}

/// Every column the admin-tunable `system_defaults` registry covers
/// is passed in here as `Some(...)` from the route layer; the SQL
/// column DEFAULTs become a fallback ("nothing in `system_defaults`,
/// nothing the caller wants to set") for legacy callers like tests
/// that don't want to know about the registry. The route layer
/// (`routes::courses::create_course`) always supplies every field so
/// that admin edits in `/admin/defaults` flow straight into new
/// workspaces; existing ones are unaffected.
pub struct CreateWorkspace {
    /// `minerva_core::models::WorkspaceKind::as_str()`. A `course` gets its
    /// subtype row from [`create_course`]; use that instead of [`create`].
    pub kind: &'static str,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Uuid,
    pub daily_cost_limit_usd: Decimal,
    /// Snapshotted from the `course.conversation_soft_token_limit` /
    /// `course.conversation_hard_token_limit` system defaults at create
    /// time, like every other CourseAi knob. `None` falls through to the
    /// column DEFAULT for the dev-seed / test callers.
    pub conversation_soft_token_limit: Option<i64>,
    pub conversation_hard_token_limit: Option<i64>,
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
}

pub struct UpdateWorkspace {
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
    /// Per-conversation token ceilings. `None` = no change.
    pub conversation_soft_token_limit: Option<i64>,
    pub conversation_hard_token_limit: Option<i64>,
}

/// Insert a workspace row without a subtype row. For a course use
/// [`create_course`], which adds the `courses` row in the same transaction.
pub async fn create<'e, E>(
    db: E,
    id: Uuid,
    input: &CreateWorkspace,
) -> Result<WorkspaceRow, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    // One unconditional INSERT, with `COALESCE($N, <literal>)` falling
    // through to the migration's column DEFAULT for any field the
    // caller passes as `None`. The route layer always supplies every
    // field from `system_defaults`, so the literals here only matter for
    // `dev_seed` + tests which knowingly pass `None`. The literals
    // mirror the values in the workspaces-table migrations; drifting them
    // would only affect those legacy callers.
    sqlx::query_as!(
        WorkspaceRow,
        r#"INSERT INTO workspaces (
            id, name, description, owner_id, daily_cost_limit_usd,
            model, temperature, context_ratio, max_chunks, min_score,
            strategy, tool_use_enabled, embedding_provider, embedding_model, system_prompt,
            kind, reranker_model,
            conversation_soft_token_limit, conversation_hard_token_limit
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
            COALESCE($17, 'jinaai/jina-reranker-v2-base-multilingual'),
            COALESCE($18::BIGINT, 300000),
            COALESCE($19::BIGINT, 1000000)
        )
        RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, conversation_soft_token_limit, conversation_hard_token_limit, active, created_at, updated_at, kind"#,
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
        input.kind,
        input.reranker_model.as_deref(),
        input.conversation_soft_token_limit,
        input.conversation_hard_token_limit,
    )
    .fetch_one(db)
    .await
}

/// Create a course: the workspace row (`input.kind` must be `course`) and
/// its `courses` subtype row, atomically. `semester_label` is NULL for an
/// ad-hoc course; the route layer validates the format.
pub async fn create_course(
    db: &PgPool,
    id: Uuid,
    input: &CreateWorkspace,
    semester_label: Option<&str>,
) -> Result<WorkspaceRow, sqlx::Error> {
    debug_assert_eq!(input.kind, "course");
    let mut tx = db.begin().await?;
    let row = create(&mut *tx, id, input).await?;
    super::courses::insert(&mut *tx, id, semester_label, None, false).await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkspaceRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, conversation_soft_token_limit, conversation_hard_token_limit, active, created_at, updated_at, kind FROM workspaces WHERE id = $1 AND active = true",
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn list_by_owner(db: &PgPool, owner_id: Uuid) -> Result<Vec<WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkspaceRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, conversation_soft_token_limit, conversation_hard_token_limit, active, created_at, updated_at, kind FROM workspaces WHERE owner_id = $1 AND active = true ORDER BY updated_at DESC",
        owner_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_by_member(db: &PgPool, user_id: Uuid) -> Result<Vec<WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkspaceRow,
        r#"SELECT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.reranker_model, c.daily_cost_limit_usd, c.conversation_soft_token_limit, c.conversation_hard_token_limit, c.active, c.created_at, c.updated_at, c.kind
        FROM workspaces c
        JOIN workspace_members cm ON cm.workspace_id = c.id
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
pub async fn list_for_teacher(
    db: &PgPool,
    user_id: Uuid,
) -> Result<Vec<WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkspaceRow,
        r#"SELECT DISTINCT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.reranker_model, c.daily_cost_limit_usd, c.conversation_soft_token_limit, c.conversation_hard_token_limit, c.active, c.created_at, c.updated_at, c.kind
        FROM workspaces c
        LEFT JOIN workspace_members cm ON cm.workspace_id = c.id AND cm.user_id = $1
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
) -> Result<Vec<WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkspaceRow,
        r#"SELECT DISTINCT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.min_score, c.strategy, c.tool_use_enabled, c.embedding_provider, c.embedding_model, c.embedding_version, c.reranker_model, c.daily_cost_limit_usd, c.conversation_soft_token_limit, c.conversation_hard_token_limit, c.active, c.created_at, c.updated_at, c.kind
        FROM workspaces c
        LEFT JOIN workspace_members cm ON cm.workspace_id = c.id AND cm.user_id = $1
        WHERE c.active = true
          AND (c.owner_id = $1 OR cm.role = 'teacher')
        ORDER BY c.updated_at DESC"#,
        user_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_all(db: &PgPool) -> Result<Vec<WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkspaceRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, conversation_soft_token_limit, conversation_hard_token_limit, active, created_at, updated_at, kind FROM workspaces WHERE active = true ORDER BY updated_at DESC",
    )
    .fetch_all(db)
    .await
}

pub async fn update(
    db: &PgPool,
    id: Uuid,
    input: &UpdateWorkspace,
) -> Result<Option<WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkspaceRow,
        r#"UPDATE workspaces SET
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
            reranker_model = COALESCE($15, reranker_model),
            conversation_soft_token_limit = COALESCE($16, conversation_soft_token_limit),
            conversation_hard_token_limit = COALESCE($17, conversation_hard_token_limit),
            updated_at = NOW()
        WHERE id = $1 AND active = true
        RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, conversation_soft_token_limit, conversation_hard_token_limit, active, created_at, updated_at, kind"#,
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
        input.reranker_model,
        input.conversation_soft_token_limit,
        input.conversation_hard_token_limit,
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
        r#"UPDATE workspaces
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
               WHERE workspace_id = $1
               RETURNING 1
           )
           SELECT COUNT(*) FROM updated"#,
        id,
    )
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);

    // Topic-switch vectors live on user messages rather than in the document
    // worker queue, but they share the course embedding space. Invalidate
    // them in this same transaction as the generation bump so no committed
    // state can claim a new course version while retaining usable-looking old
    // vectors. The chat-side spawned task lazily repairs its bounded six-turn
    // window on the next message.
    sqlx::query!(
        r#"UPDATE messages m
           SET topic_embedding = NULL,
               topic_embedding_model = NULL,
               topic_embedding_version = NULL
          FROM conversations c
         WHERE m.conversation_id = c.id
           AND c.workspace_id = $1
           AND (m.topic_embedding IS NOT NULL
                OR m.topic_embedding_model IS NOT NULL
                OR m.topic_embedding_version IS NOT NULL)"#,
        id,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(RotateEmbeddingOutcome {
        new_version,
        requeued_documents,
    })
}

pub async fn archive(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE workspaces SET active = false, updated_at = NOW() WHERE id = $1 AND active = true",
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[derive(Debug)]
pub struct MemberRow {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub eppn: Option<String>,
    pub display_name: Option<String>,
}

pub async fn add_member(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT (workspace_id, user_id) DO UPDATE SET role = $3",
        workspace_id,
        user_id,
        role,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn remove_member(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        workspace_id,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_members(db: &PgPool, workspace_id: Uuid) -> Result<Vec<MemberRow>, sqlx::Error> {
    sqlx::query_as!(
        MemberRow,
        r#"SELECT cm.workspace_id, cm.user_id, cm.role, cm.added_at, u.eppn AS "eppn?", u.display_name
        FROM workspace_members cm
        JOIN users u ON u.id = cm.user_id
        WHERE cm.workspace_id = $1
        ORDER BY cm.added_at"#,
        workspace_id,
    )
    .fetch_all(db)
    .await
}

pub async fn is_member(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        workspace_id,
        user_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

pub async fn is_teacher(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2 AND role IN ('teacher', 'ta')",
        workspace_id,
        user_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Strict teacher check: `role = 'teacher'` only (TAs excluded). Use this for
/// operations TAs must not perform (LTI, API keys, invites, play designations).
pub async fn is_teacher_strict(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2 AND role = 'teacher'",
        workspace_id,
        user_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Returns the viewer's course_member role, or None if not a member.
pub async fn get_member_role(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        workspace_id,
        user_id,
    )
    .fetch_optional(db)
    .await
}

/// Count the `student`-role members of a course. This is the population
/// the per-student daily spend cap (`courses.daily_cost_limit_usd`)
/// applies to, so the admin / teacher UIs multiply it by the cap to show
/// the course-wide theoretical daily max ("N students x cap"). Teachers
/// and TAs are excluded: the cap is framed per *student*, and they're not
/// the budgeted population. Students reliably land in `course_members`
/// (LTI launch auto-enrols them, and chat requires membership), so this
/// is an accurate denominator.
pub async fn count_students(db: &PgPool, workspace_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT COUNT(*) FROM workspace_members WHERE workspace_id = $1 AND role = 'student'",
        workspace_id,
    )
    .fetch_one(db)
    .await
    .map(|n| n.unwrap_or(0))
}

/// Batched variant of [`count_students`] for the multi-course list
/// endpoints (teacher/admin course listings). One grouped aggregate over
/// `course_members` instead of one COUNT per course, so listing N courses
/// stays a single query rather than adding an Nth round-trip to the
/// already-per-course flag/offering resolution. Courses with zero student
/// members are simply absent from the map; callers default to 0.
pub async fn count_students_by_workspace(
    db: &PgPool,
) -> Result<std::collections::HashMap<Uuid, i64>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT workspace_id, COUNT(*) AS "n!"
           FROM workspace_members
           WHERE role = 'student'
           GROUP BY workspace_id"#,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.workspace_id, r.n)).collect())
}

/// Every course including archived ones (active first, then by
/// recency). Admin-only listing surface; the regular `list_all` hides
/// archived courses so they don't leak into teacher/student views.
pub async fn list_all_including_archived(db: &PgPool) -> Result<Vec<WorkspaceRow>, sqlx::Error> {
    sqlx::query_as!(
        WorkspaceRow,
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, tool_use_enabled, embedding_provider, embedding_model, embedding_version, reranker_model, daily_cost_limit_usd, conversation_soft_token_limit, conversation_hard_token_limit, active, created_at, updated_at, kind FROM workspaces ORDER BY active DESC, updated_at DESC",
    )
    .fetch_all(db)
    .await
}

/// Restore a soft-archived course (the inverse of `archive`). No-op if
/// the course is already active.
pub async fn unarchive(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE workspaces SET active = true, updated_at = NOW() WHERE id = $1 AND active = false",
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
///   * usage_daily: summed per (user, day, model), so spend stays priced at
///     the model that served it.
///   * Course-scoped config carrying a uniqueness constraint
///     (play designations, canvas connections, role suggestions,
///     feature flags, relink queue, suggested questions): survivor wins;
///     non-colliding source rows move, the rest are dropped.
///   * Everything else with a plain course_id (conversations, api keys,
///     signed urls, LTI links, KG caches, token usage, Daisy offerings,
///     visual extraction jobs and figures): re-pointed at the survivor.
///   * Moved figures lose `indexed_at`, so the indexing sweep rewrites their
///     Qdrant points with the survivor's id and text model. Person crops
///     are never indexed and keep theirs.
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
               WHERE s.workspace_id = $1
                 AND s.orphaned_at IS NULL
                 AND (
                   (s.parent_document_id IS NULL AND s.content_hash IS NOT NULL AND EXISTS (
                       SELECT 1 FROM documents t
                       WHERE t.workspace_id = $2 AND t.parent_document_id IS NULL
                         AND t.orphaned_at IS NULL AND t.content_hash = s.content_hash))
                   OR (s.source_ref IS NOT NULL AND EXISTS (
                       SELECT 1 FROM documents t
                       WHERE t.workspace_id = $2 AND t.orphaned_at IS NULL
                         AND t.source_ref = s.source_ref
                         AND t.source_system IS NOT DISTINCT FROM s.source_system))
                   OR (s.source_url IS NOT NULL AND EXISTS (
                       SELECT 1 FROM documents t
                       WHERE t.workspace_id = $2 AND t.source_url = s.source_url))
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

    // The source_url index ignores orphaned_at, so an already-orphaned
    // source doc can collide too.
    sqlx::query!(
        r#"UPDATE documents s SET source_url = NULL
           WHERE s.workspace_id = $1
             AND s.orphaned_at IS NOT NULL
             AND s.source_url IS NOT NULL
             AND EXISTS (SELECT 1 FROM documents t
                         WHERE t.workspace_id = $2 AND t.source_url = s.source_url)"#,
        source_id,
        survivor_id,
    )
    .execute(&mut *tx)
    .await?;

    // Re-point every source document at the survivor.
    let moved_ids: Vec<Uuid> = sqlx::query_scalar!(
        "UPDATE documents SET workspace_id = $1 WHERE workspace_id = $2 RETURNING id",
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
               UPDATE conversations SET workspace_id = $1 WHERE workspace_id = $2 RETURNING 1
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
               INSERT INTO workspace_members (workspace_id, user_id, role, added_at)
               SELECT $1, user_id, role, added_at FROM workspace_members WHERE workspace_id = $2
               ON CONFLICT (workspace_id, user_id) DO UPDATE SET role = CASE
                   WHEN (CASE EXCLUDED.role WHEN 'teacher' THEN 3 WHEN 'ta' THEN 2 ELSE 1 END)
                      > (CASE workspace_members.role WHEN 'teacher' THEN 3 WHEN 'ta' THEN 2 ELSE 1 END)
                   THEN EXCLUDED.role ELSE workspace_members.role END
               RETURNING 1
           )
           SELECT COUNT(*) FROM ins"#,
        survivor_id,
        source_id,
    )
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);
    sqlx::query!(
        "DELETE FROM workspace_members WHERE workspace_id = $1",
        source_id
    )
    .execute(&mut *tx)
    .await?;

    // usage_daily: sum per (user, day, model) so spend caps stay accurate.
    sqlx::query!(
        r#"INSERT INTO usage_daily
               (user_id, workspace_id, date, model, provider, prompt_tokens, completion_tokens,
                embedding_tokens, research_prompt_tokens, research_completion_tokens, request_count)
           SELECT user_id, $1, date, model, provider, prompt_tokens, completion_tokens,
                  embedding_tokens, research_prompt_tokens, research_completion_tokens, request_count
           FROM usage_daily WHERE workspace_id = $2
           ON CONFLICT (user_id, workspace_id, date, model) DO UPDATE SET
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
    sqlx::query!("DELETE FROM usage_daily WHERE workspace_id = $1", source_id)
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
        r#"UPDATE feature_flags f SET workspace_id = $1
           WHERE f.workspace_id = $2
             AND NOT EXISTS (SELECT 1 FROM feature_flags q
                             WHERE q.workspace_id = $1 AND q.flag = f.flag)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM feature_flags WHERE workspace_id = $1",
        source_id
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        r#"UPDATE relink_queue r SET workspace_id = $1
           WHERE r.workspace_id = $2
             AND NOT EXISTS (SELECT 1 FROM relink_queue q WHERE q.workspace_id = $1)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM relink_queue WHERE workspace_id = $1",
        source_id
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        r#"UPDATE workspace_suggested_questions c SET workspace_id = $1
           WHERE c.workspace_id = $2
             AND NOT EXISTS (SELECT 1 FROM workspace_suggested_questions q WHERE q.workspace_id = $1)"#,
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM workspace_suggested_questions WHERE workspace_id = $1",
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
        "UPDATE document_relations SET workspace_id = $1 WHERE workspace_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE rejected_edge_pairs SET workspace_id = $1 WHERE workspace_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE linker_decisions SET workspace_id = $1 WHERE workspace_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE workspace_token_usage SET workspace_id = $1 WHERE workspace_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE visual_extraction_jobs SET workspace_id = $1 WHERE workspace_id = $2",
        survivor_id,
        source_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"UPDATE document_figures
           SET workspace_id = $1,
               indexed_at = CASE WHEN shows_person THEN indexed_at END
           WHERE workspace_id = $2"#,
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
        "UPDATE workspaces SET active = false, updated_at = NOW() WHERE id = $1",
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
