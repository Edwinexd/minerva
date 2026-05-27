//! Admin-facing routes for the Daisy auto-import staging surface.
//!
//! These sit under `/api/admin/...` and require admin role.
//! Workflow:
//!   1. `GET /admin/daisy-pending` lists every row in
//!      `daisy_pending_imports` plus the current `auto_apply` flag.
//!   2. `POST /admin/daisy-pending/apply` takes `{ids: [...]}`,
//!      runs each through `service::apply_one`, and deletes the
//!      staging row on success.
//!   3. `DELETE /admin/daisy-pending/{id}` dismisses a row without
//!      applying it (the next daily sync will re-stage if Daisy
//!      still lists the offering).
//!   4. `PUT /admin/daisy-settings/auto-apply` flips the toggle so
//!      future syncs bypass staging entirely.

use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::service::{
    apply_one, DaisyCourseInputPayload, DaisyImportSummary, DaisyParticipantInput,
};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/daisy-pending", get(list_pending))
        .route("/daisy-pending/apply", post(apply_pending))
        .route("/daisy-pending/{id}", delete(dismiss_pending))
        .route("/daisy-settings/auto-apply", put(set_auto_apply))
}

fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize)]
struct PendingImportView {
    id: Uuid,
    momenttillf_id: String,
    course_code: String,
    name: String,
    semester_label: String,
    daisy_info_url: Option<String>,
    daisy_syllabus_url: Option<String>,
    daisy_unit: Option<String>,
    /// Total resolved participants. The admin UI uses this for the
    /// "5 staff" chip; the full list is in `participants` below.
    participant_count: usize,
    /// Pretty-printed roster summary for the UI: per-participant
    /// display_name + their daisy_roles. Lets the admin spot-check
    /// who'd land in the course without expanding a per-row drawer.
    participants: Vec<PendingParticipantView>,
    /// NULL = brand-new course offering (Apply will INSERT); Some =
    /// existing courses.id (Apply will refresh metadata + additively
    /// sync members). Frontend renders a "New" or "Update" badge.
    existing_course_id: Option<Uuid>,
    first_seen_at: chrono::DateTime<chrono::Utc>,
    last_seen_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct PendingParticipantView {
    display_name: Option<String>,
    /// First eppn is the canonical primary; the rest are alternates
    /// that the apply path would register as aliases. Frontend shows
    /// the primary in the table cell and the rest in a tooltip.
    eppns: Vec<String>,
    daisy_roles: Vec<String>,
    kind: String,
}

#[derive(Serialize)]
struct PendingListResponse {
    auto_apply: bool,
    auto_apply_updated_at: chrono::DateTime<chrono::Utc>,
    /// Admin who last flipped the toggle. Null on initial state
    /// (the migration seeds the row without an owner).
    auto_apply_updated_by: Option<Uuid>,
    pending: Vec<PendingImportView>,
}

async fn list_pending(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<PendingListResponse>, AppError> {
    require_admin(&user)?;

    let settings = minerva_db::queries::daisy_settings::get(&state.db).await?;
    let rows = minerva_db::queries::daisy_pending_imports::list_all(&state.db).await?;

    let mut pending = Vec::with_capacity(rows.len());
    for row in rows {
        // The JSONB participants column round-trips through
        // `DaisyParticipantInput`'s Serialize/Deserialize pair.
        // A row whose JSON has somehow drifted (manual SQL edit?
        // schema change?) gets logged + skipped rather than crashing
        // the whole admin page; the admin can dismiss it manually.
        let participants: Vec<DaisyParticipantInput> =
            match serde_json::from_value(row.participants.clone()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        pending_id = %row.id,
                        momenttillf_id = %row.momenttillf_id,
                        error = %e,
                        "daisy-pending: malformed participants JSON, treating as empty",
                    );
                    Vec::new()
                }
            };
        let participant_count = participants.len();
        let participant_views = participants
            .into_iter()
            .map(|p| PendingParticipantView {
                display_name: p.display_name,
                eppns: p.eppns,
                daisy_roles: p.daisy_roles,
                kind: p.kind,
            })
            .collect();
        pending.push(PendingImportView {
            id: row.id,
            momenttillf_id: row.momenttillf_id,
            course_code: row.course_code,
            name: row.name,
            semester_label: row.semester_label,
            daisy_info_url: row.daisy_info_url,
            daisy_syllabus_url: row.daisy_syllabus_url,
            daisy_unit: row.daisy_unit,
            participant_count,
            participants: participant_views,
            existing_course_id: row.existing_course_id,
            first_seen_at: row.first_seen_at,
            last_seen_at: row.last_seen_at,
        });
    }

