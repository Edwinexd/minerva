use axum::extract::{Extension, Path, State};
use axum::routing::get;
use axum::{Json, Router};
use minerva_core::models::User;
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn course_router() -> Router<AppState> {
    Router::new().route("/usage", get(get_course_usage))
}

pub fn admin_router() -> Router<AppState> {
    Router::new().route("/usage", get(get_all_usage))
}

#[derive(Serialize)]
struct UsageResponse {
    user_id: Uuid,
    course_id: Uuid,
    date: chrono::NaiveDate,
    prompt_tokens: i64,
    completion_tokens: i64,
    embedding_tokens: i64,
    request_count: i32,
}

async fn get_course_usage(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<UsageResponse>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let rows = minerva_db::queries::usage::get_course_usage(&state.db, course_id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| UsageResponse {
                user_id: r.user_id,
                course_id: r.course_id,
                date: r.date,
                prompt_tokens: r.prompt_tokens,
                completion_tokens: r.completion_tokens,
                embedding_tokens: r.embedding_tokens,
                request_count: r.request_count,
            })
            .collect(),
    ))
}

async fn get_all_usage(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<UsageResponse>>, AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let rows = minerva_db::queries::usage::get_all_usage(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| UsageResponse {
                user_id: r.user_id,
                course_id: r.course_id,
                date: r.date,
                prompt_tokens: r.prompt_tokens,
                completion_tokens: r.completion_tokens,
                embedding_tokens: r.embedding_tokens,
                request_count: r.request_count,
            })
            .collect(),
    ))
}
