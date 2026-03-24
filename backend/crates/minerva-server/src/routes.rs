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
        .route_layer(middleware::from_fn_with_state(state, auth_middleware));

    Router::new()
        .route("/health", get(health))
        .merge(authed)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "minerva" }))
}

async fn me(Extension(user): Extension<User>) -> Json<Value> {
    Json(json!({
        "id": user.id,
        "eppn": user.eppn,
        "display_name": user.display_name,
        "role": user.role,
    }))
}
