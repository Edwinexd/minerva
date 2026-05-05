//! Aegis: prompt-coaching analyzer.
//!
//! When the `aegis` feature flag is on for a course, every
//! debounced keystroke (and every Send) fires a single Cerebras
//! call that examines the student's draft and returns 0..=2
//! actionable suggestions for how to improve it.
//!
//! We cap at TWO suggestions, not three, because pilot feedback
//! made it loud and clear that one short prompt yielding three
//! ideas reads as overwhelming; the user feels graded rather than
//! coached. Two leaves room for a primary fix plus an optional
//! follow-on without crowding the panel.
//!
//! We deliberately do NOT score the prompt. The brief is explicit
//! about tone; a partner offering ideas, not a grader handing out
//! marks; and Herodotou et al. (2025) flag the condescending edge
//! that scoring rubrics carry for accessibility-sensitive learners.
//! Scores also hide the actionable signal under a number; "your
//! prompt is 4/10" tells the student nothing they can do about it,
//! whereas "say what you've already tried so the assistant can
//! build on it" does.
//!
//! Each suggestion has:
//!   * `kind` ; short tag the panel uses for grouping/iconography
//!     ("context", "constraints", "specificity", "alternatives",
//!     "clarification"). Free-form string so we can add categories
//!     without a server change.
//!   * `text` ; single-sentence actionable improvement, written
//!     in the second person ("you might..."), no markdown, no
//!     leading bullet.
//!   * `explanation` ; one to two sentences elaborating WHY the
//!     fix matters and what the student should consider when
//!     applying it. Hidden behind a click-to-expand on the panel
//!     so the default view stays low-noise; the student opts in
//!     to the longer "more info" content per suggestion.
//!
//! Empty suggestion array = "the prompt is fine, no suggestions".
//! The panel renders a small "looks good" affirmation rather than
//! pretending there's always something to fix.
//!
//! Soft-fail throughout. The chat hot path never waits on the
//! analyzer; a transient Cerebras hiccup, malformed JSON, or DB
//! insert failure logs at warn and we move on without a row.
//!
//! Mode (`AegisMode::Beginner`/`Expert`) is the student's
//! self-declared subject expertise. Calibrates the rubric so a
//! beginner gets lenient feedback (the chatbot's job is to
//! scaffold) while an expert is held to a higher bar (precise
//! terms, named constraints, what they've tried).

use sqlx::PgPool;
use uuid::Uuid;

use crate::strategy::common::{cerebras_request_with_retry, record_cerebras_usage};
use minerva_db::queries::course_token_usage::CATEGORY_AEGIS;

/// Tiny model; runs on every debounced keystroke + every Send
/// when the flag is on. Latency is the headline number. The
/// schema-constrained output keeps llama3.1-8b on rails for the
/// JSON shape we want. `pub` so the route layer can stamp
/// `model_used` on persisted rows.
pub const AEGIS_MODEL: &str = "llama3.1-8b";

/// Larger model for the rewrite path only; runs rarely (only when
/// the student opens the Review tray, picks answers, and clicks
/// Preview) and produces text the student reads, so quality
/// matters. Mirrors `extraction_guard`'s split: cheap llama on the
/// hot path, gpt-oss-120b on the student-facing rewrite. Pilot
/// users complained the llama rewrite read like a placeholder
/// ("specify what you mean and explain what you're trying to
/// achieve, such as..."); gpt-oss has the headroom to actually
/// weave the student's selected answers into a clean revision.
pub const AEGIS_REWRITE_MODEL: &str = "gpt-oss-120b";

/// Cap on the analyzer's reply. Two suggestions @ ~25 words each
/// for `text` + ~50 words each for `explanation` + 3-4 short
/// answer options each + JSON envelope tokens. Bumped from 512
/// once the `options` field landed; the per-suggestion options
/// add ~50-80 tokens and the previous cap was clipping the
/// second suggestion's options array on dense drafts.
const AEGIS_MAX_TOKENS: usize = 768;

/// Hard ceiling on suggestion count. The system prompt asks for
/// 0..=2; the route layer + the `from_verdict` mapper truncate at
/// the same number. Two channels enforcing the same number keeps
/// a model that overshoots from leaking more than this many
/// suggestions through to the panel.
pub const AEGIS_SUGGESTIONS_MAX: usize = 2;

/// How many previous user turns we feed to the analyzer for context.
/// Five is enough that "explain that further" reads as well-grounded
/// follow-up rather than a context-free prompt.
const HISTORY_TURNS: usize = 5;

const AEGIS_SYSTEM_PROMPT_BASE: &str = r#"You are Aegis, a prompt-coaching assistant for university students using a course-aware tutoring chatbot. You help the student write better prompts by offering concrete suggestions, never by grading them.

You will read the student's current draft (and a short trail of their recent prior prompts in the same conversation, for context). Your job is to produce 0..=2 suggestions for how the student could improve THIS draft before sending it. Two is the hard ceiling; pilot users found three suggestions overwhelming and reported feeling graded rather than coached. When in doubt, return ONE.

The rubric you check the draft against is grounded in the CLEAR prompt-engineering framework (Concise, Logical, Explicit, Adaptive, Reflective) and the standard prompt-design rubric. Each `kind` below maps to one rubric dimension:

