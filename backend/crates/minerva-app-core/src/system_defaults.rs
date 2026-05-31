//! Admin-tunable system-wide defaults: registry, typed accessors,
//! and startup seeder.
//!
//! The DB-side storage (`system_defaults` JSONB table) is in
//! `minerva_db::queries::system_defaults`. This module is the policy
//! layer on top: it knows the *list* of configurable knobs, each
//! knob's type discipline (int with range, enum with options, etc.),
//! the env-var name we used to read it from (used as a one-shot seed
//! on fresh installs), and a hard-coded fallback for the deployment
//! that has neither row nor env var set.
//!
//! Two categories of knobs:
//!
//! * `CourseAi` ; defaults snapshotted into a new course row at
//!   `POST /courses` time. Editing them later only affects courses
//!   created *after* the edit. Existing courses keep their per-
//!   course overrides.
//! * `Platform` ; values the runtime reads live: per-owner daily
//!   token cap, upload byte caps, sync interval hours, observation
//!   TTL. Editing them affects the *next* read; restart not needed
//!   except where annotated (axum's body limit is set at router
//!   build time, see `routes/documents.rs`).
//!
//! Validation lives here too: `validate(def, value)` is the single
//! place that decides whether an admin's edit is well-formed. Both
//! the seed path and the admin PUT route call into it so an env-var
//! typo can't ship a malformed row past startup.

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sqlx::PgPool;

/// Stable string keys for `system_defaults` rows. Using `&'static
/// str` constants (rather than an enum) keeps the JSON wire format
/// the same as the DB column and lets the registry table live in
/// one place.
pub mod keys {
    // ----- Course AI defaults (snapshotted on course create) -----
    pub const COURSE_MODEL: &str = "course.model";
    pub const COURSE_TEMPERATURE: &str = "course.temperature";
    pub const COURSE_CONTEXT_RATIO: &str = "course.context_ratio";
    pub const COURSE_MAX_CHUNKS: &str = "course.max_chunks";
    pub const COURSE_MIN_SCORE: &str = "course.min_score";
    pub const COURSE_STRATEGY: &str = "course.strategy";
    pub const COURSE_TOOL_USE_ENABLED: &str = "course.tool_use_enabled";
    pub const COURSE_EMBEDDING_PROVIDER: &str = "course.embedding_provider";
    pub const COURSE_SYSTEM_PROMPT: &str = "course.system_prompt";
    pub const COURSE_DAILY_TOKEN_LIMIT: &str = "course.daily_token_limit";

    // ----- Platform-wide knobs (read live) -----
    pub const OWNER_DAILY_TOKEN_LIMIT: &str = "platform.owner_daily_token_limit";
    pub const MAX_UPLOAD_BYTES: &str = "platform.max_upload_bytes";
    pub const MAX_MBZ_UPLOAD_BYTES: &str = "platform.max_mbz_upload_bytes";
    pub const CANVAS_AUTO_SYNC_INTERVAL_HOURS: &str = "platform.canvas_auto_sync_interval_hours";
    pub const LTI_NRPS_SYNC_INTERVAL_HOURS: &str = "platform.lti_nrps_sync_interval_hours";
    pub const OBSERVATION_TTL_DAYS: &str = "platform.observation_ttl_days";
}

