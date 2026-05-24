//! LTI Advantage Names and Role Provisioning Service (NRPS) client + the
//! roster reconcile that backs automatic add/remove of course members.
//!
//! Flow (per syncable context, see `lti_nrps_contexts`):
//!   1. Mint an RS256 `client_assertion` JWT with the tool's LTI key.
//!   2. Exchange it at the platform's OAuth2 token endpoint
//!      (`client_credentials` grant) for an access token scoped to the
//!      NRPS contextmembership.readonly scope.
//!   3. GET the `context_memberships_url` (paginated via the `Link` header)
//!      with that token, asking for the NRPS membership container media type.
//!   4. Reconcile: provision Active members (mirroring the launch handler's
//!      identity + role-suggestion behaviour) and remove members that this
//!      context previously provisioned but that have since left the roster.
//!
//! Removal is "LTI-sourced only": the reconcile loop only ever removes a
//! course member it can find a provenance row for in `lti_nrps_memberships`,
//! and never the course owner. Shibboleth direct-login users and
//! manually-added members are therefore untouched.

use std::collections::{HashMap, HashSet};

use jsonwebtoken::{encode, Algorithm, Header};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::AppState;
use minerva_db::queries::lti_nrps::NrpsContextRow;

/// OAuth2 scope required to read a context's membership roster.
const NRPS_SCOPE: &str =
    "https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly";
/// Media type the platform must serve the membership container as.
const MEMBERSHIP_ACCEPT: &str = "application/vnd.ims.lti-nrps.v2.membershipcontainer+json";
/// `client_assertion_type` value mandated by the LTI security framework.
const CLIENT_ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
/// Defensive cap so a misbehaving platform paginating in a loop can't spin
/// the reconcile forever.
const MAX_PAGES: usize = 50;

