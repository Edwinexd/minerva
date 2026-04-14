use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use minerva_core::models::{
    RoleRule, RoleRuleCondition, RoleRuleWithConditions, RuleOperator, User, UserRole,
};
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::AppError;
use crate::rules::{self, SUPPORTED_ATTRIBUTES};
use crate::state::AppState;

/// Cookie attributes for clearing the external-auth cookie. Sent on 401
/// responses for revoked/expired ext: tokens to break the front-end's
/// reload-on-401 retry loop -- otherwise the browser keeps re-presenting
/// the bad cookie and getting another 401.
const CLEAR_EXT_COOKIE: &str = "minerva_ext=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax";

/// Extracts user from Shibboleth headers set by Apache mod_shib (ShibUseHeaders On).
/// mod_shib sets `eppn` header with the eduPersonPrincipalName (e.g. edsu8469@su.se).
/// EPPN is lowercased before lookup so SU.SE / su.se map to the same user row.
///
/// In dev mode (MINERVA_DEV_MODE=true):
/// - Reads X-Dev-User header instead of eppn
/// - Falls back to first admin in MINERVA_ADMINS, or "dev@su.se"
pub async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let raw_eppn = if state.config.dev_mode {
        // Dev mode: X-Dev-User header, or fall back to first admin / default
        headers
            .get("X-Dev-User")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| {
                headers
                    .get("eppn")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| {
                state
                    .config
                    .admin_usernames
                    .first()
                    .map(|u| format!("{}@su.se", u))
                    .unwrap_or_else(|| "dev@su.se".to_string())
            })
    } else {
        headers
            .get("eppn")
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::Unauthorized)?
            .to_string()
    };

    // Normalize to lowercase so `alice@su.se` and `alice@SU.SE` resolve to
    // the same user row. Preserve the `ext:` prefix casing (it's a literal).
    let eppn = if let Some(rest) = raw_eppn.strip_prefix("ext:") {
        format!("ext:{}", rest.to_lowercase())
    } else {
        raw_eppn.to_lowercase()
    };

    let display_name = headers
        .get("displayName")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // External-auth users carry the `ext:` prefix and a JTI header (set by
    // Apache mod_lua after HMAC validation). Apache already verified the
    // signature; we additionally enforce per-invite revocation via the DB so
    // an admin can kill a single token without rotating the shared secret.
    //
    // Any failure here returns 401 *with the cookie cleared*. The frontend
    // reloads on 401 to recover from expired Shib sessions, so leaving the
    // bad cookie in place would just trigger an infinite loop.
    if eppn.starts_with("ext:") {
        let Some(jti) = headers
            .get("X-Minerva-Ext-Jti")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
        else {
            return Ok(unauthorized_clear_ext_cookie());
        };
        let invite = minerva_db::queries::external_auth::find_by_jti(&state.db, jti).await?;
        match invite {
            Some(inv)
                if inv.revoked_at.is_none()
                    && chrono::Utc::now() <= inv.expires_at
                    && inv.eppn == eppn => {}
            _ => return Ok(unauthorized_clear_ext_cookie()),
        }
    }

    // Snapshot every Shib header an admin can reference in a role rule.
    // External-auth and dev users typically only have `eppn` set; rules
    // that key on attrs they lack simply won't match.
    let attrs = collect_rule_attrs(&headers, &eppn, display_name.as_deref());

    let user = upsert_user(&state, &eppn, display_name.as_deref(), &attrs).await?;

    if user.suspended {
        return Err(AppError::Forbidden);
    }

    request.extensions_mut().insert(user);

    Ok(next.run(request).await)
}

fn collect_rule_attrs(
    headers: &HeaderMap,
    eppn: &str,
    display_name: Option<&str>,
) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(SUPPORTED_ATTRIBUTES.len());
    // Use the normalized eppn (already lowercased) so rule comparisons are
    // case-stable.
    out.insert("eppn".into(), eppn.to_string());
    if let Some(d) = display_name {
        out.insert("displayName".into(), d.to_string());
    }
    for attr in SUPPORTED_ATTRIBUTES {
        if *attr == "eppn" || *attr == "displayName" {
            continue;
        }
        if let Some(v) = headers.get(*attr).and_then(|v| v.to_str().ok()) {
            out.insert((*attr).to_string(), v.to_string());
        }
    }
    out
}

