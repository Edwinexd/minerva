//! Latency samples of streamed chat answers (migration
//! `20261111000001_chat_model_latency.sql`), summarized per model for the
//! teacher's model picker.

use sqlx::PgPool;

/// Samples older than this are deleted by the scheduler.
pub const RETENTION_DAYS: i32 = 30;

/// Window the picker summarizes.
pub const SUMMARY_DAYS: i32 = 7;

/// Decode windows shorter than this are too coarse for a rate: a reply
/// that arrived in one or two chunks would read as tens of thousands of
/// tokens a second. Kept low because Cerebras writes a short answer in
/// well under a quarter second (measured 69-194 ms, Nov 2026).
pub const MIN_DECODE_MS: i32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub ttft_ms: i32,
    pub decode_ms: i32,
    pub output_tokens: i32,
    pub prompt_tokens: i32,
}

pub async fn insert(
    db: &PgPool,
    model: &str,
    provider: &str,
    sample: &Sample,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO chat_model_latency
               (model, provider, ttft_ms, decode_ms, output_tokens, prompt_tokens)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
        model,
        provider,
        sample.ttft_ms.max(0),
        sample.decode_ms.max(0),
        sample.output_tokens.max(0),
        sample.prompt_tokens.max(0),
    )
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub model: String,
    pub samples: i64,
    pub ttft_p50_ms: Option<f64>,
    pub ttft_p90_ms: Option<f64>,
    /// Median output tokens per second over samples with a decode window
    /// of at least `MIN_DECODE_MS`.
    pub tokens_per_second_p50: Option<f64>,
}

/// Per-model medians over the last `SUMMARY_DAYS`, every provider that
/// served the model pooled.
pub async fn summaries(db: &PgPool) -> Result<Vec<Summary>, sqlx::Error> {
    sqlx::query_as!(
        Summary,
        r#"SELECT model AS "model!",
                  COUNT(*) AS "samples!",
                  percentile_cont(0.5) WITHIN GROUP (ORDER BY ttft_ms::float8) AS ttft_p50_ms,
                  percentile_cont(0.9) WITHIN GROUP (ORDER BY ttft_ms::float8) AS ttft_p90_ms,
                  percentile_cont(0.5) WITHIN GROUP (
                      ORDER BY output_tokens * 1000.0::float8 / decode_ms
                  ) FILTER (WHERE decode_ms >= $2 AND output_tokens > 0)
                      AS tokens_per_second_p50
           FROM chat_model_latency
           WHERE recorded_at > now() - make_interval(days => $1)
           GROUP BY model"#,
        SUMMARY_DAYS,
        MIN_DECODE_MS,
    )
    .fetch_all(db)
    .await
}

pub async fn delete_older_than(db: &PgPool, days: i32) -> Result<u64, sqlx::Error> {
    let res = sqlx::query!(
        "DELETE FROM chat_model_latency WHERE recorded_at < now() - make_interval(days => $1)",
        days,
    )
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}