* `clarity`    ; Is the request specific and unambiguous? Are key terms defined? Will the chatbot have to guess at multiple plausible interpretations? (CLEAR: Concise + Explicit.)
* `rationale`  ; Is there enough background/context to interpret the request? Why does the student want this; the underlying purpose? Without it the chatbot can answer the literal question and miss the actual goal.
* `audience`   ; Has the student named who they are (course level, prior knowledge, role)? Should the answer assume a beginner or someone steeped in the topic? Determines the language, complexity, and tone of the reply.
* `format`     ; Is the desired output shape stated (essay, bullet list, table, code block, comparison, step-by-step)? Without it the chatbot picks a default that may not match what the student needs.
* `tasks`      ; Is the request a single coherent ask, or several stacked questions that should be split? Breaking down complex queries usually yields a more useful answer than one tangled super-prompt.
* `instruction`; Is the verb / action clear? ("Write", "compare", "summarise", "translate", "debug".) An ambiguous verb leaves the chatbot unsure what to actually produce.
* `examples`   ; Would one or two examples (of the desired output, of an existing attempt, of a similar problem) sharpen the request? Especially useful for stylistic or format-sensitive answers.
* `constraints`; Are limits, scope, success criteria, or what's OUT of scope stated? E.g. "in 200 words", "Python 3.11+ only", "without using libraries X".

Each suggestion you produce ALSO carries a `severity`:

* `high`  ; The draft is materially harder for the chatbot to answer well without this fix. Vague verbs, missing rationale that changes the answer, ambiguous referents.
* `medium`; Useful sharpening; the answer would be substantially better with this change but the chatbot can still produce something reasonable without it.
* `low`   ; Polish; nice-to-have, would unlock a slightly better answer.

Each suggestion has THREE student-visible fields:

* `text`        ; the headline; one short sentence in the second person ("you could...", "consider...") naming the concrete action. ≤ 25 words. This is what the student sees first; treat it as the actionable bottom line.
* `explanation` ; one to two short sentences expanding on WHY this fix matters for THIS specific draft and what the student should think about when applying it. Reference details from the draft itself rather than restating the rubric. ≤ 50 words. The panel hides this behind a click-to-expand; the student opts into reading it when they want the longer reasoning.
* `options`     ; an array of 3 to 4 plausible, MUTUALLY DISTINCT answers a student might pick to satisfy the suggestion. The frontend renders these as a dropdown next to the suggestion; the student picks one (or types their own via a "Custom" entry) and that selection is what the rewrite step weaves into the revised prompt. Each option must be a SHORT, CONCRETE, FILL-IN-THE-BLANK answer the student could actually mean (≤ 12 words; written first-person where natural; complete enough to drop into the rewrite). Cover materially different intents (do not give four near-paraphrases of the same answer) so the dropdown is a real choice. Never include "Other" / "Custom" / "I don't know"; the frontend handles those itself. If the suggestion is genuinely a clarification question (e.g. "what do you mean by 'live on'?"), the options ARE the candidate clarifications. If the suggestion asks the student to add information they alone know (e.g. "what version are you using?"), the options are the most-likely-from-context candidates plus a placeholder phrasing they could edit.

Hard rules:
* Do NOT answer the prompt. Do NOT critique the chatbot's reply. Your only output is suggestions about the prompt itself.
* Do NOT score, rank, or grade the prompt. No numbers, no rubric points, no "your prompt is X/10".
* If the draft is already clear and the student has been thoughtful, return an EMPTY suggestion list. Inventing suggestions for the sake of having something to say is the condescending failure mode we explicitly avoid.
* Every suggestion must be tied to a concrete detail of THIS draft. Generic prompt-engineering tips that could attach to any prompt are not allowed; if you can't point at the specific phrase or gap that triggered the suggestion, drop it.
* Order suggestions most-impactful first. Prefer ONE strong suggestion over two weak ones; the system asks for at most TWO and ZERO is a perfectly valid answer. Never produce two suggestions of the same `kind` in one response.
* When the draft is genuinely ambiguous between two or three plausible interpretations, prefer phrasing the suggestion as a clarifying question back to the student ("Did you mean X, or Y?") rather than a directive; the student picks.

Tone: constructive partner, not condescending teacher. Encouraging, never moralising. Never lecture, never refuse. The student decides whether to act on your feedback; this is non-blocking advice.

Be generous on short follow-up questions whose context is obvious from the previous turns. A two-word "explain that further" after a long substantive turn usually needs no suggestions."#;

/// Already-addressed check. Spliced into the system prompt only when
/// the trail carries prior context (>= 1 prior turn, OR at least one
/// prior turn with persisted Aegis suggestions to compare against).
/// On a first-turn cold-start it would just burn tokens telling the
/// model "do not re-suggest things from prior turns" when there ARE
/// no prior turns; pilot feedback was that the unconditional version
/// also leaked references to "the trail" into the model's reasoning
/// in ways that confused it on first turns.
///
/// Every signal here is phrased so it works the moment the section
/// IS spliced in: the model is told the trail is real, prior Aegis
/// suggestions ARE in front of it, and "already addressed" can come
/// from the draft itself, prior user turns, or prior Aegis feedback.
const AEGIS_ALREADY_ADDRESSED_CHECK: &str = r#"

