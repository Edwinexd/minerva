//! LTI 1.3 RSA key pair: deterministic key derivation + JWKS response.
//!
//! This is the key material `AppState` holds. The launch-JWT validation
//! and claim parsing stay in the api crate (`minerva_server::lti`),
//! which re-exports `LtiKeyPair` from here so `crate::lti::LtiKeyPair`
//! keeps resolving. Kept axum-free so the worker/scheduler can build
//! `AppState` without the route tree.

use base64::Engine;
use rand_chacha::ChaCha20Rng;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::rand_core::SeedableRng;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use sha2::{Digest, Sha256};

/// Holds the tool's RSA key pair and pre-built JWKS response.
pub struct LtiKeyPair {
    pub kid: String,
    pub encoding_key: jsonwebtoken::EncodingKey,
    pub jwks_json: serde_json::Value,
}

impl LtiKeyPair {
    /// Derive an RSA-2048 key pair deterministically from a seed string.
    /// Same seed always produces the same key -> stable JWKS.
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

/// Map LTI role URIs to a Minerva course member role. Shared by the api's
/// LTI launch handler and the NRPS reconcile loop (`crate::lti_nrps`).
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
