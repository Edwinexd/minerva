use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use minerva_core::models::{User, UserRole};
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

    let user = upsert_user(&state, eppn, display_name.as_deref()).await?;
    request.extensions_mut().insert(user);

    Ok(next.run(request).await)
}

async fn upsert_user(state: &AppState, eppn: &str, display_name: Option<&str>) -> Result<User, AppError> {
    let is_admin = state.config.is_admin(eppn);

    let existing = minerva_db::queries::users::find_by_eppn(&state.db, eppn).await?;

    if let Some(row) = existing {
        let role = if is_admin { UserRole::Admin } else { UserRole::parse(&row.role) };

        minerva_db::queries::users::update_login(&state.db, row.id, display_name, role.as_str()).await?;

        return Ok(User {
            id: row.id,
            eppn: row.eppn,
            display_name: display_name.map(|s| s.to_string()).or(row.display_name),
            role,
            created_at: row.created_at,
            updated_at: row.updated_at,
        });
    }

    let id = Uuid::new_v4();
    let role = if is_admin { UserRole::Admin } else { UserRole::Student };

    minerva_db::queries::users::insert(&state.db, id, eppn, display_name, role.as_str()).await?;

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
