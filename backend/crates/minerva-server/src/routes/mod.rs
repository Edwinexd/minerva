mod admin;
mod api_keys;
pub(crate) mod canvas;
mod chat;
mod courses;
pub(crate) mod documents;
pub mod embed;
mod external_auth;
mod health;
pub mod integration;
pub mod lti;
mod play_designations;
pub mod service;
mod signed_urls;
mod system;
mod usage;

use axum::extract::{Extension, State};
use axum::middleware;
use axum::routing::get;
use axum::{Json, Router};
use minerva_core::models::User;
use serde_json::{json, Value};

use crate::auth::auth_middleware;
use crate::error::AppError;
use crate::state::AppState;
use uuid::Uuid;

/// Reject the request if the course owner has hit their aggregate daily
/// token cap (summed across every course they own). 0 = unlimited.
/// Called from chat + embed routes before invoking the LLM. A missing
/// owner row is treated as Internal: courses.owner_id has a FK to users,
/// so this only happens if data has been deleted out from under us, in
/// which case continuing would silently disable the cap for that course.
pub(crate) async fn enforce_owner_cap(state: &AppState, owner_id: Uuid) -> Result<(), AppError> {
    let Some(owner) = minerva_db::queries::users::find_by_id(&state.db, owner_id).await? else {
        return Err(AppError::Internal(format!(
            "course owner {owner_id} not found in users table"
        )));
    };
    if owner.owner_daily_token_limit <= 0 {
        return Ok(());
    }
    let used = minerva_db::queries::usage::get_owner_daily_tokens(&state.db, owner_id).await?;
    if used >= owner.owner_daily_token_limit {
        return Err(AppError::OwnerQuotaExceeded);
    }
    Ok(())
}

pub fn api_router(state: AppState) -> Router<AppState> {
    let authed = Router::new()
        .route("/auth/me", get(me))
        .nest("/courses", courses::router())
        .nest("/courses/{course_id}/documents", documents::router())
        .nest("/courses/{course_id}", chat::router())
        .nest("/courses/{course_id}", signed_urls::course_router())
        .nest("/courses/{course_id}", api_keys::router())
        .nest("/courses/{course_id}", play_designations::router())
        .merge(play_designations::catalog_router())
        .nest("/courses/{course_id}", lti::course_router())
        .nest("/courses/{course_id}", canvas::course_router())
        .nest("/courses/{course_id}", usage::course_router())
        .nest("/admin", admin::router())
        .nest("/admin", external_auth::admin_router())
        .nest("/admin", system::router())
        .merge(usage::admin_router())
        .merge(signed_urls::join_router())
        .route_layer(middleware::from_fn_with_state(state, auth_middleware));

    Router::new()
        .route("/health", get(health::health))
        .route("/models", get(health::models))
        .route("/embedding-benchmarks", get(health::embedding_benchmarks))
        .route("/dev/config", get(dev_config))
        .nest("/integration", integration::router())
        .nest("/service", service::router())
        .nest("/embed", embed::router())
        .merge(external_auth::public_router())
        .merge(authed)
}

async fn me(Extension(user): Extension<User>) -> Json<Value> {
    Json(json!({
        "id": user.id,
        "eppn": user.eppn,
        "display_name": user.display_name,
        "role": user.role,
        "suspended": user.suspended,
    }))
}

/// Returns dev mode config (available dev users). Only responds in dev mode.
async fn dev_config(State(state): State<AppState>) -> Json<Value> {
    if !state.config.dev_mode {
        return Json(json!({ "dev_mode": false }));
    }

    let mut dev_users = vec![
        json!({ "eppn": "student@su.se", "label": "Student" }),
        json!({ "eppn": "teacher@su.se", "label": "Teacher" }),
    ];

    for admin in &state.config.admin_usernames {
        dev_users.push(json!({
            "eppn": format!("{}@su.se", admin),
            "label": format!("Admin ({})", admin),
        }));
    }

    Json(json!({
        "dev_mode": true,
        "users": dev_users,
    }))
}
