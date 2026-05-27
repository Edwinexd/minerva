use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use minerva_core::models::{RuleOperator, User};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::rules::{validate_regex, SUPPORTED_ATTRIBUTES};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{id}/role", put(update_user_role))
        .route("/users/{id}/role-lock", delete(clear_role_lock))
        .route("/users/{id}/suspended", put(update_user_suspended))
        .route(
            "/users/{id}/owner-daily-token-limit",
            put(update_owner_daily_token_limit),
        )
        .route("/users/{id}/daily-usage", delete(reset_user_daily_usage))
        .route("/role-rules", get(list_role_rules).post(create_role_rule))
        .route(
            "/role-rules/{id}",
            put(update_role_rule).delete(delete_role_rule),
        )
        .route("/role-rules/{id}/conditions", post(create_rule_condition))
        .route(
            "/role-rules/conditions/{cond_id}",
            delete(delete_rule_condition),
        )
        .route(
            "/role-rules/attribute-values",
            get(list_role_rule_attribute_values),
        )
        .route("/classification-stats", get(get_classification_stats))
        .route("/backfill-classifications", post(backfill_classifications))
        .route(
            "/courses/{course_id}/feature-flags",
            get(get_course_feature_flags).put(set_course_feature_flags),
        )
        .route(
            "/embedding-models",
            get(list_embedding_models).put(update_embedding_model_enabled),
        )
        .route(
            "/embedding-models/default",
            put(set_default_embedding_model),
        )
        .route("/embedding-benchmark", post(run_embedding_benchmark))
        .route(
            "/system-defaults",
            get(list_system_defaults).put(update_system_default),
        )
        .route("/system-defaults/{key}", delete(reset_system_default))
}

fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize)]
struct UserResponse {
    id: Uuid,
    eppn: String,
    display_name: Option<String>,
    role: String,
    suspended: bool,
    role_manually_set: bool,
    owner_daily_token_limit: i64,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn list_users(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<UserResponse>>, AppError> {
    require_admin(&user)?;

    let rows = minerva_db::queries::users::list_all(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| UserResponse {
                id: r.id,
                eppn: r.eppn,
                display_name: r.display_name,
                role: r.role,
                suspended: r.suspended,
                role_manually_set: r.role_manually_set,
                owner_daily_token_limit: r.owner_daily_token_limit,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct UpdateRoleRequest {
    role: String,
}

async fn update_user_role(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRoleRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    // Allow setting to student, teacher, or integrator. Admin stays env-only
    // (MINERVA_ADMINS), so it is intentionally not assignable here.
    if body.role != "teacher" && body.role != "student" && body.role != "integrator" {
        return Err(AppError::bad_request("admin.role_invalid"));
    }

    // Sets role_manually_set=true so subsequent rule evaluations leave the
    // user alone; admin choice wins until they call /role-lock DELETE.
    let updated = minerva_db::queries::users::update_role(&state.db, id, &body.role).await?;
    if !updated {
        return Err(AppError::NotFound);
    }

    Ok(Json(serde_json::json!({ "updated": true })))
}

async fn clear_role_lock(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let updated = minerva_db::queries::users::clear_role_lock(&state.db, id).await?;
    if !updated {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "updated": true })))
}

#[derive(Deserialize)]
struct UpdateSuspendedRequest {
    suspended: bool,
}

async fn update_user_suspended(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSuspendedRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    // Prevent admins from suspending themselves
    if id == user.id {
        return Err(AppError::bad_request("admin.cannot_suspend_self"));
    }

    let updated = minerva_db::queries::users::set_suspended(&state.db, id, body.suspended).await?;
    if !updated {
        return Err(AppError::NotFound);
    }

    Ok(Json(serde_json::json!({ "updated": true })))
}

#[derive(Deserialize)]
struct UpdateOwnerLimitRequest {
    limit: i64,
}

/// Sanity ceiling on the per-owner daily cap. Picked to leave 6+ orders of
/// magnitude of headroom before a sum across all owned courses overflows
/// BIGINT (i64::MAX is ~9.2e18). 1 trillion tokens/day is also wildly
/// beyond any realistic spend, so this is purely a footgun guard against
/// admin typos / fat-finger.
const OWNER_LIMIT_MAX: i64 = 1_000_000_000_000;

async fn update_owner_daily_token_limit(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateOwnerLimitRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    if body.limit < 0 {
        return Err(AppError::bad_request("admin.limit_negative"));
    }
    if body.limit > OWNER_LIMIT_MAX {
        return Err(AppError::bad_request_with(
            "admin.limit_too_large",
            [("max", OWNER_LIMIT_MAX.to_string())],
        ));
    }
    let updated =
        minerva_db::queries::users::update_owner_daily_token_limit(&state.db, id, body.limit)
            .await?;
    if !updated {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "updated": true })))
}

