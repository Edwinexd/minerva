//! Admin-managed catalog of chat / utility LLM models. Mirrors
//! `embedding_models` / `reranker_models` but adds the provider
//! reference and per-model USD pricing. See migration
//! `20260610000001_chat_models.sql` for the schema, the two
//! single-default partial-unique indexes, and the
//! enabled-requires-price CHECK.
//!
//! Two "default" roles share one catalog so the admin manages a single
//! list: `is_default` (the course-chat default) and `is_utility_default`
//! (classification / KG / aegis / suggested-questions).
//!
//! Price NULL vs 0 is load-bearing: NULL = unknown (model unusable),
//! 0 = genuinely free (on-prem, usable). `rates_of` collapses an
//! unknown price to `None` so the cost path can hard-error rather than
//! silently bill $0.

use rust_decimal::Decimal;
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct ChatModelRow {
    pub model: String,
    pub provider: String,
    pub display_name: String,
    pub enabled: bool,
    /// The single course-chat default (zero or one row; enforced by a
    /// partial unique index). Set via `set_default`.
    pub is_default: bool,
    /// The single utility default for classification / KG / aegis /
    /// suggested-questions (zero or one row). Set via
    /// `set_utility_default`.
    pub is_utility_default: bool,
    /// USD per 1M input tokens. NULL = unknown (unusable); 0 = free.
    pub input_usd_per_mtok: Option<Decimal>,
    /// USD per 1M output tokens. NULL = unknown (unusable); 0 = free.
    pub output_usd_per_mtok: Option<Decimal>,
    pub supports_logprobs: bool,
    pub supports_tool_use: bool,
    pub price_source_url: Option<String>,
    pub price_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Every row, ordered by model id for stable UI rendering.
pub async fn list_all(db: &PgPool) -> Result<Vec<ChatModelRow>, sqlx::Error> {
    sqlx::query_as!(
        ChatModelRow,
        r#"SELECT model, provider, display_name, enabled, is_default,
                  is_utility_default, input_usd_per_mtok, output_usd_per_mtok,
                  supports_logprobs, supports_tool_use, price_source_url,
                  price_updated_at, created_at, updated_at
           FROM chat_models
           ORDER BY model ASC"#,
    )
    .fetch_all(db)
    .await
}

/// Look up one row. `Ok(None)` if the model id isn't registered.
pub async fn find(db: &PgPool, model: &str) -> Result<Option<ChatModelRow>, sqlx::Error> {
    sqlx::query_as!(
        ChatModelRow,
        r#"SELECT model, provider, display_name, enabled, is_default,
                  is_utility_default, input_usd_per_mtok, output_usd_per_mtok,
                  supports_logprobs, supports_tool_use, price_source_url,
                  price_updated_at, created_at, updated_at
           FROM chat_models
           WHERE model = $1"#,
        model,
    )
    .fetch_optional(db)
    .await
}

/// Cheap scalar lookup used by the course PUT validator. Unknown model
/// reads as disabled.
pub async fn is_enabled(db: &PgPool, model: &str) -> Result<bool, sqlx::Error> {
    let row: Option<bool> =
        sqlx::query_scalar!("SELECT enabled FROM chat_models WHERE model = $1", model)
            .fetch_optional(db)
            .await?;
    Ok(row.unwrap_or(false))
}

/// The provider id a model belongs to (`Ok(None)` if unregistered).
/// Used at the chat-route boundary to resolve the `LlmRegistry` provider.
pub async fn provider_of(db: &PgPool, model: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!("SELECT provider FROM chat_models WHERE model = $1", model)
        .fetch_optional(db)
        .await
}

/// The `(input, output)` USD-per-Mtok rates for a model, or `None` when
/// the model is unregistered OR either rate is NULL (unknown price). A
/// `None` here means "do not bill / do not run"; an explicit `0` rate
/// returns `Some((0, 0))` (genuinely free, billable at $0).
pub async fn rates_of(db: &PgPool, model: &str) -> Result<Option<(Decimal, Decimal)>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT input_usd_per_mtok, output_usd_per_mtok FROM chat_models WHERE model = $1",
        model,
    )
    .fetch_optional(db)
    .await?;
    Ok(
        row.and_then(|r| match (r.input_usd_per_mtok, r.output_usd_per_mtok) {
            (Some(i), Some(o)) => Some((i, o)),
            _ => None,
        }),
    )
}

/// Toggle `enabled`. `Ok(None)` if no row matches. Enabling a row whose
/// rates are NULL violates the `chat_models_enabled_requires_price`
/// CHECK and surfaces as a DB error; the admin route pre-checks and
/// returns `chat_model.price_required` before reaching here.
pub async fn set_enabled(
    db: &PgPool,
    model: &str,
    enabled: bool,
) -> Result<Option<ChatModelRow>, sqlx::Error> {
    sqlx::query_as!(
        ChatModelRow,
        r#"UPDATE chat_models
           SET enabled = $2, updated_at = NOW()
           WHERE model = $1
           RETURNING model, provider, display_name, enabled, is_default,
                     is_utility_default, input_usd_per_mtok, output_usd_per_mtok,
                     supports_logprobs, supports_tool_use, price_source_url,
                     price_updated_at, created_at, updated_at"#,
        model,
        enabled,
    )
    .fetch_optional(db)
    .await
}

