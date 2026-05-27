use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use minerva_core::models::{User, UserRole};
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::AppError;
use crate::rules::{self, SUPPORTED_ATTRIBUTES};
use crate::state::AppState;

/// Promote the raw DB `UserRow` to the typed `User` model.
///
/// `auth_middleware` does this implicitly for every Shibboleth request,
/// but routes that authenticate out-of-band (e.g. embed/integration
/// token routes) need the same conversion to call shared helpers like
/// `Pseudonymizer::for_viewer`. Centralised here so the field list only
/// has to be maintained in one place; `From` impls aren't an option
/// because both types live in foreign crates (orphan rule).
pub(crate) fn user_from_row(row: minerva_db::queries::users::UserRow) -> User {
    User {
        id: row.id,
        eppn: row.eppn,
        display_name: row.display_name,
        role: UserRole::parse(&row.role),
        suspended: row.suspended,
        role_manually_set: row.role_manually_set,
        owner_daily_token_limit: row.owner_daily_token_limit,
        privacy_acknowledged_at: row.privacy_acknowledged_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Cookie attributes for clearing the external-auth cookie. Sent on 401
/// responses for revoked/expired ext: tokens to break the front-end's
/// reload-on-401 retry loop; otherwise the browser keeps re-presenting
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

    // Stash the (attribute, value) pairs we just saw so the admin UI can
    // suggest concrete values when authoring role-rule conditions. Failure
    // here is best-effort; it must not break auth. We log and move on.
    if let Err(e) = observe_rule_attributes(&state.db, user.id, &attrs).await {
        tracing::warn!(user = %user.id, error = %e, "failed to record rule-attribute observations");
    }

    request.extensions_mut().insert(user);

    Ok(next.run(request).await)
}

/// Persist every observed (attribute, value) pair for the user, splitting
/// multi-valued Shib headers on `;` so each atomic value is recorded
/// separately. Matches the semantics `contains` uses at evaluation time, so
/// any value the admin sees in the suggestion list will reliably match the
/// header it came from.
async fn observe_rule_attributes(
    db: &sqlx::PgPool,
    user_id: Uuid,
    attrs: &HashMap<String, String>,
) -> Result<(), sqlx::Error> {
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(attrs.len());
    for attr in SUPPORTED_ATTRIBUTES {
        let Some(raw) = attrs.get(*attr) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        // eppn / displayName are never semicolon-delimited in practice;
        // splitting still produces a single-element vector so the code path
        // stays uniform. Multi-valued attrs (affiliation, entitlement)
        // explode into one row per element.
        for piece in raw.split(';') {
            let trimmed = piece.trim();
            if trimmed.is_empty() {
                continue;
            }
            pairs.push(((*attr).to_string(), trimmed.to_string()));
        }
    }
    minerva_db::queries::role_rule_attribute_observations::observe_for_user(db, user_id, &pairs)
        .await
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
/// present but the backing invite is gone/revoked/expired; next request
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

    // Fetch existing row (or its owner via the alias table) to honor
    // manual lock + preserve current role when no rule promotes. Clamp
    // a stale stored Admin to Teacher when the user is no longer in
    // MINERVA_ADMINS (see decide_role docs).
    //
    // If the inbound eppn matched via the alias table (not the primary
    // `users.eppn`), promote it to primary BEFORE the upsert below.
    // Otherwise the `ON CONFLICT (eppn)` clause in `users::upsert`
    // wouldn't match (the inbound eppn isn't a primary yet) and we'd
    // INSERT a duplicate user row instead of refreshing the existing
    // one. Daisy staff profiles list every login a person has held;
    // this swap keeps the user-visible "current SU login" in sync with
    // whatever SAML hands us most recently.
    let existing = match minerva_db::queries::users::find_by_eppn_or_alias(&state.db, eppn).await? {
        Some((row, true)) => {
            minerva_db::queries::user_eppn_aliases::swap_primary_with_alias(
                &state.db, row.id, eppn,
            )
            .await?;
            // Re-fetch so the row's `eppn` field reflects the swap; the
            // upsert below will then ON CONFLICT cleanly. role /
            // role_manually_set / display_name / token-limit are stable
            // across the swap, so we could in principle just mutate the
            // local struct, but a re-fetch avoids divergence if the
            // swap helper ever grows extra side-effects.
            minerva_db::queries::users::find_by_id(&state.db, row.id).await?
        }
        Some((row, false)) => Some(row),
        None => None,
    };

    let existing_role = existing.as_ref().map(|r| UserRole::parse(&r.role));
    let role_locked = existing
        .as_ref()
        .map(|r| r.role_manually_set)
        .unwrap_or(false);

    let dev_teacher = state.config.dev_mode && eppn.starts_with("teacher");
    let rule_role = if is_admin || dev_teacher || role_locked {
        // Skip the rule eval; the higher-precedence branches in
        // decide_role will win regardless.
        None
    } else {
        // Cheap Arc snapshot; read lock dropped before evaluate. Rules
        // are pre-compiled (regexes included) by RuleCache.
        let snapshot = state.rules.snapshot().await;
        rules::evaluate(&snapshot, attrs)
    };

    let role = decide_role(is_admin, dev_teacher, role_locked, existing_role, rule_role);

    let row = minerva_db::queries::users::upsert(
        &state.db,
        Uuid::new_v4(),
        eppn,
        display_name,
        role.as_str(),
        crate::system_defaults::owner_daily_token_limit(&state.db).await,
    )
    .await?;

    Ok(user_from_row(row))
}

/// Pure role-decision dispatch, separated from `upsert_user` so the
/// precedence ordering (admin > dev-teacher > manual lock > additive merge
/// of stored + rule-derived) can be exercised without a DB or HTTP layer.
///
/// Ex-admin clamp: when `is_admin` is FALSE we strip Admin from
/// `existing_role` because MINERVA_ADMINS is the source of truth for admin
/// status. Otherwise, removing someone from the env would leave them admin
/// in the DB forever (rules can't fix that since they cap at Teacher).
fn decide_role(
    is_admin: bool,
    dev_teacher: bool,
    role_locked: bool,
    existing_role: Option<UserRole>,
    rule_role: Option<UserRole>,
) -> UserRole {
    let existing_role = existing_role.map(|r| if is_admin { r } else { r.clamp_below_admin() });

    if is_admin {
        UserRole::Admin
    } else if dev_teacher {
        UserRole::Teacher
    } else if role_locked {
        existing_role.unwrap_or(UserRole::Student)
    } else {
        match (existing_role, rule_role) {
            (Some(prev), Some(rr)) => UserRole::max(prev, rr),
            (Some(prev), None) => prev,
            (None, Some(rr)) => rr,
            (None, None) => UserRole::Student,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_allowlist_always_wins() {
        // Even if the stored role is Student and the user is locked, env
        // membership in MINERVA_ADMINS overrides everything.
        assert_eq!(
            decide_role(true, false, true, Some(UserRole::Student), None),
            UserRole::Admin,
        );
    }

    #[test]
    fn dev_teacher_shortcut() {
        assert_eq!(
            decide_role(false, true, false, None, None),
            UserRole::Teacher,
        );
    }

    #[test]
    fn manual_lock_preserves_existing_role_over_rules() {
        // Rule would promote to Teacher, but lock keeps Student.
        assert_eq!(
            decide_role(
                false,
                false,
                true,
                Some(UserRole::Student),
                Some(UserRole::Teacher)
            ),
            UserRole::Student,
        );
    }

    #[test]
    fn additive_merge_takes_higher_of_existing_and_rule() {
        assert_eq!(
            decide_role(
                false,
                false,
                false,
                Some(UserRole::Student),
                Some(UserRole::Teacher)
            ),
            UserRole::Teacher,
        );
        assert_eq!(
            decide_role(false, false, false, Some(UserRole::Teacher), None),
            UserRole::Teacher,
        );
        assert_eq!(
            decide_role(false, false, false, None, Some(UserRole::Teacher)),
            UserRole::Teacher,
        );
        assert_eq!(
            decide_role(false, false, false, None, None),
            UserRole::Student,
        );
    }

    #[test]
    fn ex_admin_clamped_to_teacher_when_removed_from_env() {
        // User was Admin in DB, env now says they're not. Clamp to Teacher.
        assert_eq!(
            decide_role(false, false, false, Some(UserRole::Admin), None),
            UserRole::Teacher,
        );
        // Same applies under the manual-lock branch: a previously-admin
        // user who was then locked still loses admin via the clamp.
        assert_eq!(
            decide_role(false, false, true, Some(UserRole::Admin), None),
            UserRole::Teacher,
        );
        // Active admin (env still has them) keeps Admin via the early
        // is_admin branch; the clamp on existing_role is irrelevant.
        assert_eq!(
            decide_role(true, false, false, Some(UserRole::Admin), None),
            UserRole::Admin,
        );
    }

    #[test]
    fn integrator_persists_across_logins() {
        // Integrator is granted either by an admin (lock=true) or by a rule
        // (lock=false). The lock branch returns the stored role unchanged,
        // and the ex-admin clamp must leave Integrator alone since it isn't
        // env-sourced.
        assert_eq!(
            decide_role(false, false, true, Some(UserRole::Integrator), None),
            UserRole::Integrator,
        );
        // Unlocked: a Teacher-target rule still leaves a stored Integrator
        // alone via the additive merge (max wins).
        assert_eq!(
            decide_role(
                false,
                false,
                false,
                Some(UserRole::Integrator),
                Some(UserRole::Teacher)
            ),
            UserRole::Integrator,
        );
        // Rules can now target Integrator directly; the merge picks it up
        // unchanged when there's no stored role to combine with.
        assert_eq!(
            decide_role(false, false, false, None, Some(UserRole::Integrator)),
            UserRole::Integrator,
        );
        // Env admin still wins over a stored Integrator.
        assert_eq!(
            decide_role(true, false, false, Some(UserRole::Integrator), None),
            UserRole::Admin,
        );
    }

    #[test]
    fn integrator_capabilities_and_rank() {
        assert!(UserRole::Integrator.can_manage_site_integrations());
        assert!(UserRole::Admin.can_manage_site_integrations());
        assert!(!UserRole::Teacher.can_manage_site_integrations());
        assert!(!UserRole::Student.can_manage_site_integrations());
        // Full teacher powers, but not admin.
        assert!(UserRole::Integrator.is_teacher_or_above());
        assert!(!UserRole::Integrator.is_admin());
        // Sits strictly between Teacher and Admin.
        assert!(UserRole::Integrator.rank() > UserRole::Teacher.rank());
        assert!(UserRole::Integrator.rank() < UserRole::Admin.rank());
        // Not env-sourced, so the ex-admin clamp leaves it intact.
        assert_eq!(
            UserRole::Integrator.clamp_below_admin(),
            UserRole::Integrator,
        );
        // Round-trips through string form.
        assert_eq!(UserRole::parse("integrator"), UserRole::Integrator);
        assert_eq!(UserRole::Integrator.as_str(), "integrator");
    }

    #[test]
    fn rule_cannot_promote_locked_user() {
        // Even an Admin-target rule (impossible at the API layer, but
        // belt-and-braces) can't move a locked Student.
        assert_eq!(
            decide_role(
                false,
                false,
                true,
                Some(UserRole::Student),
                Some(UserRole::Admin)
            ),
            UserRole::Student,
        );
    }
}