/// Per-knob discipline. The admin route uses this to reject malformed
/// edits before they hit the DB; the frontend uses it to pick the
/// right input widget (number vs text vs checkbox vs select).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnobKind {
    /// `true` / `false`.
    Bool,
    /// JSON integer in `[min, max]`. Stored as i64.
    Int { min: i64, max: i64 },
    /// JSON number in `[min, max]`. Stored as f64.
    Float { min: f64, max: f64 },
    /// Free-form string. `multiline=true` -> textarea in the UI;
    /// `max_len` is a soft ceiling enforced at validation.
    Text {
        multiline: bool,
        max_len: usize,
        /// Allow empty string (and represent in JSON as `null`).
        nullable: bool,
    },
    /// String picked from a small static set. Validates membership.
    Enum { options: &'static [&'static str] },
    /// Free-form string treated as a Cerebras chat-model id. The
    /// frontend renders a dropdown sourced from `GET /models`; the
    /// backend just enforces non-empty.
    ChatModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    CourseAi,
    Platform,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct KnobDef {
    pub key: &'static str,
    pub category: Category,
    /// i18n key for the field label. Resolved by the frontend.
    pub label_key: &'static str,
    /// i18n key for the helper text shown under the field.
    pub description_key: &'static str,
    pub kind: KnobKind,
    /// Legacy env-var name used to seed this row on a fresh install.
    /// `None` for knobs that never had an env-var counterpart (most
    /// of the course AI defaults).
    pub env_var: Option<&'static str>,
    /// Hard-coded value used when neither the DB row nor the env var
    /// is present. Also the "Reset to default" target in the UI.
    pub fallback: Value,
}

