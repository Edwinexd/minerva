//! Stable text constants for classifier and chat-time prompts.
//!
//! Keeping these as `const` (not `format!`-built) helps two things:
//! 1. **Cerebras prompt cache.** A byte-stable system prompt across calls
//!    keeps cache hits warm. Mutable parts (filename, doc text) live in
//!    the user message.
//! 2. **Reviewability.** All the language a student or teacher might
//!    eventually see is in one file.

/// System prompt for the per-document classifier. The model is required
/// to return JSON matching the schema declared in `document.rs`.
pub const CLASSIFIER_SYSTEM_PROMPT: &str = r#"You classify a single course document into one of these kinds:

- "lecture": slides, lecture notes, instructor-authored expository material teaching a topic. Structured, prepared content.
- "lecture_transcript": auto-generated speech-to-text transcript of a lecture recording (verbatim spoken language, often with timestamps, filler words, "um/uh", incomplete sentences, no headings). Same teaching purpose as a lecture but the prose is messy and unstructured. Pick this over "lecture" when the text reads like a transcription rather than prepared notes/slides.
- "reading": textbook chapters, papers, supplementary articles, links to external readings.
- "tutorial_exercise": Swedish "övning" / English "tutorial" / "exercise" / "practice problems". OPTIONAL practice material that students work through but is NOT graded; typically marked "frivillig", "ej obligatorisk", "voluntary", "for practice", "self-study", or similar. Distinct from assignment_brief (which is graded). When in doubt between tutorial_exercise and assignment_brief, look for grading language, deadlines, submission instructions; those make it an assignment_brief.
- "assignment_brief": the description of a GRADED assignment students must complete and submit. Numbered steps, "your task", "implement", grading criteria, deliverables, due dates, "submit by".
- "sample_solution": a worked-out solution, model answer, grading rubric with answers, or any document whose primary purpose is to show students the answer to a graded problem.
- "lab_brief": a practical lab or exercise description, similar to assignment_brief but for hands-on/lab work. If unsure between assignment_brief and lab_brief, prefer assignment_brief.
- "exam": past exams, mock exams, exam-style problem sets without solutions.
- "syllabus": course overview, schedule, policies, admin/logistics, reading list, learning objectives.
- "unknown": none of the above clearly applies, or the document is genuinely off-topic.

You will reply with a single JSON object, nothing else, matching the schema:

{
  "kind": one of the strings above,
  "confidence": float in [0.0, 1.0],
  "rationale": short string (one sentence, < 200 chars),
  "suspicious_flags": array of zero or more short strings flagging things the user (a teacher) might want to double-check, e.g. "might_be_solution", "contains_worked_examples", "ambiguous_between_assignment_and_lab", "could_be_exam_with_solutions".
}

Important guidance:
- Classify based on the actual content of the document. You are NOT given the filename, because filenames are unreliable: courses routinely contain "F18_OO.pdf" that is actually a solution, "lab.pdf" that's a syllabus, "övning.pdf" that's a graded assignment. The mime_type tells you only the file format.
- If a document contains both an exercise statement AND its solution, classify as "sample_solution"; the solution-bearing nature dominates.
- If a document is mostly a worked example used for teaching (not the answer to a graded problem), classify as "lecture" or "reading", not "sample_solution".
- Distinguishing tutorial_exercise from assignment_brief is a CONTENT decision: look for grading language ("graded", "submit by", "deadline", "betyg", "inlämning"), submission instructions, and rubrics. Their absence; combined with explicit "frivillig", "voluntary", "for self-study", "practice problems" framing; points to tutorial_exercise.
- Be calibrated: confidence should reflect actual uncertainty. If the document is 3 pages of mixed content with no clear signal, that's 0.4--0.6, not 0.95.
- If the excerpt is empty or near-empty (e.g. a URL stub, a scanned PDF without OCR, an unsupported file the extractor couldn't read), classify as "unknown" with low confidence and add a "no_text_extracted" suspicious_flag.
- The "suspicious_flags" array lets you escalate things a teacher might want to double-check: e.g. "might_be_solution" when the content has worked-out answers, "ambiguous_between_assignment_and_lab", "could_be_exam_with_solutions", "language_mixed_swedish_english". Use these to surface uncertainty, not to dilute the kind decision.
"#;

/// User-message template. `{mime_type}`, `{excerpt}` are substituted
/// at call time. The excerpt is head-then-tail-truncated by
/// `document::truncate_for_classification`.
///
/// Filename is intentionally NOT included: filenames in real DSV
/// courses are too unreliable to be a signal (lecturers reuse
/// templates, copy/paste from previous semesters with stale names,
/// upload "F18_OO.pdf" that's actually a solution, etc.). Classifier
/// must decide from the document's actual content; the structural
/// linker pass uses filename markers separately for *pairing*, which
/// is a different problem.
pub const CLASSIFIER_USER_TEMPLATE: &str = r#"mime_type: {mime_type}

document excerpt (may be truncated):
---
{excerpt}
---

Reply with the JSON object only."#;

/// Bullet added to the base system prompt's "What you will not do" list.
/// Kept short; most of the heavy lifting is the per-turn addendum below
/// when an actual assignment_brief similarity match is detected.
pub const PASTED_PROBLEM_RULE: &str = "- Do not produce a complete solution to a problem the student has pasted verbatim from course materials with no work of their own; instead help them reason about it step by step.";

/// Per-turn addendum, appended at the END of the system prompt for the
/// turn (after course materials) when retrieval surfaces a high-similarity
/// match against a doc whose `kind` is `assignment_brief|lab_brief|exam`.
/// `{filenames}` is replaced with a comma-separated list at call time.
///
/// Placed at the end so the stable prefix (base + custom_prompt + course
/// materials) stays byte-identical for prompt-cache reuse, with this
/// addendum costing one cache miss per matched-turn rather than poisoning
/// the whole conversation.
pub const ASSIGNMENT_MATCH_ADDENDUM_TEMPLATE: &str = r#"

## Assignment match for this turn
The student's input has high similarity to assignment material in this course ({filenames}). Do not produce a complete solution. Instead: ask what they have already tried, clarify the underlying concept, or break the problem into smaller steps. Discussing concepts and giving worked examples on adjacent (not identical) problems is fine."#;
