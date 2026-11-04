//! OAuth authorization server state for the MCP endpoint; see migration
//! `20261104000005_mcp_oauth.sql`. Every token and code is passed in
//! already hashed: plaintext never reaches the database.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ClientRow {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub registration: String,
    pub fetched_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn find_client(db: &PgPool, client_id: &str) -> Result<Option<ClientRow>, sqlx::Error> {
    sqlx::query_as!(
        ClientRow,
        r#"SELECT client_id, client_name, redirect_uris, registration, fetched_at, created_at
           FROM oauth_clients WHERE client_id = $1"#,
        client_id,
    )
    .fetch_optional(db)
    .await
}

/// Insert or refresh a client. A metadata document refetch overwrites the
/// name and redirect URIs with what the document says now.
pub async fn upsert_client(
    db: &PgPool,
    client_id: &str,
    client_name: &str,
    redirect_uris: &[String],
    registration: &str,
) -> Result<ClientRow, sqlx::Error> {
    sqlx::query_as!(
        ClientRow,
        r#"INSERT INTO oauth_clients (client_id, client_name, redirect_uris, registration, fetched_at)
           VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'cimd' THEN NOW() END)
           ON CONFLICT (client_id) DO UPDATE SET
               client_name = EXCLUDED.client_name,
               redirect_uris = EXCLUDED.redirect_uris,
               fetched_at = EXCLUDED.fetched_at
           RETURNING client_id, client_name, redirect_uris, registration, fetched_at, created_at"#,
        client_id,
        client_name,
        redirect_uris,
        registration,
    )
    .fetch_one(db)
    .await
}

pub struct NewPending<'a> {
    pub id: Uuid,
    pub client_id: &'a str,
    pub user_id: Uuid,
    pub redirect_uri: &'a str,
    pub state: Option<&'a str>,
    pub code_challenge: &'a str,
    pub scope: &'a str,
    pub resource: &'a str,
    pub expires_at: DateTime<Utc>,
}

