mod admin;
mod api_keys;
mod chat;
mod courses;
pub(crate) mod documents;
pub mod embed;
mod health;
pub mod integration;
pub mod lti;
mod signed_urls;
mod usage;

use axum::extract::{Extension, State};
use axum::middleware;
use axum::routing::get;
use axum::{Json, Router};
use minerva_core::models::User;
use serde_json::{json, Value};

use crate::auth::auth_middleware;
use crate::state::AppState;

pub fn api_router(state: AppState) -> Router<AppState> {
    let authed = Router::new()
        .route("/auth/me", get(me))
        .nest("/courses", courses::router())
        .nest("/courses/{course_id}/documents", documents::router())
        .nest("/courses/{course_id}", chat::router())
        .nest("/courses/{course_id}", signed_urls::course_router())
        .nest("/courses/{course_id}", api_keys::router())
        .nest("/courses/{course_id}", lti::course_router())
        .nest("/courses/{course_id}", usage::course_router())
        .nest("/admin", admin::router())
        .merge(usage::admin_router())
        .merge(signed_urls::join_router())
        .route_layer(middleware::from_fn_with_state(state, auth_middleware));

    Router::new()
        .route("/health", get(health::health))
        .route("/models", get(health::models))
        .route("/dev/config", get(dev_config))
        .nest("/integration", integration::router())
        .nest("/embed", embed::router())
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
        json!({ "eppn": "student@SU.SE", "label": "Student" }),
        json!({ "eppn": "teacher@SU.SE", "label": "Teacher" }),
    ];

    for admin in &state.config.admin_usernames {
        dev_users.push(json!({
            "eppn": format!("{}@SU.SE", admin),
            "label": format!("Admin ({})", admin),
        }));
    }

    Json(json!({
        "dev_mode": true,
        "users": dev_users,
    }))
}
