//! Course-only extensions of the workspace API, mounted under
//! `/api/courses/{course_id}`. Generic workspace endpoints (config,
//! members, documents, chat) live in `routes::workspaces`.

use axum::extract::{Extension, Path, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use minerva_core::models::User;
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::guards::{require_workspace_owner, require_workspace_teacher, TeacherScope};
use crate::state::AppState;

/// Route layer for everything under `/courses/{course_id}`: those
/// endpoints manage course-only rows (LTI, Canvas, join links, Moodle
/// keys, Play), whose foreign keys only accept a course. Any other
/// workspace kind is reported as not found rather than surfacing a
/// constraint error.
pub(crate) async fn require_course(
    State(state): State<AppState>,
    Path(params): Path<std::collections::HashMap<String, String>>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let id = params
        .get("course_id")
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or(AppError::NotFound)?;
    if minerva_db::queries::courses::find(&state.db, id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }
    Ok(next.run(req).await)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/role-suggestions", get(list_role_suggestions))
        .route(
            "/role-suggestions/{suggestion_id}/approve",
            post(approve_role_suggestion),
        )
        .route(
            "/role-suggestions/{suggestion_id}/decline",
            post(decline_role_suggestion),
        )
}

#[derive(Serialize)]
struct RoleSuggestionResponse {
    id: Uuid,
    user_id: Uuid,
    eppn: Option<String>,
    display_name: Option<String>,
    current_role: Option<String>,
    suggested_role: String,
    source: String,
    source_detail: Option<serde_json::Value>,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Anyone who can see the member list (owner, admin, course teacher) can
/// see pending suggestions, so the UI can show a badge. Approve/decline is
/// stricter; only owner or admin.
async fn list_role_suggestions(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<RoleSuggestionResponse>>, AppError> {
    require_workspace_teacher(&state, id, &user, TeacherScope::WithAssistants).await?;

    let rows =
        minerva_db::queries::role_suggestions::list_pending_for_course(&state.db, id).await?;
    let ps = crate::ext_obfuscate::Pseudonymizer::for_viewer(
        &state.db,
        &user,
        &state.config.hmac_secret,
    )
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let (eppn, display_name) =
                    crate::ext_obfuscate::apply(ps.as_ref(), r.user_id, r.eppn, r.display_name);
                RoleSuggestionResponse {
                    id: r.id,
                    user_id: r.user_id,
                    eppn,
                    display_name,
                    current_role: r.current_role,
                    suggested_role: r.suggested_role,
                    source: r.source,
                    source_detail: r.source_detail,
                    created_at: r.created_at,
                }
            })
            .collect(),
    ))
}

async fn approve_role_suggestion(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((id, suggestion_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_workspace_owner(&state, id, &user).await?;

    let suggestion =
        minerva_db::queries::role_suggestions::find_pending_by_id(&state.db, suggestion_id)
            .await?
            .ok_or(AppError::NotFound)?;
    // Defend against a suggestion id from a different course being POSTed
    // through this course's URL.
    if suggestion.course_id != id {
        return Err(AppError::NotFound);
    }

    minerva_db::queries::workspaces::add_member(
        &state.db,
        suggestion.course_id,
        suggestion.user_id,
        &suggestion.suggested_role,
    )
    .await?;
    minerva_db::queries::role_suggestions::mark_approved(&state.db, suggestion.id, user.id).await?;

    Ok(Json(serde_json::json!({ "approved": true })))
}

async fn decline_role_suggestion(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((id, suggestion_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_workspace_owner(&state, id, &user).await?;

    let suggestion =
        minerva_db::queries::role_suggestions::find_pending_by_id(&state.db, suggestion_id)
            .await?
            .ok_or(AppError::NotFound)?;
    if suggestion.course_id != id {
        return Err(AppError::NotFound);
    }

    minerva_db::queries::role_suggestions::mark_declined(&state.db, suggestion.id, user.id).await?;

    Ok(Json(serde_json::json!({ "declined": true })))
}