/// The whole registry. Constructed as a `Vec` rather than a const
/// because `serde_json::Value` isn't const-friendly; cheap enough
/// (called on startup and on each `GET /admin/system-defaults`).
pub fn registry() -> Vec<KnobDef> {
    use Category::*;
    vec![
        // ---------- Course AI defaults ----------
        KnobDef {
            key: keys::COURSE_MODEL,
            category: CourseAi,
            label_key: "defaults.course.model.label",
            description_key: "defaults.course.model.description",
            kind: KnobKind::ChatModel,
            env_var: None,
            // Matches the courses-table column DEFAULT
            // (`20260527000002_default_inference_model_gpt_oss.sql`).
            // Bumped from qwen-3-235b-a22b-instruct-2507 when
            // Cerebras deprecated that model on 2026-05-27.
            fallback: json!("gpt-oss-120b"),
        },
        KnobDef {
            key: keys::COURSE_TEMPERATURE,
            category: CourseAi,
            label_key: "defaults.course.temperature.label",
            description_key: "defaults.course.temperature.description",
            kind: KnobKind::Float { min: 0.0, max: 1.0 },
            env_var: None,
            fallback: json!(0.3),
        },
        KnobDef {
            key: keys::COURSE_CONTEXT_RATIO,
            category: CourseAi,
            label_key: "defaults.course.contextRatio.label",
            description_key: "defaults.course.contextRatio.description",
            kind: KnobKind::Float {
                min: 0.1,
                max: 0.95,
            },
            env_var: None,
            fallback: json!(0.7),
        },
        KnobDef {
            key: keys::COURSE_MAX_CHUNKS,
            category: CourseAi,
            label_key: "defaults.course.maxChunks.label",
            description_key: "defaults.course.maxChunks.description",
            kind: KnobKind::Int { min: 1, max: 100 },
            env_var: None,
            fallback: json!(10),
        },
        KnobDef {
            key: keys::COURSE_MIN_SCORE,
            category: CourseAi,
            label_key: "defaults.course.minScore.label",
            description_key: "defaults.course.minScore.description",
            kind: KnobKind::Float { min: 0.0, max: 1.0 },
            env_var: None,
            fallback: json!(0.0),
        },
        KnobDef {
            key: keys::COURSE_STRATEGY,
            category: CourseAi,
            label_key: "defaults.course.strategy.label",
            description_key: "defaults.course.strategy.description",
            kind: KnobKind::Enum {
                options: &["simple", "flare"],
            },
            env_var: None,
            fallback: json!("simple"),
        },
        KnobDef {
            key: keys::COURSE_TOOL_USE_ENABLED,
            category: CourseAi,
            label_key: "defaults.course.toolUse.label",
            description_key: "defaults.course.toolUse.description",
            kind: KnobKind::Bool,
            env_var: None,
            fallback: json!(false),
        },
        KnobDef {
            key: keys::COURSE_EMBEDDING_PROVIDER,
            category: CourseAi,
            label_key: "defaults.course.embeddingProvider.label",
            description_key: "defaults.course.embeddingProvider.description",
            kind: KnobKind::Enum {
                options: &["local", "openai"],
            },
            env_var: None,
            fallback: json!("local"),
        },
        KnobDef {
            key: keys::COURSE_SYSTEM_PROMPT,
            category: CourseAi,
            label_key: "defaults.course.systemPrompt.label",
            description_key: "defaults.course.systemPrompt.description",
            kind: KnobKind::Text {
                multiline: true,
                max_len: 20_000,
                nullable: true,
            },
            env_var: None,
            fallback: Value::Null,
        },
        KnobDef {
            key: keys::COURSE_DAILY_TOKEN_LIMIT,
            category: CourseAi,
            label_key: "defaults.course.dailyTokenLimit.label",
            description_key: "defaults.course.dailyTokenLimit.description",
            // 0 means unlimited; max is a sanity ceiling not a policy.
            kind: KnobKind::Int {
                min: 0,
                max: 1_000_000_000,
            },
            env_var: Some("MINERVA_DEFAULT_COURSE_DAILY_TOKEN_LIMIT"),
            fallback: json!(100_000),
        },
        // ---------- Platform-wide knobs ----------
        KnobDef {
            key: keys::OWNER_DAILY_TOKEN_LIMIT,
            category: Platform,
            label_key: "defaults.platform.ownerDailyTokenLimit.label",
            description_key: "defaults.platform.ownerDailyTokenLimit.description",
            kind: KnobKind::Int {
                min: 0,
                max: 10_000_000_000,
            },
            env_var: Some("MINERVA_DEFAULT_OWNER_DAILY_TOKEN_LIMIT"),
            fallback: json!(500_000),
        },
        KnobDef {
            key: keys::MAX_UPLOAD_BYTES,
            category: Platform,
            label_key: "defaults.platform.maxUploadBytes.label",
            description_key: "defaults.platform.maxUploadBytes.description",
            // Ceiling matches the axum DefaultBodyLimit set at
            // router build time (see `routes/documents.rs`); admin
            // can dial *down* but not above that without bumping
            // the ceiling and restarting.
            kind: KnobKind::Int {
                min: 1_000_000,
                max: BODY_LIMIT_CEILING,
            },
            env_var: None,
            fallback: json!(50 * 1_000_000_i64),
        },
        KnobDef {
            key: keys::MAX_MBZ_UPLOAD_BYTES,
            category: Platform,
            label_key: "defaults.platform.maxMbzUploadBytes.label",
            description_key: "defaults.platform.maxMbzUploadBytes.description",
            kind: KnobKind::Int {
                min: 10_000_000,
                max: MBZ_BODY_LIMIT_CEILING,
            },
            env_var: None,
            fallback: json!(1_000_000_000_i64),
        },
        KnobDef {
            key: keys::CANVAS_AUTO_SYNC_INTERVAL_HOURS,
            category: Platform,
            label_key: "defaults.platform.canvasAutoSyncIntervalHours.label",
            description_key: "defaults.platform.canvasAutoSyncIntervalHours.description",
            kind: KnobKind::Int { min: 0, max: 720 },
            env_var: Some("MINERVA_CANVAS_AUTO_SYNC_INTERVAL_HOURS"),
            fallback: json!(24),
        },
        KnobDef {
            key: keys::LTI_NRPS_SYNC_INTERVAL_HOURS,
            category: Platform,
            label_key: "defaults.platform.ltiNrpsSyncIntervalHours.label",
            description_key: "defaults.platform.ltiNrpsSyncIntervalHours.description",
            kind: KnobKind::Int { min: 0, max: 720 },
            env_var: Some("MINERVA_LTI_NRPS_SYNC_INTERVAL_HOURS"),
            fallback: json!(6),
        },
        KnobDef {
            key: keys::OBSERVATION_TTL_DAYS,
            category: Platform,
            label_key: "defaults.platform.observationTtlDays.label",
            description_key: "defaults.platform.observationTtlDays.description",
            kind: KnobKind::Int { min: 1, max: 365 },
            env_var: None,
            fallback: json!(7),
        },
    ]
}

