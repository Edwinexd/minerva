use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

pub fn course_router() -> Router<AppState> {
    Router::new()
        .route(
            "/signed-urls",
            get(list_signed_urls).post(create_signed_url),
        )
        .route("/signed-urls/{sid}", delete(delete_signed_url))
}

pub fn join_router() -> Router<AppState> {
    Router::new().route("/join/{token}", get(join_via_token))
}

#[derive(Serialize)]
struct SignedUrlResponse {
    id: Uuid,
    course_id: Uuid,
    token: String,
    url: String,
    expires_at: chrono::DateTime<chrono::Utc>,
    max_uses: Option<i32>,
    use_count: i32,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct CreateSignedUrlRequest {
    expires_in_hours: Option<i64>,
    max_uses: Option<i32>,
}

async fn create_signed_url(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<CreateSignedUrlRequest>,
) -> Result<Json<SignedUrlResponse>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let id = Uuid::new_v4();
    let hours = body.expires_in_hours.unwrap_or(168); // Default 1 week
    let expires_at = chrono::Utc::now() + chrono::Duration::hours(hours);

    // Generate HMAC token
    let mut mac = HmacSha256::new_from_slice(state.config.hmac_secret.as_bytes())
        .map_err(|_| AppError::Internal("hmac key error".to_string()))?;
    mac.update(format!("{}:{}:{}", course_id, id, expires_at.timestamp()).as_bytes());
    let token = hex::encode(mac.finalize().into_bytes());

    let row = minerva_db::queries::signed_urls::create(
        &state.db,
        id,
        course_id,
        user.id,
        &token,
        expires_at,
        body.max_uses,
    )
    .await?;

    Ok(Json(SignedUrlResponse {
        id: row.id,
        course_id: row.course_id,
        token: row.token.clone(),
        url: format!("/join/{}", row.token),
        expires_at: row.expires_at,
        max_uses: row.max_uses,
        use_count: row.use_count,
        created_at: row.created_at,
    }))
}

async fn list_signed_urls(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<SignedUrlResponse>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let rows = minerva_db::queries::signed_urls::list_by_course(&state.db, course_id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| SignedUrlResponse {
                id: r.id,
                course_id: r.course_id,
                token: r.token.clone(),
                url: format!("/join/{}", r.token),
                expires_at: r.expires_at,
                max_uses: r.max_uses,
                use_count: r.use_count,
                created_at: r.created_at,
            })
            .collect(),
    ))
}

async fn delete_signed_url(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, sid)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    minerva_db::queries::signed_urls::delete(&state.db, sid).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn join_via_token(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let signed_url = minerva_db::queries::signed_urls::find_by_token(&state.db, &token)
        .await?
        .ok_or(AppError::NotFound)?;

    // Check expiry
    let now = chrono::Utc::now();
    if now > signed_url.expires_at {
        return Err(AppError::BadRequest("signed URL has expired".to_string()));
    }

    // Check max uses
    if let Some(max) = signed_url.max_uses {
        if signed_url.use_count >= max {
            return Err(AppError::BadRequest(
                "signed URL has reached max uses".to_string(),
            ));
        }
    }

    // Add user to course
    minerva_db::queries::courses::add_member(&state.db, signed_url.course_id, user.id, "student")
        .await?;

    // Increment use count
    minerva_db::queries::signed_urls::increment_use(&state.db, signed_url.id).await?;

    Ok(Json(serde_json::json!({
        "joined": true,
        "course_id": signed_url.course_id,
    })))
}
