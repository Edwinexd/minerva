use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use minerva_core::models::{RuleOperator, User};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, LocalizedMessage};
use crate::rules::{validate_regex, SUPPORTED_ATTRIBUTES};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{id}/role", put(update_user_role))
        .route("/users/{id}/role-lock", delete(clear_role_lock))
        .route("/users/{id}/suspended", put(update_user_suspended))
        .route(
            "/users/{id}/owner-daily-cost-limit-usd",
            put(update_owner_daily_cost_limit_usd),
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
        // Admin course management: list (incl. archived), merge,
        // archive / restore. Distinct from the teacher-facing
        // `/courses` router so it can surface archived rows and the
        // admin-only merge.
        .route("/courses", get(list_all_courses))
        .route("/courses/merge-suggestions", get(merge_suggestions))
        .route("/courses/merge", post(merge_courses))
        .route("/courses/{id}/archive", post(archive_course))
        .route("/courses/{id}/unarchive", post(unarchive_course))
        // Bulk actions over a multi-selected set of courses: a settings
        // + feature-flag patch (`/bulk`), or a lifecycle action
        // (`/bulk-archive`, `/bulk-unarchive`). Each reports per-course
        // results so a partial batch is transparent.
        .route("/courses/bulk", post(bulk_update_courses))
        .route("/courses/bulk-archive", post(bulk_archive_courses))
        .route("/courses/bulk-unarchive", post(bulk_unarchive_courses))
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
            "/reranker-models",
            get(list_reranker_models).put(update_reranker_model_enabled),
        )
        .route("/reranker-models/default", put(set_default_reranker_model))
        .route("/reranker-benchmark", post(run_reranker_benchmark))
        .route(
            "/chat-models",
            get(list_chat_models).put(update_chat_model_enabled),
        )
        .route("/chat-models/default", put(set_default_chat_model))
        .route(
            "/chat-models/utility-default",
            put(set_utility_default_chat_model),
        )
        .route("/chat-models/price", put(set_chat_model_price))
        .route(
            "/chat-models/{model}/scrape-price",
            post(scrape_chat_model_price),
        )
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

/// All courses including archived ones, in the same wire shape as
/// `GET /courses` so the admin frontend can reuse the `Course` type.
/// The teacher-facing `/courses` route hides archived courses; admins
/// need to see them here to restore or merge them.
async fn list_all_courses(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<crate::routes::courses::CourseResponse>>, AppError> {
    require_admin(&user)?;
    let rows = minerva_db::queries::courses::list_all_including_archived(&state.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(crate::routes::courses::admin_course_response(&state.db, row).await);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct MergeCoursesRequest {
    /// The course that survives and absorbs the source's data.
    survivor_id: Uuid,
    /// The course whose data is moved into the survivor; archived after.
    source_id: Uuid,
}

/// Relocate a source course's document bytes on disk into the
/// survivor's directory. Documents live at
/// `{docs_path}/{course_id}/{doc_id}.{ext}`, so a merge that re-points
/// `course_id` in the DB must also move the files or the worker (which
/// rebuilds the path from `course_id`) can't find them.
///
/// We COPY (not move) so the bytes exist under both course-ids for the
/// duration of the merge transaction: before commit the docs still read
/// from the source dir, after commit from the survivor dir, and the
/// worker never sees a gap. The caller deletes the source dir after the
/// transaction commits. Doc ids are globally unique, so filenames never
/// clash in the destination.
async fn relocate_course_docs(
    docs_path: &str,
    source_id: Uuid,
    survivor_id: Uuid,
) -> Result<(), AppError> {
    let src_dir = format!("{}/{}", docs_path, source_id);
    let dst_dir = format!("{}/{}", docs_path, survivor_id);
    let mut entries = match tokio::fs::read_dir(&src_dir).await {
        Ok(rd) => rd,
        // Source course never had any uploads; nothing to relocate.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(AppError::Internal(format!(
                "merge: read source docs dir: {e}"
            )))
        }
    };
    tokio::fs::create_dir_all(&dst_dir)
        .await
        .map_err(|e| AppError::Internal(format!("merge: create survivor docs dir: {e}")))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| AppError::Internal(format!("merge: iterate source docs: {e}")))?
    {
        let file_type = entry
            .file_type()
            .await
            .map_err(|e| AppError::Internal(format!("merge: stat source doc: {e}")))?;
        if !file_type.is_file() {
            continue;
        }
        let dst = std::path::Path::new(&dst_dir).join(entry.file_name());
        tokio::fs::copy(entry.path(), &dst)
            .await
            .map_err(|e| AppError::Internal(format!("merge: copy doc file: {e}")))?;
    }
    Ok(())
}

