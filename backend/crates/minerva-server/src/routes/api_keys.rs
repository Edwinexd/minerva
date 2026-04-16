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
        .route("/api-keys", get(list_api_keys).post(create_api_key))
        .route("/api-keys/{key_id}", delete(delete_api_key))
}

#[derive(Serialize)]
struct ApiKeyResponse {
    id: Uuid,
    name: String,
    key_prefix: String,
    created_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
struct ApiKeyCreatedResponse {
    id: Uuid,
    name: String,
    key: String,
    key_prefix: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct CreateApiKeyRequest {
    name: String,
}

async fn list_api_keys(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<ApiKeyResponse>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let rows = minerva_db::queries::api_keys::list_by_course(&state.db, course_id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| ApiKeyResponse {
                id: r.id,
                name: r.name,
                key_prefix: r.key_prefix,
                created_at: r.created_at,
                last_used_at: r.last_used_at,
            })
            .collect(),
    ))
}

async fn create_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<ApiKeyCreatedResponse>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let name = body.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(AppError::bad_request("api_keys.name_invalid_length"));
    }

    let id = Uuid::new_v4();

    // Generate a random API key: mnrv_{32 hex chars}
    let random_bytes: [u8; 16] = rand::random();
    let raw_key = format!("mnrv_{}", hex::encode(random_bytes));
    let key_prefix = format!("mnrv_{}...", &hex::encode(random_bytes)[..8]);

    // Store SHA-256 hash of the key
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    let row = minerva_db::queries::api_keys::insert(
        &state.db,
        id,
        course_id,
        user.id,
        &name,
        &key_hash,
        &key_prefix,
    )
    .await?;

    // Return the full key only once, at creation time
    Ok(Json(ApiKeyCreatedResponse {
        id: row.id,
        name: row.name,
        key: raw_key,
        key_prefix: row.key_prefix,
        created_at: row.created_at,
    }))
}

async fn delete_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, key_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let deleted = minerva_db::queries::api_keys::delete(&state.db, key_id, course_id).await?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}