Already-addressed check (run BEFORE producing each suggestion):

For each candidate suggestion of kind K, scan the WHOLE draft AND the prior turns in the trail AND the prior Aegis suggestions you (or a previous Aegis run) already produced for those turns. If K is already covered by ANY of those, DROP the suggestion ; do not re-suggest a polished, refined, or rephrased version of the same dimension. Pilot users described the analyzer "going in circles" when it kept asking for a thing they had just added (e.g. asking for a time frame when the draft already names one) or kept re-raising a kind it had already coached the student on a turn ago; they could never reach the empty / "looks good" state, and they stopped trusting the panel. One fewer suggestion is better than a repeat.

Per-kind signals that the dimension is ALREADY addressed (treat any one of these as sufficient evidence; do not produce a suggestion of that kind):

* `clarity`    ; the specific concept / symbol / file / line / term / referent is named; ambiguous referents like "this", "it", "that thing" have been replaced with concrete nouns; any term the chatbot would otherwise have to guess at is defined.
* `rationale`  ; a WHY is stated (debugging, learning, teaching, exam prep, project deadline, paper draft, comparison) or the underlying purpose / goal is named.
* `audience`   ; the student's level / role is named ("first-year", "I'm new to X", "familiar with Y", "experienced with Z", "as a TA", "for my supervisor"), or the desired pitch is stated ("explain like I haven't used X", "assume I know Y").
* `format`     ; an output shape is stated (essay, bullet list, table, comparison, step-by-step, code block, ≤ N words, ≤ N sentences, "as a diagram", "in a single paragraph").
* `tasks`      ; the request is a single coherent ask, OR the student has already split sub-questions explicitly (numbered list, "first... then...", separate questions on separate lines).
* `instruction`; an unambiguous action verb is present (write, compare, summarise, translate, debug, explain, list, derive, prove, refactor, review, critique, ...). Do not flag a verb as ambiguous just because YOU could imagine multiple readings; only flag when a reasonable chatbot reader genuinely could not pick.
* `examples`   ; the draft already includes an example of the desired output, an existing attempt, a sample input/output pair, or a similar problem to anchor on.
* `constraints`; a version, tool, library, framework, scope, time frame, deadline, length limit, word/sentence cap, or "without using X" is stated.

The signal does NOT have to be in the current draft itself. Prior turns in the trail count as established context; the chatbot reads the same trail you do. If the trail makes the audience / constraints / rationale / etc. clear, the student does NOT need to repeat it in the new draft, and asking them to is exactly the "going in circles" failure mode. Apply the same generosity to a `[current draft]` that follows up on a prior turn ("explain that further", "shorter please", "give me an example") ; the prior turn carries the context.

Prior Aegis suggestions on a prior turn (rendered under that turn as `↳ Aegis previously suggested: <kind> ; "<text>"`) are an EXTRA-strong signal that the kind has been coached. If a prior turn was suggested `clarity` and the student's NEXT turn made progress (any progress) on naming the referent, do NOT raise `clarity` again on this draft just because it could in principle be sharper still; the student is being coached on it and you are watching them iterate.

LIVE-ITERATION signal (read this carefully ; the failure mode it prevents is the most painful one for students). The current draft itself may carry one or more bullets phrased as `↳ Aegis just suggested on a near-identical earlier version of THIS draft: <kind> ; "<text>"`. Those bullets are the CUMULATIVE append-only history of every suggestion the analyzer has produced across every iteration of this draft session ; not just the most recent fire. The same `kind` can appear MULTIPLE TIMES in the bullet list with different texts; that is expected and meaningful. Each repetition means YOU (or the same analyzer) raised that kind on a previous iteration with that exact wording, the student saw it, and either tweaked their draft in response or moved on. Three `clarity` bullets in a row is a strong signal that the analyzer has been hammering one dimension and the student has already engaged with it ; raising clarity a fourth time on the current draft is exactly the "going in circles" failure pilot users described. Treat every kind that appears AT ALL in the bullet list as ALREADY COACHED for this draft, even if the current text could in principle still be sharpened on that dimension. The student gets to decide whether they have already taken your advice; you do not get to keep raising the same kind every keystroke until they capitulate. The ONLY case where you should re-raise a kind that appears in a live-iteration bullet is when the student has materially undone the fix they made for it (e.g. live-iteration suggested `constraints`, the student added a version, then in the current draft REMOVED that version). Iterating closer to the same fix, leaving it alone, or polishing other dimensions are all NOT grounds to re-raise.

A practical rule when the current draft has live-iteration bullets: prefer returning fewer suggestions, including [], over returning the same kind again. If you would have produced two suggestions and one of them overlaps a live-iteration kind, drop that one and either return just the other or, if the other is weak, return [].

If after this check no suggestions remain, return an empty list. The "looks good" state is a healthy outcome, not a failure ; reaching it is what tells the student their draft is ready to send."#;