    Ok(Json(PendingListResponse {
        auto_apply: settings.auto_apply,
        auto_apply_updated_at: settings.updated_at,
        auto_apply_updated_by: settings.updated_by,
        pending,
    }))
}

#[derive(Deserialize)]
struct ApplyPendingRequest {
    /// IDs of staging rows to apply. Empty = no-op. Frontend's "Apply
    /// all" button just sends every visible id; we don't accept a
    /// magic "all" sentinel so a stale UI list can't accidentally
    /// re-apply rows that arrived between page load and click.
    ids: Vec<Uuid>,
}

async fn apply_pending(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<ApplyPendingRequest>,
) -> Result<Json<DaisyImportSummary>, AppError> {
    require_admin(&user)?;

    let mut summary = DaisyImportSummary {
        courses_received: body.ids.len(),
        staged_for_review: false,
        ..Default::default()
    };

    if body.ids.is_empty() {
        return Ok(Json(summary));
    }

    // apply_one resolves the owner itself via
    // `users::find_or_create_by_eppn` from the kursansvarig Daisy
    // surfaced; we only need the admin-default embedding model here.
    let default_embedding_model =
        minerva_db::queries::embedding_models::current_default(&state.db).await?;

    for id in body.ids {
        // Re-fetch each row inside the loop so a concurrent
        // dismiss/apply by another admin produces a clean "no longer
        // pending" error per-id rather than a stale-read mid-batch.
        let Some(row) =
            minerva_db::queries::daisy_pending_imports::find_by_id(&state.db, id).await?
        else {
            summary
                .errors
                .push(format!("{id}: staging row no longer present"));
            continue;
        };

        // Reconstruct the payload the service endpoint would have
        // seen had auto_apply been ON at sync time.
        let participants: Vec<DaisyParticipantInput> =
            match serde_json::from_value(row.participants.clone()) {
                Ok(v) => v,
                Err(e) => {
                    summary.errors.push(format!(
                        "{}: malformed participants JSON: {e}",
                        row.momenttillf_id,
                    ));
                    continue;
                }
            };
        let payload = DaisyCourseInputPayload {
            momenttillf_id: row.momenttillf_id.clone(),
            beteckning: row.course_code.clone(),
            name: row.name.clone(),
            semester_label: Some(row.semester_label.clone()),
            info_url: row.daisy_info_url.clone(),
            syllabus_url: row.daisy_syllabus_url.clone(),
            unit: row.daisy_unit.clone(),
            participants,
        };

        match apply_one(
            &state,
            &payload,
            default_embedding_model.as_deref(),
            &mut summary,
        )
        .await
        {
            Ok(()) => {
                // Only drop the staging row after apply succeeds, so
                // a transient failure (e.g. play_designations race)
                // leaves the admin a chance to retry.
                let _ = minerva_db::queries::daisy_pending_imports::delete(&state.db, id).await?;
            }
            Err(e) => {
                summary
                    .errors
                    .push(format!("{}: {}", row.momenttillf_id, e));
                tracing::warn!(
                    pending_id = %id,
                    momenttillf_id = %row.momenttillf_id,
                    error = %e,
                    "daisy admin apply: per-row failure",
                );
            }
        }
    }

    tracing::info!(
        "daisy admin apply: requested={} created={} updated={} members_added={} aliases={} errors={}",
        summary.courses_received,
        summary.courses_created,
        summary.courses_updated,
        summary.members_added,
        summary.aliases_registered,
        summary.errors.len(),
    );
    Ok(Json(summary))
}

async fn dismiss_pending(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let deleted = minerva_db::queries::daisy_pending_imports::delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({ "deleted": deleted.is_some() })))
}

#[derive(Deserialize)]
struct SetAutoApplyRequest {
    enabled: bool,
}

#[derive(Serialize)]
struct AutoApplyResponse {
    auto_apply: bool,
    updated_at: chrono::DateTime<chrono::Utc>,
    updated_by: Option<Uuid>,
}

async fn set_auto_apply(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<SetAutoApplyRequest>,
) -> Result<Json<AutoApplyResponse>, AppError> {
    require_admin(&user)?;
    let row =
        minerva_db::queries::daisy_settings::set_auto_apply(&state.db, body.enabled, Some(user.id))
            .await?;
    tracing::info!(
        admin = %user.id,
        auto_apply = body.enabled,
        "daisy auto-apply toggled",
    );
    Ok(Json(AutoApplyResponse {
        auto_apply: row.auto_apply,
        updated_at: row.updated_at,
        updated_by: row.updated_by,
    }))
}
