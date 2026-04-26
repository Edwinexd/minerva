//! Closed set of document kinds. Mirrored in the SQL CHECK constraint on
//! `documents.kind` (migration 20260425000001_document_kind.sql) -- keep in
//! sync.

/// Canonical kind strings. The DB CHECK constraint enforces this exact
/// set; we re-validate at the API boundary so a teacher PATCH with a
/// junk value is a 400 rather than a 500 from the DB.
pub const ALL_KINDS: &[&str] = &[
    "lecture",
    "lecture_transcript",
    "reading",
    "tutorial_exercise",
    "assignment_brief",
    "sample_solution",
    "lab_brief",
    "exam",
    "syllabus",
    "unknown",
];

/// Strongly-typed view used by the classifier and tests. The DB layer
/// stores strings (matching the CHECK constraint), so we serialise via
/// `as_str` rather than carrying the enum across the crate boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Lecture,
    /// Auto-generated speech-to-text transcript from a lecture
    /// recording. Same semantic role as `Lecture` but typically
    /// noisier; kept distinct so teachers can spot why retrieval
    /// surfaces awkward speech-to-text blocks.
    LectureTranscript,
    Reading,
    /// Swedish "övning" -- practice/exercise material that's NOT
    /// graded. Distinct from `AssignmentBrief` (graded mandatory
    /// work): the chat path can discuss tutorial exercises freely.
    TutorialExercise,
    AssignmentBrief,
    SampleSolution,
    LabBrief,
    Exam,
    Syllabus,
    Unknown,
}

impl DocumentKind {
    /// Stable string form, matching the SQL CHECK enum. Used by the
    /// route handler and backfill binary; tests round-trip via
    /// `DocumentKind::from_str(self.as_str()) == Some(self)`.
    #[allow(dead_code)] // used by the kind-override route handler (V2 commit)
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentKind::Lecture => "lecture",
            DocumentKind::LectureTranscript => "lecture_transcript",
            DocumentKind::Reading => "reading",
            DocumentKind::TutorialExercise => "tutorial_exercise",
            DocumentKind::AssignmentBrief => "assignment_brief",
            DocumentKind::SampleSolution => "sample_solution",
            DocumentKind::LabBrief => "lab_brief",
            DocumentKind::Exam => "exam",
            DocumentKind::Syllabus => "syllabus",
            DocumentKind::Unknown => "unknown",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "lecture" => Some(DocumentKind::Lecture),
            "lecture_transcript" => Some(DocumentKind::LectureTranscript),
            "reading" => Some(DocumentKind::Reading),
            "tutorial_exercise" => Some(DocumentKind::TutorialExercise),
            "assignment_brief" => Some(DocumentKind::AssignmentBrief),
            "sample_solution" => Some(DocumentKind::SampleSolution),
            "lab_brief" => Some(DocumentKind::LabBrief),
            "exam" => Some(DocumentKind::Exam),
            "syllabus" => Some(DocumentKind::Syllabus),
            "unknown" => Some(DocumentKind::Unknown),
            _ => None,
        }
    }
}

/// Kinds whose chunks must NEVER appear in the prompt context. They may
/// be embedded into Qdrant for similarity-based detection (so the chat
/// path can recognise that a student's input matches an assignment), but
/// the chunk *text* never lands in the system prompt.
///
/// `sample_solution` is the strongest case -- those docs aren't even
/// embedded; the worker short-circuits before it gets to the embedder.
/// This list catches stale data and the assignment-brief signal channel.
pub fn is_signal_only_kind(kind: &str) -> bool {
    matches!(
        kind,
        "assignment_brief" | "lab_brief" | "exam" | "sample_solution"
    )
}

/// Kinds that should never be embedded into Qdrant in the first place.
/// Currently just `sample_solution` -- the others stay in Qdrant as a
/// detection signal even though their text never enters the prompt.
///
/// This is the contract the ingest pipeline enforces; exposed for tests
/// and the planned backfill binary so they stay consistent with it.
#[allow(dead_code)] // referenced by tests below + planned backfill binary
pub fn skips_embedding(kind: &str) -> bool {
    kind == "sample_solution"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_kinds() {
        for s in ALL_KINDS {
            let k = DocumentKind::from_str(s).expect("known kind");
            assert_eq!(k.as_str(), *s);
        }
    }

    #[test]
    fn unknown_strings_reject() {
        assert!(DocumentKind::from_str("essay").is_none());
        assert!(DocumentKind::from_str("").is_none());
    }

    #[test]
    fn signal_only_kinds_are_correct() {
        assert!(is_signal_only_kind("assignment_brief"));
        assert!(is_signal_only_kind("lab_brief"));
        assert!(is_signal_only_kind("exam"));
        assert!(is_signal_only_kind("sample_solution"));
        assert!(!is_signal_only_kind("lecture"));
        assert!(!is_signal_only_kind("lecture_transcript"));
        assert!(!is_signal_only_kind("reading"));
        // tutorial_exercise is NOT signal-only: it's optional practice
        // material the chat path is allowed to walk through with the
        // student, unlike the graded assessment kinds.
        assert!(!is_signal_only_kind("tutorial_exercise"));
        assert!(!is_signal_only_kind("syllabus"));
        assert!(!is_signal_only_kind("unknown"));
    }

    #[test]
    fn only_sample_solution_skips_embedding() {
        assert!(skips_embedding("sample_solution"));
        for k in ALL_KINDS.iter().filter(|k| **k != "sample_solution") {
            assert!(
                !skips_embedding(k),
                "kind {} unexpectedly skips embedding",
                k
            );
        }
    }
}