/// Calibration addendum. Calibrates the rubric against the
/// student's self-declared subject expertise. The two addendums
/// are deliberately written to produce VISIBLY different output
/// for the same draft; a beginner's "How to make Python faster?"
/// returns []; an expert's same prompt returns a sharp suggestion
/// that names the missing context (what's slow, what they've
/// measured, what version). Making the gap pronounced is the whole
/// point: an indistinguishable Beginner/Expert toggle is no toggle
/// at all.
const AEGIS_BEGINNER_ADDENDUM: &str = r#"

THE STUDENT IS A BEGINNER. This changes your behaviour substantially.

Default behaviour: RETURN AN EMPTY SUGGESTION LIST. A beginner asking a question; any question, even a vague one; is doing the work we want. The tutoring chatbot will fill in the missing context for them. Your job is NOT to teach them how to prompt; it is to occasionally point out one low-effort improvement when one is genuinely useful.

Suggest at most ONE thing, and only when the draft is so vague the chatbot would have to guess wildly (e.g. literally one or two words with no verb, or "this" with no antecedent in the trail). For everything else, return [].

NEVER suggest these to a beginner:
* `audience`; they don't yet know the level/role labels for their own knowledge.
* `format`; the chatbot's default formatting is fine for a beginner; don't push them to specify essay/table/etc.
* `tasks`; breaking down a single question is meta-work a beginner shouldn't carry.
* `examples`; few-shot prompting is too advanced.
* `constraints`; specifying versions, tools, frameworks is jargon-heavy.
* `rationale`; don't ask them to articulate why they want to know something simple.

