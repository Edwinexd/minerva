//! Daisy offerings linked to a Minerva course.
//!
//! A Minerva course can map to many Daisy offerings (e.g. the same
//! project course delivered as both a 7.5 and a 15 ECTS offering, each
//! with its own momenttillfID). The daily Daisy sync matches on
//! `momenttillf_id` here and feeds whichever course the offering points
//! at; the admin course-merge re-points a source's offerings at the
//! survivor so both keep syncing into one course. See migration
//! `20260528000001_course_daisy_offerings.sql`.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct DaisyOfferingRow {
    /// Daisy momenttillfID, e.g. `7620`. Globally unique (one offering
    /// maps to exactly one Minerva course).
    pub momenttillf_id: String,
    pub course_id: Uuid,
    /// Daisy `beteckning`, e.g. `PROG2`.
    pub course_code: Option<String>,
    /// Swedish course name for this offering.
    pub name: Option<String>,
    pub semester_label: Option<String>,
    pub info_url: Option<String>,
    pub syllabus_url: Option<String>,
    pub unit: Option<String>,
    pub last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Every Daisy offering linked to a course, oldest-first. Empty for
/// manually-created (non-Daisy) courses.
pub async fn list_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<DaisyOfferingRow>, sqlx::Error> {
    sqlx::query_as!(
        DaisyOfferingRow,
        r#"SELECT momenttillf_id, course_id, course_code, name, semester_label,
                  info_url, syllabus_url, unit, last_synced_at, created_at
           FROM course_daisy_offerings
           WHERE course_id = $1
           ORDER BY created_at ASC, momenttillf_id ASC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

/// Fetch a single offering by its Daisy momenttillfID. Used by the
/// staging diff to read the snapshot the last apply wrote, so the
/// admin review page can show which metadata fields a re-apply would
/// actually change rather than a blanket "Update".
pub async fn find_by_momenttillf_id(
    db: &PgPool,
    momenttillf_id: &str,
) -> Result<Option<DaisyOfferingRow>, sqlx::Error> {
    sqlx::query_as!(
        DaisyOfferingRow,
        r#"SELECT momenttillf_id, course_id, course_code, name, semester_label,
                  info_url, syllabus_url, unit, last_synced_at, created_at
           FROM course_daisy_offerings
           WHERE momenttillf_id = $1"#,
        momenttillf_id,
    )
    .fetch_optional(db)
    .await
}

/// Bump `last_synced_at` on a single offering without touching its
/// metadata. Called after the Daisy apply finishes its membership
/// additions so the timestamp reflects the full sync.
pub async fn touch_synced(db: &PgPool, momenttillf_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE course_daisy_offerings SET last_synced_at = NOW() WHERE momenttillf_id = $1",
        momenttillf_id,
    )
    .execute(db)
    .await?;
    Ok(())
}
