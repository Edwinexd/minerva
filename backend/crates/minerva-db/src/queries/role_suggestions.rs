use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct RoleSuggestionRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub suggested_role: String,
    pub source: String,
    pub source_detail: Option<JsonValue>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub resolved_by: Option<Uuid>,
    pub resolution: Option<String>,
}

#[derive(Debug)]
pub struct PendingSuggestionWithUser {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub eppn: Option<String>,
    pub display_name: Option<String>,
    pub current_role: Option<String>,
    pub suggested_role: String,
    pub source: String,
    pub source_detail: Option<JsonValue>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Insert a pending suggestion. A prior suggestion for the same
/// (course, user, role) -- pending, approved, or declined -- is left
/// untouched: declined stays declined forever, approved has already been
/// acted on, and a duplicate pending row would just be noise.
///
/// Returns true when a new row was created.
pub async fn upsert_pending(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    user_id: Uuid,
    suggested_role: &str,
    source: &str,
    source_detail: Option<&JsonValue>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"INSERT INTO course_member_role_suggestions
            (id, course_id, user_id, suggested_role, source, source_detail)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (course_id, user_id, suggested_role) DO NOTHING"#,
        id,
        course_id,
        user_id,
        suggested_role,
        source,
        source_detail,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Has this (course, user, role) tuple already been resolved (approved or
/// declined)? Used by the LTI handler to skip creating a suggestion when
/// a prior decline should remain sticky.
pub async fn is_resolved(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
    suggested_role: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"SELECT 1 FROM course_member_role_suggestions
           WHERE course_id = $1 AND user_id = $2 AND suggested_role = $3
             AND resolution IS NOT NULL"#,
        course_id,
        user_id,
        suggested_role,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

pub async fn list_pending_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<PendingSuggestionWithUser>, sqlx::Error> {
    sqlx::query_as!(
        PendingSuggestionWithUser,
        r#"SELECT s.id, s.course_id, s.user_id,
                  u.eppn, u.display_name,
                  cm.role AS current_role,
                  s.suggested_role, s.source, s.source_detail,
                  s.created_at
           FROM course_member_role_suggestions s
           JOIN users u ON u.id = s.user_id
           LEFT JOIN course_members cm
             ON cm.course_id = s.course_id AND cm.user_id = s.user_id
           WHERE s.course_id = $1 AND s.resolution IS NULL
           ORDER BY s.created_at ASC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn find_pending_by_id(
    db: &PgPool,
    id: Uuid,
) -> Result<Option<RoleSuggestionRow>, sqlx::Error> {
    sqlx::query_as!(
        RoleSuggestionRow,
        r#"SELECT id, course_id, user_id, suggested_role, source, source_detail,
                  created_at, resolved_at, resolved_by, resolution
           FROM course_member_role_suggestions
           WHERE id = $1 AND resolution IS NULL"#,
        id,
    )
    .fetch_optional(db)
    .await
}

/// Mark the suggestion as approved. The caller is responsible for bumping
/// the course_members row -- keeping those as two explicit steps makes the
/// route handler's authorisation boundary obvious.
pub async fn mark_approved(db: &PgPool, id: Uuid, resolved_by: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE course_member_role_suggestions
           SET resolution = 'approved', resolved_at = NOW(), resolved_by = $2
           WHERE id = $1 AND resolution IS NULL"#,
        id,
        resolved_by,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_declined(db: &PgPool, id: Uuid, resolved_by: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE course_member_role_suggestions
           SET resolution = 'declined', resolved_at = NOW(), resolved_by = $2
           WHERE id = $1 AND resolution IS NULL"#,
        id,
        resolved_by,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