/// Merge `source_id` into `survivor_id`: move all the source's data
/// (documents, conversations, members, Daisy offerings, integrations,
/// usage, ...) into the survivor, re-embed moved documents into the
/// survivor's vector space, then archive the source. Irreversible from
/// the UI's perspective (the source is archived, not deleted, but its
/// content now lives under the survivor).
async fn merge_courses(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<MergeCoursesRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    if body.survivor_id == body.source_id {
        return Err(AppError::bad_request("admin.merge_same_course"));
    }

    // Both must exist and be active (find_by_id filters archived).
    let survivor = minerva_db::queries::courses::find_by_id(&state.db, body.survivor_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let source = minerva_db::queries::courses::find_by_id(&state.db, body.source_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Copy the source's document bytes into the survivor's dir BEFORE
    // the DB transaction commits (see relocate_course_docs).
    relocate_course_docs(&state.config.docs_path, source.id, survivor.id).await?;

    let outcome =
        minerva_db::queries::courses::merge_courses(&state.db, survivor.id, source.id).await?;

    // The DB now points every moved doc at the survivor, so the source
    // dir is dead weight. Best-effort removal; a leftover dir is
    // harmless (nothing references it).
    let src_dir = format!("{}/{}", state.config.docs_path, source.id);
    if let Err(e) = tokio::fs::remove_dir_all(&src_dir).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                source = %source.id,
                error = %e,
                "merge: failed to remove source docs dir (non-fatal)",
            );
        }
    }

    tracing::info!(
        survivor = %survivor.id,
        source = %source.id,
        documents_moved = outcome.documents_moved,
        documents_orphaned = outcome.documents_orphaned,
        documents_requeued = outcome.documents_requeued,
        conversations_moved = outcome.conversations_moved,
        members_merged = outcome.members_merged,
        offerings_moved = outcome.offerings_moved,
        "admin merged course",
    );

    Ok(Json(serde_json::json!({
        "merged": true,
        "documents_moved": outcome.documents_moved,
        "documents_orphaned": outcome.documents_orphaned,
        "documents_requeued": outcome.documents_requeued,
        "conversations_moved": outcome.conversations_moved,
        "members_merged": outcome.members_merged,
        "offerings_moved": outcome.offerings_moved,
    })))
}

async fn archive_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let archived = minerva_db::queries::courses::archive(&state.db, id).await?;
    if !archived {
        // Either the course doesn't exist or it's already archived.
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "archived": true })))
}

async fn unarchive_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let restored = minerva_db::queries::courses::unarchive(&state.db, id).await?;
    if !restored {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "restored": true })))
}

#[derive(Serialize)]
struct MergeSuggestionCourse {
    id: Uuid,
    name: String,
    course_code: Option<String>,
    semester_label: Option<String>,
    owner_id: Uuid,
    auto_managed: bool,
}

#[derive(Serialize)]
struct MergeSuggestionGroup {
    /// Normalized core course code shared by every course in the group
    /// (section / campus / mode markers stripped), e.g. `PROLED` for
    /// PROLED (AB) / PROLED (CD) / PROLED-S, or `FODS` for MAR-FODS /
    /// FODS.
    code: String,
    /// The semester every course in the group belongs to; groups never
    /// span semesters. None only when all members predate semester
    /// tracking (and therefore only group with other unlabelled rows).
    semester_label: Option<String>,
    courses: Vec<MergeSuggestionCourse>,
}

/// Reduce a raw course code to its core for grouping. Strips
/// parenthetical / bracketed section markers (`PROLED (AB)` -> `PROLED`)
/// and known section / campus / delivery-mode tokens that mark a variant
/// rather than the course itself, from either end:
///   * `MAR-FODS` -> `FODS`  (campus prefix)
///   * `SUPCOM-DIST` / `SUPCOM-HI` -> `SUPCOM`  (delivery-mode suffix)
///   * `PROLED-S` -> `PROLED`
///
/// Returns None when there is no usable code. `SECTION_MARKERS` is the
/// one knob to extend as new DSV variant codes appear; deliberately
/// conservative so genuinely-distinct courses (e.g. `MAR-IP` vs
/// `INTROPROG`) are NOT collapsed together.
fn normalize_course_code(raw: &str) -> Option<String> {
    const SECTION_MARKERS: &[&str] = &["MAR", "DIST", "HI", "S", "AB", "CD"];

    // Drop anything inside (), [] or {} (group / section markers).
    let mut stripped = String::with_capacity(raw.len());
    let mut depth: i32 = 0;
    for ch in raw.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => stripped.push(ch),
            _ => {}
        }
    }

    let upper = stripped.trim().to_uppercase();
    if upper.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = upper
        .split(|c: char| c == '-' || c.is_whitespace())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let core: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|t| !SECTION_MARKERS.contains(t))
        .collect();
    // If every token was a marker (unusual), keep the full token list so
    // the code still groups with identical full codes rather than
    // collapsing to nothing.
    let core = if core.is_empty() { tokens } else { core };
    let joined = core.join("-");
    (!joined.is_empty()).then_some(joined)
}

