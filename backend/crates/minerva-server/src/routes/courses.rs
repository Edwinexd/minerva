use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_courses).post(create_course))
        .route(
            "/{id}",
            get(get_course).put(update_course).delete(archive_course),
        )
        .route("/{id}/members", get(list_members).post(add_member))
        .route("/{id}/members/{user_id}", delete(remove_member))
        .route("/{id}/role-suggestions", get(list_role_suggestions))
        .route(
            "/{id}/role-suggestions/{suggestion_id}/approve",
            post(approve_role_suggestion),
        )
        .route(
            "/{id}/role-suggestions/{suggestion_id}/decline",
            post(decline_role_suggestion),
        )
}

#[derive(Deserialize)]
struct CreateCourseRequest {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct UpdateCourseRequest {
    name: Option<String>,
    description: Option<String>,
    context_ratio: Option<f64>,
    temperature: Option<f64>,
    model: Option<String>,
    system_prompt: Option<String>,
    max_chunks: Option<i32>,
    min_score: Option<f32>,
    strategy: Option<String>,
    embedding_provider: Option<String>,
    embedding_model: Option<String>,
    daily_token_limit: Option<i64>,
}

#[derive(Serialize)]
struct CourseResponse {
    id: Uuid,
    name: String,
    description: Option<String>,
    owner_id: Uuid,
    context_ratio: f64,
    temperature: f64,
    model: String,
    system_prompt: Option<String>,
    max_chunks: i32,
    min_score: f32,
    strategy: String,
    embedding_provider: String,
    embedding_model: String,
    /// Bumped each time `embedding_provider` or `embedding_model`
    /// rotates. Surfaced so the UI can correlate post-rotation
    /// re-ingestion progress with the current embedding generation.
    embedding_version: i32,
    daily_token_limit: i64,
    active: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    /// Viewer's course_member role ("teacher"|"ta"|"student"), or None if
    /// viewer is admin-only / not a member. Lets the frontend gate UI tabs.
    my_role: Option<String>,
    /// Per-course feature-flag state, resolved through the same path
    /// the runtime uses (course-scoped row > global row > compiled-in
    /// default). Frontend reads this to decide whether to show
    /// KG-related tabs / badges / dialogs.
    feature_flags: CourseFeatureFlagsView,
}

#[derive(Serialize, Default)]
struct CourseFeatureFlagsView {
    course_kg: bool,
}

impl CourseResponse {
    fn from_row(
        row: minerva_db::queries::courses::CourseRow,
        my_role: Option<String>,
        feature_flags: CourseFeatureFlagsView,
    ) -> Self {
        Self {
            id: row.id,
            name: row.name,
            description: row.description,
            owner_id: row.owner_id,
            context_ratio: row.context_ratio,
            temperature: row.temperature,
            model: row.model,
            system_prompt: row.system_prompt,
            max_chunks: row.max_chunks,
            min_score: row.min_score,
            strategy: row.strategy,
            embedding_provider: row.embedding_provider,
            embedding_model: row.embedding_model,
            embedding_version: row.embedding_version,
            daily_token_limit: row.daily_token_limit,
            active: row.active,
            created_at: row.created_at,
            updated_at: row.updated_at,
            my_role,
            feature_flags,
        }
    }
}

/// Resolve every course-scoped feature flag for the response. Single
/// place to extend when new flags land -- callers don't have to know
/// the flag list.
async fn resolve_course_flags(db: &sqlx::PgPool, course_id: Uuid) -> CourseFeatureFlagsView {
    CourseFeatureFlagsView {
        course_kg: crate::feature_flags::course_kg_enabled(db, course_id).await,
    }
}

async fn list_courses(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<CourseResponse>>, AppError> {
    let rows = if user.role.is_admin() {
        minerva_db::queries::courses::list_all(&state.db).await?
    } else if user.role.is_teacher_or_above() {
        // Teachers see courses they own + courses they teach/TA on
        minerva_db::queries::courses::list_for_teacher(&state.db, user.id).await?
    } else {
        // Students see courses they're a member of
        minerva_db::queries::courses::list_by_member(&state.db, user.id).await?
    };

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let my_role =
            minerva_db::queries::courses::get_member_role(&state.db, row.id, user.id).await?;
        let flags = resolve_course_flags(&state.db, row.id).await;
        out.push(CourseResponse::from_row(row, my_role, flags));
    }
    Ok(Json(out))
}

async fn create_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreateCourseRequest>,
) -> Result<Json<CourseResponse>, AppError> {
    if !user.role.is_teacher_or_above() {
        return Err(AppError::Forbidden);
    }

    let id = Uuid::new_v4();
    let input = minerva_db::queries::courses::CreateCourse {
        name: body.name,
        description: body.description,
        owner_id: user.id,
        // Apply the platform-wide default per-student-per-day cap. Teachers
        // can adjust (including to 0 = unlimited) via PUT afterwards; the
        // per-owner aggregate cap on `users` is the real spend backstop.
        daily_token_limit: state.config.default_course_daily_token_limit,
    };

    let row = minerva_db::queries::courses::create(&state.db, id, &input).await?;

    // Auto-add owner as teacher member
    minerva_db::queries::courses::add_member(&state.db, id, user.id, "teacher").await?;

    let flags = resolve_course_flags(&state.db, row.id).await;
    Ok(Json(CourseResponse::from_row(
        row,
        Some("teacher".into()),
        flags,
    )))
}