/// Zeroes out today's token usage for a user so both their per-course
/// student cap and their contribution to any owner aggregate cap reset
/// immediately, without waiting for UTC midnight. Implemented as a DELETE
/// of today's `usage_daily` rows; `record_usage` upserts, so the next
/// request just re-creates the row from zero.
async fn reset_user_daily_usage(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    // 404 if the user doesn't exist; otherwise we'd silently return
    // `rows_deleted: 0` for a bad UUID, which hides typos.
    if minerva_db::queries::users::find_by_id(&state.db, id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }

    let deleted = minerva_db::queries::usage::reset_user_daily_usage(&state.db, id).await?;
    Ok(Json(
        serde_json::json!({ "reset": true, "rows_deleted": deleted }),
    ))
}

// ------------------------- Role rules -------------------------

#[derive(Serialize)]
struct RoleRuleResponse {
    id: Uuid,
    name: String,
    target_role: String,
    enabled: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    conditions: Vec<RoleRuleConditionResponse>,
}

#[derive(Serialize)]
struct RoleRuleConditionResponse {
    id: Uuid,
    rule_id: Uuid,
    attribute: String,
    operator: String,
    value: String,
}

async fn list_role_rules(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<RoleRuleResponse>>, AppError> {
    require_admin(&user)?;
    let rules = minerva_db::queries::role_rules::list_all(&state.db).await?;
    let ids: Vec<Uuid> = rules.iter().map(|r| r.id).collect();
    let conds = minerva_db::queries::role_rules::list_conditions_for_rules(&state.db, &ids).await?;
    let mut by_rule: std::collections::HashMap<Uuid, Vec<RoleRuleConditionResponse>> =
        std::collections::HashMap::new();
    for c in conds {
        by_rule
            .entry(c.rule_id)
            .or_default()
            .push(RoleRuleConditionResponse {
                id: c.id,
                rule_id: c.rule_id,
                attribute: c.attribute,
                operator: c.operator,
                value: c.value,
            });
    }
    Ok(Json(
        rules
            .into_iter()
            .map(|r| RoleRuleResponse {
                conditions: by_rule.remove(&r.id).unwrap_or_default(),
                id: r.id,
                name: r.name,
                target_role: r.target_role,
                enabled: r.enabled,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct CreateRoleRuleRequest {
    name: String,
    target_role: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

fn validate_target_role(role: &str) -> Result<(), AppError> {
    // Admin promotion via rules is intentionally disallowed; admins must
    // be in MINERVA_ADMINS so the env stays the source of truth. Integrator
    // is allowed: it is a DB-granted superset of Teacher (site-wide
    // integration powers, no user-management), so delegating it via a rule
    // is the same trust boundary as granting Teacher via a rule plus an
    // explicit "and trust them with site integrations" choice by the admin.
    if role != "teacher" && role != "student" && role != "integrator" {
        return Err(AppError::bad_request("admin.target_role_invalid"));
    }
    Ok(())
}

async fn create_role_rule(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreateRoleRuleRequest>,
) -> Result<Json<RoleRuleResponse>, AppError> {
    require_admin(&user)?;
    validate_target_role(&body.target_role)?;
    if body.name.trim().is_empty() {
        return Err(AppError::bad_request("admin.rule_name_required"));
    }
    let row = minerva_db::queries::role_rules::create_rule(
        &state.db,
        Uuid::new_v4(),
        body.name.trim(),
        &body.target_role,
        body.enabled,
    )
    .await?;
    state.rules.reload(&state.db).await?;
    Ok(Json(RoleRuleResponse {
        id: row.id,
        name: row.name,
        target_role: row.target_role,
        enabled: row.enabled,
        created_at: row.created_at,
        updated_at: row.updated_at,
        conditions: vec![],
    }))
}

#[derive(Deserialize)]
struct UpdateRoleRuleRequest {
    name: String,
    target_role: String,
    enabled: bool,
}

async fn update_role_rule(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRoleRuleRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    validate_target_role(&body.target_role)?;
    if body.name.trim().is_empty() {
        return Err(AppError::bad_request("admin.rule_name_required"));
    }
    let updated = minerva_db::queries::role_rules::update_rule(
        &state.db,
        id,
        body.name.trim(),
        &body.target_role,
        body.enabled,
    )
    .await?;
    if !updated {
        return Err(AppError::NotFound);
    }
    state.rules.reload(&state.db).await?;
    Ok(Json(serde_json::json!({ "updated": true })))
}

async fn delete_role_rule(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let deleted = minerva_db::queries::role_rules::delete_rule(&state.db, id).await?;
    if !deleted {
        return Err(AppError::NotFound);
    }
    state.rules.reload(&state.db).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

#[derive(Deserialize)]
struct CreateConditionRequest {
    attribute: String,
    operator: String,
    value: String,
}

async fn create_rule_condition(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(rule_id): Path<Uuid>,
    Json(body): Json<CreateConditionRequest>,
) -> Result<Json<RoleRuleConditionResponse>, AppError> {
    require_admin(&user)?;
    if !SUPPORTED_ATTRIBUTES.contains(&body.attribute.as_str()) {
        return Err(AppError::bad_request_with(
            "admin.condition_attribute_unsupported",
            [
                ("attribute", body.attribute.clone()),
                ("supported", SUPPORTED_ATTRIBUTES.join(", ")),
            ],
        ));
    }
    let op = RuleOperator::parse(&body.operator)
        .ok_or_else(|| AppError::bad_request("admin.condition_operator_invalid"))?;
    if matches!(op, RuleOperator::Regex | RuleOperator::NotRegex) {
        validate_regex(&body.value).map_err(|e| {
            AppError::bad_request_with("admin.condition_regex_invalid", [("detail", e.to_string())])
        })?;
    }
    let row = minerva_db::queries::role_rules::create_condition(
        &state.db,
        Uuid::new_v4(),
        rule_id,
        &body.attribute,
        op.as_str(),
        &body.value,
    )
    .await?;
    state.rules.reload(&state.db).await?;
    Ok(Json(RoleRuleConditionResponse {
        id: row.id,
        rule_id: row.rule_id,
        attribute: row.attribute,
        operator: row.operator,
        value: row.value,
    }))
}

async fn delete_rule_condition(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(cond_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let deleted = minerva_db::queries::role_rules::delete_condition(&state.db, cond_id).await?;
    if !deleted {
        return Err(AppError::NotFound);
    }
    state.rules.reload(&state.db).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

#[derive(Serialize)]
struct AttributeValueSuggestionResponse {
    /// Concrete value observed on at least `MIN_SUGGESTION_USERS` distinct
    /// users (post-`;`-split for multi-valued Shib headers). Suitable to
    /// drop straight into a `contains` rule condition unchanged.
    value: String,
    /// Number of distinct users whose login produced this value. Surfaced
    /// to the admin so they can see "ranked by how common" at a glance and
    /// distinguish a popular affiliation from a borderline-singleton one.
    user_count: i64,
}

#[derive(Serialize)]
struct AttributeValuesResponse {
    /// Per-attribute suggestion buckets, keyed by the attribute name (the
    /// same identifier admins pick from the attribute dropdown). Attributes
    /// with zero qualifying values are omitted entirely; the frontend falls
    /// back to free-text-only for those.
    by_attribute: std::collections::BTreeMap<String, Vec<AttributeValueSuggestionResponse>>,
    /// Echo the threshold we filtered with so the UI can render an
    /// accurate "suggestions are values observed on >= N users" caption
    /// without having to hard-code the same constant in two places.
    min_users: i64,
}

/// Suggestions are only surfaced once a value has been seen across at least
/// this many distinct users. Two is the floor that lets a value be "shared"
/// at all; a one-user observation would let an admin browsing this list
/// fish out the affiliation/entitlement of a specific person without ever
/// querying them by name. Bump if we ever want to be more conservative;
/// the UI shows this number to the admin so the contract stays honest.
const MIN_SUGGESTION_USERS: i64 = 2;

async fn list_role_rule_attribute_values(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<AttributeValuesResponse>, AppError> {
    require_admin(&user)?;

    let rows =
        minerva_db::queries::role_rule_attribute_observations::list_suggestions_above_threshold(
            &state.db,
            MIN_SUGGESTION_USERS,
        )
        .await?;

    let mut by_attribute: std::collections::BTreeMap<
        String,
        Vec<AttributeValueSuggestionResponse>,
    > = std::collections::BTreeMap::new();
    for row in rows {
        // Defensive filter: the SUPPORTED_ATTRIBUTES list is the contract
        // for what the rule engine reads. A stale observation row for an
        // attribute we no longer support (renamed header, etc.) would
        // surface a useless suggestion; drop it here instead of leaking
        // it to the UI.
        if !SUPPORTED_ATTRIBUTES.contains(&row.attribute.as_str()) {
            continue;
        }
        by_attribute
            .entry(row.attribute)
            .or_default()
            .push(AttributeValueSuggestionResponse {
                value: row.value,
                user_count: row.user_count,
            });
    }

    Ok(Json(AttributeValuesResponse {
        by_attribute,
        min_users: MIN_SUGGESTION_USERS,
    }))
}

// ── Classification backfill (admin-scoped) ─────────────────────────
//
// `GET /admin/classification-stats` lets the admin UI show the current
// state of classification coverage before they decide to backfill.
// `POST /admin/backfill-classifications` fans out the classifier across
// every eligible doc in a spawned task and returns immediately. The
// task respects `kind_locked_by_teacher` (defense in depth on top of
// the SQL filter) and skips docs whose status isn't `ready`.

#[derive(Serialize)]
struct ClassificationStatsResponse {
    total_ready: i64,
    classified: i64,
    unclassified: i64,
    locked_by_teacher: i64,
    /// Progress of the most recent admin backfill (or `None` if no
    /// backfill has run since the last server restart). Cleared when
    /// the next backfill kicks off; the UI uses this to show a
    /// progress bar with ok/errors/skipped counts ticking up in
    /// real time.
    backfill: Option<BackfillProgressResponse>,
}

#[derive(Serialize)]
struct BackfillProgressResponse {
    started_at: chrono::DateTime<chrono::Utc>,
    total: usize,
    ok: usize,
    errors: usize,
    skipped: usize,
    finished: bool,
}

async fn get_classification_stats(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<ClassificationStatsResponse>, AppError> {
    require_admin(&user)?;
    let stats = minerva_db::queries::documents::classification_stats(&state.db).await?;
    let backfill = state
        .backfill_tracker
        .snapshot()
        .map(|p| BackfillProgressResponse {
            started_at: p.started_at,
            total: p.total,
            ok: p.ok,
            errors: p.errors,
            skipped: p.skipped,
            finished: p.finished,
        });
    Ok(Json(ClassificationStatsResponse {
        total_ready: stats.total_ready,
        classified: stats.classified,
        unclassified: stats.unclassified,
        locked_by_teacher: stats.locked_by_teacher,
        backfill,
    }))
}

#[derive(Serialize)]
struct BackfillResponse {
    /// Number of docs the spawned task will work through. Refresh
    /// the stats endpoint to watch it tick down.
    queued: usize,
}

/// Hard cap on docs claimed in a single backfill invocation. Stops a
/// single click from spawning a runaway batch on a huge installation;
/// admin can re-click to drain another batch when the queue drains.
const BACKFILL_BATCH_LIMIT: i64 = 5_000;

async fn backfill_classifications(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<BackfillResponse>, AppError> {
    require_admin(&user)?;

    let candidates = minerva_db::queries::documents::list_needing_classification(
        &state.db,
        BACKFILL_BATCH_LIMIT,
    )
    .await?;
    let queued = candidates.len();

    if queued == 0 {
        tracing::info!("admin: backfill-classifications requested but queue is empty");
        return Ok(Json(BackfillResponse { queued }));
    }

    tracing::info!(
        "admin: backfill-classifications queued {} doc(s) (capped at {})",
        queued,
        BACKFILL_BATCH_LIMIT,
    );

    // Initialise progress tracker before spawning so the UI's first
    // poll sees the new backfill, not a stale "finished" state from
    // a previous run.
    state.backfill_tracker.start(queued);

    let state_clone = state.clone();
    tokio::spawn(async move {
        let mut ok = 0usize;
        let mut errs = 0usize;
        let mut touched_courses: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for doc in candidates {
            let course_id = doc.course_id;
            match crate::routes::documents::run_classify_one(&state_clone, &doc).await {
                Ok(Some(_)) => {
                    ok += 1;
                    touched_courses.insert(course_id);
                    state_clone.backfill_tracker.record_ok();
                }
                Ok(None) => {
                    // race: teacher locked between SELECT and now; skip silently
                    state_clone.backfill_tracker.record_skipped();
                }
                Err(e) => {
                    errs += 1;
                    state_clone.backfill_tracker.record_error();
                    tracing::warn!(
                        "admin: backfill doc {} ({}) failed: {:?}",
                        doc.id,
                        doc.filename,
                        e,
                    );
                }
            }
        }
        tracing::info!(
            "admin: backfill-classifications finished ({} ok, {} errors)",
            ok,
            errs,
        );

        // Hand each touched course to the relink sweeper rather than
        // running the linker inline. The sweeper picks them up on its
        // next tick and runs them sequentially, so we never burst many
        // concurrent linker calls at Cerebras when a backfill spans
        // courses.
        for course_id in touched_courses {
            state_clone
                .relink_scheduler
                .mark_dirty_immediate(course_id)
                .await;
        }
        state_clone.backfill_tracker.finish();
    });

    Ok(Json(BackfillResponse { queued }))
}

// ── Per-course feature flags (admin-managed) ───────────────────────
//
// We surface these as a generic toggle list (each known flag = a
// switch the admin can flip per course) so adding a new flag
// later is just appending to `feature_flags::ALL_FLAGS` and the UI
// renders an extra row. The admin UI submits the FULL desired flag
// state on PUT, mirroring how /admin/users/{id}/role works; avoids
// drift between an in-memory list and a stale DB row.

#[derive(Serialize)]
struct FeatureFlagStateResponse {
    /// Flag name (matches `feature_flags::ALL_FLAGS`).
    flag: String,
    /// Effective state for this course (course-scoped row > global
    /// row > compiled-in default = false).
    enabled: bool,
    /// Whether the course has its own row (vs inheriting from
    /// global/default). Lets the admin UI show a "set explicitly"
    /// indicator.
    course_override: bool,
}

#[derive(Serialize)]
struct CourseFeatureFlagsResponse {
    course_id: Uuid,
    flags: Vec<FeatureFlagStateResponse>,
}

async fn get_course_feature_flags(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<CourseFeatureFlagsResponse>, AppError> {
    require_admin(&user)?;

    // 404 if course doesn't exist (avoid leaking flags for a nonexistent id).
    if minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }

    let course_rows =
        minerva_db::queries::feature_flags::list_for_course(&state.db, course_id).await?;
    let course_overrides: std::collections::HashMap<String, bool> = course_rows
        .into_iter()
        .map(|r| (r.flag, r.enabled))
        .collect();

    let mut flags = Vec::with_capacity(crate::feature_flags::ALL_FLAGS.len());
    for &flag in crate::feature_flags::ALL_FLAGS {
        let course_override = course_overrides.contains_key(flag);
        // Resolve effective state through the same path the
        // application uses, so the admin UI cannot disagree with
        // runtime behaviour.
        let enabled = minerva_db::queries::feature_flags::is_enabled_for_course(
            &state.db, flag, course_id, false,
        )
        .await?;
        flags.push(FeatureFlagStateResponse {
            flag: flag.to_string(),
            enabled,
            course_override,
        });
    }

    Ok(Json(CourseFeatureFlagsResponse { course_id, flags }))
}

#[derive(Deserialize)]
struct SetCourseFeatureFlagsRequest {
    /// Map of flag-name -> desired state. Flags not in the map are
    /// left untouched; admin can selectively patch by sending only
    /// the changed entries.
    ///
    /// To revert a course back to the global default, set the value
    /// to `null` (which `serde` deserialises as `None`); the row is
    /// then deleted rather than overwritten with `false`.
    flags: std::collections::HashMap<String, Option<bool>>,
}

async fn set_course_feature_flags(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<SetCourseFeatureFlagsRequest>,
) -> Result<Json<CourseFeatureFlagsResponse>, AppError> {
    require_admin(&user)?;
    if minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }

    // Validate every flag the request mentions is one we recognise --
    // a typo from the admin UI shouldn't quietly persist a row whose
    // flag string nothing reads.
    for flag in body.flags.keys() {
        if !crate::feature_flags::ALL_FLAGS.contains(&flag.as_str()) {
            return Err(AppError::bad_request_with(
                "admin.feature_flag_unknown",
                [("flag", flag.clone())],
            ));
        }
    }

    for (flag, desired) in &body.flags {
        match desired {
            Some(enabled) => {
                minerva_db::queries::feature_flags::set(
                    &state.db,
                    flag,
                    minerva_db::queries::feature_flags::Scope::Course(course_id),
                    *enabled,
                )
                .await?;
            }
            None => {
                minerva_db::queries::feature_flags::delete(
                    &state.db,
                    flag,
                    minerva_db::queries::feature_flags::Scope::Course(course_id),
                )
                .await?;
            }
        }
    }

    // Reuse the GET handler's shape so the client can apply the
    // response directly without an extra round-trip.
    get_course_feature_flags(State(state), Extension(user), Path(course_id)).await
}

// ── Embedding model benchmarks (admin-scoped) ──────────────────────
//
// `GET /admin/embedding-models` returns the full whitelist with
// dimensions, latest benchmark (if any), and a `running` flag so the
// UI can grey out the buttons while a run is in flight.
//
// `POST /admin/embedding-benchmark` runs the benchmark for a single
// model and returns the result. `FastEmbedder::benchmark_one`
// internally `try_lock`s a serialization mutex; if a second admin
// click lands while the first is still running we map that to
// `admin.benchmark_busy` (400). Loading two heavy candle/ONNX models
// at once on the prod pod would OOM-kill us.

#[derive(Serialize)]
struct EmbeddingModelEntry {
    model: String,
    dimensions: u64,
    /// Latest benchmark result for this model, if it has been run since
    /// the server started. Boot-time `STARTUP_BENCHMARK_MODELS` are
    /// always populated; the rest are only populated after an admin
    /// runs them on demand.
    benchmark: Option<minerva_ingest::fastembed_embedder::BenchmarkResult>,
    /// True if this model is in the boot warmup set. The admin UI
    /// uses this purely as a hint; nothing depends on it server-side.
    warmed_at_startup: bool,
    /// Admin-managed picker policy. When false, teachers can't pick
    /// this model in the per-course config dropdown; courses already
    /// using it keep working (rotation requires admin or no model
    /// change). Backed by the `embedding_models` table.
    enabled: bool,
    /// True for the single model new courses are created with. Lifted
    /// out of the `courses` SQL DEFAULT so admins can swap it from the
    /// UI without a migration. Exactly one row in the response should
    /// carry this flag (enforced by a partial unique index server-side).
    is_default: bool,
    /// How many courses currently have this model selected. Surfaced so
    /// the admin can see the impact of disabling before they do it.
    /// Counted against `courses` filtered to `embedding_provider='local'`
    /// + `active=true`; archived courses don't count.
    courses_using: i64,
}

#[derive(Serialize)]
struct EmbeddingModelsResponse {
    models: Vec<EmbeddingModelEntry>,
    /// True while a benchmark is currently running. The frontend
    /// disables every "Run benchmark" button on the page when this is
    /// true to avoid a guaranteed-409 click.
    running: bool,
}

async fn list_embedding_models(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<EmbeddingModelsResponse>, AppError> {
    require_admin(&user)?;

    let benchmarks = state.fastembed.get_benchmarks().await;
    let lookup: std::collections::HashMap<
        &str,
        &minerva_ingest::fastembed_embedder::BenchmarkResult,
    > = benchmarks.iter().map(|b| (b.model.as_str(), b)).collect();

    let warm: std::collections::HashSet<&str> = minerva_ingest::pipeline::STARTUP_BENCHMARK_MODELS
        .iter()
        .map(|(name, _)| *name)
        .collect();

    // Pull the admin-managed enabled flags + is_default. Catalog
    // entries that somehow aren't in the DB yet (shouldn't happen
    // post-startup-sync, but defend anyway) default to
    // `enabled=false, is_default=false` in the response.
    let policy: std::collections::HashMap<String, (bool, bool)> =
        minerva_db::queries::embedding_models::list_all(&state.db)
            .await?
            .into_iter()
            .map(|r| (r.model, (r.enabled, r.is_default)))
            .collect();

    // Per-model usage counts. One scan over `courses` -> hashmap.
    // Filtered to active + local-provider rows: archived courses
    // wouldn't be re-embedded if the admin disabled the model anyway,
    // and openai-provider rows don't surface in the picker.
    let usage_rows = sqlx::query!(
        r#"SELECT embedding_model, COUNT(*)::BIGINT AS "count!"
           FROM courses
           WHERE active = true
             AND embedding_provider = 'local'
           GROUP BY embedding_model"#,
    )
    .fetch_all(&state.db)
    .await?;
    let usage: std::collections::HashMap<String, i64> = usage_rows
        .into_iter()
        .map(|r| (r.embedding_model, r.count))
        .collect();

    let models = minerva_ingest::pipeline::VALID_LOCAL_MODELS
        .iter()
        .map(|(name, dims)| {
            let (enabled, is_default) = policy.get(*name).copied().unwrap_or((false, false));
            EmbeddingModelEntry {
                model: (*name).to_string(),
                dimensions: *dims,
                benchmark: lookup.get(name).map(|b| (*b).clone()),
                warmed_at_startup: warm.contains(name),
                enabled,
                is_default,
                courses_using: usage.get(*name).copied().unwrap_or(0),
            }
        })
        .collect();

    Ok(Json(EmbeddingModelsResponse {
        models,
        running: state.fastembed.is_benchmark_running().await,
    }))
}

#[derive(Deserialize)]
struct UpdateEmbeddingModelRequest {
    /// Catalog model id. Carried in the body, not the URL, because
    /// HuggingFace ids contain forward slashes ("Qwen/Qwen3-...");
    /// axum's path router collapses %2F-encoded slashes back into
    /// segment boundaries, so a path-param route would silently 404
    /// on every multi-segment id. Body avoids the whole class of bug.
    model: String,
    enabled: bool,
}

#[derive(Serialize)]
struct UpdateEmbeddingModelResponse {
    model: String,
    enabled: bool,
}

/// Toggle the admin-managed `enabled` flag for one catalog model.
///
/// Disabling a model only affects future picker decisions: courses
/// already using it keep working until an admin force-migrates them
/// (which is just `PUT /courses/{id}` with a different model; admins
/// bypass the enabled check there). Returns 404 for ids the catalog
/// doesn't know about; 500 for ids that are catalog members but
/// missing the policy row (indicates a startup-sync bug, not a
/// user-facing error).
async fn update_embedding_model_enabled(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<UpdateEmbeddingModelRequest>,
) -> Result<Json<UpdateEmbeddingModelResponse>, AppError> {
    require_admin(&user)?;

    let in_catalog = minerva_ingest::pipeline::VALID_LOCAL_MODELS
        .iter()
        .any(|(name, _)| *name == body.model.as_str());
    if !in_catalog {
        return Err(AppError::NotFound);
    }

    let row =
        minerva_db::queries::embedding_models::set_enabled(&state.db, &body.model, body.enabled)
            .await?
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "embedding_models row missing for catalog entry {} (startup sync should have seeded it)",
                    body.model,
                ))
            })?;

    tracing::info!(
        "admin {} set embedding model {} enabled={}",
        user.id,
        row.model,
        row.enabled,
    );

    Ok(Json(UpdateEmbeddingModelResponse {
        model: row.model,
        enabled: row.enabled,
    }))
}

#[derive(Deserialize)]
struct SetDefaultEmbeddingModelRequest {
    /// Catalog model id to mark as the default for new courses. Carried
    /// in the body for the same reason as the enabled toggle: HF ids
    /// contain forward slashes and axum path-routing collapses them.
    /// Must already be `enabled = TRUE`; the route returns 400 with a
    /// friendly i18n code otherwise.
    model: String,
}

#[derive(Serialize)]
struct SetDefaultEmbeddingModelResponse {
    model: String,
    is_default: bool,
}

/// Promote one catalog model to be the default for new courses.
///
/// Atomicity is in `set_default`: the previous default's flag is
/// cleared and the new default is set in a single transaction so the
/// partial unique index never sees two `TRUE` rows.
///
/// Existing courses are not touched; they keep whatever embedding
/// model they were created with. This endpoint only affects the model
/// inserted by future `POST /courses` calls.
async fn set_default_embedding_model(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<SetDefaultEmbeddingModelRequest>,
) -> Result<Json<SetDefaultEmbeddingModelResponse>, AppError> {
    require_admin(&user)?;

    // Catalog membership check up front, same pattern as the enabled
    // toggle. Shaves a transaction-open + rollback off the 404 path.
    let in_catalog = minerva_ingest::pipeline::VALID_LOCAL_MODELS
        .iter()
        .any(|(name, _)| *name == body.model.as_str());
    if !in_catalog {
        return Err(AppError::NotFound);
    }

    let row = match minerva_db::queries::embedding_models::set_default(&state.db, &body.model).await
    {
        Ok(row) => row,
        Err(minerva_db::queries::embedding_models::SetDefaultError::NotFound) => {
            return Err(AppError::NotFound);
        }
        Err(minerva_db::queries::embedding_models::SetDefaultError::Disabled) => {
            return Err(AppError::bad_request_with(
                "admin.embedding_default_disabled",
                [("model", body.model.clone())],
            ));
        }
        Err(minerva_db::queries::embedding_models::SetDefaultError::Db(e)) => {
            return Err(AppError::from(e));
        }
    };

    tracing::info!(
        "admin {} set embedding model {} as default for new courses",
        user.id,
        row.model,
    );

    Ok(Json(SetDefaultEmbeddingModelResponse {
        model: row.model,
        is_default: row.is_default,
    }))
}

#[derive(Deserialize)]
struct RunBenchmarkRequest {
    /// HuggingFace-style model id, must be a member of
    /// `VALID_LOCAL_MODELS`. Anything else is rejected here rather
    /// than being passed through to fastembed.
    model: String,
}

#[derive(Serialize)]
struct RunBenchmarkResponse {
    result: minerva_ingest::fastembed_embedder::BenchmarkResult,
}

async fn run_embedding_benchmark(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<RunBenchmarkRequest>,
) -> Result<Json<RunBenchmarkResponse>, AppError> {
    require_admin(&user)?;

    // Look up dimensions from the whitelist; reject unknown ids
    // before paying the cost of a model load.
    let dimensions = minerva_ingest::pipeline::VALID_LOCAL_MODELS
        .iter()
        .find_map(|(n, d)| (*n == body.model).then_some(*d))
        .ok_or_else(|| {
            AppError::bad_request_with(
                "admin.embedding_model_unknown",
                [("model", body.model.clone())],
            )
        })?;

    match state.fastembed.benchmark_one(&body.model, dimensions).await {
        Ok(result) => Ok(Json(RunBenchmarkResponse { result })),
        Err(minerva_ingest::fastembed_embedder::BenchmarkError::Busy) => {
            Err(AppError::bad_request("admin.benchmark_busy"))
        }
        Err(minerva_ingest::fastembed_embedder::BenchmarkError::Failed(e)) => {
            // Failed loads (network/HF, candle init errors, …) are
            // surfaced as Internal so the operator looks at the logs;
            // we don't want to leak stack traces to the client.
            Err(AppError::Internal(format!(
                "embedding benchmark failed for {}: {}",
                body.model, e
            )))
        }
    }
}

// ============================================================
// System defaults: admin-tunable knobs that used to live in env
// vars or `pub const`s. Registry + typed accessors live in
// `crate::system_defaults`; this is the HTTP layer.
// ============================================================

#[derive(Serialize)]
struct SystemDefaultEntry {
    key: &'static str,
    category: crate::system_defaults::Category,
    label_key: &'static str,
    description_key: &'static str,
    kind: crate::system_defaults::KnobKind,
    /// Legacy env-var name (used to seed this row on a fresh install
    /// and shown to the admin as documentation). `None` for knobs
    /// that never had an env-var counterpart.
    env_var: Option<&'static str>,
    /// Hard-coded fallback ; what "Reset to default" sets the row to.
    fallback: serde_json::Value,
    /// Current effective value (DB row if present, fallback otherwise).
    value: serde_json::Value,
    /// `true` when a `system_defaults` row exists for this key. A
    /// `false` value means the response is showing the fallback;
    /// startup seeding should make this rare but the API surfaces it
    /// so the UI can distinguish "explicitly set" from "default".
    has_row: bool,
    /// Timestamp of the last UI edit; `None` when no row exists.
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
struct SystemDefaultsResponse {
    defaults: Vec<SystemDefaultEntry>,
}

/// Snapshot of the registry + each knob's current value. Cheap (one
/// `SELECT *` and a registry walk); not paginated since the table has
/// <50 rows for the foreseeable future.
async fn list_system_defaults(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<SystemDefaultsResponse>, AppError> {
    require_admin(&user)?;

    let rows = minerva_db::queries::system_defaults::list_all(&state.db).await?;
    let lookup: std::collections::HashMap<
        String,
        &minerva_db::queries::system_defaults::SystemDefaultRow,
    > = rows.iter().map(|r| (r.key.clone(), r)).collect();

    let mut defaults = Vec::new();
    for def in crate::system_defaults::registry() {
        let row = lookup.get(def.key);
        let (value, has_row, updated_at) = match row {
            Some(r) => (r.value.clone(), true, Some(r.updated_at)),
            None => (def.fallback.clone(), false, None),
        };
        defaults.push(SystemDefaultEntry {
            key: def.key,
            category: def.category,
            label_key: def.label_key,
            description_key: def.description_key,
            kind: def.kind,
            env_var: def.env_var,
            fallback: def.fallback,
            value,
            has_row,
            updated_at,
        });
    }

    Ok(Json(SystemDefaultsResponse { defaults }))
}

#[derive(Deserialize)]
struct UpdateSystemDefaultRequest {
    /// Registry key. Carried in the body, not the URL, for consistency
    /// with `PUT /admin/embedding-models` (keys may contain dots which
    /// path-routing can normalize unpredictably across reverse proxies).
    key: String,
    /// New JSON value. Validated against the registry's kind before
    /// being written.
    value: serde_json::Value,
}

#[derive(Serialize)]
struct UpdateSystemDefaultResponse {
    key: String,
    value: serde_json::Value,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// Set or update one system default. Validates against the registry
/// (type, range, enum membership) before writing; returns 400 with
/// an i18n code on validation failure, 404 if the key isn't in the
/// registry at all. The DB-side row is upserted; this is also the
/// path the UI uses to *initialize* a default that's still riding
/// the fallback (`has_row=false`).
async fn update_system_default(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<UpdateSystemDefaultRequest>,
) -> Result<Json<UpdateSystemDefaultResponse>, AppError> {
    require_admin(&user)?;

    let def = crate::system_defaults::find(&body.key).ok_or(AppError::NotFound)?;
    crate::system_defaults::validate(&def, &body.value).map_err(AppError::bad_request)?;

    let row = minerva_db::queries::system_defaults::set(&state.db, &body.key, &body.value).await?;

    tracing::info!(
        "admin {} set system_default `{}` = {}",
        user.id,
        row.key,
        row.value,
    );

    Ok(Json(UpdateSystemDefaultResponse {
        key: row.key,
        value: row.value,
        updated_at: row.updated_at,
    }))
}

/// Drop the row for one key; the next read falls back to env var (if
/// set, on the next startup) or hard-coded fallback. UI uses this as
/// "Reset to default". Returns 404 if the key isn't in the registry;
/// 200 with no body whether or not a row was actually deleted (idempotent).
async fn reset_system_default(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    let _ = crate::system_defaults::find(&key).ok_or(AppError::NotFound)?;

    let removed = minerva_db::queries::system_defaults::delete(&state.db, &key).await?;
    tracing::info!(
        "admin {} reset system_default `{}` (removed={})",
        user.id,
        key,
        removed,
    );

    Ok(Json(serde_json::json!({ "removed": removed })))
}
