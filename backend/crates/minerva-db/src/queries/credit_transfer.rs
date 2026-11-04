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
