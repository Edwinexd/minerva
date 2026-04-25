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

- "lecture": slides, lecture notes, recordings transcripts, instructor-authored expository material teaching a topic.
- "reading": textbook chapters, papers, supplementary articles, links to external readings.
- "assignment_brief": the description of an assignment students must complete and submit. Numbered steps, "your task", "implement", grading criteria, deliverables, due dates.
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
- If a document contains both an exercise statement AND its solution, classify as "sample_solution" -- the solution-bearing nature dominates.
- If a document is mostly a worked example used for teaching (not the answer to a graded problem), classify as "lecture" or "reading", not "sample_solution".
- Filenames are weak signals; prefer the actual content. But if filename strongly suggests "solution"/"answer"/"key"/"facit"/"lösningsförslag", flag "might_be_solution" even if you ultimately classify otherwise.
- Be calibrated: confidence should reflect actual uncertainty. If the document is 3 pages of mixed content with no clear signal, that's 0.4--0.6, not 0.95.
"#;

/// User-message template. `{filename}`, `{mime_type}`, `{excerpt}` are
/// substituted at call time. The excerpt is head-then-tail-truncated by
/// `document::truncate_for_classification`.
pub const CLASSIFIER_USER_TEMPLATE: &str = r#"filename: {filename}
mime_type: {mime_type}

document excerpt (may be truncated):
---
{excerpt}
---

Reply with the JSON object only."#;

/// Bullet added to the base system prompt's "What you will not do" list.
/// Kept short -- most of the heavy lifting is the per-turn addendum below
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
