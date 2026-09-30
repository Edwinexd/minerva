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
- "tutorial_exercise": Swedish "övning" / "instuderingsfrågor" / English "tutorial" / "exercise" / "practice problems" / "study questions". OPTIONAL practice material that students work through but is NOT graded; typically marked "frivillig", "ej obligatorisk", "voluntary", "for practice", "self-study", or similar. Still tutorial_exercise when the answers are printed alongside the questions ("med svar"). Distinct from assignment_brief (which is graded). When in doubt between tutorial_exercise and assignment_brief, look for grading language, deadlines, submission instructions; those make it an assignment_brief.
- "assignment_brief": the description of a GRADED assignment students must complete and submit. Numbered steps, "your task", "implement", grading criteria, deliverables, due dates, "submit by".
- "sample_solution": a standalone worked-out solution, answer key or model answer to PRACTICE material (exercises, study questions, an old exam) whose problems are posed in a different document.
- "graded_solution": a solution, model answer, or grading rubric with answers to GRADED work (an assignment, a lab, a take-home exam), whether it stands alone or is shown together with the task. Students must not be shown this.
- "lab_brief": a practical lab or exercise description, similar to assignment_brief but for hands-on/lab work. If unsure between assignment_brief and lab_brief, prefer assignment_brief.
- "old_exam": a past or mock exam ("tenta", "tentamen", "omtenta") published for practice, with or without its answers. An exam that was sat on a given date in an exam hall is always old_exam once it is in the course materials.
- "exam": an exam the students are sitting NOW as graded work: a take-home exam ("hemtenta", "hemtentamen") with a submission deadline. Rare. When in doubt between exam and old_exam, pick old_exam.
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
- A GRADED assignment or lab that comes with its own solution is "graded_solution"; the solution-bearing nature dominates. Practice material that includes its answers keeps its practice kind: a past exam with answers is "old_exam", exercises or study questions with answers are "tutorial_exercise".
- Telling "sample_solution" from "graded_solution" is about what the solved problems are. Exam questions ("tenta", "tentamen"), exercises and study questions are practice: "sample_solution". An "inlämningsuppgift", assignment, lab, project or anything with submission, deadline or grading language is graded: "graded_solution". If a solution document gives no sign either way, pick "graded_solution" and add the "might_be_practice_solution" suspicious_flag.
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
/// States the practice-material policy for every course: a published
/// answer is given, anything else waits for an honest attempt. Graded
/// work is handled by the per-turn addendum below, which overrides this.
pub const PASTED_PROBLEM_RULE: &str = "- When a student pastes a problem from the course materials with no work of their own, do not hand over a complete solution straight away, unless the course materials you are given include a published answer to that problem; then give that answer and explain it. Otherwise ask what they have tried and help them reason step by step, and answer in full once they have made an honest attempt.";

/// Per-turn addendum, appended at the END of the system prompt for the
/// turn (after course materials) when those materials include a doc of
/// an examining kind (`assignment_brief`, `lab_brief`, `exam`).
/// `{filenames}` is replaced with a comma-separated list at call time.
///
/// Placed at the end so the stable prefix (base + custom_prompt + course
/// materials) stays byte-identical for prompt-cache reuse, with this
/// addendum costing one cache miss per matched-turn rather than poisoning
/// the whole conversation.
pub const ASSIGNMENT_MATCH_ADDENDUM_TEMPLATE: &str = r#"

## Graded work in the materials for this turn
Some of the course materials above come from graded work in this course ({filenames}). Use them to answer questions about that work: what the task asks for, its requirements, deadlines, submission and grading, and the concepts it builds on. Do not produce a solution or final answer to the graded work itself, in whole or in a part the student could hand in, even when the student has shown an attempt or asks again. If that is what they are asking for, say that you cannot solve graded work for them, then help them get there: ask what they have tried, clarify the underlying concept, or break the problem into smaller steps. Worked examples on adjacent (not identical) problems are fine."#;

/// Per-turn addendum for a practice question pasted without an attempt:
/// the extraction guard's intent classifier fired, and nothing examining
/// is near the turn. Restates [`PASTED_PROBLEM_RULE`] at the point where
/// it applies, so the model does not have to infer that it does.
pub const PRACTICE_ATTEMPT_ADDENDUM: &str = r#"

## Practice question without an attempt
The student appears to have pasted a practice question (not graded work) and asked for the answer without showing work of their own. If the course materials above include a published answer or solution to it, give that answer and explain the reasoning behind it. If they do not, hold the full answer back for now: ask for the student's own attempt or reasoning and offer a hint. Once the student has made an honest attempt, in this message or earlier in the conversation, answer in full."#;