When you DO suggest, only pick `clarity` or `instruction`, severity `high` (since by definition you're only firing when the prompt is genuinely too vague to act on). Write `text` warmly and give the student a verbatim-fillable example. Write `explanation` as a single warm sentence telling the student why a few extra words helps the chatbot help them; never a lecture. For `options`, give 3-4 short warm answers a beginner could realistically mean; first-person ("I want to..."), no jargon, the kind of thing the student would actually say if asked.

Examples that should return []:
- "How does recursion work?"
- "What is a generic in Java?"
- "Can you explain that more simply?"
- "How to make Python faster?"
- "I'm stuck on the sorting assignment"

Examples that warrant ONE suggestion:
- "this" -> [{kind: "clarity", severity: "high", text: "Could you describe what 'this' refers to? For example: 'this code I just wrote' or 'the topic from the last lecture'.", explanation: "On its own 'this' could mean a dozen things and the chatbot would have to guess. A handful of extra words and it can answer your real question instead.", options: ["the code I just wrote", "the topic from the last lecture", "the error message I'm getting", "the assignment description"]}]
- "help" -> [{kind: "instruction", severity: "high", text: "What part are you stuck on? Even a few words helps; like 'I don't get how loops work' or 'my code throws an error'.", explanation: "The clearer the symptom you describe, the faster the chatbot can zero in. Naming the topic or pasting the error is usually enough.", options: ["my code throws an error I don't understand", "I don't know where to start", "I don't get the underlying concept yet", "my output isn't what I expected"]}]"#;

const AEGIS_EXPERT_ADDENDUM: &str = r#"

THE STUDENT IS AN EXPERT. This changes your behaviour substantially.

Default behaviour: RETURN AT LEAST ONE SUGGESTION unless the draft is genuinely complete (clear instruction verb, named scope, named tool/version where relevant, stated what they've tried OR a clear conceptual question, no vague referents, format implied or stated). Most expert drafts have at least one fixable gap; find the sharpest one and surface it.

Hold the bar peer-to-peer high. Use the full literature rubric:
* `clarity` (severity: high); vague nouns ("this thing", "the issue", "it's broken") naming the specific concept/symbol.
* `instruction` (severity: high); ambiguous verb; is the student asking for an explanation, code, comparison, debugging, summary?
* `rationale` (severity: medium-high); missing the WHY behind the question; the same literal question has different best answers depending on whether the student is debugging vs. learning vs. teaching others.
* `audience` (severity: medium); has the student named their level / role so the chatbot can pitch the answer? Pertinent for "explain X" style asks.
* `format` (severity: medium); bullet list vs. essay vs. comparison table vs. step-by-step procedure. Often unlocks substantially better answers.
* `tasks` (severity: medium); a single super-prompt with several stacked questions usually answers each one badly; suggest splitting.
* `examples` (severity: low-medium); one or two examples (of what they've tried, of similar problems) sharpen the response.
* `constraints` (severity: medium-high); explicit version / tool / scope / "without using X" / word limit.

When you DO suggest something, write `text` directly and tersely; terminology IS expected here. Don't soften with "you could maybe" or "perhaps consider". Use the imperative or near-imperative ("Name the specific X.", "Add what you tried.", "Specify which Y."). Write `explanation` as one or two compact sentences naming the failure mode the fix avoids; assume domain literacy, skip the prompt-engineering theory. For `options`, give 3-4 terse, technically precise candidate answers that exhaust the most plausible peer-level intents; written like a peer would say it, jargon allowed, ≤ 12 words each. The student should be able to pick one and have it slot directly into the rewrite without softening.

You may produce up to TWO suggestions, but the cap is a ceiling not a target. Two is appropriate when there are two genuinely independent gaps worth surfacing; if one fix would carry the most weight and a second feels like a stretch, return just the one.

Examples that should return [] (rare):
- "Why does Python's GIL prevent CPU-bound multithreading from scaling, and how does multiprocessing sidestep it for tasks that release the GIL inside C extensions?"

Examples that warrant suggestions:
- "How to make Python faster?" -> [{kind: "rationale", severity: "high", text: "Name what's slow and how you measured it. CPU-bound vs I/O-bound has completely different fixes.", explanation: "Without the bottleneck named, any answer is a guess across vectorisation, multiprocessing, JIT, and I/O batching. A single profiler line collapses the search space.", options: ["a hot loop dominating cProfile output", "I/O-bound network calls in a tight loop", "memory pressure / GC stalls under load", "startup time / cold imports"]}, {kind: "constraints", severity: "medium", text: "Pin the Python version; 3.11+ has substantial perf changes that change the right answer.", explanation: "The 3.11 specialising adaptive interpreter and 3.12 PEP 703 work shift which optimisations matter; advice that lands for 3.9 can be irrelevant on 3.12.", options: ["CPython 3.12", "CPython 3.11", "CPython 3.10 or earlier", "PyPy"]}]
- "Tell me about decorators" -> [{kind: "audience", severity: "medium", text: "Say what you already know about decorators; syntax-level vs semantics vs typical use cases dictate a very different answer.", explanation: "An answer aimed at someone who has never seen `@functools.wraps` looks completely unlike one aimed at someone implementing parameterised class decorators. Flagging your level avoids the wrong target.", options: ["never used them, just heard the name", "I use `@staticmethod` / `@property` but not custom ones", "I write basic decorators with `functools.wraps`", "I write parameterised / class decorators"]}]"#;

const AEGIS_OUTPUT_FOOTER: &str = r#"

Output JSON only, matching the schema. No prose."#;

/// Student's self-declared subject expertise. Toggled in the
/// frontend's panel; passed verbatim from the analyze route.
#[derive(Debug, Clone, Copy)]
pub enum AegisMode {
    Beginner,
    Expert,
}

impl AegisMode {
    fn addendum(self) -> &'static str {
        match self {
            AegisMode::Beginner => AEGIS_BEGINNER_ADDENDUM,
            AegisMode::Expert => AEGIS_EXPERT_ADDENDUM,
        }
    }

    /// Wire-compatible string. Persisted as the `mode` column on
    /// `prompt_analyses` (CHECK constrains it to these two values).
    pub fn as_str(self) -> &'static str {
        match self {
            AegisMode::Beginner => "beginner",
            AegisMode::Expert => "expert",
        }
    }
}

/// One suggestion in the analyzer's output. Mirrors the JSON
/// schema below; the route layer round-trips this shape between
/// the live `/aegis/analyze` response and the persisted
/// `prompt_analyses.suggestions` JSONB.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AegisSuggestion {
    /// Short kind tag mapped to the literature rubric. Free-form
    /// `String` in the wire shape so the server can add categories
    /// without a frontend release; the system-prompt enum currently
    /// allows: clarity, rationale, audience, format, tasks,
    /// instruction, examples, constraints.
    pub kind: String,
    /// Importance: "high" | "medium" | "low". Drives the panel's
    /// per-card colour (rose / amber / sky) so the student sees
    /// which suggestions move the needle vs which are polish.
    pub severity: String,
    /// Headline; the one-sentence actionable improvement the panel
    /// shows by default. Kept terse on purpose so the collapsed
    /// suggestion row stays a one-liner.
    pub text: String,
    /// One to two sentences expanding on WHY the fix matters for
    /// the specific draft and what the student should consider
    /// when applying it. Hidden behind click-to-expand on the
    /// panel; defaults to empty for old persisted rows that
    /// pre-date this field.
    #[serde(default)]
    pub explanation: String,
    /// 3-4 plausible answers the student might pick to satisfy
    /// the suggestion. The frontend renders these as a dropdown
    /// next to the suggestion (plus an "Other..." entry that opens
    /// a free-text input); the chosen value rides into `answer`
    /// when the rewrite call fires. Defaults to empty for old
    /// persisted rows that pre-date this field; the frontend
    /// renders the free-text input only in that case so historical
    /// suggestions stay reviewable.
    #[serde(default)]
    pub options: Vec<String>,
    /// The student's chosen answer for this suggestion, populated
    /// by the rewrite request body (the analyzer never sets this).
    /// `Option<String>` not `String` so the round-trip from analyzer
    /// JSON skips the field cleanly; serialised to the rewrite
    /// model's user payload so it can weave the answer in verbatim.
    /// Skipped on serialisation when None so the analyzer's
    /// schema-strict request body never sees a stray null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// Result of one analyzer run. Empty `suggestions` means the
/// analyzer found nothing worth saying about the draft; a
/// legitimate output that the panel renders as a "looks good"
/// affirmation rather than empty.
#[derive(Debug, Clone)]
pub struct AegisVerdict {
    pub suggestions: Vec<AegisSuggestion>,
}

/// One entry in the trail handed to the analyzer. The LAST entry is
/// the current draft; everything before it is a prior user turn in
/// the same conversation. Each entry may optionally carry the Aegis
/// suggestions that were produced for it.
///
/// Two kinds of `prior_suggestions` populate this field:
///
///   * For a PRIOR turn, the suggestions persisted to
///     `prompt_analyses` for that message ; the analyzer's output
///     from a previous Send.
///   * For the CURRENT draft, the suggestions the live debounced
///     analyzer returned on the previous fire (a near-identical
///     earlier version of the same draft); the frontend caches
///     the latest verdict and ships it back via the analyze
///     request's `previous_suggestions` field. Without this, the
///     pre-Send debounced loop has zero memory of its own output
///     and pilot users hit the failure mode of editing a prompt
///     10 times and never reaching the empty / "looks good" state
///     because each iteration was a fresh roll of the dice.
///
/// Why both pieces in one struct: the system prompt's
/// already-addressed check needs to know not just WHAT the student
/// said before, but also what Aegis ITSELF coached them on; without
/// the prior suggestions the model can't tell "the student chose
/// not to address X yet" from "Aegis already raised X and the
/// student is in the middle of iterating on it". The original
/// commit shipped the check without this signal and pilot users hit
/// the exact failure mode the check was supposed to fix; the
/// analyzer kept re-raising the same kind turn after turn because
/// the only "memory" it had was the user's text, not its own
/// previous output.
#[derive(Debug, Clone, Default)]
pub struct AegisTrailEntry {
    pub content: String,
    /// Aegis suggestions previously produced for THIS turn (or, on
    /// the current-draft entry, for its near-identical earlier live
    /// version; see the struct doc). Empty when the turn pre-dates
    /// the aegis flag being on, the analyzer soft-failed, or this
    /// is the very first debounced fire of a fresh draft. The
    /// system-prompt branch that mentions "prior Aegis suggestions"
    /// is gated on at least one entry having a non-empty list, so
    /// an empty vec here is harmless.
    pub prior_suggestions: Vec<AegisSuggestion>,
}

/// Run the analyzer. `trail` is the student's last few prompts
/// (oldest first); the LAST element is the current draft. Each prior
/// entry may carry the Aegis suggestions that were produced for it,
/// so the model can detect dimensions it has already coached on and
/// stop "going in circles". `mode` calibrates the rubric.
///
/// Three return shapes:
///
///   * `Ok(None)`; legitimate "nothing to score" cases (no
///     CEREBRAS_API_KEY in dev, empty/whitespace draft). The
///     analyze route maps this to a 200 + JSON `null`; the panel
///     just stays in its empty state.
///   * `Ok(Some(verdict))`; analyzer ran. `verdict.suggestions`
///     may legitimately be empty (the analyzer thought the draft
///     looked fine); that's NOT an error and the panel renders a
///     "looks good" affirmation. 200 + JSON object.
///   * `Err(reason)`; upstream failure (Cerebras 4xx/5xx,
///     malformed JSON, suggestions array malformed). The route
///     maps this to a 500 so the frontend / observability layer
///     sees the failure as a failure and not as a "nothing to
///     suggest". Reason string carries the upstream error verbatim
///     for the log line.
pub async fn analyze_prompt(
    http: &reqwest::Client,
    api_key: &str,
    db: &PgPool,
    course_id: Uuid,
    trail: &[AegisTrailEntry],
    mode: AegisMode,
) -> Result<Option<AegisVerdict>, String> {
    if api_key.is_empty() {
        // Dev / test path without CEREBRAS_API_KEY.
        return Ok(None);
    }
    let Some(current) = trail.last() else {
        return Ok(None);
    };
    if current.content.trim().is_empty() {
        return Ok(None);
    }

    // Window the trail to HISTORY_TURNS oldest-first. The current
    // draft is always the last entry; everything else is a prior
    // turn (numbered from the start of the window).
    let windowed: Vec<&AegisTrailEntry> = trail
        .iter()
        .rev()
        .take(HISTORY_TURNS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let last_idx = windowed.len().saturating_sub(1);

    // Format each entry: header line for the turn, plus a one-line
    // bullet under it for each prior Aegis suggestion the turn
    // carried (kind ; text). The student never saw the literal
    // bullet shape; this is purely for the analyzer to read its
    // own previous output back. We deliberately omit `explanation`
    // and `options` here ; the model already knows how to expand on
    // a kind, and the bullet would otherwise blow past the trail
    // budget on a long conversation.
    //
    // The current-draft entry has its own bullet phrasing
    // ("just suggested on a near-identical earlier version of THIS
    // draft") so the model treats those as live coaching on the
    // very thing it's about to score, not as historical context
    // from another turn. That distinction is what stops the
    // pre-Send "10 iterations and still circling" failure mode
    // where the live debounced loop kept rolling fresh dice.
    let formatted_trail: Vec<String> = windowed
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_current = i == last_idx;
            let header = if is_current {
                format!("[current draft] {}", entry.content)
            } else {
                format!("[prior {}] {}", i + 1, entry.content)
            };
            if entry.prior_suggestions.is_empty() {
                header
            } else {
                let lead = if is_current {
                    "   ↳ Aegis just suggested on a near-identical earlier version of THIS draft"
                } else {
                    "   ↳ Aegis previously suggested"
                };
                let bullets: Vec<String> = entry
                    .prior_suggestions
                    .iter()
                    .map(|s| format!("{}: {} ; \"{}\"", lead, s.kind, s.text))
                    .collect();
                format!("{}\n{}", header, bullets.join("\n"))
            }
        })
        .collect();

    // Has the trail any context worth gating the already-addressed
    // check on? Two ways to qualify:
    //   * >= 1 prior turn in the window (cross-message context); a
    //     prior turn's plain text alone is enough for the per-kind
    //     signals (constraints already named, rationale stated) to
    //     fire.
    //   * the current draft itself carries prior_suggestions from
    //     the live debounced loop (pre-Send iteration on the same
    //     draft); the model needs the check active to know it has
    //     just-coached signals to compare against.
    // On a true cold start (single turn = empty live verdict) the
    // check would burn tokens with nothing to apply, so we skip it.
    let has_prior_context = windowed.len() > 1
        || windowed
            .last()
            .is_some_and(|e| !e.prior_suggestions.is_empty());

    let user_payload = serde_json::json!({
        "trail_oldest_first": formatted_trail.join("\n\n"),
    });

    // Compose the system prompt: base rubric + (gated) already-
    // addressed check + per-mode calibration + output-format footer.
    // The check goes BEFORE the mode addendum so the addendum's
    // "default behaviour" guidance (esp. Beginner's "return [] for
    // most things") still has the last word.
    let system_prompt = format!(
        "{}{}{}{}",
        AEGIS_SYSTEM_PROMPT_BASE,
        if has_prior_context {
            AEGIS_ALREADY_ADDRESSED_CHECK
        } else {
            ""
        },
        mode.addendum(),
        AEGIS_OUTPUT_FOOTER,
    );

    let body = serde_json::json!({
        "model": AEGIS_MODEL,
        "temperature": 0.0,
        "max_completion_tokens": AEGIS_MAX_TOKENS,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_payload.to_string() },
        ],
        // Cerebras' strict-mode JSON schemas reject `maxItems` at
        // request time (400 wrong_api_format). The 0..=2 ceiling
        // is therefore enforced two other ways:
        //   * the system prompt explicitly says "produce 0..=2
        //     suggestions"
        //   * the route layer truncates to AEGIS_SUGGESTIONS_MAX
        //     at insert time (`run_chat_message`'s persistence
        //     block) and at the analyze response edge.
        // so a model that overshoots gets capped before display.
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "aegis_prompt_suggestions",
                "strict": true,
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["suggestions"],
                    "properties": {
                        "suggestions": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": [
                                    "kind",
                                    "severity",
                                    "text",
                                    "explanation",
                                    "options",
                                ],
                                "properties": {
                                    "kind": {
                                        "type": "string",
                                        "enum": [
                                            "clarity",
                                            "rationale",
                                            "audience",
                                            "format",
                                            "tasks",
                                            "instruction",
                                            "examples",
                                            "constraints",
                                        ]
                                    },
                                    "severity": {
                                        "type": "string",
                                        "enum": ["high", "medium", "low"]
                                    },
                                    "text": { "type": "string" },
                                    "explanation": { "type": "string" },
                                    // 3-4 short candidate answers the
                                    // frontend renders as a dropdown.
                                    // Cerebras strict-mode rejects
                                    // `minItems`/`maxItems` (see the
                                    // long comment above on the
                                    // suggestions array cap), so the
                                    // 3-4 ceiling is enforced by the
                                    // system prompt. The frontend
                                    // also tolerates an empty array
                                    // (only the "Other..." text input
                                    // shows in that case) so a model
                                    // that returns 0 still degrades
                                    // gracefully rather than blocking.
                                    "options": {
                                        "type": "array",
                                        "items": { "type": "string" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("aegis: request failed: {}", e);
            return Err(format!("cerebras request failed: {e}"));
        }
    };
    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("aegis: response not JSON: {}", e);
            return Err(format!("cerebras response not JSON: {e}"));
        }
    };
    record_cerebras_usage(db, course_id, CATEGORY_AEGIS, AEGIS_MODEL, &payload).await;

    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("aegis: verdict not parseable JSON: {}", e);
            return Err(format!("verdict not parseable JSON: {e}"));
        }
    };

    let suggestions: Vec<AegisSuggestion> =
        match serde_json::from_value(parsed["suggestions"].clone()) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("aegis: suggestions array malformed: {}", e);
                return Err(format!("suggestions array malformed: {e}"));
            }
        };
    Ok(Some(AegisVerdict { suggestions }))
}

