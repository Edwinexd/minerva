//! Application-level feature-flag wrappers.
//!
//! The DB-layer module (`minerva_db::queries::feature_flags`) handles
//! storage and resolution; this module gives the rest of the server
//! crate stable flag-name constants and the small set of "is X enabled
//! here?" helpers we actually call.
//!
//! Flag-name constants live here so a typo in one call site can't
//! desync from another; everywhere that gates on a flag goes
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

/// Extraction guard: pre-generation intent classifier (catches
/// pasted-assignment-asking-for-implementation), output-side
/// solution-detection check, Socratic rewriter, multi-turn
/// proximity tracking via the KG. Independently flagged from
/// `course_kg` so admins can opt courses into the graph view
/// without the harder student-facing constraints (or vice versa
/// once the guard stabilises).
pub const FLAG_EXTRACTION_GUARD: &str = "extraction_guard";

/// Aegis: prompt-coaching feedback panel. When on, every user
/// turn is scored by a small LLM along five dimensions (clarity,
/// context, constraints, reasoning demand, critical thinking) and
/// surfaced to the student in a non-blocking right-rail panel
/// alongside per-turn analysis history. Designed to nudge
/// students toward more intentional prompting without gating the
/// inference path; the analysis call runs in parallel with the
/// generation strategy and never blocks the assistant reply.
/// See `crate::classification::aegis` for the analyzer.
pub const FLAG_AEGIS: &str = "aegis";

/// Concept knowledge graph (eureka-2). Distinct from `course_kg`,
/// which is the document-level relation graph. When on for a
/// course, admins can run per-document concept extraction via the
/// `minerva-eureka` integration crate; the resulting concept graph
/// (vertices, edges, supports) is admin-viewable and the eureka
/// migrations are applied on app startup. Toggling off does not
/// drop the persisted graph; it just hides the admin endpoints.
pub const FLAG_CONCEPT_GRAPH: &str = "concept_graph";

/// Study mode: turns the course into a research-evaluation pipeline
/// (consent screen -> pre-survey -> N hardcoded tasks -> post-survey
/// -> thank-you + lockout). Configuration lives in the `study_courses`
/// table plus `study_tasks` and `study_surveys`; this flag is the
/// runtime gate that activates the pipeline. Forces Aegis on for the
/// duration of the study regardless of the course's own aegis flag,
/// so researchers don't have to remember to set both. See
/// `crate::routes::study` for the participant + admin endpoints.
pub const FLAG_STUDY_MODE: &str = "study_mode";

/// All flags the application currently knows about. The admin UI
/// uses this to enumerate available toggles per course; new flags
/// must be added here AND have a `pub const` above.
pub const ALL_FLAGS: &[&str] = &[
    FLAG_COURSE_KG,
    FLAG_EXTRACTION_GUARD,
    FLAG_AEGIS,
    FLAG_CONCEPT_GRAPH,
    FLAG_STUDY_MODE,
];

/// True iff the KG bundle is enabled for this course. Resolution:
/// course-scoped row -> global row -> default (FALSE).
///
/// Errors are logged and treated as "not enabled"; the safer
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

/// True iff the extraction guard is enabled for this course. Same
/// resolution + fail-closed semantics as `course_kg_enabled`.
/// Used by the chat strategies (wired in a follow-up commit).
#[allow(dead_code)]
pub async fn extraction_guard_enabled(db: &PgPool, course_id: Uuid) -> bool {
    match minerva_db::queries::feature_flags::is_enabled_for_course(
        db,
        FLAG_EXTRACTION_GUARD,
        course_id,
        false,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "feature_flags: extraction_guard lookup for course {} failed ({}); treating as disabled",
                course_id,
                e,
            );
            false
        }
    }
}