/// Hard ceilings used by the axum `DefaultBodyLimit::max(...)`
/// declarations in `routes/documents.rs`. The admin-tunable value
/// lives at or below these; raising them requires a deploy. Kept
/// here so the registry's `Int { max }` matches what the router
/// will actually accept.
pub const BODY_LIMIT_CEILING: i64 = 2_000_000_000;
pub const MBZ_BODY_LIMIT_CEILING: i64 = 5_000_000_000;

/// Linear lookup by key. ~16 entries; not worth a HashMap.
pub fn find(key: &str) -> Option<KnobDef> {
    registry().into_iter().find(|d| d.key == key)
}

/// Reject malformed admin edits before they hit the DB. Returns a
/// short i18n code suitable for `AppError::bad_request`. Same
/// function is reused by the seeder so an env-var typo also fails
/// loudly at startup rather than rotting in the table.
pub fn validate(def: &KnobDef, value: &Value) -> Result<(), &'static str> {
    match &def.kind {
        KnobKind::Bool => {
            value.as_bool().ok_or("defaults.invalid_bool")?;
        }
        KnobKind::Int { min, max } => {
            let n = value.as_i64().ok_or("defaults.invalid_int")?;
            if n < *min || n > *max {
                return Err("defaults.out_of_range");
            }
        }
        KnobKind::Float { min, max } => {
            let n = value.as_f64().ok_or("defaults.invalid_float")?;
            if !n.is_finite() || n < *min || n > *max {
                return Err("defaults.out_of_range");
            }
        }
        KnobKind::Text {
            max_len, nullable, ..
        } => {
            if value.is_null() {
                if !nullable {
                    return Err("defaults.invalid_string");
                }
            } else {
                let s = value.as_str().ok_or("defaults.invalid_string")?;
                if s.len() > *max_len {
                    return Err("defaults.too_long");
                }
            }
        }
        KnobKind::Enum { options } => {
            let s = value.as_str().ok_or("defaults.invalid_string")?;
            if !options.contains(&s) {
                return Err("defaults.invalid_enum");
            }
        }
        KnobKind::ChatModel => {
            let s = value.as_str().ok_or("defaults.invalid_string")?;
            if s.trim().is_empty() {
                return Err("defaults.invalid_string");
            }
        }
    }
    Ok(())
}

/// Insert every registered key that isn't already in the DB. Env-var
/// values take precedence over the hard-coded fallback. Run once at
/// startup from `AppState::new`; subsequent admin edits in the UI
/// persist and are never overwritten.
pub async fn seed_all(db: &PgPool) -> Result<(), sqlx::Error> {
    for def in registry() {
        // Pick the seed value: env var (if set + valid) > fallback.
        let seed_value = def
            .env_var
            .and_then(|name| std::env::var(name).ok())
            .filter(|s| !s.is_empty())
            .and_then(|raw| parse_env_value(&def, &raw))
            .unwrap_or_else(|| def.fallback.clone());

        // Defend against a registry typo: if the chosen value
        // doesn't validate, fall back to the hard-coded default and
        // log loudly. We never persist a malformed seed.
        let final_value = match validate(&def, &seed_value) {
            Ok(()) => seed_value,
            Err(code) => {
                tracing::error!(
                    "system_defaults: seed for `{}` failed validation ({}); using hard-coded fallback {}",
                    def.key,
                    code,
                    def.fallback,
                );
                def.fallback.clone()
            }
        };

        let inserted =
            minerva_db::queries::system_defaults::seed_if_missing(db, def.key, &final_value)
                .await?;
        if inserted {
            tracing::info!("system_defaults: seeded `{}` = {}", def.key, final_value,);
        }
    }
    Ok(())
}