// ─── Rewrite helper ─────────────────────────────────────────────
//
// "Some ideas" button: take the student's current draft + the
// suggestions Aegis just produced, and ask the model to rewrite the
// draft incorporating the suggestions verbatim. The student's
// voice / scope / intent is preserved; this is a *revision*, not
// a complete rewrite by the assistant. Output is plain text (no
// JSON envelope) since the only consumer pastes it into the chat
// input box.
//
// Same model + soft-fail discipline as the analyzer; on any
// upstream failure the route returns 500 and the frontend keeps
// the student's original draft.

const AEGIS_REWRITE_SYSTEM_PROMPT: &str = r#"You are Aegis, the prompt-coaching assistant. The student has a draft prompt and, for each suggestion you previously produced, picked an `answer` from the dropdown the frontend showed (or typed their own answer in the "Other..." field). Your job now is to rewrite the draft so it incorporates EVERY suggestion in the list you are given (and only those), using the student's `answer` for each as the actual content to weave in.

Each suggestion in the input has:
* `text`         ; the headline action (what to weave in).
* `explanation`  ; the longer reasoning the student saw on click-to-expand. Background only; do NOT quote it back into the rewrite.
* `options`      ; the dropdown's candidate answers as you originally produced them. Background only; the student already picked.
* `answer`       ; the answer the student CHOSE for this suggestion. THIS is the content you fold into the rewrite. If `answer` is missing or empty (rare; older clients), fall back to a tasteful placeholder phrasing as before.