/// Suggest groups of active courses that look like the same course
/// delivered under several codes: courses in the SAME semester whose
/// normalized core course codes match (see `normalize_course_code`).
/// Code-only and same-semester by design, so e.g. an HT and a VT
/// offering of PROLED are never lumped together, and a shared `MAR-`
/// campus prefix can't chain unrelated courses into one blob. Pure
/// heuristic; the admin still picks the survivor and confirms.
async fn merge_suggestions(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<MergeSuggestionGroup>>, AppError> {
    require_admin(&user)?;

    let courses: Vec<minerva_db::queries::courses::CourseRow> =
        minerva_db::queries::courses::list_all_including_archived(&state.db)
            .await?
            .into_iter()
            .filter(|c| c.active)
            .collect();

    // Bucket by (semester, normalized core code). Courses without a
    // course code are never suggested.
    let mut buckets: std::collections::HashMap<(Option<String>, String), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, c) in courses.iter().enumerate() {
        let Some(code) = c.course_code.as_deref().and_then(normalize_course_code) else {
            continue;
        };
        buckets
            .entry((c.semester_label.clone(), code))
            .or_default()
            .push(i);
    }

    let mut out: Vec<MergeSuggestionGroup> = Vec::new();
    for ((semester_label, code), members) in buckets {
        if members.len() < 2 {
            continue;
        }
        let mut group: Vec<MergeSuggestionCourse> = members
            .iter()
            .map(|&i| {
                let c = &courses[i];
                MergeSuggestionCourse {
                    id: c.id,
                    name: c.name.clone(),
                    course_code: c.course_code.clone(),
                    semester_label: c.semester_label.clone(),
                    owner_id: c.owner_id,
                    auto_managed: c.auto_managed,
                }
            })
            .collect();
        group.sort_by(|a, b| {
            a.course_code
                .cmp(&b.course_code)
                .then_with(|| a.name.cmp(&b.name))
        });
        out.push(MergeSuggestionGroup {
            code,
            semester_label,
            courses: group,
        });
    }

    // Largest groups first, then by code, so the listing is stable.
    out.sort_by(|a, b| {
        b.courses
            .len()
            .cmp(&a.courses.len())
            .then_with(|| a.code.cmp(&b.code))
    });

    Ok(Json(out))
}

#[cfg(test)]
mod merge_suggestion_tests {
    use super::normalize_course_code;

    #[test]
    fn strips_section_and_campus_markers() {
        assert_eq!(
            normalize_course_code("PROLED (AB)").as_deref(),
            Some("PROLED")
        );
        assert_eq!(
            normalize_course_code("PROLED (CD)").as_deref(),
            Some("PROLED")
        );
        assert_eq!(normalize_course_code("PROLED-S").as_deref(), Some("PROLED"));
        assert_eq!(
            normalize_course_code("SUPCOM-HI").as_deref(),
            Some("SUPCOM")
        );
        assert_eq!(
            normalize_course_code("SUPCOM-DIST").as_deref(),
            Some("SUPCOM")
        );
        assert_eq!(normalize_course_code("MAR-FODS").as_deref(), Some("FODS"));
        assert_eq!(normalize_course_code("MAR-NLP").as_deref(), Some("NLP"));
    }

    #[test]
    fn keeps_distinct_codes_distinct() {
        // Same name, different real codes: must NOT collapse together.
        assert_eq!(normalize_course_code("MAR-IP").as_deref(), Some("IP"));
        assert_eq!(
            normalize_course_code("INTROPROG").as_deref(),
            Some("INTROPROG")
        );
        assert_ne!(
            normalize_course_code("MAR-IP"),
            normalize_course_code("INTROPROG")
        );
    }

    #[test]
    fn empty_or_markers_only() {
        assert_eq!(normalize_course_code("").as_deref(), None);
        assert_eq!(normalize_course_code("  ()  ").as_deref(), None);
        // All-marker code falls back to the full token list.
        assert_eq!(normalize_course_code("MAR").as_deref(), Some("MAR"));
    }
}

#[derive(Serialize)]
struct UserResponse {
    id: Uuid,
    eppn: String,
    display_name: Option<String>,
    role: String,
    suspended: bool,
    role_manually_set: bool,
    owner_daily_cost_limit_usd: rust_decimal::Decimal,
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
                owner_daily_cost_limit_usd: r.owner_daily_cost_limit_usd,
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
    /// Per-owner daily spending cap in USD. 0 = unlimited.
    limit: rust_decimal::Decimal,
}

