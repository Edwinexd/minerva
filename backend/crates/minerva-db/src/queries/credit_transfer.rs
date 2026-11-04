//! The credit transfer subtype of a workspace (staff tool for assessing
//! credit transfer applications against DSV course plans). See migration
//! `20261104000003_credit_transfer_workspaces.sql`; generic workspace
//! queries live in `queries::workspaces`.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CreditTransferRow {
    pub workspace_id: Uuid,
    /// Cases and their uploaded syllabuses are deleted this many days
    /// after they were last touched.
    pub case_retention_days: i32,
    /// Lowest `users.role` with implicit access (via `workspace_access`).
    pub access_min_role: String,
}

/// Insert the subtype row for a workspace whose `kind` is
/// `credit_transfer`. Column defaults apply where `None`.
pub async fn insert<'e, E>(
    db: E,
    workspace_id: Uuid,
    case_retention_days: Option<i32>,
) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query!(
        r#"INSERT INTO credit_transfer_workspaces (workspace_id, case_retention_days)
           VALUES ($1, COALESCE($2, 90))"#,
        workspace_id,
        case_retention_days,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn find(
    db: &PgPool,
    workspace_id: Uuid,
) -> Result<Option<CreditTransferRow>, sqlx::Error> {
    sqlx::query_as!(
        CreditTransferRow,
        r#"SELECT workspace_id, case_retention_days, access_min_role
           FROM credit_transfer_workspaces WHERE workspace_id = $1"#,
        workspace_id,
    )
    .fetch_optional(db)
    .await
}

/// Every active credit transfer workspace; the syllabus loop keeps each
/// one's copy of the DSV course plans current.
pub async fn list_active(db: &PgPool) -> Result<Vec<CreditTransferRow>, sqlx::Error> {
    sqlx::query_as!(
        CreditTransferRow,
        r#"SELECT ct.workspace_id, ct.case_retention_days, ct.access_min_role
           FROM credit_transfer_workspaces ct
           JOIN workspaces w ON w.id = ct.workspace_id
           WHERE w.active"#,
    )
    .fetch_all(db)
    .await
}

/// What the workspace page and its home card say about the plan archive
/// and the viewer's own cases.
pub struct Summary {
    /// DSV plans with a current version available to compare against.
    pub plan_count: i64,
    /// Plans whose current version arrived or changed in the last week.
    pub plans_changed_this_week: i64,
    /// When the archive was last checked for new or revised PDFs.
    pub plans_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub my_case_count: i64,
}

pub async fn summary(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Summary, sqlx::Error> {
    sqlx::query_as!(
        Summary,
        r#"WITH latest AS (
               SELECT v.code, MAX(v.valid_from_term_id) AS term_id
               FROM syllabus_versions v
               JOIN syllabus_plans p ON p.code = v.code
               WHERE v.delisted_at IS NULL AND NOT p.is_discontinued
               GROUP BY v.code
           ),
           current AS (
               SELECT v.code,
                      BOOL_OR(v.content_hash IS NOT NULL) AS downloaded,
                      MAX(GREATEST(v.first_seen_at, v.content_changed_at)) AS changed_at
               FROM syllabus_versions v
               JOIN latest l ON l.code = v.code AND l.term_id = v.valid_from_term_id
               WHERE v.delisted_at IS NULL
               GROUP BY v.code
           )
           SELECT
               (SELECT COUNT(*) FROM current WHERE downloaded) AS "plan_count!",
               (SELECT COUNT(*) FROM current
                WHERE downloaded AND changed_at > NOW() - INTERVAL '7 days') AS "plans_changed_this_week!",
               (SELECT MAX(last_checked_at) FROM syllabus_versions) AS plans_checked_at,
               (SELECT COUNT(*) FROM conversations
                WHERE workspace_id = $1 AND user_id = $2) AS "my_case_count!""#,
        workspace_id,
        user_id,
    )
    .fetch_one(db)
    .await
}

/// One of the viewer's cases, for the case list.
pub struct CaseRow {
    pub id: Uuid,
    pub title: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub attachment_names: Vec<String>,
    /// Strongest candidate of the latest assessment, if one was made.
    pub best_code: Option<String>,
    pub best_verdict: Option<String>,
    pub candidate_count: Option<i32>,
}

pub async fn list_cases(
    db: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<CaseRow>, sqlx::Error> {
    sqlx::query_as!(
        CaseRow,
        r#"SELECT c.id, c.title, c.created_at, c.updated_at,
                  COALESCE(
                      (SELECT array_agg(a.filename ORDER BY a.created_at)
                       FROM conversation_attachments a WHERE a.conversation_id = c.id),
                      '{}') AS "attachment_names!",
                  latest.best_code, latest.best_verdict,
                  latest.candidate_count AS "candidate_count?"
           FROM conversations c
           LEFT JOIN LATERAL (
               SELECT best_code, best_verdict, candidate_count
               FROM case_assessments ca
               WHERE ca.conversation_id = c.id
               ORDER BY ca.created_at DESC
               LIMIT 1
           ) latest ON TRUE
           WHERE c.workspace_id = $1 AND c.user_id = $2
           ORDER BY c.updated_at DESC"#,
        workspace_id,
        user_id,
    )
    .fetch_all(db)
    .await
}

/// The workspace's mirrored plan documents, as (document id, plan code),
/// so a retrieval hit can be traced back to the plan it came from.
pub async fn plan_documents(
    db: &PgPool,
    workspace_id: Uuid,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id, split_part(source_ref, '/', 1) AS "code!"
           FROM documents
           WHERE workspace_id = $1 AND source_system = 'syllabus_archive'
             AND source_ref IS NOT NULL"#,
        workspace_id,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.code)).collect())
}

/// Indexing status of the workspace's current plan documents, keyed by
/// their `source_ref` (`{code}/{language}`).
pub async fn plan_document_statuses(
    db: &PgPool,
    workspace_id: Uuid,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT source_ref AS "source_ref!", status
           FROM documents
           WHERE workspace_id = $1 AND source_system = 'syllabus_archive'
             AND source_ref IS NOT NULL AND orphaned_at IS NULL"#,
        workspace_id,
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.source_ref, r.status)).collect())
}
