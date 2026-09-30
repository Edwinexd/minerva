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

/// Document classification: every ingested document gets a `kind`
/// (lecture, assignment_brief, old_exam, graded_solution, ...), which
/// teachers can override, and the chat path handles each chunk by its
/// document's kind: graded work may be asked about but not solved,
/// solutions to graded work are withheld, unclassified documents wait.
/// The base the two flags below build on.
pub const FLAG_DOCUMENT_KINDS: &str = "document_kinds";

/// Course knowledge graph: the cross-document linker, the graph
/// viewer, and graph-driven context expansion. Needs
/// `document_kinds`, since the linker reads kinds.
pub const FLAG_COURSE_KG: &str = "course_kg";

/// Extraction guard: intent classifier, reply check with Socratic
/// rewrite on graded work, attempt-first nudge on practice questions,
/// teacher flags. Needs `document_kinds`, which is how it knows what
/// is graded; independent of `course_kg`.
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

/// All flags the application currently knows about. The admin UI
/// uses this to enumerate available toggles per course; new flags
/// must be added here AND have a `pub const` above.
pub const ALL_FLAGS: &[&str] = &[
    FLAG_DOCUMENT_KINDS,
    FLAG_COURSE_KG,
    FLAG_EXTRACTION_GUARD,
    FLAG_AEGIS,
    FLAG_CONCEPT_GRAPH,
];

/// Resolution: course-scoped row -> global row -> default (FALSE).
///
/// Every flag fails closed: a lookup error is logged and reported as
/// "not enabled". Each of these reads sits on a request path that must
/// not retry, so a flaky DB degrades to pre-feature behaviour rather
/// than stalling the caller.
async fn flag_enabled(db: &PgPool, flag: &'static str, course_id: Uuid) -> bool {
    match minerva_db::queries::feature_flags::is_enabled_for_course(db, flag, course_id, false)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "feature_flags: {} lookup for course {} failed ({}); treating as disabled",
                flag,
                course_id,
                e,
            );
            false
        }
    }
}

/// True iff documents in this course are classified and handled by
/// kind. Failing closed avoids half-classified state.
pub async fn document_kinds_enabled(db: &PgPool, course_id: Uuid) -> bool {
    flag_enabled(db, FLAG_DOCUMENT_KINDS, course_id).await
}

/// True iff the knowledge graph is enabled for this course, which
/// takes `document_kinds` as well: a graph flag on a course without
/// classification has nothing to link and resolves to off.
pub async fn course_kg_enabled(db: &PgPool, course_id: Uuid) -> bool {
    flag_enabled(db, FLAG_COURSE_KG, course_id).await && document_kinds_enabled(db, course_id).await
}

/// True iff the extraction guard is enabled for this course, which
/// takes `document_kinds` as well: without kinds the guard cannot tell
/// graded work from practice and resolves to off.
pub async fn extraction_guard_enabled(db: &PgPool, course_id: Uuid) -> bool {
    flag_enabled(db, FLAG_EXTRACTION_GUARD, course_id).await
        && document_kinds_enabled(db, course_id).await
}

/// True iff aegis prompt-coaching is enabled for this course at the
/// course/umbrella level. Failing closed reverts to pre-aegis
/// behaviour transparently.
pub async fn aegis_enabled(db: &PgPool, course_id: Uuid) -> bool {
    flag_enabled(db, FLAG_AEGIS, course_id).await
}

/// True iff the eureka concept-graph integration is enabled for
/// this course. Gates the admin endpoints in
/// `routes::admin::concept_graph` and any future read-side
/// integrations.
pub async fn concept_graph_enabled(db: &PgPool, course_id: Uuid) -> bool {
    flag_enabled(db, FLAG_CONCEPT_GRAPH, course_id).await
}