async fn get_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<CourseResponse>, AppError> {
    let row = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Check access: owner, admin, or member
    let my_role = minerva_db::queries::courses::get_member_role(&state.db, id, user.id).await?;
    if row.owner_id != user.id && !user.role.is_admin() && my_role.is_none() {
        return Err(AppError::Forbidden);
    }

    let flags = resolve_course_flags(&state.db, row.id).await;
    Ok(Json(CourseResponse::from_row(row, my_role, flags)))
}

async fn update_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCourseRequest>,
) -> Result<Json<CourseResponse>, AppError> {
    let existing = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if existing.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    // Threshold is a magnitude (we filter on abs(score)), so a negative
    // value would never match. Reject it before it hits the CHECK constraint.
    if let Some(score) = body.min_score {
        if !(0.0..=1.0).contains(&score) || score.is_nan() {
            return Err(AppError::bad_request("course.min_score_out_of_range"));
        }
    }

    // Validate embedding_provider
    if let Some(ref provider) = body.embedding_provider {
        if !minerva_ingest::pipeline::VALID_EMBEDDING_PROVIDERS.contains(&provider.as_str()) {
            return Err(AppError::bad_request_with(
                "course.embedding_provider_invalid",
                [("provider", provider.clone())],
            ));
        }
    }

    // Validate embedding_model based on the effective provider
    let effective_provider = body
        .embedding_provider
        .as_deref()
        .unwrap_or(&existing.embedding_provider);

    if effective_provider == "local" {
        if let Some(ref model) = body.embedding_model {
            let valid = minerva_ingest::pipeline::VALID_LOCAL_MODELS
                .iter()
                .any(|(name, _)| *name == model.as_str());
            if !valid {
                return Err(AppError::bad_request_with(
                    "course.local_embedding_model_invalid",
                    [("model", model.clone())],
                ));
            }
        }
    }

    // For openai provider, force the embedding_model to the canonical value
    let embedding_model = if effective_provider == "openai" {
        body.embedding_model
            .as_ref()
            .map(|_| minerva_ingest::pipeline::OPENAI_EMBEDDING_MODEL.to_string())
            .or_else(|| {
                // If switching to openai, ensure the model column is updated
                if body.embedding_provider.is_some() {
                    Some(minerva_ingest::pipeline::OPENAI_EMBEDDING_MODEL.to_string())
                } else {
                    None
                }
            })
    } else {
        body.embedding_model
    };

    // Detect a real embedding rotation. We compare against the
    // existing row so a no-op PUT (frontend echoing the current
    // values back) doesn't trigger a wasteful re-ingest of the whole
    // course. Either provider or model changing counts -- a same-dim
    // model swap (e.g. MiniLM-L6 -> BGE-small, both 384) silently
    // degrades retrieval quality without a re-embed, so we still
    // rotate.
    let new_provider_value = body
        .embedding_provider
        .as_deref()
        .unwrap_or(&existing.embedding_provider);
    let new_model_value = embedding_model
        .as_deref()
        .unwrap_or(&existing.embedding_model);
    let rotation_needed = new_provider_value != existing.embedding_provider
        || new_model_value != existing.embedding_model;

    if rotation_needed {
        // Lazy migration: bump `embedding_version` so the next
        // ingest writes to a fresh `course_{id}_v{n}` Qdrant
        // collection, and re-queue every document so the worker
        // re-chunks + re-embeds them. The previous-model collection
        // is left untouched -- orphaned, not deleted -- so a
        // mistaken rotation can be rolled back manually by the ops
        // team. The rotation runs in a transaction (see
        // `rotate_embedding`) so the version bump and the document
        // re-queue cannot be observed in a partial state.
        let outcome = minerva_db::queries::courses::rotate_embedding(
            &state.db,
            id,
            new_provider_value,
            new_model_value,
        )
        .await?;
        tracing::info!(
            "course {} rotated to embedding_provider={}, embedding_model={}, version={} ({} documents re-queued)",
            id,
            new_provider_value,
            new_model_value,
            outcome.new_version,
            outcome.requeued_documents,
        );
    }

    // Apply the rest of the update. Provider/model are intentionally
    // omitted -- if a rotation just ran they're already persisted; if
    // it didn't, COALESCE on `None` is a no-op anyway.
    let input = minerva_db::queries::courses::UpdateCourse {
        name: body.name,
        description: body.description,
        context_ratio: body.context_ratio,
        temperature: body.temperature,
        model: body.model,
        system_prompt: body.system_prompt,
        max_chunks: body.max_chunks,
        min_score: body.min_score,
        strategy: body.strategy,
        embedding_provider: None,
        embedding_model: None,
        daily_token_limit: body.daily_token_limit,
    };

    let row = minerva_db::queries::courses::update(&state.db, id, &input)
        .await?
        .ok_or(AppError::NotFound)?;

    let my_role = minerva_db::queries::courses::get_member_role(&state.db, id, user.id).await?;
    let flags = resolve_course_flags(&state.db, row.id).await;
    Ok(Json(CourseResponse::from_row(row, my_role, flags)))
}

