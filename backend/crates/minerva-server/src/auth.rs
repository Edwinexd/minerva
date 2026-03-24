use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use minerva_core::models::{User, UserRole, ADMIN_USERNAME};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Extracts user from Shibboleth headers set by Apache mod_shib.
/// REMOTE_USER contains the eppn (e.g. edsu8469@SU.SE).
pub async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let eppn = headers
        .get("REMOTE_USER")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let display_name = headers
        .get("displayName")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let user = upsert_user(&state.db, eppn, display_name.as_deref()).await?;
    request.extensions_mut().insert(user);

    Ok(next.run(request).await)
}

async fn upsert_user(db: &PgPool, eppn: &str, display_name: Option<&str>) -> Result<User, AppError> {
    let username = eppn.split('@').next().unwrap_or(eppn);
    let is_admin = username == ADMIN_USERNAME;

    // Check existing user first
    let existing = sqlx::query_as::<_, (Uuid, String, Option<String>, String, chrono::NaiveDateTime, chrono::NaiveDateTime)>(
        "SELECT id, eppn, display_name, role, created_at, updated_at FROM users WHERE eppn = $1"
    )
    .bind(eppn)
    .fetch_optional(db)
    .await?;

    if let Some(row) = existing {
        let role = if is_admin { UserRole::Admin } else { UserRole::parse(&row.3) };

        // Update display name and timestamp
        sqlx::query("UPDATE users SET display_name = COALESCE($1, display_name), role = $2, updated_at = NOW() WHERE id = $3")
            .bind(display_name)
            .bind(role.as_str())
            .bind(row.0)
            .execute(db)
            .await?;

        return Ok(User {
            id: row.0,
            eppn: row.1,
            display_name: display_name.map(|s| s.to_string()).or(row.2),
            role,
            created_at: row.4,
            updated_at: row.5,
        });
    }

    // New user
    let id = Uuid::new_v4();
    let role = if is_admin { UserRole::Admin } else { UserRole::Student };

    sqlx::query("INSERT INTO users (id, eppn, display_name, role) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(eppn)
        .bind(display_name)
        .bind(role.as_str())
        .execute(db)
        .await?;

    let now = chrono::Utc::now().naive_utc();
    Ok(User {
        id,
        eppn: eppn.to_string(),
        display_name: display_name.map(|s| s.to_string()),
        role,
        created_at: now,
        updated_at: now,
    })
}