#[derive(Debug, Clone)]
pub struct SyncOutcome {
    pub added: i32,
    pub removed: i32,
    /// Actionable warning surfaced even on success. `None` for a clean run;
    /// `Some(text)` when the sync revealed an LMS-side misconfiguration the
    /// admin should fix (e.g. identity claims absent for every active member
    /// in the roster).
    pub warning: Option<String>,
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ClientAssertionClaims {
    iss: String,
    sub: String,
    aud: String,
    iat: i64,
    exp: i64,
    jti: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct MembershipContainer {
    #[serde(default)]
    members: Vec<Member>,
}

#[derive(Deserialize)]
struct Member {
    /// Platform-side stable user id (the launch `sub`).
    user_id: String,
    /// Membership status. Absent is treated as Active per the NRPS spec.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    /// Per-member LTI message payload; carries the custom params (notably
    /// `user_eppn`) that we use for identity resolution, exactly like a
    /// launch JWT's custom claim.
    #[serde(default)]
    message: Vec<MemberMessage>,
}

#[derive(Deserialize)]
struct MemberMessage {
    #[serde(rename = "https://purl.imsglobal.org/spec/lti/claim/custom", default)]
    custom: Option<HashMap<String, serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// Token + fetch
// ---------------------------------------------------------------------------

/// How far to backdate `iat` to absorb clock drift between this tool and the
/// LTI platform. Moodle (and other firebase/php-jwt-based platforms) reject a
/// client_assertion the instant our `iat` lands even slightly in their future,
/// since the library's default leeway is zero. 60 seconds is the same buffer
/// used by AWS, Google and the IMS reference tool; well inside our 5 minute
/// `exp` window.
const IAT_CLOCK_SKEW_TOLERANCE_SECS: i64 = 60;

pub(crate) fn mint_client_assertion(
    state: &AppState,
    client_id: &str,
    token_url: &str,
) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp();
    let claims = ClientAssertionClaims {
        iss: client_id.to_string(),
        sub: client_id.to_string(),
        aud: token_url.to_string(),
        iat: now - IAT_CLOCK_SKEW_TOLERANCE_SECS,
        // Short-lived; the platform only needs it for the token exchange.
        exp: now + 300,
        jti: Uuid::new_v4().to_string(),
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(state.lti.kid.clone());
    Ok(encode(&header, &claims, &state.lti.encoding_key)?)
}

async fn fetch_access_token(
    state: &AppState,
    token_url: &str,
    client_id: &str,
) -> anyhow::Result<String> {
    let assertion = mint_client_assertion(state, client_id, token_url)?;
    // application/x-www-form-urlencoded body, built by hand (reqwest's `.form`
    // helper isn't compiled in this workspace's feature set).
    let body = format!(
        "grant_type=client_credentials\
         &client_assertion_type={}\
         &client_assertion={}\
         &scope={}",
        urlencoding::encode(CLIENT_ASSERTION_TYPE),
        urlencoding::encode(&assertion),
        urlencoding::encode(NRPS_SCOPE),
    );
    let resp = state
        .http_client
        .post(token_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("token endpoint {} returned {}: {}", token_url, status, body);
    }
    let tok: TokenResponse = resp.json().await?;
    Ok(tok.access_token)
}

/// Fetch every page of the membership container, following `Link: rel="next"`.
async fn fetch_all_members(
    state: &AppState,
    memberships_url: &str,
    access_token: &str,
) -> anyhow::Result<Vec<Member>> {
    let mut out = Vec::new();
    let mut next = Some(memberships_url.to_string());
    let mut pages = 0;
    while let Some(url) = next.take() {
        pages += 1;
        if pages > MAX_PAGES {
            anyhow::bail!("NRPS pagination exceeded {} pages", MAX_PAGES);
        }
        let resp = state
            .http_client
            .get(&url)
            .bearer_auth(access_token)
            .header(reqwest::header::ACCEPT, MEMBERSHIP_ACCEPT)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("NRPS endpoint {} returned {}: {}", url, status, body);
        }
        let next_link = resp
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_next_link);
        let container: MembershipContainer = resp.json().await?;
        out.extend(container.members);
        // Guard against a platform that returns its own URL as `next`.
        next = next_link.filter(|n| n != &url);
    }
    Ok(out)
}

/// Extract the `rel="next"` target from an RFC 5988 `Link` header.
fn parse_next_link(header: &str) -> Option<String> {
    for part in header.split(',') {
        let part = part.trim();
        if part.contains("rel=\"next\"") || part.contains("rel=next") {
            let start = part.find('<')?;
            let end = part.find('>')?;
            if start < end {
                return Some(part[start + 1..end].to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Identity resolution (mirrors the launch handler)
// ---------------------------------------------------------------------------

/// Resolve a member's Minerva eppn the same way the launch handler does:
///   a) custom `user_eppn` param, b) email claim, c) synthetic
///      `lti_<source_id>_<sub>`.
/// Returns `(eppn, is_claimed)` where `is_claimed` is false for the synthetic
/// fallback (which is trivially distinct from any real eppn and so is exempt
/// from the platform's eppn-domain allowlist, matching the launch path).
fn resolve_member_eppn(m: &Member, source_identifier: &str) -> (String, bool) {
    let claimed = m
        .message
        .iter()
        .find_map(|msg| {
            msg.custom
                .as_ref()
                .and_then(|c| c.get("user_eppn"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| m.email.clone());
    match claimed {
        Some(e) => (e.to_lowercase(), true),
        None => (
            format!("lti_{}_{}", source_identifier, m.user_id).to_lowercase(),
            false,
        ),
    }
}

fn eppn_in_allowlist(allowed: &Option<Vec<String>>, eppn: &str) -> bool {
    let Some(domains) = allowed.as_ref() else {
        return true;
    };
    if domains.is_empty() {
        return true;
    }
    domains
        .iter()
        .any(|d| eppn.ends_with(&format!("@{}", d.to_lowercase())))
}

// ---------------------------------------------------------------------------
// Reconcile
// ---------------------------------------------------------------------------

/// Per-source connection details the reconcile needs.
struct SourceConfig {
    client_id: String,
    token_url: String,
    /// Used to build the synthetic eppn fallback; matches the launch
    /// handler's `ResolvedSource::identifier()` (the source row's UUID).
    source_identifier: String,
    /// Platform-only eppn allowlist (None for per-course registrations).
    allowed_eppn_domains: Option<Vec<String>>,
}

async fn resolve_source_config(
    state: &AppState,
    ctx: &NrpsContextRow,
) -> anyhow::Result<SourceConfig> {
    match (ctx.registration_id, ctx.platform_id) {
        (Some(rid), None) => {
            let r = minerva_db::queries::lti::find_registration_by_id(&state.db, rid)
                .await?
                .ok_or_else(|| anyhow::anyhow!("registration {} not found", rid))?;
            Ok(SourceConfig {
                client_id: r.client_id,
                token_url: r.auth_token_url,
                source_identifier: r.id.to_string(),
                allowed_eppn_domains: None,
            })
        }
        (None, Some(pid)) => {
            let p = minerva_db::queries::lti::find_platform_by_id(&state.db, pid)
                .await?
                .ok_or_else(|| anyhow::anyhow!("platform {} not found", pid))?;
            Ok(SourceConfig {
                client_id: p.client_id,
                token_url: p.auth_token_url,
                source_identifier: p.id.to_string(),
                allowed_eppn_domains: p.allowed_eppn_domains,
            })
        }
        _ => anyhow::bail!("nrps context {} has no source", ctx.id),
    }
}

/// Pull the roster for one NRPS context and reconcile it against Minerva
/// course membership. Returns the number of members added / removed.
pub async fn reconcile_context(
    state: &AppState,
    ctx: &NrpsContextRow,
) -> anyhow::Result<SyncOutcome> {
    let db = &state.db;

    // Archived/deleted course: nothing to reconcile. (A hard delete cascades
    // the context row away; archive just flips active=false.)
    let course = match minerva_db::queries::courses::find_by_id(db, ctx.course_id).await? {
        Some(c) => c,
        None => {
            return Ok(SyncOutcome {
                added: 0,
                removed: 0,
                warning: None,
            })
        }
    };

    let cfg = resolve_source_config(state, ctx).await?;

    let token = fetch_access_token(state, &cfg.token_url, &cfg.client_id).await?;
    let members = fetch_all_members(state, &ctx.memberships_url, &token).await?;

    let mut added = 0i32;
    let mut active_user_ids: HashSet<Uuid> = HashSet::new();
    // Track how many Active members we processed and how many of those
    // fell through to the synthetic-eppn fallback (no `user_eppn` custom
    // claim AND no `email`). When ALL active members are synthetic, the
    // LMS has identity-sharing locked down and the admin needs to flip
    // the relevant tool-privacy switches; we surface this as a warning
    // independent of the sync's success/error status below.
    let mut active_count: i32 = 0;
    let mut synthetic_count: i32 = 0;

    for m in &members {
        // Absent status means Active per spec; anything else means the user
        // is no longer an active participant and should not be provisioned.
        if m.status.as_deref().unwrap_or("Active") != "Active" {
            continue;
        }
        active_count += 1;

        let (eppn, is_claimed) = resolve_member_eppn(m, &cfg.source_identifier);
        if !is_claimed {
            synthetic_count += 1;
        }
        // A real (claimed) eppn must satisfy the platform's allowlist, same
        // as on launch; the synthetic fallback is exempt.
        if is_claimed && !eppn_in_allowlist(&cfg.allowed_eppn_domains, &eppn) {
            continue;
        }

        let (user, _) = minerva_db::queries::users::find_or_create_by_eppn(
            db,
            &eppn,
            m.name.as_deref(),
            "student",
            state.config.default_owner_daily_token_limit,
        )
        .await?;
        active_user_ids.insert(user.id);

        let existing =
            minerva_db::queries::courses::get_member_role(db, ctx.course_id, user.id).await?;
        if existing.is_none() {
            minerva_db::queries::courses::add_member(db, ctx.course_id, user.id, "student").await?;
            added += 1;
        }

        // Teacher elevation is a suggestion only; NRPS can't promote directly,
        // mirroring `apply_course_membership` on the launch path.
        if crate::lti::lti_roles_to_course_role(&m.roles) == "teacher"
            && existing.as_deref() != Some("teacher")
        {
            let detail = serde_json::json!({ "lti_roles": m.roles, "source": "nrps" });
            let _ = minerva_db::queries::role_suggestions::upsert_pending(
                db,
                Uuid::new_v4(),
                ctx.course_id,
                user.id,
                "teacher",
                "lti",
                Some(&detail),
            )
            .await?;
        }

        minerva_db::queries::lti_nrps::upsert_membership(db, ctx.id, user.id, &m.user_id, "Active")
            .await?;
    }

    // Removal pass: any member this context previously provisioned that is no
    // longer in the Active roster. Guarded so we never drop the owner or a
    // user still active via another context bound to the same course.
    let mut removed = 0i32;
    for row in minerva_db::queries::lti_nrps::list_memberships(db, ctx.id).await? {
        if active_user_ids.contains(&row.user_id) {
            continue;
        }
        if row.user_id == course.owner_id {
            // Owner left the LMS roster: drop the stale provenance row but
            // keep their Minerva ownership intact.
            minerva_db::queries::lti_nrps::delete_membership(db, ctx.id, row.user_id).await?;
            continue;
        }
        if minerva_db::queries::lti_nrps::user_active_in_other_context(
            db,
            ctx.course_id,
            row.user_id,
            ctx.id,
        )
        .await?
        {
            // Still enrolled via a sibling context bound to this course;
            // forget this context's claim but keep the membership.
            minerva_db::queries::lti_nrps::delete_membership(db, ctx.id, row.user_id).await?;
            continue;
        }
        if minerva_db::queries::courses::remove_member(db, ctx.course_id, row.user_id).await? {
            removed += 1;
        }
        minerva_db::queries::lti_nrps::delete_membership(db, ctx.id, row.user_id).await?;
    }

    // Identity-sharing health check. Fires only on a non-empty roster where
    // EVERY active member fell through to the synthetic-eppn fallback (so
    // we're confident this is platform-side privacy lockdown, not a per-user
    // hole). A partial population is left alone: that's a different problem
    // (a specific member missing fields), not a tool-config one.
    let warning = if active_count > 0 && synthetic_count == active_count {
        Some(format!(
            "The LMS did not share identity claims for any of the {} active member(s) in this roster (no `name`, `email`, or `user_eppn` custom claim). Members were added with synthetic ids and will NOT match the same person if they ever log in directly via Shibboleth. To fix: in the LMS tool settings, enable identity sharing for this tool. In Moodle: External tool > Privacy > set 'Share launcher's name with tool' and 'Share launcher's email with tool' to 'Always'. The setup instructions at /admin/lti/setup document this.",
            active_count
        ))
    } else {
        None
    };

    Ok(SyncOutcome {
        added,
        removed,
        warning,
    })
}

// ---------------------------------------------------------------------------
// Platform health probe (orphan-LMS detection)
// ---------------------------------------------------------------------------

/// Probe a platform's token endpoint with a throwaway `client_credentials`
/// JWT. Used by the daily platform-health sweep to detect when the LMS
/// has deleted our registration on its side (so we can clean up our row
/// after a grace period). Returns the bucketed status string
/// `record_platform_health` expects:
///   * `ok`              -> token endpoint accepted the assertion
///   * `invalid_client`  -> platform rejected the client_id (orphan)
///   * `http_<code>`     -> other non-2xx (transient or platform bug)
///   * `network`         -> request didn't complete (LMS down, DNS, etc.)
///   * `parse_error`     -> response body wasn't parseable
///
/// We DO NOT use the access token here: the health probe only needs to
/// know whether the platform still recognises us. Same scope the NRPS
/// reconcile would ask for, since restricting to a scope the platform
/// won't grant could itself trigger `invalid_client` on some platforms;
/// using the always-valid NRPS scope keeps the probe a faithful proxy
/// for "could a real NRPS sync succeed if we ran it now."
pub async fn probe_platform_health(
    state: &AppState,
    platform: &minerva_db::queries::lti::PlatformRow,
) -> String {
    let assertion =
        match mint_client_assertion(state, &platform.client_id, &platform.auth_token_url) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    "lti health: mint failed for platform {}: {}",
                    platform.id,
                    e
                );
                return "parse_error".into();
            }
        };
    let body = format!(
        "grant_type=client_credentials&client_assertion_type={}&client_assertion={}&scope={}",
        urlencoding::encode(CLIENT_ASSERTION_TYPE),
        urlencoding::encode(&assertion),
        urlencoding::encode(NRPS_SCOPE),
    );
    let resp = state
        .http_client
        .post(&platform.auth_token_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(_) => return "network".into(),
    };
    let status = resp.status();
    if status.is_success() {
        return "ok".into();
    }
    // Try to read the OAuth2 error code. Per RFC 6749 the response body
    // is `{"error": "invalid_client", ...}`. Some platforms also use 401
    // without a body; treat those as `invalid_client` too because there
    // is no other realistic interpretation of "token endpoint refused
    // our client_credentials".
    let text = resp.text().await.unwrap_or_default();
    let parsed: Option<serde_json::Value> = serde_json::from_str(&text).ok();
    let err_code = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if err_code == "invalid_client"
        || (status.as_u16() == 401 && err_code.is_empty())
        || (status.as_u16() == 404 && err_code.is_empty())
    {
        return "invalid_client".into();
    }
    format!("http_{}", status.as_u16())
}
