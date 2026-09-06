//! Shared codegen for the admin-managed model catalogs
//! (`embedding_models`, `reranker_models`, `chat_models`).
//!
//! sqlx's `query!`/`query_as!` macros require the SQL as a plain string
//! literal, so the table name cannot be interpolated at runtime. These
//! macros instead take each full SQL string as a literal argument and
//! generate the per-table module glue (fns + error type), keeping every
//! query compile-time checked against the committed `.sqlx` cache.

/// Generates the per-catalog `SetDefaultError` enum. Each catalog module
/// gets its own type (minerva-server matches on the module-scoped
/// variants) with catalog-specific `Display` messages.
macro_rules! define_set_default_error {
    (not_found: $nf_msg:literal, disabled: $dis_msg:literal $(,)?) => {
        #[derive(Debug)]
        pub enum SetDefaultError {
            NotFound,
            Disabled,
            Db(sqlx::Error),
        }

        impl std::fmt::Display for SetDefaultError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    SetDefaultError::NotFound => write!(f, $nf_msg),
                    SetDefaultError::Disabled => write!(f, $dis_msg),
                    SetDefaultError::Db(e) => write!(f, "{}", e),
                }
            }
        }

        impl std::error::Error for SetDefaultError {}
    };
}

/// Generates the standard catalog query surface
/// (`list_all` / `find` / `is_enabled` / `set_enabled` /
/// `current_default` / `set_default` / `seed_if_missing`) plus the
/// module's `SetDefaultError`, given the row struct and each full SQL
/// string as a literal.
macro_rules! model_catalog_queries {
    (
        row: $Row:ident,
        list_all: $list_all_sql:literal,
        find: $find_sql:literal,
        is_enabled: $is_enabled_sql:literal,
        set_enabled: $set_enabled_sql:literal,
        current_default: $current_default_sql:literal,
        lock_target: $lock_sql:literal,
        clear_default: $clear_sql:literal,
        promote_default: $promote_sql:literal,
        seed_if_missing: $seed_sql:literal,
        not_found_msg: $nf_msg:literal,
        disabled_msg: $dis_msg:literal $(,)?
    ) => {
        /// Every row in the table, ordered by model id for stable UI
        /// rendering. Cheap (the whole table is at most a handful of
        /// rows).
        pub async fn list_all(db: &sqlx::PgPool) -> Result<Vec<$Row>, sqlx::Error> {
            sqlx::query_as!($Row, $list_all_sql).fetch_all(db).await
        }

        /// Look up one row. Returns `Ok(None)` if the model id isn't
        /// registered (admin route maps that to a 404).
        pub async fn find(db: &sqlx::PgPool, model: &str) -> Result<Option<$Row>, sqlx::Error> {
            sqlx::query_as!($Row, $find_sql, model).fetch_optional(db).await
        }

        /// Cheap scalar lookup used by the course PUT validator. Returns
        /// `Ok(false)` for a model that isn't even registered (treats
        /// unknown as disabled; the route layer separately rejects with a
        /// clearer code).
        pub async fn is_enabled(db: &sqlx::PgPool, model: &str) -> Result<bool, sqlx::Error> {
            let row: Option<bool> = sqlx::query_scalar!($is_enabled_sql, model)
                .fetch_optional(db)
                .await?;
            Ok(row.unwrap_or(false))
        }

        /// Toggle the `enabled` flag for an existing row. Returns
        /// `Ok(None)` if no row matches the model id, so the admin route
        /// can 404 properly.
        pub async fn set_enabled(
            db: &sqlx::PgPool,
            model: &str,
            enabled: bool,
        ) -> Result<Option<$Row>, sqlx::Error> {
            sqlx::query_as!($Row, $set_enabled_sql, model, enabled)
                .fetch_optional(db)
                .await
        }

        /// Read the model id new courses should default to. Returns
        /// `Ok(None)` if no row has `is_default = TRUE` (shouldn't happen
        /// post-migration, but the route layer falls back to the column
        /// DEFAULT in that case). The partial index on `is_default` makes
        /// this an index-only fetch.
        pub async fn current_default(db: &sqlx::PgPool) -> Result<Option<String>, sqlx::Error> {
            sqlx::query_scalar!($current_default_sql).fetch_optional(db).await
        }

        /// Atomically promote one model to the default and demote the
        /// previous holder. Both writes happen in a single transaction so
        /// the partial unique index never sees two `TRUE` rows mid-flip.
        ///
        /// The target must already exist in the table and must be
        /// `enabled = TRUE`; a disabled default is a contradiction (the
        /// picker would refuse to surface it). Returns the updated row,
        /// or a typed error so the admin route can map "missing" -> 404
        /// and "disabled" -> 400 without parsing SQL strings.
        pub async fn set_default(db: &sqlx::PgPool, model: &str) -> Result<$Row, SetDefaultError> {
            let mut tx = db.begin().await.map_err(SetDefaultError::Db)?;

            // Lock the target row inside the transaction so a concurrent
            // `set_enabled(false)` between the check and the UPDATE can't
            // slip through.
            let target = sqlx::query!($lock_sql, model)
                .fetch_optional(&mut *tx)
                .await
                .map_err(SetDefaultError::Db)?
                .ok_or(SetDefaultError::NotFound)?;
            if !target.enabled {
                return Err(SetDefaultError::Disabled);
            }

            // Clear any existing default first; the partial unique index
            // would otherwise reject the promotion below.
            sqlx::query!($clear_sql)
                .execute(&mut *tx)
                .await
                .map_err(SetDefaultError::Db)?;

            let row = sqlx::query_as!($Row, $promote_sql, model)
                .fetch_one(&mut *tx)
                .await
                .map_err(SetDefaultError::Db)?;

            tx.commit().await.map_err(SetDefaultError::Db)?;
            Ok(row)
        }

        crate::queries::model_catalog::define_set_default_error!(
            not_found: $nf_msg,
            disabled: $dis_msg,
        );

        /// Idempotent insert used by the runtime sync at startup. Newly
        /// catalogued model ids land here with `enabled = $2` (the
        /// caller's "default policy" choice) only on first sight;
        /// subsequent boots leave the row untouched so an admin's runtime
        /// toggle survives restarts. Returns `true` if a row was
        /// inserted.
        pub async fn seed_if_missing(
            db: &sqlx::PgPool,
            model: &str,
            initial_enabled: bool,
        ) -> Result<bool, sqlx::Error> {
            let result = sqlx::query!($seed_sql, model, initial_enabled)
                .execute(db)
                .await?;
            Ok(result.rows_affected() > 0)
        }
    };
}

pub(crate) use define_set_default_error;
pub(crate) use model_catalog_queries;
