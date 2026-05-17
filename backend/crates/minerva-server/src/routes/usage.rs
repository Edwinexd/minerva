use axum::extract::{Extension, Path, State};
use axum::routing::get;
use axum::{Json, Router};
use minerva_core::models::User;
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn course_router() -> Router<AppState> {
    Router::new()
        .route("/usage", get(get_course_usage))
        .route("/kg-token-usage", get(get_course_kg_token_usage))
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
    /// Subtotal of `prompt + completion` consumed by the research /
    /// agentic phase across this row's chat calls. Lets the
    /// teacher/admin usage views break the daily total into
    /// research vs writeup. Zero on days where no `tool_use_enabled`
    /// chat traffic happened.
    research_tokens: i64,
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

    if course.owner_id != user.id
        && !user.role.is_admin()
        && !minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await?
    {
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
                research_tokens: r.research_tokens,
                request_count: r.request_count,
            })
            .collect(),
    ))
}

/// Per-category token-usage breakdown for the KG / extraction-guard
/// pipeline. Window defaults to the last 30 days; the dashboard
/// shows totals + per-(category, model) split. Distinct from
/// `/usage` (which tracks per-student chat tokens); KG operations
/// burn tokens course-wide for things the teacher / system did, not
/// the students.
#[derive(Serialize)]
struct KgTokenUsageRow {
    category: String,
    model: String,
    call_count: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
}

#[derive(Serialize)]
struct KgTokenUsageResponse {
    /// ISO-8601 timestamp of the window start. The aggregate is
    /// "everything since this point". Frontend renders it as a
    /// "since YYYY-MM-DD" subtitle.
    since: chrono::DateTime<chrono::Utc>,
    rows: Vec<KgTokenUsageRow>,
}

async fn get_course_kg_token_usage(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<KgTokenUsageResponse>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Same access rule as the chat-usage endpoint above: course
    // owner, course teacher, or admin.
    if course.owner_id != user.id
        && !user.role.is_admin()
        && !minerva_db::queries::courses::is_course_teacher(&state.db, course_id, user.id).await?
    {
        return Err(AppError::Forbidden);
    }

    // 30-day rolling window. Long enough to cover a course's
    // ingest-burst week and the steady chat-time guard cost,
    // short enough that the dashboard stays focused on "current
    // term" usage rather than lifetime totals.
    let since = chrono::Utc::now() - chrono::Duration::days(30);
    let rows = minerva_db::queries::course_token_usage::aggregate_by_category_for_course(
        &state.db, course_id, since,
    )
    .await?;

    Ok(Json(KgTokenUsageResponse {
        since,
        rows: rows
            .into_iter()
            .map(|r| KgTokenUsageRow {
                category: r.category,
                model: r.model,
                call_count: r.call_count,
                prompt_tokens: r.prompt_tokens,
                completion_tokens: r.completion_tokens,
            })
            .collect(),
    }))
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
                research_tokens: r.research_tokens,
                request_count: r.request_count,
            })
            .collect(),
    ))
}
