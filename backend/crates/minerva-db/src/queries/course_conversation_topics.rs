//! Durable cache for semantic themes shown on the teacher conversation
//! dashboard. Theme generation and cache invalidation live in the server;
//! this module only owns the database representation.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConversationTopicsCacheRow {
    pub course_id: Uuid,
    pub topics: Value,
    pub source_hash: String,
    pub model: String,
    pub generated_at: DateTime<Utc>,
}

pub async fn get(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Option<ConversationTopicsCacheRow>, sqlx::Error> {
    sqlx::query_as::<_, ConversationTopicsCacheRow>(
        r#"SELECT course_id, topics, source_hash, model, generated_at
           FROM course_conversation_topics
           WHERE course_id = $1"#,
    )
    .bind(course_id)
    .fetch_optional(db)
    .await
}

pub async fn upsert(
    db: &PgPool,
    course_id: Uuid,
    topics: &Value,
    source_hash: &str,
    model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO course_conversation_topics
              (course_id, topics, source_hash, model, generated_at)
           VALUES ($1, $2, $3, $4, NOW())
           ON CONFLICT (course_id) DO UPDATE
              SET topics       = EXCLUDED.topics,
                  source_hash  = EXCLUDED.source_hash,
                  model        = EXCLUDED.model,
                  generated_at = NOW()"#,
    )
    .bind(course_id)
    .bind(topics)
    .bind(source_hash)
    .bind(model)
    .execute(db)
    .await
    .map(|_| ())
}
