//! Minerva-side integration layer for the
//! [`eureka-2`](https://github.com/Edwinexd/eureka-2) concept knowledge
//! graph crate.
//!
//! This crate is intentionally thin: it exposes only what the rest of
//! Minerva needs to surface eureka functionality behind the
//! `minerva-server` `eureka` cargo feature. The heavy lifting
//! (extraction, dedup, schema) lives in `eureka-2`.
//!
//! The integration is gated at two layers:
//!
//! 1. The `eureka` cargo feature on `minerva-server` decides whether
//!    the integration is compiled in at all.
//! 2. At runtime, every concept-graph operation is gated on the
//!    `courses.concept_graph_enabled` boolean, so individual courses
//!    can opt in even after the feature has shipped.
//!
//! v0.2 of this crate exposes the eureka-2 surface and the helpers
//! Minerva needs to keep route, ingest, and admin code paths in sync
//! on graph identifiers.

pub use eureka_2;
pub use eureka_2::MIGRATOR as EUREKA_MIGRATOR;

use uuid::Uuid;

/// Compute the eureka-2 graph namespace for a Minerva course.
///
/// `eureka-2` keys graphs by `(namespace, name)`; Minerva uses
/// `minerva:course` as the namespace and the course's UUID as the
/// graph name. Centralising this mapping avoids divergent strings
/// across ingest, query, and admin code paths.
#[must_use]
pub fn namespace_for_course() -> &'static str {
    "minerva:course"
}

/// Compute the eureka-2 graph name for a Minerva course id (i64).
#[must_use]
pub fn graph_name_for_course(course_id: i64) -> String {
    course_id.to_string()
}

/// Compute the eureka-2 graph name for a Minerva course UUID.
///
/// Minerva's `courses.id` is a UUID; eureka-2 graph names are arbitrary
/// strings. Using the UUID's hyphenated string form keeps the mapping
/// stable across restarts and human-readable in logs / GraphML exports.
#[must_use]
pub fn graph_name_for_course_uuid(course_id: Uuid) -> String {
    course_id.to_string()
}

/// Apply the eureka-2 schema migrations on top of Minerva's pool.
///
/// Safe to run unconditionally on startup: `eureka-2`'s migrations are
/// namespaced under the `eureka_` table prefix and won't collide with
/// Minerva's tables. Returns immediately if the migrations have already
/// been applied.
pub async fn apply_migrations(pool: &sqlx::PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    EUREKA_MIGRATOR.run(pool).await
}
