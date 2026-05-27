//! Staging area for the Daisy auto-import.
//!
//! Each row mirrors what `courses::upsert_from_daisy` would write
//! given the same input payload, except nothing reaches the live
//! `courses` table until an admin clicks Apply (or `daisy_settings.
//! auto_apply` is flipped ON). See migration 20260527000004 for the
//! full table comment.

use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct PendingImportRow {
    pub id: Uuid,
    pub momenttillf_id: String,
    pub course_code: String,
    pub name: String,
    pub semester_label: String,
    pub daisy_info_url: Option<String>,
    pub daisy_syllabus_url: Option<String>,
    pub daisy_unit: Option<String>,
    pub participants: JsonValue,
    pub existing_course_id: Option<Uuid>,
    pub first_seen_at: chrono::DateTime<chrono::Utc>,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

/// Input bag for the staging upsert. Borrowed for the hot path so we
/// don't allocate on the daily-sync flow.
pub struct StageInput<'a> {
    pub momenttillf_id: &'a str,
    pub course_code: &'a str,
    pub name: &'a str,
    pub semester_label: &'a str,
    pub daisy_info_url: Option<&'a str>,
    pub daisy_syllabus_url: Option<&'a str>,
    pub daisy_unit: Option<&'a str>,
    pub participants: &'a JsonValue,
    /// `Some(course_id)` when an Apply would refresh an existing
    /// row; `None` for brand-new imports. Computed by the caller via
    /// `courses::find_by_daisy_momenttillf_id` so the staging row
    /// carries the diff context the admin UI needs.
    pub existing_course_id: Option<Uuid>,
}

/// Upsert a staging row keyed on `momenttillf_id`. Subsequent syncs
/// refresh `name`, `semester_label`, daisy metadata, participants,
/// and `existing_course_id`; `first_seen_at` stays pinned to the
/// original sighting so the admin can spot pendings that have been
/// sitting unreviewed for a long time.
pub async fn upsert(db: &PgPool, input: &StageInput<'_>) -> Result<PendingImportRow, sqlx::Error> {
    sqlx::query_as!(
        PendingImportRow,
        r#"INSERT INTO daisy_pending_imports
            (momenttillf_id, course_code, name, semester_label,
             daisy_info_url, daisy_syllabus_url, daisy_unit,
             participants, existing_course_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (momenttillf_id) DO UPDATE SET
            course_code = EXCLUDED.course_code,
            name = EXCLUDED.name,
            semester_label = EXCLUDED.semester_label,
            daisy_info_url = EXCLUDED.daisy_info_url,
            daisy_syllabus_url = EXCLUDED.daisy_syllabus_url,
            daisy_unit = EXCLUDED.daisy_unit,
            participants = EXCLUDED.participants,
            existing_course_id = EXCLUDED.existing_course_id,
            last_seen_at = NOW()
        RETURNING id, momenttillf_id, course_code, name, semester_label,
                  daisy_info_url, daisy_syllabus_url, daisy_unit,
                  participants AS "participants!: JsonValue",
                  existing_course_id, first_seen_at, last_seen_at"#,
        input.momenttillf_id,
        input.course_code,
        input.name,
        input.semester_label,
        input.daisy_info_url,
        input.daisy_syllabus_url,
        input.daisy_unit,
        input.participants,
        input.existing_course_id,
    )
    .fetch_one(db)
    .await
}

/// All staging rows, oldest-first (so the admin sees the oldest
/// unprocessed pendings at the top of the table; freshly-staged
/// rows scroll down).
pub async fn list_all(db: &PgPool) -> Result<Vec<PendingImportRow>, sqlx::Error> {
    sqlx::query_as!(
        PendingImportRow,
        r#"SELECT id, momenttillf_id, course_code, name, semester_label,
                  daisy_info_url, daisy_syllabus_url, daisy_unit,
                  participants AS "participants!: JsonValue",
                  existing_course_id, first_seen_at, last_seen_at
        FROM daisy_pending_imports
        ORDER BY first_seen_at ASC, momenttillf_id ASC"#,
    )
    .fetch_all(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<PendingImportRow>, sqlx::Error> {
    sqlx::query_as!(
        PendingImportRow,
        r#"SELECT id, momenttillf_id, course_code, name, semester_label,
                  daisy_info_url, daisy_syllabus_url, daisy_unit,
                  participants AS "participants!: JsonValue",
                  existing_course_id, first_seen_at, last_seen_at
        FROM daisy_pending_imports
        WHERE id = $1"#,
        id,
    )
    .fetch_optional(db)
    .await
}

/// Delete by id; returns the deleted row (or None if it was already
/// gone). The admin Apply path uses the returned payload to drive
/// the actual `courses` write, so we hand the row back rather than
/// just a bool.
pub async fn delete(db: &PgPool, id: Uuid) -> Result<Option<PendingImportRow>, sqlx::Error> {
    sqlx::query_as!(
        PendingImportRow,
        r#"DELETE FROM daisy_pending_imports WHERE id = $1
        RETURNING id, momenttillf_id, course_code, name, semester_label,
                  daisy_info_url, daisy_syllabus_url, daisy_unit,
                  participants AS "participants!: JsonValue",
                  existing_course_id, first_seen_at, last_seen_at"#,
        id,
    )
    .fetch_optional(db)
    .await
}

pub async fn count(db: &PgPool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(r#"SELECT COUNT(*) AS "n!" FROM daisy_pending_imports"#)
        .fetch_one(db)
        .await?;
    Ok(row.n)
}