pub async fn insert_pending(db: &PgPool, p: &NewPending<'_>) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO oauth_pending_authorizations
               (id, client_id, user_id, redirect_uri, state, code_challenge, scope, resource, expires_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        p.id,
        p.client_id,
        p.user_id,
        p.redirect_uri,
        p.state,
        p.code_challenge,
        p.scope,
        p.resource,
        p.expires_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct PendingRow {
    pub id: Uuid,
    pub client_id: String,
    pub client_name: String,
    pub user_id: Uuid,
    pub redirect_uri: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub scope: String,
    pub resource: String,
}

/// An unexpired pending authorization of this user.
pub async fn find_pending(
    db: &PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<PendingRow>, sqlx::Error> {
    sqlx::query_as!(
        PendingRow,
        r#"SELECT p.id, p.client_id, c.client_name, p.user_id, p.redirect_uri, p.state,
                  p.code_challenge, p.scope, p.resource
           FROM oauth_pending_authorizations p
           JOIN oauth_clients c ON c.client_id = p.client_id
           WHERE p.id = $1 AND p.user_id = $2 AND p.expires_at > NOW()"#,
        id,
        user_id,
    )
    .fetch_optional(db)
    .await
}

pub async fn delete_pending(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM oauth_pending_authorizations WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(())
}

/// Turn an approved pending authorization into a grant plus a one-time
/// code, atomically consuming the pending row.
pub async fn approve(
    db: &PgPool,
    pending: &PendingRow,
    grant_id: Uuid,
    code_hash: &str,
    code_expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    let removed = sqlx::query!(
        "DELETE FROM oauth_pending_authorizations WHERE id = $1",
        pending.id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if removed == 0 {
        // Approved twice (double click, replayed form): the first won.
        return Err(sqlx::Error::RowNotFound);
    }
    sqlx::query!(
        r#"INSERT INTO oauth_grants (id, client_id, user_id, scope, resource)
           VALUES ($1, $2, $3, $4, $5)"#,
        grant_id,
        pending.client_id,
        pending.user_id,
        pending.scope,
        pending.resource,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"INSERT INTO oauth_authorization_codes
               (code_hash, grant_id, redirect_uri, code_challenge, expires_at)
           VALUES ($1, $2, $3, $4, $5)"#,
        code_hash,
        grant_id,
        pending.redirect_uri,
        pending.code_challenge,
        code_expires_at,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

#[derive(Debug, Clone)]
pub struct CodeRow {
    pub grant_id: Uuid,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub resource: String,
    pub scope: String,
    pub expired: bool,
    pub consumed: bool,
    pub grant_revoked: bool,
}

/// Look up a code and mark it consumed in the same statement, returning
/// its state from before. A code seen as already consumed is a replay.
pub async fn consume_code(db: &PgPool, code_hash: &str) -> Result<Option<CodeRow>, sqlx::Error> {
    sqlx::query_as!(
        CodeRow,
        r#"WITH prev AS (
               SELECT code_hash, consumed_at FROM oauth_authorization_codes WHERE code_hash = $1
           ),
           upd AS (
               UPDATE oauth_authorization_codes SET consumed_at = COALESCE(consumed_at, NOW())
               WHERE code_hash = $1
               RETURNING grant_id, redirect_uri, code_challenge, expires_at
           )
           SELECT upd.grant_id, g.client_id, upd.redirect_uri, upd.code_challenge,
                  g.resource, g.scope,
                  (upd.expires_at <= NOW()) AS "expired!",
                  ((SELECT consumed_at FROM prev) IS NOT NULL) AS "consumed!",
                  (g.revoked_at IS NOT NULL) AS "grant_revoked!"
           FROM upd JOIN oauth_grants g ON g.id = upd.grant_id"#,
        code_hash,
    )
    .fetch_optional(db)
    .await
}

/// Store a freshly issued access/refresh pair for a grant.
pub async fn insert_token_pair(
    db: &PgPool,
    grant_id: Uuid,
    access_hash: &str,
    access_expires_at: DateTime<Utc>,
    refresh_hash: &str,
    refresh_expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO oauth_tokens (token_hash, grant_id, kind, expires_at)
           VALUES ($1, $3, 'access', $2), ($4, $3, 'refresh', $5)"#,
        access_hash,
        access_expires_at,
        grant_id,
        refresh_hash,
        refresh_expires_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct RefreshRow {
    pub grant_id: Uuid,
    pub client_id: String,
    pub resource: String,
    pub scope: String,
    pub expired: bool,
    pub rotated: bool,
    pub grant_revoked: bool,
}

/// Mark a refresh token rotated and return its state from before. A token
/// that was already rotated is a replay: the caller revokes the grant.
pub async fn rotate_refresh(
    db: &PgPool,
    refresh_hash: &str,
) -> Result<Option<RefreshRow>, sqlx::Error> {
    sqlx::query_as!(
        RefreshRow,
        r#"WITH prev AS (
               SELECT rotated_at FROM oauth_tokens WHERE token_hash = $1 AND kind = 'refresh'
           ),
           upd AS (
               UPDATE oauth_tokens SET rotated_at = COALESCE(rotated_at, NOW())
               WHERE token_hash = $1 AND kind = 'refresh'
               RETURNING grant_id, expires_at
           )
           SELECT upd.grant_id, g.client_id, g.resource, g.scope,
                  (upd.expires_at <= NOW()) AS "expired!",
                  ((SELECT rotated_at FROM prev) IS NOT NULL) AS "rotated!",
                  (g.revoked_at IS NOT NULL) AS "grant_revoked!"
           FROM upd JOIN oauth_grants g ON g.id = upd.grant_id"#,
        refresh_hash,
    )
    .fetch_optional(db)
    .await
}

/// Revoke a grant: every token under it stops working at once.
pub async fn revoke_grant(db: &PgPool, grant_id: Uuid) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query!(
        "UPDATE oauth_grants SET revoked_at = COALESCE(revoked_at, NOW()) WHERE id = $1",
        grant_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM oauth_tokens WHERE grant_id = $1", grant_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// The grant a token (access or refresh) belongs to, if the token exists.
pub async fn grant_of_token(db: &PgPool, token_hash: &str) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT grant_id FROM oauth_tokens WHERE token_hash = $1",
        token_hash
    )
    .fetch_optional(db)
    .await
}

#[derive(Debug, Clone)]
pub struct AccessRow {
    pub grant_id: Uuid,
    pub user_id: Uuid,
    pub resource: String,
    pub scope: String,
}

/// Resolve a live access token: unexpired, of an unrevoked grant. Touches
/// the grant's `last_used_at` for the connected-apps list.
pub async fn resolve_access(
    db: &PgPool,
    access_hash: &str,
) -> Result<Option<AccessRow>, sqlx::Error> {
    sqlx::query_as!(
        AccessRow,
        r#"UPDATE oauth_grants g SET last_used_at = NOW()
           FROM oauth_tokens t
           WHERE t.token_hash = $1 AND t.kind = 'access' AND t.expires_at > NOW()
             AND g.id = t.grant_id AND g.revoked_at IS NULL
           RETURNING g.id AS grant_id, g.user_id, g.resource, g.scope"#,
        access_hash,
    )
    .fetch_optional(db)
    .await
}

#[derive(Debug, Clone)]
pub struct ConnectedApp {
    pub grant_id: Uuid,
    pub client_id: String,
    pub client_name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// A user's live grants, newest first.
pub async fn list_connected(db: &PgPool, user_id: Uuid) -> Result<Vec<ConnectedApp>, sqlx::Error> {
    sqlx::query_as!(
        ConnectedApp,
        r#"SELECT g.id AS grant_id, g.client_id, c.client_name, g.created_at, g.last_used_at
           FROM oauth_grants g JOIN oauth_clients c ON c.client_id = g.client_id
           WHERE g.user_id = $1 AND g.revoked_at IS NULL
           ORDER BY g.created_at DESC"#,
        user_id,
    )
    .fetch_all(db)
    .await
}

/// Revoke one of the user's own grants. False when it is not theirs.
pub async fn revoke_own_grant(
    db: &PgPool,
    grant_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let owned = sqlx::query_scalar!(
        r#"SELECT 1 AS "one!" FROM oauth_grants WHERE id = $1 AND user_id = $2"#,
        grant_id,
        user_id,
    )
    .fetch_optional(db)
    .await?
    .is_some();
    if owned {
        revoke_grant(db, grant_id).await?;
    }
    Ok(owned)
}

/// Drop expired pending authorizations, codes and tokens.
pub async fn delete_expired(db: &PgPool) -> Result<u64, sqlx::Error> {
    let mut n = 0;
    n += sqlx::query!("DELETE FROM oauth_pending_authorizations WHERE expires_at < NOW()")
        .execute(db)
        .await?
        .rows_affected();
    n += sqlx::query!(
        "DELETE FROM oauth_authorization_codes WHERE expires_at < NOW() - INTERVAL '1 day'"
    )
    .execute(db)
    .await?
    .rows_affected();
    n += sqlx::query!("DELETE FROM oauth_tokens WHERE expires_at < NOW() - INTERVAL '1 day'")
        .execute(db)
        .await?
        .rows_affected();
    Ok(n)
}