/// 401 response that also clears `minerva_ext`. Used when the cookie is
/// present but the backing invite is gone/revoked/expired -- next request
/// from the frontend's reload retry will not carry the bad cookie, so the
/// loop terminates (the user falls through to the Shib path).
fn unauthorized_clear_ext_cookie() -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, CLEAR_EXT_COOKIE.parse().unwrap());
    response
}

async fn upsert_user(
    state: &AppState,
    eppn: &str,
    display_name: Option<&str>,
    attrs: &HashMap<String, String>,
) -> Result<User, AppError> {
    let is_admin = state.config.is_admin(eppn);

    // Fetch existing row (if any) to honor manual lock + preserve current role
    // when no rule promotes.
    let existing = minerva_db::queries::users::find_by_eppn(&state.db, eppn).await?;
    let existing_role = existing.as_ref().map(|r| UserRole::parse(&r.role));
    let role_locked = existing
        .as_ref()
        .map(|r| r.role_manually_set)
        .unwrap_or(false);

    let role = if is_admin {
        // Admin allowlist always wins (escape hatch + survives manual lock).
        UserRole::Admin
    } else if state.config.dev_mode && eppn.starts_with("teacher") {
        UserRole::Teacher
    } else if role_locked {
        // Admin manually pinned this role -- rules cannot move it.
        existing_role.unwrap_or(UserRole::Student)
    } else {
        let rule_role = load_and_evaluate_rules(state, attrs).await?;
        match (existing_role, rule_role) {
            // Additive: take the higher of existing and rule-derived role.
            (Some(prev), Some(rr)) => max_role(prev, rr),
            (Some(prev), None) => prev,
            (None, Some(rr)) => rr,
            (None, None) => UserRole::Student,
        }
    };

    let row = minerva_db::queries::users::upsert(
        &state.db,
        Uuid::new_v4(),
        eppn,
        display_name,
        role.as_str(),
        state.config.default_owner_daily_token_limit,
    )
    .await?;

    Ok(User {
        id: row.id,
        eppn: row.eppn,
        display_name: row.display_name,
        role: UserRole::parse(&row.role),
        suspended: row.suspended,
        role_manually_set: row.role_manually_set,
        owner_daily_token_limit: row.owner_daily_token_limit,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn load_and_evaluate_rules(
    state: &AppState,
    attrs: &HashMap<String, String>,
) -> Result<Option<UserRole>, AppError> {
    let rule_rows = minerva_db::queries::role_rules::list_enabled(&state.db).await?;
    if rule_rows.is_empty() {
        return Ok(None);
    }
    let ids: Vec<Uuid> = rule_rows.iter().map(|r| r.id).collect();
    let cond_rows =
        minerva_db::queries::role_rules::list_conditions_for_rules(&state.db, &ids).await?;

    let mut by_rule: HashMap<Uuid, Vec<RoleRuleCondition>> = HashMap::new();
    for c in cond_rows {
        let Some(op) = RuleOperator::parse(&c.operator) else {
            continue;
        };
        by_rule
            .entry(c.rule_id)
            .or_default()
            .push(RoleRuleCondition {
                id: c.id,
                rule_id: c.rule_id,
                attribute: c.attribute,
                operator: op,
                value: c.value,
                created_at: c.created_at,
            });
    }

    let rules: Vec<RoleRuleWithConditions> = rule_rows
        .into_iter()
        .map(|r| RoleRuleWithConditions {
            conditions: by_rule.remove(&r.id).unwrap_or_default(),
            rule: RoleRule {
                id: r.id,
                name: r.name,
                target_role: UserRole::parse(&r.target_role),
                enabled: r.enabled,
                created_at: r.created_at,
                updated_at: r.updated_at,
            },
        })
        .collect();

    Ok(rules::evaluate(&rules, attrs))
}

fn max_role(a: UserRole, b: UserRole) -> UserRole {
    fn rank(r: UserRole) -> u8 {
        match r {
            UserRole::Student => 0,
            UserRole::Teacher => 1,
            UserRole::Admin => 2,
        }
    }
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}
