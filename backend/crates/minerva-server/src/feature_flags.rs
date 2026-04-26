//! Application-level feature-flag wrappers.
//!
//! The DB-layer module (`minerva_db::queries::feature_flags`) handles
//! storage and resolution; this module gives the rest of the server
//! crate stable flag-name constants and the small set of "is X enabled
//! here?" helpers we actually call.
//!
//! Flag-name constants live here so a typo in one call site can't
//! desync from another -- everywhere that gates on a flag goes
//! through the same `&'static str`.
//!
//! Default policy: opt-in features default to FALSE so an unset row
//! means "behave as if the feature doesn't exist". Admins flip the
//! flag on per-course (or globally, once we trust it broadly).

use sqlx::PgPool;
use uuid::Uuid;

/// Course knowledge graph V1: per-doc kind classification + cross-doc
/// linker + graph viewer + assignment-refusal addendum + adversarial
/// chunk filter. All KG behaviour gates on this single flag.
pub const FLAG_COURSE_KG: &str = "course_kg";

/// All flags the application currently knows about. The admin UI
/// uses this to enumerate available toggles per course; new flags
/// must be added here AND have a `pub const` above.
pub const ALL_FLAGS: &[&str] = &[FLAG_COURSE_KG];

/// True iff the KG bundle is enabled for this course. Resolution:
/// course-scoped row -> global row -> default (FALSE).
///
/// Errors are logged and treated as "not enabled" -- the safer
/// choice when the DB is flaky, since failing closed avoids
/// emitting half-classified state, mark_dirty noise, etc.
pub async fn course_kg_enabled(db: &PgPool, course_id: Uuid) -> bool {
    match minerva_db::queries::feature_flags::is_enabled_for_course(
        db,
        FLAG_COURSE_KG,
        course_id,
        false,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "feature_flags: course_kg lookup for course {} failed ({}); treating as disabled",
                course_id,
                e,
            );
            false
        }
    }
}
