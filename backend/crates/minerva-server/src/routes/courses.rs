use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use minerva_core::models::User;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_courses).post(create_course))
        // Cross-course aggregation of the caller's unread
        // conversations. Returns `{course_id: count}` for any
        // course with at least one unread; courses with zero are
        // omitted so the response stays small. Drives the unread
        // badge on the "My Courses" tile. Lives in the courses
        // router (not chat) because the chat router is mounted
        // under `/courses/{course_id}` and this rollup is
        // course-agnostic.
        .route("/unread-counts", get(student_unread_counts))
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

/// Per-course unread-conversation counts for the calling user.
/// Returns `{course_id_string: count}` so the frontend's "My
/// Courses" tile can render a badge per card without N round-trips.
/// Empty object when nothing is unread; courses with zero are
/// excluded from the payload to keep it tight.
async fn student_unread_counts(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<std::collections::HashMap<String, i64>>, AppError> {
    let rows = minerva_db::queries::conversations::student_unread_conversations(&state.db, user.id)
        .await?;
    let mut by_course: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for r in rows {
        *by_course.entry(r.course_id.to_string()).or_insert(0) += 1;
    }
    Ok(Json(by_course))
}

#[derive(Deserialize)]
struct CreateCourseRequest {
    name: String,
    description: Option<String>,
    /// Required: which semester this course is delivered in (e.g.
    /// `VT2026`). Mandatory for every new course so the My Courses
    /// page can group meaningfully even when the course wasn't
    /// auto-imported from Daisy. Historical rows from before this
    /// requirement remain NULL and admins backfill via PUT.
    semester_label: String,
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
    tool_use_enabled: Option<bool>,
    embedding_provider: Option<String>,
    embedding_model: Option<String>,
    /// Per-course re-ranker model change. Validated against the
    /// `reranker_models` catalog (enabled unless admin / unchanged).
    /// No re-embed: applies on the next chat turn.
    reranker_model: Option<String>,
    daily_cost_limit_usd: Option<Decimal>,
    /// Admin / owner backfill: stamp or rewrite the per-semester
    /// label on an existing course. Format-validated identically to
    /// `CreateCourseRequest::semester_label`.
    semester_label: Option<String>,
}

/// `^(VT|HT)YYYY$` with a sane year range. VT = vårtermin (spring,
/// Jan-Jun), HT = hösttermin (autumn, Jul-Dec); year is 4 digits.
/// Anything outside `[2000, 2100)` is rejected to catch typos like
/// "VT26" or "VT20266" that would otherwise pass a looser pattern.
fn validate_semester_label(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim().to_uppercase();
    let invalid = || AppError::bad_request("course.semester_label_invalid");
    if trimmed.len() != 6 {
        return Err(invalid());
    }
    let (season, year_str) = trimmed.split_at(2);
    if season != "VT" && season != "HT" {
        return Err(invalid());
    }
    let year: i32 = year_str.parse().map_err(|_| invalid())?;
    if !(2000..2100).contains(&year) {
        return Err(invalid());
    }
    Ok(trimmed)
}

#[derive(Serialize)]
pub(crate) struct CourseResponse {
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
    /// Orthogonal to `strategy`: when TRUE, the model gains a tool
    /// catalog during a research/thinking phase before the writeup.
    /// Mirrors `courses.tool_use_enabled`.
    tool_use_enabled: bool,
    embedding_provider: String,
    embedding_model: String,
    /// Bumped each time `embedding_provider` or `embedding_model`
    /// rotates. Surfaced so the UI can correlate post-rotation
    /// re-ingestion progress with the current embedding generation.
    embedding_version: i32,
    /// Per-course cross-encoder re-ranker model id. Picked from the
    /// admin-managed `reranker_models` catalog; changing it has no
    /// re-embed cost. Frontend renders a dropdown on the config page.
    reranker_model: String,
    daily_cost_limit_usd: Decimal,
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
    /// `VT2026` / `HT2025` etc. Set by the Daisy auto-import phase;
    /// drives the per-semester grouping on the My Courses page. NULL
    /// for ad-hoc (manually-created) courses.
    semester_label: Option<String>,
    /// Daisy offerings linked to this course (possibly several after a
    /// merge). Frontend renders the "Auto-managed by Daisy sync" badge
    /// plus per-offering info / syllabus links on the settings page.
    /// Empty for manually-created courses.
    daisy_offerings: Vec<DaisyOfferingView>,
    /// TRUE when the course was created by the Daisy auto-import phase
    /// (membership sync stays additive on these; teachers shouldn't
    /// fight the import). Mirrors `courses.auto_managed`.
    auto_managed: bool,
    /// Short course code (e.g. `PROG2`). Populated by the Daisy
    /// auto-import; NULL on historical / ad-hoc courses. Frontend
    /// uses it as a chip on the My Courses tile so the term-stable
    /// identifier is visible alongside the rename-friendly `name`.
    course_code: Option<String>,
    /// Number of `student`-role members. The per-student daily spend
    /// cap (`daily_cost_limit_usd`) applies to each of these, so the
    /// admin usage table and the teacher config page multiply the two
    /// to show the course-wide theoretical daily max ("N students x
    /// cap") instead of leaving the bare per-student figure looking
    /// like a course-wide budget.
    student_count: i64,
}

#[derive(Serialize)]
struct DaisyOfferingView {
    momenttillf_id: String,
    course_code: Option<String>,
    name: Option<String>,
    semester_label: Option<String>,
    info_url: Option<String>,
    syllabus_url: Option<String>,
    unit: Option<String>,
    last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl DaisyOfferingView {
    fn from_row(o: minerva_db::queries::course_daisy_offerings::DaisyOfferingRow) -> Self {
        Self {
            momenttillf_id: o.momenttillf_id,
            course_code: o.course_code,
            name: o.name,
            semester_label: o.semester_label,
            info_url: o.info_url,
            syllabus_url: o.syllabus_url,
            unit: o.unit,
            last_synced_at: o.last_synced_at,
        }
    }
}

/// Per-course feature-flag snapshot, resolved through the runtime
/// flag path (course row > global > compiled-in default). Shared
/// between the Shibboleth `/courses/{id}` route and the embed
/// `/embed/course/{id}` route; both surface the same shape so the
/// frontend can gate UI uniformly regardless of how the user
/// reached the chat. Add new flags here AND in `resolve_course_flags`.
#[derive(Serialize, Default)]
pub(crate) struct CourseFeatureFlagsView {
    pub(crate) course_kg: bool,
    /// Aegis prompt-coaching feedback panel. When TRUE the chat UI
    /// renders a third right-side column with the per-prompt
    /// scoring + history. Resolves through the same path as
    /// `course_kg` (course row -> global -> default false).
    pub(crate) aegis: bool,
    /// Concept knowledge graph (eureka-2). When TRUE the admin UI
    /// surfaces concept-graph extraction + viewer for the course;
    /// distinct from `course_kg` (the document-level graph).
    /// Resolves through the same path as the others.
    pub(crate) concept_graph: bool,
}

impl CourseResponse {
    fn from_row(
        row: minerva_db::queries::courses::CourseRow,
        my_role: Option<String>,
        feature_flags: CourseFeatureFlagsView,
        daisy_offerings: Vec<DaisyOfferingView>,
        student_count: i64,
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
            tool_use_enabled: row.tool_use_enabled,
            embedding_provider: row.embedding_provider,
            embedding_model: row.embedding_model,
            embedding_version: row.embedding_version,
            reranker_model: row.reranker_model,
            daily_cost_limit_usd: row.daily_cost_limit_usd,
            active: row.active,
            created_at: row.created_at,
            updated_at: row.updated_at,
            my_role,
            feature_flags,
            semester_label: row.semester_label,
            daisy_offerings,
            auto_managed: row.auto_managed,
            course_code: row.course_code,
            student_count,
        }
    }
}

/// Resolve every course-scoped feature flag for the response. Single
/// place to extend when new flags land; callers don't have to know
/// the flag list. Shared with the embed route so its course response
/// stays in lockstep without each consumer duplicating the resolver.
pub(crate) async fn resolve_course_flags(
    db: &sqlx::PgPool,
    course_id: Uuid,
) -> CourseFeatureFlagsView {
    CourseFeatureFlagsView {
        course_kg: crate::feature_flags::course_kg_enabled(db, course_id).await,
        aegis: crate::feature_flags::aegis_enabled(db, course_id).await,
        concept_graph: crate::feature_flags::concept_graph_enabled(db, course_id).await,
    }
}

/// Resolve every Daisy offering linked to a course for the response.
/// Empty for manually-created courses. A read failure degrades to an
/// empty list rather than failing the whole course fetch.
async fn resolve_course_offerings(db: &sqlx::PgPool, course_id: Uuid) -> Vec<DaisyOfferingView> {
    minerva_db::queries::course_daisy_offerings::list_by_course(db, course_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(DaisyOfferingView::from_row)
        .collect()
}

/// Build the wire `CourseResponse` for an admin listing (the admin
/// `/admin/courses` surface, which includes archived courses). `my_role`
/// is left None: the admin courses table doesn't gate on per-course
/// membership the way the teacher/student views do.
pub(crate) async fn admin_course_response(
    db: &sqlx::PgPool,
    row: minerva_db::queries::courses::CourseRow,
    student_count: i64,
) -> CourseResponse {
    let flags = resolve_course_flags(db, row.id).await;
    let offerings = resolve_course_offerings(db, row.id).await;
    CourseResponse::from_row(row, None, flags, offerings, student_count)
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

    // One grouped query for every course's student count, rather than a
    // per-course COUNT inside the loop below.
    let student_counts = minerva_db::queries::courses::count_students_by_course(&state.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let my_role =
            minerva_db::queries::courses::get_member_role(&state.db, row.id, user.id).await?;
        let flags = resolve_course_flags(&state.db, row.id).await;
        let offerings = resolve_course_offerings(&state.db, row.id).await;
        let student_count = student_counts.get(&row.id).copied().unwrap_or(0);
        out.push(CourseResponse::from_row(
            row,
            my_role,
            flags,
            offerings,
            student_count,
        ));
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

    let semester_label = validate_semester_label(&body.semester_label)?;

    let id = Uuid::new_v4();
    // Snapshot every admin-tunable course default into the new row.
    // `embedding_model` is special-cased: the legacy
    // `embedding_models.is_default` flag still wins because that's
    // the table admins actually edit on the System tab. If no row is
    // marked default (shouldn't happen post-startup-sync), we fall
    // through to the SQL column DEFAULT via the COALESCE in
    // `queries::courses::create`. Every other knob is read straight
    // from `system_defaults`; teachers can override per-course via
    // PUT /courses/{id} afterwards.
    let default_embedding_model =
        minerva_db::queries::embedding_models::current_default(&state.db).await?;
    // Same pattern for the re-ranker: snapshot the admin-chosen catalog
    // default. Falls through to the column DEFAULT if no row is marked.
    let default_reranker_model =
        minerva_db::queries::reranker_models::current_default(&state.db).await?;

    let input = minerva_db::queries::courses::CreateCourse {
        name: body.name,
        description: body.description,
        owner_id: user.id,
        semester_label,
        daily_cost_limit_usd: crate::system_defaults::course_daily_cost_limit_usd(&state.db).await,
        model: Some(crate::system_defaults::course_model(&state.db).await),
        temperature: Some(crate::system_defaults::course_temperature(&state.db).await),
        context_ratio: Some(crate::system_defaults::course_context_ratio(&state.db).await),
        max_chunks: Some(crate::system_defaults::course_max_chunks(&state.db).await),
        min_score: Some(crate::system_defaults::course_min_score(&state.db).await),
        strategy: Some(crate::system_defaults::course_strategy(&state.db).await),
        tool_use_enabled: Some(crate::system_defaults::course_tool_use_enabled(&state.db).await),
        embedding_provider: Some(
            crate::system_defaults::course_embedding_provider(&state.db).await,
        ),
        embedding_model: default_embedding_model,
        reranker_model: default_reranker_model,
        system_prompt: crate::system_defaults::course_system_prompt(&state.db).await,
    };

    let row = minerva_db::queries::courses::create(&state.db, id, &input).await?;

    // Auto-add owner as teacher member
    minerva_db::queries::courses::add_member(&state.db, id, user.id, "teacher").await?;

    let flags = resolve_course_flags(&state.db, row.id).await;
    // A just-created course has only its owner (added as a teacher
    // above), so there are no student members yet.
    Ok(Json(CourseResponse::from_row(
        row,
        Some("teacher".into()),
        flags,
        Vec::new(),
        0,
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
    let offerings = resolve_course_offerings(&state.db, row.id).await;
    let student_count = minerva_db::queries::courses::count_students(&state.db, row.id).await?;
    Ok(Json(CourseResponse::from_row(
        row,
        my_role,
        flags,
        offerings,
        student_count,
    )))
}

/// Sanity ceiling for the per-course, per-student daily USD spend cap.
/// Matches the `course.daily_cost_limit_usd` system-default knob's max
/// (`system_defaults::registry`) so a per-course override can't exceed
/// the range its own default is drawn from. A footgun guard far above any
/// realistic per-student daily spend; 0 still means unlimited.
const MAX_COURSE_DAILY_COST_LIMIT_USD: i64 = 1000;

/// The mutable subset of a course, with every field optional so a
/// partial update only touches what's present. Shared between the
/// owner/admin `PUT /courses/{id}` route and the admin bulk-edit
/// endpoint so both run the *identical* validation + rotation path
/// (see [`apply_course_update`]); there is no second, drifting copy of
/// the model-capability / embedding / reranker checks.
pub(crate) struct CourseUpdateFields {
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_ratio: Option<f64>,
    pub temperature: Option<f64>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub max_chunks: Option<i32>,
    pub min_score: Option<f32>,
    pub strategy: Option<String>,
    pub tool_use_enabled: Option<bool>,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    pub reranker_model: Option<String>,
    pub daily_cost_limit_usd: Option<Decimal>,
    pub semester_label: Option<String>,
}

/// Validate a partial course update against `existing`, perform an
/// embedding rotation if the provider/model changed, and persist the
/// rest. Returns the freshly-updated row.
///
/// `is_admin` controls two bypasses: admins may force a course onto a
/// catalog model that's currently disabled in the picker (embedding or
/// reranker), the same affordance the per-course force-migrate dialog
/// relies on. Authorization (owner-or-admin) is the caller's job; this
/// helper assumes the caller already proved they may edit the course.
pub(crate) async fn apply_course_update(
    state: &AppState,
    existing: &minerva_db::queries::courses::CourseRow,
    body: CourseUpdateFields,
    is_admin: bool,
) -> Result<minerva_db::queries::courses::CourseRow, AppError> {
    let id = existing.id;

    // Threshold is a magnitude (we filter on abs(score)), so a negative
    // value would never match. Reject it before it hits the CHECK constraint.
    if let Some(score) = body.min_score {
        if !(0.0..=1.0).contains(&score) || score.is_nan() {
            return Err(AppError::bad_request("course.min_score_out_of_range"));
        }
    }

    // The per-course daily spend cap is a budget in USD. A negative value
    // would silently disable the cap (enforcement treats only `> 0` as a
    // live limit), so reject it, and guard a fat-finger ceiling. Mirrors
    // the owner-cap route's ergonomics; 0 stays "unlimited".
    if let Some(limit) = body.daily_cost_limit_usd {
        if limit < Decimal::ZERO {
            return Err(AppError::bad_request("course.daily_cost_limit_negative"));
        }
        let max = Decimal::from(MAX_COURSE_DAILY_COST_LIMIT_USD);
        if limit > max {
            return Err(AppError::bad_request_with(
                "course.daily_cost_limit_too_large",
                [("max", max.to_string())],
            ));
        }
    }

    // Capability check: reject mismatches between the chosen
    // (model, strategy, tool_use) triple before persisting. The
    // registry lives in `model_capabilities`; unknown models are
    // treated as supporting neither tools nor logprobs so a
    // teacher can't enable a feature the runtime cannot deliver.
    // We resolve effective values by overlaying the request body
    // on the existing row so a partial PUT (changing only one
    // field) still validates the resulting triple, not just the
    // delta.
    let effective_model = body.model.as_deref().unwrap_or(existing.model.as_str());
    let effective_strategy = body
        .strategy
        .as_deref()
        .unwrap_or(existing.strategy.as_str());
    let effective_tool_use = body.tool_use_enabled.unwrap_or(existing.tool_use_enabled);
    // Capabilities are read from the chat_models catalog
    // (provider-agnostic), not a Cerebras probe: an Anthropic model
    // declares supports_logprobs=false and is therefore rejected for the
    // FLARE strategy. A model not in the catalog default-denies both
    // capabilities, same conservative posture as an unprobeable model.
    let caps = match minerva_db::queries::chat_models::find(&state.db, effective_model).await? {
        Some(row) => crate::model_capabilities::Capabilities {
            supports_tools: row.supports_tool_use,
            supports_logprobs: row.supports_logprobs,
        },
        None => crate::model_capabilities::Capabilities::none(),
    };
    if let Err(mismatch) =
        crate::model_capabilities::validate_caps(caps, effective_strategy, effective_tool_use)
    {
        return Err(AppError::bad_request_with(
            mismatch.translation_key(),
            [("model", effective_model.to_string())],
        ));
    }

    // Validate the chat model against the admin-managed `chat_models`
    // catalog: a teacher may only switch to an enabled model. Same
    // two-layer ergonomics as the embedding / reranker gates, bypassed
    // when the value is unchanged or the caller is an admin (admins
    // force-migrate courses onto any model, including disabled ones).
    // The model's provider is resolved from `chat_models.provider` at
    // chat time via the registry.
    if let Some(ref model) = body.model {
        let unchanged = model.as_str() == existing.model.as_str();
        if !unchanged && !is_admin {
            let enabled = minerva_db::queries::chat_models::is_enabled(&state.db, model).await?;
            if !enabled {
                return Err(AppError::bad_request_with(
                    "course.chat_model_disabled",
                    [("model", model.clone())],
                ));
            }
        }
    }

    // Validate embedding_provider
    if let Some(ref provider) = body.embedding_provider {
        if !minerva_catalog::VALID_EMBEDDING_PROVIDERS.contains(&provider.as_str()) {
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
            // Two-layer check: catalog membership (compile-time list) +
            // admin-managed enabled flag (DB-backed). Catalog rejects
            // typos with a clear error; the enabled gate prevents
            // teachers from picking a model the admin has switched
            // off. Bypassed in two cases:
            //   * `model == existing.embedding_model`; no rotation
            //     happens in the rotate path below, so any other
            //     unrelated PUT (rename / temperature change / …) on a
            //     course currently sitting on a now-disabled model
            //     still saves.
            //   * caller is an admin; admins use the same route to
            //     force-migrate any course onto any catalog model,
            //     including currently-disabled ones (a typical
            //     workflow is "disable model X site-wide, then walk
            //     each course off it"). The catalog membership check
            //     still applies.
            let in_catalog = minerva_catalog::VALID_LOCAL_MODELS
                .iter()
                .any(|(name, _)| *name == model.as_str());
            if !in_catalog {
                return Err(AppError::bad_request_with(
                    "course.local_embedding_model_invalid",
                    [("model", model.clone())],
                ));
            }
            let unchanged = model.as_str() == existing.embedding_model.as_str();
            if !unchanged && !is_admin {
                let enabled =
                    minerva_db::queries::embedding_models::is_enabled(&state.db, model).await?;
                if !enabled {
                    return Err(AppError::bad_request_with(
                        "course.local_embedding_model_disabled",
                        [("model", model.clone())],
                    ));
                }
            }
        }
    }

    // Validate the re-ranker model (independent of embedding provider;
    // the cross-encoder reads chunk text either way). Same two-layer
    // check as embeddings: catalog membership + admin-managed enabled
    // flag, bypassed when the value is unchanged or the caller is an
    // admin. Unlike embeddings there is NO rotation: switching the
    // re-ranker has no re-embed cost, so the new value just lands via
    // the COALESCE update below and applies on the next chat turn.
    if let Some(ref model) = body.reranker_model {
        let in_catalog = minerva_catalog::VALID_RERANKER_MODELS.contains(&model.as_str());
        if !in_catalog {
            return Err(AppError::bad_request_with(
                "course.reranker_model_invalid",
                [("model", model.clone())],
            ));
        }
        let unchanged = model.as_str() == existing.reranker_model.as_str();
        if !unchanged && !is_admin {
            let enabled =
                minerva_db::queries::reranker_models::is_enabled(&state.db, model).await?;
            if !enabled {
                return Err(AppError::bad_request_with(
                    "course.reranker_model_disabled",
                    [("model", model.clone())],
                ));
            }
        }
    }

    // For openai provider, force the embedding_model to the canonical value
    let embedding_model = if effective_provider == "openai" {
        body.embedding_model
            .as_ref()
            .map(|_| minerva_catalog::OPENAI_EMBEDDING_MODEL.to_string())
            .or_else(|| {
                // If switching to openai, ensure the model column is updated
                if body.embedding_provider.is_some() {
                    Some(minerva_catalog::OPENAI_EMBEDDING_MODEL.to_string())
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
    // course. Either provider or model changing counts; a same-dim
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
        // is left untouched; orphaned, not deleted; so a
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

    // Validate the optional semester relabel before applying. We
    // intentionally normalise to upper-case via `validate_semester_label`
    // so the stored value is consistent regardless of how a teacher
    // typed it ("vt2026" -> "VT2026").
    let validated_semester_label = match body.semester_label.as_deref() {
        Some(raw) => Some(validate_semester_label(raw)?),
        None => None,
    };

    // Apply the rest of the update. Provider/model are intentionally
    // omitted; if a rotation just ran they're already persisted; if
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
        tool_use_enabled: body.tool_use_enabled,
        embedding_provider: None,
        embedding_model: None,
        reranker_model: body.reranker_model,
        daily_cost_limit_usd: body.daily_cost_limit_usd,
        semester_label: validated_semester_label,
    };

    minerva_db::queries::courses::update(&state.db, id, &input)
        .await?
        .ok_or(AppError::NotFound)
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

    let fields = CourseUpdateFields {
        name: body.name,
        description: body.description,
        context_ratio: body.context_ratio,
        temperature: body.temperature,
        model: body.model,
        system_prompt: body.system_prompt,
        max_chunks: body.max_chunks,
        min_score: body.min_score,
        strategy: body.strategy,
        tool_use_enabled: body.tool_use_enabled,
        embedding_provider: body.embedding_provider,
        embedding_model: body.embedding_model,
        reranker_model: body.reranker_model,
        daily_cost_limit_usd: body.daily_cost_limit_usd,
        semester_label: body.semester_label,
    };

    let row = apply_course_update(&state, &existing, fields, user.role.is_admin()).await?;

    let my_role = minerva_db::queries::courses::get_member_role(&state.db, id, user.id).await?;
    let flags = resolve_course_flags(&state.db, row.id).await;
    let offerings = resolve_course_offerings(&state.db, row.id).await;
    let student_count = minerva_db::queries::courses::count_students(&state.db, row.id).await?;
    Ok(Json(CourseResponse::from_row(
        row,
        my_role,
        flags,
        offerings,
        student_count,
    )))
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
        crate::system_defaults::owner_daily_cost_limit_usd(&state.db).await,
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
/// stricter; only owner or admin.
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

#[cfg(test)]
mod semester_label_tests {
    use super::validate_semester_label;

    #[test]
    fn accepts_canonical_labels() {
        assert_eq!(validate_semester_label("VT2026").unwrap(), "VT2026");
        assert_eq!(validate_semester_label("HT2099").unwrap(), "HT2099");
    }

    #[test]
    fn normalises_case_and_trims_whitespace() {
        // Teachers type the label by hand; we accept any case and
        // surrounding spaces but store the canonical upper-case form.
        assert_eq!(validate_semester_label("  vt2026 ").unwrap(), "VT2026");
        assert_eq!(validate_semester_label("Ht2027").unwrap(), "HT2027");
    }

    #[test]
    fn rejects_wrong_shape() {
        // Common typos: 2-digit year, 5-digit year, wrong season,
        // missing season, embedded space.
        for bad in [
            "VT26", "VT20266", "ST2026", "2026", "VT 2026", "VTVT26", "", "  ",
        ] {
            assert!(
                validate_semester_label(bad).is_err(),
                "expected {bad:?} to be rejected",
            );
        }
    }

    #[test]
    fn rejects_out_of_range_years() {
        // Guard against e.g. "VT0026" passing because the digits
        // happen to fit. Range is [2000, 2100).
        assert!(validate_semester_label("VT1999").is_err());
        assert!(validate_semester_label("HT2100").is_err());
    }
}
