//! Closed set of document kinds. Mirrored in the SQL CHECK constraint on
//! `documents.kind` (migration 20260425000001_document_kind.sql); keep in
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
    "old_exam",
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
    /// Swedish "övning"; practice/exercise material that's NOT
    /// graded. Distinct from `AssignmentBrief` (graded mandatory
    /// work): the chat path can discuss tutorial exercises freely.
    TutorialExercise,
    AssignmentBrief,
    SampleSolution,
    LabBrief,
    /// An examining exam the student is sitting now (take-home exam).
    Exam,
    /// A past or mock exam published for practice, with or without its
    /// answers. Ordinary course material, unlike `Exam`.
    OldExam,
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
            DocumentKind::OldExam => "old_exam",
            DocumentKind::Syllabus => "syllabus",
            DocumentKind::Unknown => "unknown",
        }
    }

    // Returns `Option<Self>` rather than `Result<Self, _>` because every
    // call site uses `.is_none()` or `.expect()`; matching the
    // `std::str::FromStr` trait shape would force a wrapper error type
    // we'd never inspect. The clippy lint only flagged this once the
    // crate gained a `lib` target in Phase 3 (the binary-only crate
    // never saw this method as part of an exposed API surface).
    #[allow(clippy::should_implement_trait)]
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
            "old_exam" => Some(DocumentKind::OldExam),
            "syllabus" => Some(DocumentKind::Syllabus),
            "unknown" => Some(DocumentKind::Unknown),
            _ => None,
        }
    }
}

/// Kinds that examine the student. Their text is ordinary context, so
/// questions about the work get answered, but a turn that retrieves
/// them runs under the no-full-solution policy and the extraction
/// guard never lets a full solution to them through.
pub fn is_examining_kind(kind: &str) -> bool {
    minerva_db::queries::documents::EXAMINING_KINDS.contains(&kind)
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
    fn only_graded_kinds_are_examining() {
        let examining = ["assignment_brief", "lab_brief", "exam"];
        for k in ALL_KINDS {
            assert_eq!(is_examining_kind(k), examining.contains(k), "kind {}", k);
        }
    }
}
