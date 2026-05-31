//! LTI 1.3 core logic: RSA key management, JWKS, JWT validation, claim parsing.

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::collections::HashMap;

use crate::error::AppError;

// `LtiKeyPair` (RSA key material + JWKS) moved to the axum-free
// `minerva-app-core::lti`; re-exported so `crate::lti::LtiKeyPair`
// keeps resolving for NRPS minting and AppState.
pub use minerva_app_core::lti::LtiKeyPair;

// ---------------------------------------------------------------------------
// LTI 1.3 JWT claims
// ---------------------------------------------------------------------------

/// The full set of claims in an LTI 1.3 launch JWT (id_token).
#[derive(Debug, Deserialize)]
pub struct LtiLaunchClaims {
    pub iss: String,
    pub sub: String,
    pub aud: AudClaim,
    pub exp: u64,
    pub iat: u64,
    pub nonce: String,

    pub name: Option<String>,
    pub email: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/message_type",
        default
    )]
    pub message_type: Option<String>,

    #[serde(rename = "https://purl.imsglobal.org/spec/lti/claim/version", default)]
    pub version: Option<String>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/deployment_id",
        default
    )]
    pub deployment_id: Option<String>,

    #[serde(rename = "https://purl.imsglobal.org/spec/lti/claim/roles", default)]
    pub roles: Vec<String>,

    #[serde(rename = "https://purl.imsglobal.org/spec/lti/claim/context", default)]
    pub context: Option<LtiContext>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/resource_link",
        default
    )]
    pub resource_link: Option<LtiResourceLink>,

    #[serde(rename = "https://purl.imsglobal.org/spec/lti/claim/custom", default)]
    pub custom: Option<HashMap<String, serde_json::Value>>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/launch_presentation",
        default
    )]
    pub launch_presentation: Option<LtiLaunchPresentation>,

    /// LTI Advantage Names and Role Provisioning Service. Present only when
    /// the platform has the NRPS service enabled for this tool; carries the
    /// `context_memberships_url` we later poll to reconcile the roster.
    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti-nrps/claim/namesroleservice",
        default
    )]
    pub names_role_service: Option<LtiNamesRoleService>,
}

/// The `aud` claim can be a single string or an array of strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AudClaim {
    Single(String),
    Multi(Vec<String>),
}

impl AudClaim {
    pub fn contains(&self, value: &str) -> bool {
        match self {
            AudClaim::Single(s) => s == value,
            AudClaim::Multi(v) => v.iter().any(|s| s == value),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LtiContext {
    pub id: Option<String>,
    pub label: Option<String>,
    pub title: Option<String>,
    #[serde(rename = "type", default)]
    pub context_type: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct LtiResourceLink {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LtiLaunchPresentation {
    pub document_target: Option<String>,
    pub return_url: Option<String>,
    pub locale: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LtiNamesRoleService {
    pub context_memberships_url: String,
    #[serde(default)]
    pub service_versions: Vec<String>,
}

// ---------------------------------------------------------------------------
// JWT validation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<serde_json::Value>,
}

/// The subset of platform metadata validate_launch_jwt actually needs.
/// Implemented for both `RegistrationRow` (per-course setup) and `PlatformRow`
/// (site-level setup) so the launch handler doesn't care which it's running.
pub struct PlatformConfig<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub platform_jwks_url: &'a str,
}

impl<'a> From<&'a minerva_db::queries::lti::RegistrationRow> for PlatformConfig<'a> {
    fn from(r: &'a minerva_db::queries::lti::RegistrationRow) -> Self {
        Self {
            issuer: &r.issuer,
            client_id: &r.client_id,
            platform_jwks_url: &r.platform_jwks_url,
        }
    }
}

impl<'a> From<&'a minerva_db::queries::lti::PlatformRow> for PlatformConfig<'a> {
    fn from(p: &'a minerva_db::queries::lti::PlatformRow) -> Self {
        Self {
            issuer: &p.issuer,
            client_id: &p.client_id,
            platform_jwks_url: &p.platform_jwks_url,
        }
    }
}

/// Validate an LTI 1.3 launch id_token JWT against a platform's config
/// (either a per-course registration or a site-level platform).
pub async fn validate_launch_jwt(
    platform: PlatformConfig<'_>,
    id_token: &str,
    expected_nonce: &str,
    http_client: &reqwest::Client,
) -> Result<LtiLaunchClaims, AppError> {
    // 1. Decode JWT header to get kid.
    let header = decode_header(id_token).map_err(|e| {
        AppError::bad_request_with("lti.jwt_header_invalid", [("detail", e.to_string())])
    })?;
    let kid = header.kid.as_deref();

    // 2. Fetch platform JWKS.
    let jwks_resp: JwksResponse = http_client
        .get(platform.platform_jwks_url)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("failed to fetch platform JWKS: {}", e)))?
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("invalid JWKS response: {}", e)))?;

    // 3. Find matching key by kid (or first RSA key).
    let jwk_value = jwks_resp
        .keys
        .iter()
        .find(|k| {
            if let Some(expected_kid) = kid {
                k.get("kid").and_then(|v| v.as_str()) == Some(expected_kid)
            } else {
                k.get("kty").and_then(|v| v.as_str()) == Some("RSA")
            }
        })
        .ok_or_else(|| AppError::bad_request("lti.no_jwks_key"))?;

    let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(jwk_value.clone())
        .map_err(|e| AppError::Internal(format!("failed to parse JWK: {}", e)))?;

    let decoding_key = DecodingKey::from_jwk(&jwk)
        .map_err(|e| AppError::Internal(format!("invalid JWK for decoding: {}", e)))?;

    // 4. Validate signature, issuer, audience, expiry.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[platform.issuer]);
    validation.set_audience(&[platform.client_id]);
    validation.validate_exp = true;

    let token_data =
        decode::<LtiLaunchClaims>(id_token, &decoding_key, &validation).map_err(|e| {
            AppError::bad_request_with("lti.jwt_validation_failed", [("detail", e.to_string())])
        })?;

    let claims = token_data.claims;

    // 5. Verify nonce.
    if claims.nonce != expected_nonce {
        return Err(AppError::bad_request("lti.nonce_mismatch"));
    }

    // 6. Verify message type.
    if let Some(ref msg_type) = claims.message_type {
        if msg_type != "LtiResourceLinkRequest" {
            return Err(AppError::bad_request_with(
                "lti.message_type_unsupported",
                [("type", msg_type.clone())],
            ));
        }
    }

    Ok(claims)
}

// ---------------------------------------------------------------------------
// LTI role helpers
// ---------------------------------------------------------------------------

// `lti_roles_to_course_role` moved to the axum-free `minerva-app-core` so
// the NRPS reconcile loop (now `minerva_app_core::lti_nrps`) can share it.
// Re-exported so the launch handler's `crate::lti::lti_roles_to_course_role`
// call sites keep resolving.
pub use minerva_app_core::lti::lti_roles_to_course_role;