/// Set the per-model USD rates and stamp `price_updated_at`. `0` is a
/// valid rate (free); pass real numbers. `Ok(None)` if no row matches.
pub async fn set_price(
    db: &PgPool,
    model: &str,
    input_usd_per_mtok: Decimal,
    output_usd_per_mtok: Decimal,
    source_url: Option<&str>,
) -> Result<Option<ChatModelRow>, sqlx::Error> {
    sqlx::query_as!(
        ChatModelRow,
        r#"UPDATE chat_models
           SET input_usd_per_mtok = $2, output_usd_per_mtok = $3,
               price_source_url = $4, price_updated_at = NOW(), updated_at = NOW()
           WHERE model = $1
           RETURNING model, provider, display_name, enabled, is_default,
                     is_utility_default, input_usd_per_mtok, output_usd_per_mtok,
                     supports_logprobs, supports_tool_use, price_source_url,
                     price_updated_at, created_at, updated_at"#,
        model,
        input_usd_per_mtok,
        output_usd_per_mtok,
        source_url,
    )
    .fetch_optional(db)
    .await
}

/// The course-chat default model id (`Ok(None)` if none set).
pub async fn current_default(db: &PgPool) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!("SELECT model FROM chat_models WHERE is_default = TRUE LIMIT 1")
        .fetch_optional(db)
        .await
}

/// The utility (classification / KG) default model id (`Ok(None)` if none).
pub async fn current_utility_default(db: &PgPool) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!("SELECT model FROM chat_models WHERE is_utility_default = TRUE LIMIT 1")
        .fetch_optional(db)
        .await
}

#[derive(Debug)]
pub enum SetDefaultError {
    NotFound,
    Disabled,
    Db(sqlx::Error),
}

impl std::fmt::Display for SetDefaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetDefaultError::NotFound => write!(f, "chat model not in catalog"),
            SetDefaultError::Disabled => {
                write!(f, "chat model is disabled and cannot be a default")
            }
            SetDefaultError::Db(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for SetDefaultError {}

/// Atomically promote one model to the course-chat default and demote
/// the previous holder. The target must exist and be enabled.
pub async fn set_default(db: &PgPool, model: &str) -> Result<ChatModelRow, SetDefaultError> {
    set_one_default(db, model, DefaultKind::Course).await
}

/// Atomically promote one model to the utility default and demote the
/// previous holder. The target must exist and be enabled.
pub async fn set_utility_default(
    db: &PgPool,
    model: &str,
) -> Result<ChatModelRow, SetDefaultError> {
    set_one_default(db, model, DefaultKind::Utility).await
}

#[derive(Clone, Copy)]
enum DefaultKind {
    Course,
    Utility,
}

async fn set_one_default(
    db: &PgPool,
    model: &str,
    kind: DefaultKind,
) -> Result<ChatModelRow, SetDefaultError> {
    let mut tx = db.begin().await.map_err(SetDefaultError::Db)?;

    let target = sqlx::query!(
        "SELECT enabled FROM chat_models WHERE model = $1 FOR UPDATE",
        model,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(SetDefaultError::Db)?
    .ok_or(SetDefaultError::NotFound)?;
    if !target.enabled {
        return Err(SetDefaultError::Disabled);
    }

    // Clear the existing holder, then promote the target, in one txn so
    // the partial unique index never sees two TRUE rows mid-flip.
    let row = match kind {
        DefaultKind::Course => {
            sqlx::query!(
                "UPDATE chat_models SET is_default = FALSE, updated_at = NOW() WHERE is_default = TRUE",
            )
            .execute(&mut *tx)
            .await
            .map_err(SetDefaultError::Db)?;
            sqlx::query_as!(
                ChatModelRow,
                r#"UPDATE chat_models
                   SET is_default = TRUE, updated_at = NOW()
                   WHERE model = $1
                   RETURNING model, provider, display_name, enabled, is_default,
                             is_utility_default, input_usd_per_mtok, output_usd_per_mtok,
                             supports_logprobs, supports_tool_use, price_source_url,
                             price_updated_at, created_at, updated_at"#,
                model,
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(SetDefaultError::Db)?
        }
        DefaultKind::Utility => {
            sqlx::query!(
                "UPDATE chat_models SET is_utility_default = FALSE, updated_at = NOW() WHERE is_utility_default = TRUE",
            )
            .execute(&mut *tx)
            .await
            .map_err(SetDefaultError::Db)?;
            sqlx::query_as!(
                ChatModelRow,
                r#"UPDATE chat_models
                   SET is_utility_default = TRUE, updated_at = NOW()
                   WHERE model = $1
                   RETURNING model, provider, display_name, enabled, is_default,
                             is_utility_default, input_usd_per_mtok, output_usd_per_mtok,
                             supports_logprobs, supports_tool_use, price_source_url,
                             price_updated_at, created_at, updated_at"#,
                model,
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(SetDefaultError::Db)?
        }
    };

    tx.commit().await.map_err(SetDefaultError::Db)?;
    Ok(row)
}

/// Idempotent insert used by the provider-fetch catalog sync. A model id
/// newly returned by a provider's `/models` listing lands disabled with
/// price NULL (unusable until an admin enables and prices it) only on
/// first sight; existing rows (admin toggles + prices) are left
/// untouched. Returns `true` if a row was inserted.
pub async fn seed_if_missing(
    db: &PgPool,
    model: &str,
    provider: &str,
    display_name: &str,
    supports_logprobs: bool,
    supports_tool_use: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"INSERT INTO chat_models
               (model, provider, display_name, supports_logprobs, supports_tool_use)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (model) DO NOTHING"#,
        model,
        provider,
        display_name,
        supports_logprobs,
        supports_tool_use,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