async fn update_owner_daily_cost_limit_usd(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateOwnerLimitRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    // Sanity ceiling on the per-owner daily USD cap: a footgun guard
    // against admin typos, far above any realistic spend.
    let owner_limit_max = rust_decimal::Decimal::from(1_000_000);
    if body.limit < rust_decimal::Decimal::ZERO {
        return Err(AppError::bad_request("admin.limit_negative"));
    }
    if body.limit > owner_limit_max {
        return Err(AppError::bad_request_with(
            "admin.limit_too_large",
            [("max", owner_limit_max.to_string())],
        ));
    }
    let updated =
        minerva_db::queries::users::update_owner_daily_cost_limit_usd(&state.db, id, body.limit)
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

// ── Bulk course actions (admin) ────────────────────────────────────
//
// The admin courses table lets an admin multi-select courses and apply
// one change to all of them: a settings patch (any subset of the
// editable course knobs), a feature-flag patch, or a lifecycle action
// (archive / restore). Each course is processed independently and the
// response reports per-course success / failure, so a partial batch
// (e.g. one course sitting on a now-disabled model) is fully
// transparent rather than failing the whole request. Settings changes
// route through the SAME `apply_course_update` path as
// `PUT /courses/{id}`, so every model-capability / embedding / reranker
// validation applies per course; admins bypass the picker-disabled
// gates exactly as they do via the per-course force-migrate dialog.

/// Sanity cap on how many courses one bulk call may touch. Guards
/// against a runaway request (and, for embedding changes, against
/// queuing a re-embed storm). Comfortably above any realistic course
/// count an admin would multi-select.
const BULK_MAX_COURSES: usize = 500;

#[derive(Deserialize, Default)]
struct BulkCoursePatch {
    // `name` / `description` are intentionally absent: they're per-course
    // identity, and setting them identically across a selection is never
    // what an admin wants. Everything else `apply_course_update` accepts
    // is bulk-settable.
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
    reranker_model: Option<String>,
    daily_cost_limit_usd: Option<rust_decimal::Decimal>,
    semester_label: Option<String>,
}

impl BulkCoursePatch {
    /// True when no settings field is present. A bulk call may carry only
    /// feature-flag changes, in which case we skip the
    /// `apply_course_update` round-trip (and its potential re-embed)
    /// entirely.
    fn is_empty(&self) -> bool {
        self.context_ratio.is_none()
            && self.temperature.is_none()
            && self.model.is_none()
            && self.system_prompt.is_none()
            && self.max_chunks.is_none()
            && self.min_score.is_none()
            && self.strategy.is_none()
            && self.tool_use_enabled.is_none()
            && self.embedding_provider.is_none()
            && self.embedding_model.is_none()
            && self.reranker_model.is_none()
            && self.daily_cost_limit_usd.is_none()
            && self.semester_label.is_none()
    }
}

#[derive(Deserialize)]
struct BulkUpdateCoursesRequest {
    course_ids: Vec<Uuid>,
    #[serde(default)]
    patch: BulkCoursePatch,
    /// flag-name -> Some(enabled) to set a course override, None (null)
    /// to revert to the global default. Flags absent from the map are
    /// left untouched. Same semantics as the per-course feature-flags PUT.
    #[serde(default)]
    feature_flags: std::collections::HashMap<String, Option<bool>>,
}

#[derive(Serialize)]
struct BulkResultItem {
    course_id: Uuid,
    ok: bool,
    /// Present only when `ok == false`; carries the same translatable
    /// `{code, params}` the single-course route would have returned, so
    /// the admin UI can render a precise per-course reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<LocalizedMessage>,
}

#[derive(Serialize)]
struct BulkResponse {
    succeeded: usize,
    failed: usize,
    results: Vec<BulkResultItem>,
}

fn validate_bulk_ids(ids: &[Uuid]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Err(AppError::bad_request("admin.bulk_no_courses"));
    }
    if ids.len() > BULK_MAX_COURSES {
        return Err(AppError::bad_request_with(
            "admin.bulk_too_many",
            [("max", BULK_MAX_COURSES.to_string())],
        ));
    }
    Ok(())
}

