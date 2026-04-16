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

    // Only allow setting to teacher or student (not admin)
    if body.role != "teacher" && body.role != "student" {
        return Err(AppError::bad_request("admin.role_invalid"));
    }

    // Sets role_manually_set=true so subsequent rule evaluations leave the
    // user alone -- admin choice wins until they call /role-lock DELETE.
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
    // Admin promotion via rules is intentionally disallowed -- admins must
    // be in MINERVA_ADMINS so the env stays the source of truth.
    if role != "teacher" && role != "student" {
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