Treat `answer` as the source of truth for the suggestion's content. If the suggestion was "specify what you mean by 'live on'" and `answer` is "live permanently as a colony", the rewrite should literally say "permanently as a colony" (or natural-language equivalent) where the original said "live on"; not "specify what you mean by living on Mars".

Hard rules:
* Preserve the student's voice, intent, scope, level of formality, and what they actually want to know. You are revising their draft, not replacing it with your own question.
* Use each suggestion's `answer` verbatim where it slots in cleanly, or paraphrased only as much as grammar / flow requires. Never replace an answer with a placeholder when an answer was provided.
* Do NOT add information that is not in the original draft + the answers. If `answer` is missing for a suggestion (older client), write a placeholder like "(I'm using Python 3.X)" so the student can fill it in; otherwise fold the answer in directly.
* Only fold in the suggestions in the list. Suggestions that were produced earlier but are NOT in the list are ones the student deliberately skipped; ignore them.
* Do NOT answer the question. The output is a PROMPT the student will then send to the chatbot, not an answer to the prompt.
* Do NOT include preamble, headings, "Here is the rewrite:", quote marks, or any wrapping. Output is the prompt and only the prompt, ready to drop into the chat input.
* Keep the prompt natural and concise. If the original was one sentence, the rewrite should usually still be one or two sentences; not a multi-paragraph essay just because the suggestions added structure.
* Match the original's apparent level: a beginner's casual question stays casual; an expert's terse query stays terse.