/// True iff aegis prompt-coaching is enabled for this course at the
/// course/umbrella level.
///
/// Resolution: study mode forces TRUE (study mode treats Aegis as
/// part of the experimental condition, so the per-course flag is
/// irrelevant); otherwise course-scoped row -> global row ->
/// default (FALSE). Errors are logged and treated as "not enabled";
/// the analyzer runs on every user turn so a flaky DB shouldn't slow
/// down the chat path with retries; falling closed reverts to
/// pre-aegis behaviour transparently.
///
/// NOTE: this is the umbrella. For chat-path decisions that should
/// also respect study mode's per-task on/off rounds, use
/// [`aegis_enabled_for_conversation`]; it short-circuits to the
/// per-task `study_tasks.aegis_enabled` when the conversation maps
/// to a study task.
pub async fn aegis_enabled(db: &PgPool, course_id: Uuid) -> bool {
    if study_mode_enabled(db, course_id).await {
        return true;
    }
    match minerva_db::queries::feature_flags::is_enabled_for_course(
        db, FLAG_AEGIS, course_id, false,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "feature_flags: aegis lookup for course {} failed ({}); treating as disabled",
                course_id,
                e,
            );
            false
        }
    }
}

/// Per-conversation Aegis gate.
///
/// In study mode the umbrella `aegis_enabled` is forced TRUE for the
/// whole course, but individual rounds opt out: the DM2731 design has
/// round 1 + 3 without support and round 2 with. Each task gets its
/// own conversation (see `study_task_conversations`), so the
/// conversation_id is the natural key for "which round is the user
/// on right now".
///
/// Resolution order:
///   1. `aegis_enabled(course_id)`; if the umbrella is off, the
///      answer is off and we don't need to do the per-task lookup.
///   2. Otherwise look up `study_tasks.aegis_enabled` joined through
///      `study_task_conversations` on the given conversation_id.
///      Found row wins; the per-task flag is the gate.
///   3. No mapping row (regular chat, or no conversation_id supplied)
///      -> fall back to the umbrella (which is TRUE here, since
///      step 1 didn't short-circuit).
///
/// DB errors during the per-task lookup log at warn and treat the
/// round as Aegis-enabled (degraded-open rather than degraded-closed);
/// the alternative would silently break round 2 on a flaky DB, which
/// is the worse failure mode for the eval.
pub async fn aegis_enabled_for_conversation(
    db: &PgPool,
    course_id: Uuid,
    conversation_id: Option<Uuid>,
) -> bool {
    if !aegis_enabled(db, course_id).await {
        return false;
    }
    let Some(conv_id) = conversation_id else {
        return true;
    };
    match minerva_db::queries::study::get_aegis_enabled_for_task_conversation(db, conv_id).await {
        Ok(Some(per_task)) => per_task,
        Ok(None) => true,
        Err(e) => {
            tracing::warn!(
                "feature_flags: per-task aegis lookup for conv {} failed ({}); treating as enabled",
                conv_id,
                e,
            );
            true
        }
    }
}

/// True iff study mode is enabled for this course. Resolution:
/// course-scoped row -> global row -> default (FALSE). Errors are
/// logged and treated as "not enabled"; the lockout + pipeline
/// surface only exists for participants in a known study, so falling
/// closed degrades to "regular course" rather than locking everyone
/// out behind an unrenderable thank-you screen.
pub async fn study_mode_enabled(db: &PgPool, course_id: Uuid) -> bool {
    match minerva_db::queries::feature_flags::is_enabled_for_course(
        db,
        FLAG_STUDY_MODE,
        course_id,
        false,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "feature_flags: study_mode lookup for course {} failed ({}); treating as disabled",
                course_id,
                e,
            );
            false
        }
    }
}

/// True iff the eureka concept-graph integration is enabled for
/// this course. Same resolution + fail-closed semantics as
/// `course_kg_enabled`. Gates the admin endpoints in
/// `routes::admin::concept_graph` and any future read-side
/// integrations.
pub async fn concept_graph_enabled(db: &PgPool, course_id: Uuid) -> bool {
    match minerva_db::queries::feature_flags::is_enabled_for_course(
        db,
        FLAG_CONCEPT_GRAPH,
        course_id,
        false,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "feature_flags: concept_graph lookup for course {} failed ({}); treating as disabled",
                course_id,
                e,
            );
            false
        }
    }
}