/// Parse a string env-var value into a JSON value matching the
/// knob's expected type. Returns `None` for an unparseable value;
/// the seeder then falls back to the hard-coded default.
fn parse_env_value(def: &KnobDef, raw: &str) -> Option<Value> {
    match &def.kind {
        KnobKind::Bool => match raw.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(Value::Bool(true)),
            "false" | "0" | "no" => Some(Value::Bool(false)),
            _ => None,
        },
        KnobKind::Int { .. } => raw.parse::<i64>().ok().map(|n| json!(n)),
        KnobKind::Float { .. } => raw.parse::<f64>().ok().map(|n| json!(n)),
        KnobKind::Text { .. } | KnobKind::Enum { .. } | KnobKind::ChatModel => {
            Some(Value::String(raw.to_string()))
        }
    }
}

// =========================================================
// Typed runtime accessors. Each wraps a single key lookup +
// fallback so call sites elsewhere in the codebase don't have
// to know about JSON. Returns the deserialized value or the
// registry's hard-coded fallback if the DB row is missing or
// malformed (the missing-row path is rare since `seed_all`
// runs at startup, but we defend against an admin manually
// deleting a row).
// =========================================================

async fn fetch<T: DeserializeOwned>(db: &PgPool, key: &'static str) -> T {
    if let Ok(Some(v)) = minerva_db::queries::system_defaults::get::<T>(db, key).await {
        return v;
    }
    let def = find(key).expect("system_defaults: registry must list every key");
    serde_json::from_value(def.fallback.clone())
        .expect("system_defaults: registry fallback must deserialize into the accessor type")
}

pub async fn course_model(db: &PgPool) -> String {
    fetch(db, keys::COURSE_MODEL).await
}

pub async fn course_temperature(db: &PgPool) -> f64 {
    fetch(db, keys::COURSE_TEMPERATURE).await
}

pub async fn course_context_ratio(db: &PgPool) -> f64 {
    fetch(db, keys::COURSE_CONTEXT_RATIO).await
}

pub async fn course_max_chunks(db: &PgPool) -> i32 {
    fetch::<i64>(db, keys::COURSE_MAX_CHUNKS).await as i32
}

pub async fn course_min_score(db: &PgPool) -> f32 {
    fetch::<f64>(db, keys::COURSE_MIN_SCORE).await as f32
}

pub async fn course_strategy(db: &PgPool) -> String {
    fetch(db, keys::COURSE_STRATEGY).await
}

pub async fn course_tool_use_enabled(db: &PgPool) -> bool {
    fetch(db, keys::COURSE_TOOL_USE_ENABLED).await
}

pub async fn course_embedding_provider(db: &PgPool) -> String {
    fetch(db, keys::COURSE_EMBEDDING_PROVIDER).await
}

pub async fn course_system_prompt(db: &PgPool) -> Option<String> {
    fetch(db, keys::COURSE_SYSTEM_PROMPT).await
}

pub async fn course_daily_token_limit(db: &PgPool) -> i64 {
    fetch(db, keys::COURSE_DAILY_TOKEN_LIMIT).await
}

pub async fn owner_daily_token_limit(db: &PgPool) -> i64 {
    fetch(db, keys::OWNER_DAILY_TOKEN_LIMIT).await
}

pub async fn max_upload_bytes(db: &PgPool) -> i64 {
    fetch(db, keys::MAX_UPLOAD_BYTES).await
}

pub async fn max_mbz_upload_bytes(db: &PgPool) -> i64 {
    fetch(db, keys::MAX_MBZ_UPLOAD_BYTES).await
}

pub async fn canvas_auto_sync_interval_hours(db: &PgPool) -> i32 {
    fetch::<i64>(db, keys::CANVAS_AUTO_SYNC_INTERVAL_HOURS).await as i32
}

pub async fn lti_nrps_sync_interval_hours(db: &PgPool) -> i32 {
    fetch::<i64>(db, keys::LTI_NRPS_SYNC_INTERVAL_HOURS).await as i32
}

pub async fn observation_ttl_days(db: &PgPool) -> i64 {
    fetch(db, keys::OBSERVATION_TTL_DAYS).await
}
