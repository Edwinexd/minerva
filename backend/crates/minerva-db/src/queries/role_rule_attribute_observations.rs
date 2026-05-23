use sqlx::PgPool;
use uuid::Uuid;

/// Rolling retention window for captured attribute observations. The
/// suggestion list is the only consumer of this table; we don't need it for
/// audit, debugging, or correctness. Dropping rows that haven't been
/// re-observed within this window keeps the GDPR footprint small: an admin
/// who removes a user *and* every other user who shared an attribute value
/// with them will see that value age out within the window even without
/// vacuum. Users who keep logging in keep refreshing `last_seen`, so active
/// data is never truncated.
pub const OBSERVATION_TTL_DAYS: i64 = 7;

/// One row per (attribute, value) seen across users, with the distinct user
/// count. Returned by `list_suggestions_above_threshold` so the admin UI can
/// auto-complete known values when authoring rule conditions.
#[derive(Debug, Clone)]
pub struct AttributeValueSuggestion {
    pub attribute: String,
    pub value: String,
    pub user_count: i64,
}

/// Bulk-upsert observed (attribute, value) pairs for a single user. The
/// primary key on (attribute, value, user_id) makes the ON CONFLICT clause
/// idempotent: re-observing the same row just refreshes `last_seen`.
///
/// Callers should pre-split multi-valued Shib headers (`;`-delimited) and
/// trim/skip empties; this layer assumes every value is a concrete atomic
/// token the `contains` operator would match exactly.
pub async fn observe_for_user(
    db: &PgPool,
    user_id: Uuid,
    pairs: &[(String, String)],
) -> Result<(), sqlx::Error> {
    if pairs.is_empty() {
        return Ok(());
    }
    // UNNEST keeps this to a single round trip regardless of how many
    // (attribute, value) pairs the user contributed.
    let attributes: Vec<String> = pairs.iter().map(|(a, _)| a.clone()).collect();
    let values: Vec<String> = pairs.iter().map(|(_, v)| v.clone()).collect();
    sqlx::query!(
        r#"INSERT INTO role_rule_attribute_observations (attribute, value, user_id)
           SELECT a.attribute, a.value, $3
           FROM UNNEST($1::TEXT[], $2::TEXT[]) AS a(attribute, value)
           ON CONFLICT (attribute, value, user_id) DO UPDATE SET last_seen = NOW()"#,
        &attributes,
        &values,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// All (attribute, value) pairs observed on at least `min_users` distinct
/// users. Sorted by attribute, then user count desc, then value asc so the
/// frontend can render a stable list without re-sorting per attribute.
///
/// The HAVING clause is the privacy guard the feature was specified with:
/// values seen on only one user would let an admin browsing the rules page
/// fish out the affiliation/entitlement of a specific person; requiring
/// >= 2 distinct users keeps the suggestion list to genuinely shared values.
pub async fn list_suggestions_above_threshold(
    db: &PgPool,
    min_users: i64,
) -> Result<Vec<AttributeValueSuggestion>, sqlx::Error> {
    sqlx::query_as!(
        AttributeValueSuggestion,
        r#"SELECT attribute, value, COUNT(*)::BIGINT AS "user_count!"
           FROM role_rule_attribute_observations
           GROUP BY attribute, value
           HAVING COUNT(*) >= $1
           ORDER BY attribute ASC, COUNT(*) DESC, value ASC"#,
        min_users,
    )
    .fetch_all(db)
    .await
}

/// Delete observation rows whose `last_seen` is older than `ttl_days` days.
/// Returns the number of rows removed so the caller can log it. Invoked
/// from a background scheduler at server startup; idempotent and safe to
/// run as often as desired (no contention with the per-request UPSERT
/// since they touch disjoint rows by definition; if a row's last_seen is
/// older than the cutoff, no one is observing it right now).
pub async fn prune_older_than(db: &PgPool, ttl_days: i64) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"DELETE FROM role_rule_attribute_observations
           WHERE last_seen < NOW() - ($1 || ' days')::INTERVAL"#,
        ttl_days.to_string(),
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