async fn archive_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let existing = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if existing.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    minerva_db::queries::courses::archive(&state.db, id).await?;
    Ok(Json(serde_json::json!({ "archived": true })))
}

#[derive(Serialize)]
struct MemberResponse {
    user_id: Uuid,
    eppn: Option<String>,
    display_name: Option<String>,
    role: String,
    added_at: chrono::DateTime<chrono::Utc>,
}

async fn list_members(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<MemberResponse>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Owner, admin, teacher, and TA can all view the member list (read-only).
    if course.owner_id != user.id
        && !user.role.is_admin()
        && !minerva_db::queries::courses::is_course_teacher(&state.db, id, user.id).await?
    {
        return Err(AppError::Forbidden);
    }

    let rows = minerva_db::queries::courses::list_members(&state.db, id).await?;
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
                MemberResponse {
                    user_id: r.user_id,
                    eppn,
                    display_name,
                    role: r.role,
                    added_at: r.added_at,
                }
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct AddMemberRequest {
    eppn: String,
    role: Option<String>,
}

async fn add_member(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMemberRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    // Find or create the user by eppn. EPPN is treated case-insensitively
    // to avoid creating duplicate accounts for `alice@su.se` vs `alice@SU.SE`.
    let eppn = body.eppn.trim().to_lowercase();
    let (target, _) = minerva_db::queries::users::find_or_create_by_eppn(
        &state.db,
        &eppn,
        None,
        "student",
        state.config.default_owner_daily_token_limit,
    )
    .await?;
    let target_id = target.id;

    let role = body.role.as_deref().unwrap_or("student");
    minerva_db::queries::courses::add_member(&state.db, id, target_id, role).await?;

    Ok(Json(
        serde_json::json!({ "added": true, "user_id": target_id }),
    ))
}

async fn remove_member(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let removed = minerva_db::queries::courses::remove_member(&state.db, id, user_id).await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
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
/// stricter -- only owner or admin.
async fn list_role_suggestions(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<RoleSuggestionResponse>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id
        && !user.role.is_admin()
        && !minerva_db::queries::courses::is_course_teacher(&state.db, id, user.id).await?
    {
        return Err(AppError::Forbidden);
    }

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
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let suggestion =
        minerva_db::queries::role_suggestions::find_pending_by_id(&state.db, suggestion_id)
            .await?
            .ok_or(AppError::NotFound)?;
    // Defend against a suggestion id from a different course being POSTed
    // through this course's URL.
    if suggestion.course_id != id {
        return Err(AppError::NotFound);
    }

    minerva_db::queries::courses::add_member(
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
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

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
