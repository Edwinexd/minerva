use axum::extract::{Extension, Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{id}/role", put(update_user_role))
}

fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize)]
struct UserResponse {
    id: Uuid,
    eppn: String,
    display_name: Option<String>,
    role: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn list_users(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<UserResponse>>, AppError> {
    require_admin(&user)?;

    let rows = minerva_db::queries::users::list_all(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| UserResponse {
                id: r.id,
                eppn: r.eppn,
                display_name: r.display_name,
                role: r.role,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct UpdateRoleRequest {
    role: String,
}

async fn update_user_role(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRoleRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    // Only allow setting to teacher or student (not admin)
    if body.role != "teacher" && body.role != "student" {
        return Err(AppError::BadRequest(
            "role must be 'teacher' or 'student'".to_string(),
        ));
    }

    let updated = minerva_db::queries::users::update_role(&state.db, id, &body.role).await?;
    if !updated {
        return Err(AppError::NotFound);
    }

    Ok(Json(serde_json::json!({ "updated": true })))
}