/// Apply the settings patch + feature-flag patch to one course. Returns
/// a per-course `AppError` (not bubbled) so the caller can record it
/// against this `course_id` and keep going.
async fn apply_bulk_one(
    state: &AppState,
    course_id: Uuid,
    patch: &BulkCoursePatch,
    feature_flags: &std::collections::HashMap<String, Option<bool>>,
) -> Result<(), AppError> {
    // Only active courses are editable (the underlying UPDATE is scoped
    // to active = true). Report archived / missing ids as a clear
    // per-course failure rather than silently skipping them.
    let existing = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or_else(|| AppError::bad_request("admin.bulk_course_not_active"))?;

    if !patch.is_empty() {
        // `name` / `description` stay None (not bulk-settable). All
        // validation and any embedding rotation happen inside
        // `apply_course_update`, identical to PUT /courses/{id}; we pass
        // is_admin = true (the route gated on `require_admin`) so a
        // disabled-picker model can still be targeted.
        let fields = crate::routes::courses::CourseUpdateFields {
            name: None,
            description: None,
            context_ratio: patch.context_ratio,
            temperature: patch.temperature,
            model: patch.model.clone(),
            system_prompt: patch.system_prompt.clone(),
            max_chunks: patch.max_chunks,
            min_score: patch.min_score,
            strategy: patch.strategy.clone(),
            tool_use_enabled: patch.tool_use_enabled,
            embedding_provider: patch.embedding_provider.clone(),
            embedding_model: patch.embedding_model.clone(),
            reranker_model: patch.reranker_model.clone(),
            daily_cost_limit_usd: patch.daily_cost_limit_usd,
            semester_label: patch.semester_label.clone(),
        };
        crate::routes::courses::apply_course_update(state, &existing, fields, true).await?;
    }

    for (flag, desired) in feature_flags {
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

    Ok(())
}

async fn bulk_update_courses(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<BulkUpdateCoursesRequest>,
) -> Result<Json<BulkResponse>, AppError> {
    require_admin(&user)?;
    validate_bulk_ids(&body.course_ids)?;

    // Reject unknown flag names for the WHOLE request up front (a typo
    // shouldn't apply to some courses and then report per-course); same
    // guard as `set_course_feature_flags`.
    for flag in body.feature_flags.keys() {
        if !crate::feature_flags::ALL_FLAGS.contains(&flag.as_str()) {
            return Err(AppError::bad_request_with(
                "admin.feature_flag_unknown",
                [("flag", flag.clone())],
            ));
        }
    }

    let mut results = Vec::with_capacity(body.course_ids.len());
    for &course_id in &body.course_ids {
        match apply_bulk_one(&state, course_id, &body.patch, &body.feature_flags).await {
            Ok(()) => results.push(BulkResultItem {
                course_id,
                ok: true,
                error: None,
            }),
            Err(e) => results.push(BulkResultItem {
                course_id,
                ok: false,
                error: Some(LocalizedMessage::from_app_error(&e)),
            }),
        }
    }

    let succeeded = results.iter().filter(|r| r.ok).count();
    let failed = results.len() - succeeded;
    tracing::info!(
        "admin {} bulk-updated {} course(s): {} ok, {} failed",
        user.id,
        results.len(),
        succeeded,
        failed,
    );
    Ok(Json(BulkResponse {
        succeeded,
        failed,
        results,
    }))
}

#[derive(Deserialize)]
struct BulkIdsRequest {
    course_ids: Vec<Uuid>,
}

async fn bulk_archive_courses(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<BulkIdsRequest>,
) -> Result<Json<BulkResponse>, AppError> {
    require_admin(&user)?;
    validate_bulk_ids(&body.course_ids)?;

    let mut results = Vec::with_capacity(body.course_ids.len());
    for &course_id in &body.course_ids {
        // A real DB failure bubbles as a 500 (it'd fail every course
        // anyway); `Ok(false)` is the benign "already archived / gone"
        // no-op, surfaced per-course.
        let changed = minerva_db::queries::courses::archive(&state.db, course_id).await?;
        results.push(BulkResultItem {
            course_id,
            ok: changed,
            error: if changed {
                None
            } else {
                Some(LocalizedMessage::new("admin.bulk_archive_noop"))
            },
        });
    }

    let succeeded = results.iter().filter(|r| r.ok).count();
    let failed = results.len() - succeeded;
    tracing::info!(
        "admin {} bulk-archived: {} ok, {} no-op",
        user.id,
        succeeded,
        failed,
    );
    Ok(Json(BulkResponse {
        succeeded,
        failed,
        results,
    }))
}

async fn bulk_unarchive_courses(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<BulkIdsRequest>,
) -> Result<Json<BulkResponse>, AppError> {
    require_admin(&user)?;
    validate_bulk_ids(&body.course_ids)?;

    let mut results = Vec::with_capacity(body.course_ids.len());
    for &course_id in &body.course_ids {
        let changed = minerva_db::queries::courses::unarchive(&state.db, course_id).await?;
        results.push(BulkResultItem {
            course_id,
            ok: changed,
            error: if changed {
                None
            } else {
                Some(LocalizedMessage::new("admin.bulk_unarchive_noop"))
            },
        });
    }

    let succeeded = results.iter().filter(|r| r.ok).count();
    let failed = results.len() - succeeded;
    tracing::info!(
        "admin {} bulk-unarchived: {} ok, {} no-op",
        user.id,
        succeeded,
        failed,
    );
    Ok(Json(BulkResponse {
        succeeded,
        failed,
        results,
    }))
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
    benchmark: Option<minerva_core::rpc::EmbedBenchmarkResult>,
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
    let lookup: std::collections::HashMap<&str, &minerva_core::rpc::EmbedBenchmarkResult> =
        benchmarks.iter().map(|b| (b.model.as_str(), b)).collect();

    let warm: std::collections::HashSet<&str> = minerva_catalog::STARTUP_BENCHMARK_MODELS
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

    let models = minerva_catalog::VALID_LOCAL_MODELS
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

    let in_catalog = minerva_catalog::VALID_LOCAL_MODELS
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
    let in_catalog = minerva_catalog::VALID_LOCAL_MODELS
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

// ── Chat / utility model catalog (admin) ───────────────────────────

/// Convert a stored `NUMERIC` rate to `f64` for the JSON wire. Storage
/// and cost computation stay `Decimal`; only the API boundary to the
/// JS frontend (whose numbers are f64 anyway) downcasts. `None` (unknown
/// price) passes through as JSON null so the UI can render an "unpriced"
/// badge distinct from a typed `0` (free).
fn rate_to_f64(d: Option<rust_decimal::Decimal>) -> Option<f64> {
    use rust_decimal::prelude::ToPrimitive;
    d.and_then(|v| v.to_f64())
}

#[derive(Serialize)]
struct ChatModelEntry {
    model: String,
    provider: String,
    display_name: String,
    enabled: bool,
    is_default: bool,
    is_utility_default: bool,
    /// USD per 1M tokens. `null` = unknown (unusable; cannot be enabled);
    /// `0.0` = genuinely free (on-prem, usable).
    input_usd_per_mtok: Option<f64>,
    output_usd_per_mtok: Option<f64>,
    supports_logprobs: bool,
    supports_tool_use: bool,
    /// Whether the provider this model belongs to has a configured key
    /// in the runtime registry. A model whose provider key is absent
    /// cannot be enabled (the UI greys out the toggle).
    provider_available: bool,
    /// How many active courses currently select this chat model.
    courses_using: i64,
    price_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
struct ChatModelsResponse {
    models: Vec<ChatModelEntry>,
}

/// List the chat-model catalog with admin policy, prices, provider
/// availability, and per-model course usage. Mirrors
/// `list_embedding_models`; adds provider + price columns.
async fn list_chat_models(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<ChatModelsResponse>, AppError> {
    require_admin(&user)?;

    // Per-model usage counts across active courses. The `courses.model`
    // column holds the chat model id.
    let usage_rows = sqlx::query!(
        r#"SELECT model, COUNT(*)::BIGINT AS "count!"
           FROM courses
           WHERE active = true
           GROUP BY model"#,
    )
    .fetch_all(&state.db)
    .await?;
    let usage: std::collections::HashMap<String, i64> =
        usage_rows.into_iter().map(|r| (r.model, r.count)).collect();

    let rows = minerva_db::queries::chat_models::list_all(&state.db).await?;
    let models = rows
        .into_iter()
        .map(|r| ChatModelEntry {
            provider_available: state.llm.has(&r.provider),
            courses_using: usage.get(&r.model).copied().unwrap_or(0),
            input_usd_per_mtok: rate_to_f64(r.input_usd_per_mtok),
            output_usd_per_mtok: rate_to_f64(r.output_usd_per_mtok),
            model: r.model,
            provider: r.provider,
            display_name: r.display_name,
            enabled: r.enabled,
            is_default: r.is_default,
            is_utility_default: r.is_utility_default,
            supports_logprobs: r.supports_logprobs,
            supports_tool_use: r.supports_tool_use,
            price_updated_at: r.price_updated_at,
        })
        .collect();

    Ok(Json(ChatModelsResponse { models }))
}

#[derive(Deserialize)]
struct UpdateChatModelRequest {
    model: String,
    enabled: bool,
}

#[derive(Serialize)]
struct UpdateChatModelResponse {
    model: String,
    enabled: bool,
}

/// Toggle the admin-managed `enabled` flag for one chat model. Enabling
/// is guarded twice: the model's provider must have a configured key
/// (`chat_model.provider_unavailable`), and both USD rates must be known
/// (`chat_model.price_required`; `0` is allowed, NULL is not). Disabling
/// is always allowed.
async fn update_chat_model_enabled(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<UpdateChatModelRequest>,
) -> Result<Json<UpdateChatModelResponse>, AppError> {
    require_admin(&user)?;

    let row = minerva_db::queries::chat_models::find(&state.db, &body.model)
        .await?
        .ok_or(AppError::NotFound)?;

    if body.enabled {
        if !state.llm.has(&row.provider) {
            return Err(AppError::bad_request_with(
                "chat_model.provider_unavailable",
                [("provider", row.provider.clone())],
            ));
        }
        if row.input_usd_per_mtok.is_none() || row.output_usd_per_mtok.is_none() {
            return Err(AppError::bad_request_with(
                "chat_model.price_required",
                [("model", row.model.clone())],
            ));
        }
    }

    let updated =
        minerva_db::queries::chat_models::set_enabled(&state.db, &body.model, body.enabled)
            .await?
            .ok_or(AppError::NotFound)?;

    tracing::info!(
        "admin {} set chat model {} enabled={}",
        user.id,
        updated.model,
        updated.enabled,
    );

    Ok(Json(UpdateChatModelResponse {
        model: updated.model,
        enabled: updated.enabled,
    }))
}

#[derive(Deserialize)]
struct SetChatModelDefaultRequest {
    model: String,
}

#[derive(Serialize)]
struct SetChatModelDefaultResponse {
    model: String,
}

/// Promote one chat model to the course-chat default for new courses.
/// The target must be enabled. Atomic in `set_default`.
async fn set_default_chat_model(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<SetChatModelDefaultRequest>,
) -> Result<Json<SetChatModelDefaultResponse>, AppError> {
    require_admin(&user)?;
    let row = map_set_default(
        minerva_db::queries::chat_models::set_default(&state.db, &body.model).await,
        &body.model,
    )?;
    tracing::info!("admin {} set chat model {} as course default", user.id, row);
    Ok(Json(SetChatModelDefaultResponse { model: row }))
}

/// Promote one chat model to the utility default (classification / KG /
/// aegis / suggested-questions). The target must be enabled.
async fn set_utility_default_chat_model(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<SetChatModelDefaultRequest>,
) -> Result<Json<SetChatModelDefaultResponse>, AppError> {
    require_admin(&user)?;
    let row = map_set_default(
        minerva_db::queries::chat_models::set_utility_default(&state.db, &body.model).await,
        &body.model,
    )?;
    tracing::info!(
        "admin {} set chat model {} as utility default",
        user.id,
        row
    );
    Ok(Json(SetChatModelDefaultResponse { model: row }))
}

/// Map a `chat_models::set_default` / `set_utility_default` result to the
/// model id or an `AppError`, sharing the NotFound / Disabled / Db arms.
fn map_set_default(
    result: Result<
        minerva_db::queries::chat_models::ChatModelRow,
        minerva_db::queries::chat_models::SetDefaultError,
    >,
    model: &str,
) -> Result<String, AppError> {
    use minerva_db::queries::chat_models::SetDefaultError;
    match result {
        Ok(row) => Ok(row.model),
        Err(SetDefaultError::NotFound) => Err(AppError::NotFound),
        Err(SetDefaultError::Disabled) => Err(AppError::bad_request_with(
            "chat_model.default_disabled",
            [("model", model.to_string())],
        )),
        Err(SetDefaultError::Db(e)) => Err(AppError::from(e)),
    }
}

#[derive(Deserialize)]
struct SetChatModelPriceRequest {
    model: String,
    /// USD per 1M tokens. `0` is valid (free / on-prem). Negative is
    /// rejected. Both are required (no way to set an unknown price back
    /// to NULL through this route once a real number is entered).
    input_usd_per_mtok: rust_decimal::Decimal,
    output_usd_per_mtok: rust_decimal::Decimal,
}

/// Set a chat model's USD rates and stamp `price_updated_at`. This is
/// what makes an unpriced model usable (enabling separately requires
/// both rates known). `0` is accepted (genuinely free); negative is not.
async fn set_chat_model_price(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<SetChatModelPriceRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    if body.input_usd_per_mtok < rust_decimal::Decimal::ZERO
        || body.output_usd_per_mtok < rust_decimal::Decimal::ZERO
    {
        return Err(AppError::bad_request("chat_model.price_negative"));
    }
    let row = minerva_db::queries::chat_models::set_price(
        &state.db,
        &body.model,
        body.input_usd_per_mtok,
        body.output_usd_per_mtok,
        None,
    )
    .await?
    .ok_or(AppError::NotFound)?;

    tracing::info!(
        "admin {} set chat model {} price in={} out={}",
        user.id,
        row.model,
        body.input_usd_per_mtok,
        body.output_usd_per_mtok,
    );

    Ok(Json(serde_json::json!({
        "model": row.model,
        "input_usd_per_mtok": rate_to_f64(row.input_usd_per_mtok),
        "output_usd_per_mtok": rate_to_f64(row.output_usd_per_mtok),
        "price_updated_at": row.price_updated_at,
    })))
}

/// Best-effort "scrape price" helper: fetch the model's provider public
/// pricing page and ask the utility model to extract the rates. Returns
/// a suggestion only; nothing is persisted (the admin reviews + saves
/// via the price PUT above).
async fn scrape_chat_model_price(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(model): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let row = minerva_db::queries::chat_models::find(&state.db, &model)
        .await?
        .ok_or(AppError::NotFound)?;
    let Some(pricing_url) = minerva_catalog::provider_pricing_url(&row.provider) else {
        return Err(AppError::bad_request_with(
            "chat_model.no_pricing_source",
            [("provider", row.provider.clone())],
        ));
    };
    let suggestion = crate::classification::pricing_scrape::scrape_price(
        &state.llm,
        &state.db,
        &state.http_client,
        &model,
        pricing_url,
    )
    .await
    .map_err(AppError::Internal)?;

    serde_json::to_value(suggestion)
        .map(Json)
        .map_err(|e| AppError::Internal(e.to_string()))
}

// ── Re-ranker model catalog (admin) ────────────────────────────────

#[derive(Serialize)]
struct RerankerModelEntry {
    model: String,
    /// Admin-managed picker policy. When false, teachers can't pick this
    /// model in the per-course config dropdown; courses already on it
    /// keep working. Backed by the `reranker_models` table.
    enabled: bool,
    /// True for the single model new courses are created with. Exactly
    /// one row in the response carries this (partial unique index).
    is_default: bool,
    /// How many active courses currently select this re-ranker. Surfaced
    /// so the admin can see the impact of disabling before they do it.
    /// No provider filter (re-ranking applies regardless of how a course
    /// embeds).
    courses_using: i64,
    /// Latest benchmark result for this model, or null if it hasn't been
    /// run since the server started. Populated on demand by the admin
    /// "Run benchmark" button.
    benchmark: Option<minerva_core::rpc::RerankBenchmarkResult>,
}

#[derive(Serialize)]
struct RerankerModelsResponse {
    models: Vec<RerankerModelEntry>,
    /// True while a benchmark is running. The frontend disables every
    /// "Run benchmark" button on the page when this is true.
    running: bool,
}

/// List the re-ranker catalog with admin policy + usage counts +
/// latest benchmark. Mirrors `list_embedding_models`; the re-ranker has
/// no dimensions column, and its benchmark metric is pairs/sec.
async fn list_reranker_models(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<RerankerModelsResponse>, AppError> {
    require_admin(&user)?;

    let benchmarks = state.reranker.get_benchmarks().await;
    let bench_lookup: std::collections::HashMap<&str, &minerva_core::rpc::RerankBenchmarkResult> =
        benchmarks.iter().map(|b| (b.model.as_str(), b)).collect();

    let policy: std::collections::HashMap<String, (bool, bool)> =
        minerva_db::queries::reranker_models::list_all(&state.db)
            .await?
            .into_iter()
            .map(|r| (r.model, (r.enabled, r.is_default)))
            .collect();

    let usage_rows = sqlx::query!(
        r#"SELECT reranker_model, COUNT(*)::BIGINT AS "count!"
           FROM courses
           WHERE active = true
           GROUP BY reranker_model"#,
    )
    .fetch_all(&state.db)
    .await?;
    let usage: std::collections::HashMap<String, i64> = usage_rows
        .into_iter()
        .map(|r| (r.reranker_model, r.count))
        .collect();

    let models = minerva_catalog::VALID_RERANKER_MODELS
        .iter()
        .map(|name| {
            let (enabled, is_default) = policy.get(*name).copied().unwrap_or((false, false));
            RerankerModelEntry {
                model: (*name).to_string(),
                enabled,
                is_default,
                courses_using: usage.get(*name).copied().unwrap_or(0),
                benchmark: bench_lookup.get(name).map(|b| (*b).clone()),
            }
        })
        .collect();

    Ok(Json(RerankerModelsResponse {
        models,
        running: state.reranker.is_benchmark_running().await,
    }))
}

#[derive(Deserialize)]
struct UpdateRerankerModelRequest {
    /// Catalog model id. In the body, not the URL, because the ids
    /// contain forward slashes (`jinaai/...`) that axum path-routing
    /// collapses.
    model: String,
    enabled: bool,
}

#[derive(Serialize)]
struct UpdateRerankerModelResponse {
    model: String,
    enabled: bool,
}

/// Toggle the admin-managed `enabled` flag for one re-ranker catalog
/// model. Disabling only affects future picker decisions; courses
/// already on it keep working (and switching re-ranker has no re-embed
/// cost anyway).
async fn update_reranker_model_enabled(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<UpdateRerankerModelRequest>,
) -> Result<Json<UpdateRerankerModelResponse>, AppError> {
    require_admin(&user)?;

    let in_catalog = minerva_catalog::VALID_RERANKER_MODELS.contains(&body.model.as_str());
    if !in_catalog {
        return Err(AppError::NotFound);
    }

    let row =
        minerva_db::queries::reranker_models::set_enabled(&state.db, &body.model, body.enabled)
            .await?
            .ok_or_else(|| {
                AppError::Internal(format!(
            "reranker_models row missing for catalog entry {} (startup sync should have seeded it)",
            body.model,
        ))
            })?;

    tracing::info!(
        "admin {} set reranker model {} enabled={}",
        user.id,
        row.model,
        row.enabled,
    );

    Ok(Json(UpdateRerankerModelResponse {
        model: row.model,
        enabled: row.enabled,
    }))
}

#[derive(Deserialize)]
struct SetDefaultRerankerModelRequest {
    model: String,
}

#[derive(Serialize)]
struct SetDefaultRerankerModelResponse {
    model: String,
    is_default: bool,
}

/// Promote one catalog re-ranker to the default for new courses. Atomic
/// flip (see `set_default`); existing courses keep their current choice.
async fn set_default_reranker_model(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<SetDefaultRerankerModelRequest>,
) -> Result<Json<SetDefaultRerankerModelResponse>, AppError> {
    require_admin(&user)?;

    let in_catalog = minerva_catalog::VALID_RERANKER_MODELS.contains(&body.model.as_str());
    if !in_catalog {
        return Err(AppError::NotFound);
    }

    let row = match minerva_db::queries::reranker_models::set_default(&state.db, &body.model).await
    {
        Ok(row) => row,
        Err(minerva_db::queries::reranker_models::SetDefaultError::NotFound) => {
            return Err(AppError::NotFound);
        }
        Err(minerva_db::queries::reranker_models::SetDefaultError::Disabled) => {
            return Err(AppError::bad_request_with(
                "admin.reranker_default_disabled",
                [("model", body.model.clone())],
            ));
        }
        Err(minerva_db::queries::reranker_models::SetDefaultError::Db(e)) => {
            return Err(AppError::from(e));
        }
    };

    tracing::info!(
        "admin {} set reranker model {} as default for new courses",
        user.id,
        row.model,
    );

    Ok(Json(SetDefaultRerankerModelResponse {
        model: row.model,
        is_default: row.is_default,
    }))
}

#[derive(Deserialize)]
struct RunRerankerBenchmarkRequest {
    /// Catalog re-ranker id. In the body, not the URL (the ids contain
    /// forward slashes that axum path-routing collapses).
    model: String,
}

#[derive(Serialize)]
struct RunRerankerBenchmarkResponse {
    result: minerva_core::rpc::RerankBenchmarkResult,
}

/// Benchmark one re-ranker model (pairs/sec). Reuses the same
/// `admin.benchmark_busy` soft error the embedding benchmark uses; a
/// failed model load surfaces as Internal so the operator checks logs.
async fn run_reranker_benchmark(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<RunRerankerBenchmarkRequest>,
) -> Result<Json<RunRerankerBenchmarkResponse>, AppError> {
    require_admin(&user)?;

    let in_catalog = minerva_catalog::VALID_RERANKER_MODELS.contains(&body.model.as_str());
    if !in_catalog {
        return Err(AppError::NotFound);
    }

    match state.reranker.benchmark_one(&body.model).await {
        Ok(result) => Ok(Json(RunRerankerBenchmarkResponse { result })),
        Err(minerva_core::rpc::BenchmarkError::Busy) => {
            Err(AppError::bad_request("admin.benchmark_busy"))
        }
        Err(minerva_core::rpc::BenchmarkError::Failed(e)) => Err(AppError::Internal(format!(
            "reranker benchmark failed for {}: {}",
            body.model, e
        ))),
    }
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
    result: minerva_core::rpc::EmbedBenchmarkResult,
}

async fn run_embedding_benchmark(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<RunBenchmarkRequest>,
) -> Result<Json<RunBenchmarkResponse>, AppError> {
    require_admin(&user)?;

    // Look up dimensions from the whitelist; reject unknown ids
    // before paying the cost of a model load.
    let dimensions = minerva_catalog::VALID_LOCAL_MODELS
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
        Err(minerva_core::rpc::BenchmarkError::Busy) => {
            Err(AppError::bad_request("admin.benchmark_busy"))
        }
        Err(minerva_core::rpc::BenchmarkError::Failed(e)) => {
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
