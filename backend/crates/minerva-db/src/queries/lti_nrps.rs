//! Queries for LTI Advantage Names and Role Provisioning Service (NRPS).
//!
//! `lti_nrps_contexts` records each syncable LMS context (its NRPS membership
//! URL + the Minerva course it reconciles into), keyed to exactly one launch
//! source (a per-course registration OR a site-level platform). The periodic
//! reconcile loop reads `find_due_for_sync` and writes results back via
//! `record_sync_result`.
//!
//! `lti_nrps_memberships` is the provenance ledger: which (context, user)
//! memberships NRPS provisioned. The reconcile loop only ever removes members
//! it finds here, so non-LTI members and the course owner are never touched.
//!
//! `lti_nrps_sync_runs` is the history behind the `last_sync_*` columns: one
//! row per run that changed membership or failed.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NrpsContextRow {
    pub id: Uuid,
    pub registration_id: Option<Uuid>,
    pub platform_id: Option<Uuid>,
    pub context_id: String,
    pub course_id: Uuid,
    pub memberships_url: String,
    /// FALSE = the periodic reconcile skips this context. Members then only
    /// join by launching the tool; nobody already provisioned is removed.
    pub sync_enabled: bool,
    pub last_sync_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_sync_status: Option<String>,
    pub last_sync_error: Option<String>,
    /// Actionable, human-readable note independent of `last_sync_status`: a
    /// sync can be `ok` and still surface a warning here (e.g. the platform
    /// answered 200 OK but isn't sharing any identity claims, leaving every
    /// member to fall back to a synthetic eppn).
    pub last_sync_warning: Option<String>,
    pub last_sync_added: Option<i32>,
    pub last_sync_removed: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Which launch source owns an NRPS context. Mirrors
/// `lti::LaunchSource`; exactly one variant maps to a non-null FK.
pub enum NrpsSource {
    Registration(Uuid),
    Platform(Uuid),
}

/// Upsert the NRPS context discovered during a launch. Keyed on
/// (source, context_id); a re-launch refreshes the membership URL and the
/// bound course without creating duplicates.
///
/// `sync_enabled` is `Some` only when the caller carries an explicit choice
/// (the bind picker). `None` leaves an existing row's setting alone and
/// defaults a new row to enabled, so an ordinary launch never flips it.
pub async fn upsert_context(
    db: &PgPool,
    id: Uuid,
    source: NrpsSource,
    context_id: &str,
    course_id: Uuid,
    memberships_url: &str,
    sync_enabled: Option<bool>,
) -> Result<NrpsContextRow, sqlx::Error> {
    match source {
        NrpsSource::Registration(registration_id) => {
            sqlx::query_as!(
                NrpsContextRow,
                r#"INSERT INTO lti_nrps_contexts
                    (id, registration_id, platform_id, context_id, course_id, memberships_url,
                     sync_enabled)
                VALUES ($1, $2, NULL, $3, $4, $5, COALESCE($6::boolean, TRUE))
                ON CONFLICT (registration_id, context_id) WHERE registration_id IS NOT NULL
                DO UPDATE SET memberships_url = EXCLUDED.memberships_url,
                              course_id = EXCLUDED.course_id,
                              sync_enabled = COALESCE($6::boolean, lti_nrps_contexts.sync_enabled),
                              updated_at = NOW()
                RETURNING id, registration_id, platform_id, context_id, course_id, memberships_url, sync_enabled,
                          last_sync_at, last_sync_status, last_sync_error, last_sync_warning,
                          last_sync_added, last_sync_removed, created_at, updated_at"#,
                id,
                registration_id,
                context_id,
                course_id,
                memberships_url,
                sync_enabled,
            )
            .fetch_one(db)
            .await
        }
        NrpsSource::Platform(platform_id) => {
            sqlx::query_as!(
                NrpsContextRow,
                r#"INSERT INTO lti_nrps_contexts
                    (id, registration_id, platform_id, context_id, course_id, memberships_url,
                     sync_enabled)
                VALUES ($1, NULL, $2, $3, $4, $5, COALESCE($6::boolean, TRUE))
                ON CONFLICT (platform_id, context_id) WHERE platform_id IS NOT NULL
                DO UPDATE SET memberships_url = EXCLUDED.memberships_url,
                              course_id = EXCLUDED.course_id,
                              sync_enabled = COALESCE($6::boolean, lti_nrps_contexts.sync_enabled),
                              updated_at = NOW()
                RETURNING id, registration_id, platform_id, context_id, course_id, memberships_url, sync_enabled,
                          last_sync_at, last_sync_status, last_sync_error, last_sync_warning,
                          last_sync_added, last_sync_removed, created_at, updated_at"#,
                id,
                platform_id,
                context_id,
                course_id,
                memberships_url,
                sync_enabled,
            )
            .fetch_one(db)
            .await
        }
    }
}