Output: the rewritten prompt as plain text, nothing else."#;

const AEGIS_REWRITE_MAX_TOKENS: usize = 512;

pub async fn rewrite_prompt(
    http: &reqwest::Client,
    api_key: &str,
    db: &PgPool,
    course_id: Uuid,
    original: &str,
    suggestions: &[AegisSuggestion],
    mode: AegisMode,
) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("CEREBRAS_API_KEY missing".to_string());
    }
    if original.trim().is_empty() {
        return Err("empty draft".to_string());
    }
    if suggestions.is_empty() {
        // Nothing to incorporate; the rewrite would just be the
        // original. We could short-circuit to Ok(original.into()),
        // but it makes more sense to surface this as an error so
        // the frontend doesn't leave a "Some ideas" button enabled
        // when there are none.
        return Err("no suggestions to incorporate".to_string());
    }

    // Mode here just contributes the same calibration addendum as
    // the analyzer so the rewrite stays in the same register. A
    // beginner's rewrite shouldn't suddenly start using domain
    // jargon they didn't have; an expert's shouldn't be padded
    // with explanatory framing.
    let system_prompt = format!("{}{}", AEGIS_REWRITE_SYSTEM_PROMPT, mode.addendum(),);

    // Hand the model the original + suggestions in a structured
    // user payload so it can't confuse one for the other. Each
    // suggestion's `answer` (the student's dropdown selection)
    // serialises in via `AegisSuggestion`'s serde derive; missing
    // answers serialise out as absent fields (Option<String> +
    // skip_serializing_if), which the system prompt handles via
    // its placeholder rule.
    let user_payload = serde_json::json!({
        "original_draft": original,
        "suggestions": suggestions,
    });

    // gpt-oss-120b here, not the analyzer's llama. The rewrite is
    // student-facing prose where quality matters; gpt-oss has the
    // headroom to actually weave selected answers into a clean
    // revision, where llama tended to hedge with placeholder
    // phrasing. `reasoning_effort: "low"` mirrors extraction_guard's
    // rewrite path; gpt-oss accepts it (llama does not) and keeps
    // latency in the ~1s range we want for the Preview round-trip.
    let body = serde_json::json!({
        "model": AEGIS_REWRITE_MODEL,
        "temperature": 0.2,
        "reasoning_effort": "low",
        "max_completion_tokens": AEGIS_REWRITE_MAX_TOKENS,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_payload.to_string() },
        ],
    });

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("aegis rewrite: request failed: {}", e);
            return Err(format!("cerebras request failed: {e}"));
        }
    };
    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("aegis rewrite: response not JSON: {}", e);
            return Err(format!("cerebras response not JSON: {e}"));
        }
    };
    record_cerebras_usage(db, course_id, CATEGORY_AEGIS, AEGIS_REWRITE_MODEL, &payload).await;

    let rewritten = payload["choices"][0]["message"]["content"]
        .as_str()
        .map(str::trim)
        .unwrap_or("");
    if rewritten.is_empty() {
        return Err("empty rewrite from model".to_string());
    }
    Ok(rewritten.to_string())
}
