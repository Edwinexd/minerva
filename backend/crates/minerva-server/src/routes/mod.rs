mod admin;
mod courses;
mod documents;
mod health;

use axum::extract::Extension;
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
        .nest("/admin", admin::router())
        .route_layer(middleware::from_fn_with_state(state, auth_middleware));

    Router::new()
        .route("/health", get(health::health))
        .merge(authed)
}

async fn me(Extension(user): Extension<User>) -> Json<Value> {
    Json(json!({
        "id": user.id,
        "eppn": user.eppn,
        "display_name": user.display_name,
        "role": user.role,
    }))
}