/// Contexts due for a reconcile: sync enabled, and never synced or last
/// synced longer ago than `interval_hours`. Oldest-first so a backlog drains
/// fairly.
pub async fn find_due_for_sync(
    db: &PgPool,
    interval_hours: i32,
) -> Result<Vec<NrpsContextRow>, sqlx::Error> {
    sqlx::query_as!(
        NrpsContextRow,
        r#"SELECT id, registration_id, platform_id, context_id, course_id, memberships_url, sync_enabled,
                  last_sync_at, last_sync_status, last_sync_error, last_sync_warning,
                  last_sync_added, last_sync_removed, created_at, updated_at
        FROM lti_nrps_contexts
        WHERE sync_enabled
          AND (last_sync_at IS NULL
               OR last_sync_at < NOW() - make_interval(hours => $1))
        ORDER BY last_sync_at NULLS FIRST"#,
        interval_hours,
    )
    .fetch_all(db)
    .await
}

pub async fn list_contexts_for_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<NrpsContextRow>, sqlx::Error> {
    sqlx::query_as!(
        NrpsContextRow,
        r#"SELECT id, registration_id, platform_id, context_id, course_id, memberships_url, sync_enabled,
                  last_sync_at, last_sync_status, last_sync_error, last_sync_warning,
                  last_sync_added, last_sync_removed, created_at, updated_at
        FROM lti_nrps_contexts
        WHERE course_id = $1
        ORDER BY created_at"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn list_contexts_for_platform(
    db: &PgPool,
    platform_id: Uuid,
) -> Result<Vec<NrpsContextRow>, sqlx::Error> {
    sqlx::query_as!(
        NrpsContextRow,
        r#"SELECT id, registration_id, platform_id, context_id, course_id, memberships_url, sync_enabled,
                  last_sync_at, last_sync_status, last_sync_error, last_sync_warning,
                  last_sync_added, last_sync_removed, created_at, updated_at
        FROM lti_nrps_contexts
        WHERE platform_id = $1
        ORDER BY created_at"#,
        platform_id,
    )
    .fetch_all(db)
    .await
}

/// Record the outcome of a reconcile run. `status` is 'ok' or 'error';
/// `error` is NULL on success. `warning` is independent of status: a
/// successful run can still carry an actionable note about the platform
/// (e.g. identity claims missing across the entire roster). Counts are NULL
/// when the run errored before it could determine them.
///
/// A run that changed membership or failed is also appended to
/// `lti_nrps_sync_runs`; a clean no-op run only moves `last_sync_at`.
pub async fn record_sync_result(
    db: &PgPool,
    id: Uuid,
    status: &str,
    error: Option<&str>,
    warning: Option<&str>,
    added: Option<i32>,
    removed: Option<i32>,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query!(
        r#"UPDATE lti_nrps_contexts
           SET last_sync_at = NOW(),
               last_sync_status = $2,
               last_sync_error = $3,
               last_sync_warning = $4,
               last_sync_added = $5,
               last_sync_removed = $6,
               updated_at = NOW()
           WHERE id = $1"#,
        id,
        status,
        error,
        warning,
        added,
        removed,
    )
    .execute(&mut *tx)
    .await?;

    let changed = added.unwrap_or(0) + removed.unwrap_or(0) > 0;
    if status != "ok" || changed {
        sqlx::query!(
            r#"INSERT INTO lti_nrps_sync_runs
                (id, nrps_context_id, status, error, warning, added, removed)
            VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            Uuid::new_v4(),
            id,
            status,
            error,
            warning,
            added,
            removed,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await
}

/// Switch periodic sync on or off for one context. Scoped to `course_id` so
/// a teacher can only reach contexts that reconcile into their own course.
/// Returns false when no such context exists.
pub async fn set_sync_enabled(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    enabled: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE lti_nrps_contexts
           SET sync_enabled = $3, updated_at = NOW()
           WHERE id = $1 AND course_id = $2"#,
        id,
        course_id,
        enabled,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[derive(Debug, Clone)]
pub struct NrpsSyncRunRow {
    pub id: Uuid,
    pub nrps_context_id: Uuid,
    pub ran_at: chrono::DateTime<chrono::Utc>,
    pub status: String,
    pub error: Option<String>,
    pub warning: Option<String>,
    pub added: Option<i32>,
    pub removed: Option<i32>,
}

/// The newest `per_context` recorded runs for each of `context_ids`, newest
/// first.
pub async fn list_recent_runs(
    db: &PgPool,
    context_ids: &[Uuid],
    per_context: i64,
) -> Result<Vec<NrpsSyncRunRow>, sqlx::Error> {
    sqlx::query_as!(
        NrpsSyncRunRow,
        r#"SELECT id AS "id!", nrps_context_id AS "nrps_context_id!", ran_at AS "ran_at!",
                  status AS "status!", error, warning, added, removed
        FROM (
            SELECT id, nrps_context_id, ran_at, status, error, warning, added, removed,
                   ROW_NUMBER() OVER (
                       PARTITION BY nrps_context_id ORDER BY ran_at DESC
                   ) AS rn
            FROM lti_nrps_sync_runs
            WHERE nrps_context_id = ANY($1)
        ) ranked
        WHERE rn <= $2
        ORDER BY ran_at DESC"#,
        context_ids,
        per_context,
    )
    .fetch_all(db)
    .await
}

#[derive(Debug, Clone)]
pub struct NrpsMembershipRow {
    pub nrps_context_id: Uuid,
    pub user_id: Uuid,
    pub lti_user_id: String,
    pub last_status: String,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

/// Record (or refresh) a provenance row for a member observed in an NRPS
/// fetch. `last_seen_at` is bumped on every observation so a member who
/// vanishes from the roster can be detected by an older timestamp.
pub async fn upsert_membership(
    db: &PgPool,
    nrps_context_id: Uuid,
    user_id: Uuid,
    lti_user_id: &str,
    last_status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO lti_nrps_memberships
            (nrps_context_id, user_id, lti_user_id, last_status, last_seen_at)
        VALUES ($1, $2, $3, $4, NOW())
        ON CONFLICT (nrps_context_id, user_id)
        DO UPDATE SET lti_user_id = EXCLUDED.lti_user_id,
                      last_status = EXCLUDED.last_status,
                      last_seen_at = NOW()"#,
        nrps_context_id,
        user_id,
        lti_user_id,
        last_status,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// All provenance rows for a context; the reconcile loop diffs the current
/// roster against this set to find members to remove.
pub async fn list_memberships(
    db: &PgPool,
    nrps_context_id: Uuid,
) -> Result<Vec<NrpsMembershipRow>, sqlx::Error> {
    sqlx::query_as!(
        NrpsMembershipRow,
        r#"SELECT nrps_context_id, user_id, lti_user_id, last_status, last_seen_at
        FROM lti_nrps_memberships
        WHERE nrps_context_id = $1"#,
        nrps_context_id,
    )
    .fetch_all(db)
    .await
}

pub async fn delete_membership(
    db: &PgPool,
    nrps_context_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM lti_nrps_memberships WHERE nrps_context_id = $1 AND user_id = $2",
        nrps_context_id,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Whether a user has an Active provenance row for the given course via ANY
/// NRPS context OTHER than `exclude_context`. Used so removal from one
/// context doesn't drop a member who is still active in another context
/// bound to the same Minerva course.
pub async fn user_active_in_other_context(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
    exclude_context: Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar!(
        r#"SELECT 1
           FROM lti_nrps_memberships m
           JOIN lti_nrps_contexts c ON c.id = m.nrps_context_id
           WHERE c.course_id = $1
             AND m.user_id = $2
             AND m.nrps_context_id <> $3
             AND m.last_status = 'Active'"#,
        course_id,
        user_id,
        exclude_context,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}
