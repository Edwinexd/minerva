mod admin;
mod api_keys;
pub(crate) mod canvas;
mod chat;
#[cfg(feature = "eureka")]
mod concept_graph;
mod courses;
pub(crate) mod documents;
pub mod embed;
mod external_auth;
mod health;
pub mod integration;
mod integration_admin;
pub mod lti;
mod play_designations;
pub mod service;
mod signed_urls;
pub(crate) mod study;
mod system;
mod usage;

use axum::extract::{Extension, State};
use axum::middleware;
use axum::routing::{get, post};
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
        .route("/auth/acknowledge-privacy", post(acknowledge_privacy))
        // Auth-gated catalog feed for the per-course teacher dropdown.
        // Path is `/embedding-catalog` rather than `/embedding-models`
        // because Apache's no-Shib carve-out regex
        // `^/api/(integration|service|embed|external-auth)` is
        // unanchored and matches anything starting with `/api/embed`,
        // including `/api/embedding-models`. That sent the request
        // through Apache without a Shib challenge, so no `eppn` header
        // arrived at the backend and auth_middleware 401'd. The apache
        // regex is being tightened in `apache/minerva-app.conf`, but
        // renaming the route is the immediate fix that doesn't depend
        // on the manual prod-server apache update.
        .route("/embedding-catalog", get(health::embedding_models))
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
        .nest("/courses/{course_id}/study", study::router())
        .nest("/admin", admin::router())
        .nest("/admin", external_auth::admin_router())
        .nest("/admin", lti::admin_router())
        .nest("/admin", integration_admin::router())
        .nest("/admin", study::admin_router())
        .nest("/admin", system::router());

    #[cfg(feature = "eureka")]
    let authed = authed.nest("/admin", concept_graph::admin_router());

    let authed = authed
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
        .nest("/lti", lti::public_api_router())
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
        "privacy_acknowledged_at": user.privacy_acknowledged_at,
    }))
}

async fn acknowledge_privacy(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Value>, AppError> {
    minerva_db::queries::users::acknowledge_privacy(&state.db, user.id).await?;
    Ok(Json(json!({ "ok": true })))
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
