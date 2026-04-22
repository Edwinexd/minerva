//! Admin routes for managing site-level integration keys used by the Moodle /
//! Canvas plugin. These site keys let the plugin mint regular per-course
//! api_keys on behalf of a Moodle teacher (identified by eppn) without the
//! teacher needing to visit Minerva first.
//!
//! The keys themselves are never usable for course data access -- they only
//! authorize the two /api/integration/site/* endpoints. That makes them low
//! enough risk to keep in a single row, high enough value to keep behind an
//! admin-only CRUD surface here.

use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/integration-keys",
            get(list_site_integration_keys).post(create_site_integration_key),
        )
        .route(
            "/integration-keys/{id}",
            delete(delete_site_integration_key),
        )
}

fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize)]
struct SiteKeyResponse {
    id: Uuid,
    name: String,
    key_prefix: String,
    created_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
struct SiteKeyCreatedResponse {
    id: Uuid,
    name: String,
    /// Full raw key, returned once. Cannot be re-fetched later.
    key: String,
    key_prefix: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct CreateSiteKeyRequest {
    name: String,
}

async fn list_site_integration_keys(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<SiteKeyResponse>>, AppError> {
    require_admin(&user)?;
    let rows = minerva_db::queries::site_integration_keys::list_all(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| SiteKeyResponse {
                id: r.id,
                name: r.name,
                key_prefix: r.key_prefix,
                created_at: r.created_at,
                last_used_at: r.last_used_at,
            })
            .collect(),
    ))
}

async fn create_site_integration_key(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreateSiteKeyRequest>,
) -> Result<Json<SiteKeyCreatedResponse>, AppError> {
    require_admin(&user)?;

    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(AppError::bad_request("api_keys.name_invalid_length"));
    }

    // Same prefix as course-scoped keys (`mnrv_`) -- no point inventing a
    // separate scheme. The lookup path is different so there's no ambiguity.
    let id = Uuid::new_v4();
    let random_bytes: [u8; 16] = rand::random();
    let raw_key = format!("mnrv_{}", hex::encode(random_bytes));
    let key_prefix = format!("mnrv_{}...", &hex::encode(random_bytes)[..8]);

    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    let row = minerva_db::queries::site_integration_keys::insert(
        &state.db,
        id,
        name,
        &key_hash,
        &key_prefix,
        user.id,
    )
    .await?;

    Ok(Json(SiteKeyCreatedResponse {
        id: row.id,
        name: row.name,
        key: raw_key,
        key_prefix: row.key_prefix,
        created_at: row.created_at,
    }))
}

async fn delete_site_integration_key(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let deleted = minerva_db::queries::site_integration_keys::delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}
