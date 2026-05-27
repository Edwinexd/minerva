//! Dev-mode-only admin routes. Every handler in this module returns
//! 404 unless `MINERVA_DEV_MODE = true` so the same compiled binary
//! can ship to prod without the surface area being reachable.
//!
//! The 404 (rather than 403) is intentional: it makes the route look
//! like it isn't registered at all, so a curious prober in prod can't
//! tell from the response whether the endpoint exists, requires
//! different auth, or simply isn't a thing. Combined with the
//! `require_admin` gate (also enforced here in case the surrounding
//! router ever changes), it's two layers of "this is dev only".

use axum::extract::{Extension, State};
use axum::routing::post;
use axum::{Json, Router};
use minerva_core::models::User;

use crate::dev_seed::{run_seed, SeedReport};
use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/dev/seed", post(seed_handler))
}

/// POST /admin/dev/seed. Triggers a destructive reseed against the
/// live DB. Costs ~1-2 seconds of wall-clock for the SQL work, plus
/// however long the background worker takes to embed the seeded
/// documents (varies with the local fastembed model's warmup; first
/// call cold-loads the ONNX weights and can take 10-30s before the
/// worker reports the docs as `ready`).
async fn seed_handler(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<SeedReport>, AppError> {
    if !state.config.dev_mode {
        // Match the "route doesn't exist" smell: same NotFound the
        // axum router would emit for an unregistered path.
        return Err(AppError::NotFound);
    }
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let report = run_seed(&state, &user.eppn).await?;
    tracing::info!(
        admin = %user.eppn,
        users = report.users,
        courses = report.courses,
        docs = report.documents,
        "dev_seed: ran via admin endpoint"
    );
    Ok(Json(report))
}
