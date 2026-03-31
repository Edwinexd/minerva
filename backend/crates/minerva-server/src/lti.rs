//! LTI 1.3 core logic: RSA key management, JWKS, JWT validation, claim parsing.

use base64::Engine;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use rand_chacha::ChaCha20Rng;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::rand_core::SeedableRng;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::error::AppError;

// ---------------------------------------------------------------------------
// RSA key pair management
// ---------------------------------------------------------------------------

/// Holds the tool's RSA key pair and pre-built JWKS response.
pub struct LtiKeyPair {
    pub kid: String,
    pub encoding_key: jsonwebtoken::EncodingKey,
    pub jwks_json: serde_json::Value,
}

impl LtiKeyPair {
    /// Derive an RSA-2048 key pair deterministically from a seed string.
    /// Same seed always produces the same key → stable JWKS.
    pub fn from_seed(seed: &str) -> anyhow::Result<Self> {
        let kid = "minerva-lti-1".to_string();

        // Hash the seed to get a 32-byte PRNG seed.
        let seed_bytes: [u8; 32] = Sha256::digest(seed.as_bytes()).into();
        let mut rng = ChaCha20Rng::from_seed(seed_bytes);

        let private_key = RsaPrivateKey::new(&mut rng, 2048)?;

        let pem_string = private_key.to_pkcs8_pem(LineEnding::LF)?;
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(pem_string.as_bytes())?;

        // Build JWKS from the public key components.
        let public_key = private_key.to_public_key();
        let n_bytes = public_key.n().to_bytes_be();
        let e_bytes = public_key.e().to_bytes_be();

        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let n_b64 = b64.encode(&n_bytes);
        let e_b64 = b64.encode(&e_bytes);

        let jwks_json = serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "alg": "RS256",
                "use": "sig",
                "kid": kid,
                "n": n_b64,
                "e": e_b64,
            }]
        });

        Ok(Self {
            kid,
            encoding_key,
            jwks_json,
        })
    }
}

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

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/version",
        default
    )]
    pub version: Option<String>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/deployment_id",
        default
    )]
    pub deployment_id: Option<String>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/roles",
        default
    )]
    pub roles: Vec<String>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/context",
        default
    )]
    pub context: Option<LtiContext>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/resource_link",
        default
    )]
    pub resource_link: Option<LtiResourceLink>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/custom",
        default
    )]
    pub custom: Option<HashMap<String, serde_json::Value>>,

    #[serde(
        rename = "https://purl.imsglobal.org/spec/lti/claim/launch_presentation",
        default
    )]
    pub launch_presentation: Option<LtiLaunchPresentation>,
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

// ---------------------------------------------------------------------------
// JWT validation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<serde_json::Value>,
}

/// Validate an LTI 1.3 launch id_token JWT against a course registration.
pub async fn validate_launch_jwt(
    registration: &minerva_db::queries::lti::RegistrationRow,
    id_token: &str,
    expected_nonce: &str,
    http_client: &reqwest::Client,
) -> Result<LtiLaunchClaims, AppError> {
    // 1. Decode JWT header to get kid.
    let header = decode_header(id_token)
        .map_err(|e| AppError::BadRequest(format!("invalid JWT header: {}", e)))?;
    let kid = header.kid.as_deref();

    // 2. Fetch platform JWKS.
    let jwks_resp: JwksResponse = http_client
        .get(&registration.platform_jwks_url)
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
        .ok_or_else(|| AppError::BadRequest("no matching key in platform JWKS".into()))?;

    let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(jwk_value.clone())
        .map_err(|e| AppError::Internal(format!("failed to parse JWK: {}", e)))?;

    let decoding_key = DecodingKey::from_jwk(&jwk)
        .map_err(|e| AppError::Internal(format!("invalid JWK for decoding: {}", e)))?;

    // 4. Validate signature, issuer, audience, expiry.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[&registration.issuer]);
    validation.set_audience(&[&registration.client_id]);
    validation.validate_exp = true;

    let token_data = decode::<LtiLaunchClaims>(id_token, &decoding_key, &validation)
        .map_err(|e| AppError::BadRequest(format!("JWT validation failed: {}", e)))?;

    let claims = token_data.claims;

    // 5. Verify nonce.
    if claims.nonce != expected_nonce {
        return Err(AppError::BadRequest("nonce mismatch".into()));
    }

    // 6. Verify message type.
    if let Some(ref msg_type) = claims.message_type {
        if msg_type != "LtiResourceLinkRequest" {
            return Err(AppError::BadRequest(format!(
                "unsupported LTI message type: {}",
                msg_type
            )));
        }
    }

    Ok(claims)
}

// ---------------------------------------------------------------------------
// LTI role helpers
// ---------------------------------------------------------------------------

const INSTRUCTOR_ROLES: &[&str] = &[
    "http://purl.imsglobal.org/vocab/lis/v2/membership#Instructor",
    "http://purl.imsglobal.org/vocab/lis/v2/institution/person#Instructor",
    "http://purl.imsglobal.org/vocab/lis/v2/membership#ContentDeveloper",
    "http://purl.imsglobal.org/vocab/lis/v2/institution/person#Faculty",
];

const ADMIN_ROLES: &[&str] = &[
    "http://purl.imsglobal.org/vocab/lis/v2/system/person#Administrator",
    "http://purl.imsglobal.org/vocab/lis/v2/institution/person#Administrator",
    "http://purl.imsglobal.org/vocab/lis/v2/membership#Administrator",
];

/// Map LTI role URIs to a Minerva course member role.
pub fn lti_roles_to_course_role(roles: &[String]) -> &'static str {
    for role in roles {
        for admin in ADMIN_ROLES {
            if role.contains(admin) {
                return "teacher";
            }
        }
    }
    for role in roles {
        for instructor in INSTRUCTOR_ROLES {
            if role.contains(instructor) {
                return "teacher";
            }
        }
    }
    "student"
}
