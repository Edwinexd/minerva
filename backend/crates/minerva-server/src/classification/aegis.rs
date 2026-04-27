//! Aegis: prompt-coaching analyzer.
//!
//! When the `aegis` feature flag is on for a course, every
//! debounced keystroke (and every Send) fires a single Cerebras
//! call that examines the student's draft and returns 0..=3
//! actionable suggestions for how to improve it.
//!
//! We deliberately do NOT score the prompt. The brief is explicit
//! about tone -- a partner offering ideas, not a grader handing out
//! marks -- and Herodotou et al. (2025) flag the condescending edge
//! that scoring rubrics carry for accessibility-sensitive learners.
//! Scores also hide the actionable signal under a number; "your
//! prompt is 4/10" tells the student nothing they can do about it,
//! whereas "say what you've already tried so the assistant can
//! build on it" does.
//!
//! Each suggestion has:
//!   * `kind`  -- short tag the panel uses for grouping/iconography
//!     ("context", "constraints", "specificity", "alternatives",
//!     "clarification"). Free-form string so we can add categories
//!     without a server change.
//!   * `text`  -- single-sentence actionable improvement, written
//!     in the second person ("you might..."), no markdown, no
//!     leading bullet.
//!
//! Empty suggestion array = "the prompt is fine, no suggestions".
//! The panel renders a small "looks good" affirmation rather than
//! pretending there's always something to fix.
//!
//! Soft-fail throughout. The chat hot path never waits on the
//! analyzer -- a transient Cerebras hiccup, malformed JSON, or DB
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

/// Tiny model -- runs on every debounced keystroke + every Send
/// when the flag is on. Latency is the headline number. The
/// schema-constrained output keeps llama3.1-8b on rails for the
/// JSON shape we want. `pub` so the route layer can stamp
/// `model_used` on persisted rows.
pub const AEGIS_MODEL: &str = "llama3.1-8b";

/// Cap on the analyzer's reply. Three suggestions @ ~30 words each
/// + a few JSON envelope tokens fits in 256 with margin.
const AEGIS_MAX_TOKENS: usize = 384;

/// How many previous user turns we feed to the analyzer for context.
/// Five is enough that "explain that further" reads as well-grounded
/// follow-up rather than a context-free prompt.
const HISTORY_TURNS: usize = 5;

const AEGIS_SYSTEM_PROMPT_BASE: &str = r#"You are Aegis, a prompt-coaching assistant for university students using a course-aware tutoring chatbot. You help the student write better prompts by offering concrete suggestions, never by grading them.

You will read the student's current draft (and a short trail of their recent prior prompts in the same conversation, for context). Your job is to produce 0..=3 suggestions for how the student could improve THIS draft before sending it.

The rubric you check the draft against is grounded in the CLEAR prompt-engineering framework (Concise, Logical, Explicit, Adaptive, Reflective) and the standard prompt-design rubric. Each `kind` below maps to one rubric dimension:

* `clarity`     -- Is the request specific and unambiguous? Are key terms defined? Will the chatbot have to guess at multiple plausible interpretations? (CLEAR: Concise + Explicit.)
* `rationale`   -- Is there enough background/context to interpret the request? Why does the student want this -- the underlying purpose? Without it the chatbot can answer the literal question and miss the actual goal.
* `audience`    -- Has the student named who they are (course level, prior knowledge, role)? Should the answer assume a beginner or someone steeped in the topic? Determines the language, complexity, and tone of the reply.
* `format`      -- Is the desired output shape stated (essay, bullet list, table, code block, comparison, step-by-step)? Without it the chatbot picks a default that may not match what the student needs.
* `tasks`       -- Is the request a single coherent ask, or several stacked questions that should be split? Breaking down complex queries usually yields a more useful answer than one tangled super-prompt.
* `instruction` -- Is the verb / action clear? ("Write", "compare", "summarise", "translate", "debug".) An ambiguous verb leaves the chatbot unsure what to actually produce.
* `examples`    -- Would one or two examples (of the desired output, of an existing attempt, of a similar problem) sharpen the request? Especially useful for stylistic or format-sensitive answers.
* `constraints` -- Are limits, scope, success criteria, or what's OUT of scope stated? E.g. "in 200 words", "Python 3.11+ only", "without using libraries X".

Each suggestion you produce ALSO carries a `severity`:

* `high`   -- The draft is materially harder for the chatbot to answer well without this fix. Vague verbs, missing rationale that changes the answer, ambiguous referents.
* `medium` -- Useful sharpening; the answer would be substantially better with this change but the chatbot can still produce something reasonable without it.
* `low`    -- Polish; nice-to-have, would unlock a slightly better answer.

Hard rules:
* Do NOT answer the prompt. Do NOT critique the chatbot's reply. Your only output is suggestions about the prompt itself.
* Do NOT score, rank, or grade the prompt. No numbers, no rubric points, no "your prompt is X/10".
* If the draft is already clear and the student has been thoughtful, return an EMPTY suggestion list. Inventing suggestions for the sake of having something to say is the condescending failure mode we explicitly avoid.
* Each suggestion is a single sentence in the second person ("you could...", "consider..."). No leading bullets, no markdown, ≤ 30 words.
* Order suggestions most-impactful first. Prefer ONE strong suggestion over three weak ones. Never produce two suggestions of the same `kind` in one response.

Tone: constructive partner, not condescending teacher. Encouraging, never moralising. Never lecture, never refuse. The student decides whether to act on your feedback; this is non-blocking advice.

Be generous on short follow-up questions whose context is obvious from the previous turns. A two-word "explain that further" after a long substantive turn usually needs no suggestions."#;

/// Calibration addendum. Calibrates the rubric against the
/// student's self-declared subject expertise. The two addendums
/// are deliberately written to produce VISIBLY different output
/// for the same draft -- a beginner's "How to make Python faster?"
/// returns []; an expert's same prompt returns a sharp suggestion
/// that names the missing context (what's slow, what they've
/// measured, what version). Making the gap pronounced is the whole
/// point: an indistinguishable Beginner/Expert toggle is no toggle
/// at all.
const AEGIS_BEGINNER_ADDENDUM: &str = r#"

THE STUDENT IS A BEGINNER. This changes your behaviour substantially.

Default behaviour: RETURN AN EMPTY SUGGESTION LIST. A beginner asking a question -- any question, even a vague one -- is doing the work we want. The tutoring chatbot will fill in the missing context for them. Your job is NOT to teach them how to prompt; it is to occasionally point out one low-effort improvement when one is genuinely useful.

Suggest at most ONE thing, and only when the draft is so vague the chatbot would have to guess wildly (e.g. literally one or two words with no verb, or "this" with no antecedent in the trail). For everything else, return [].

NEVER suggest these to a beginner:
* `audience` -- they don't yet know the level/role labels for their own knowledge.
* `format` -- the chatbot's default formatting is fine for a beginner; don't push them to specify essay/table/etc.
* `tasks` -- breaking down a single question is meta-work a beginner shouldn't carry.
* `examples` -- few-shot prompting is too advanced.
* `constraints` -- specifying versions, tools, frameworks is jargon-heavy.
* `rationale` -- don't ask them to articulate why they want to know something simple.

When you DO suggest, only pick `clarity` or `instruction`, severity `high` (since by definition you're only firing when the prompt is genuinely too vague to act on). Write the suggestion warmly and give the student a verbatim-fillable example.

Examples that should return []:
- "How does recursion work?"
- "What is a generic in Java?"
- "Can you explain that more simply?"
- "How to make Python faster?"
- "I'm stuck on the sorting assignment"

Examples that warrant ONE suggestion:
- "this" -> [{kind: "clarity", severity: "high", text: "Could you describe what 'this' refers to? For example: 'this code I just wrote' or 'the topic from the last lecture'."}]
- "help" -> [{kind: "instruction", severity: "high", text: "What part are you stuck on? Even a few words helps -- like 'I don't get how loops work' or 'my code throws an error'."}]"#;

const AEGIS_EXPERT_ADDENDUM: &str = r#"

THE STUDENT IS AN EXPERT. This changes your behaviour substantially.

Default behaviour: RETURN AT LEAST ONE SUGGESTION unless the draft is genuinely complete (clear instruction verb, named scope, named tool/version where relevant, stated what they've tried OR a clear conceptual question, no vague referents, format implied or stated). Most expert drafts have at least one fixable gap; find the sharpest one and surface it.

Hold the bar peer-to-peer high. Use the full literature rubric:
* `clarity` (severity: high) -- vague nouns ("this thing", "the issue", "it's broken") naming the specific concept/symbol.
* `instruction` (severity: high) -- ambiguous verb -- is the student asking for an explanation, code, comparison, debugging, summary?
* `rationale` (severity: medium-high) -- missing the WHY behind the question; the same literal question has different best answers depending on whether the student is debugging vs. learning vs. teaching others.
* `audience` (severity: medium) -- has the student named their level / role so the chatbot can pitch the answer? Pertinent for "explain X" style asks.
* `format` (severity: medium) -- bullet list vs. essay vs. comparison table vs. step-by-step procedure. Often unlocks substantially better answers.
* `tasks` (severity: medium) -- a single super-prompt with several stacked questions usually answers each one badly; suggest splitting.
* `examples` (severity: low-medium) -- one or two examples (of what they've tried, of similar problems) sharpen the response.
* `constraints` (severity: medium-high) -- explicit version / tool / scope / "without using X" / word limit.

When you DO suggest something, write it directly and tersely -- terminology IS expected here. Don't soften with "you could maybe" or "perhaps consider". Use the imperative or near-imperative ("Name the specific X.", "Add what you tried.", "Specify which Y.").

Examples that should return [] (rare):
- "Why does Python's GIL prevent CPU-bound multithreading from scaling, and how does multiprocessing sidestep it for tasks that release the GIL inside C extensions?"

Examples that warrant suggestions:
- "How to make Python faster?" -> [{kind: "rationale", severity: "high", text: "Name what's slow and how you measured it. CPU-bound vs I/O-bound has completely different fixes."}, {kind: "constraints", severity: "medium", text: "Pin the Python version -- 3.11+ has substantial perf changes that change the right answer."}]
- "Tell me about decorators" -> [{kind: "audience", severity: "medium", text: "Say what you already know about decorators -- syntax-level vs semantics vs typical use cases dictate a very different answer."}, {kind: "format", severity: "low", text: "Specify the depth: a one-paragraph mental model, a worked example, or a reference list of common patterns."}]"#;

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
    /// One-sentence actionable improvement.
    pub text: String,
}

/// Result of one analyzer run. Empty `suggestions` means the
/// analyzer found nothing worth saying about the draft -- a
/// legitimate output that the panel renders as a "looks good"
/// affirmation rather than empty.
#[derive(Debug, Clone)]
pub struct AegisVerdict {
    pub suggestions: Vec<AegisSuggestion>,
}

/// Run the analyzer. `recent_user_messages` is the trail of the
/// student's last few prompts (oldest first); the LAST element is
/// the current draft. `mode` calibrates the rubric.
///
/// Three return shapes:
///
///   * `Ok(None)` -- legitimate "nothing to score" cases (no
///     CEREBRAS_API_KEY in dev, empty/whitespace draft). The
///     analyze route maps this to a 200 + JSON `null`; the panel
///     just stays in its empty state.
///   * `Ok(Some(verdict))` -- analyzer ran. `verdict.suggestions`
///     may legitimately be empty (the analyzer thought the draft
///     looked fine); that's NOT an error and the panel renders a
///     "looks good" affirmation. 200 + JSON object.
///   * `Err(reason)` -- upstream failure (Cerebras 4xx/5xx,
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
    recent_user_messages: &[String],
    mode: AegisMode,
) -> Result<Option<AegisVerdict>, String> {
    if api_key.is_empty() {
        // Dev / test path without CEREBRAS_API_KEY.
        return Ok(None);
    }
    let Some(current) = recent_user_messages.last() else {
        return Ok(None);
    };
    if current.trim().is_empty() {
        return Ok(None);
    }

    // Build the trail compactly. Numbered, oldest first; the
    // current turn is highlighted as `[current]` so the model
    // never confuses prior turns with the draft under review.
    let trail: Vec<String> = recent_user_messages
        .iter()
        .rev()
        .take(HISTORY_TURNS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .enumerate()
        .map(|(i, m)| {
            if i + 1 == HISTORY_TURNS.min(recent_user_messages.len()) {
                format!("[current draft] {}", m)
            } else {
                format!("[prior {}] {}", i + 1, m)
            }
        })
        .collect();
    let user_payload = serde_json::json!({
        "trail_oldest_first": trail.join("\n\n"),
    });

    // Compose the system prompt: base rubric + per-mode calibration
    // + output-format footer.
    let system_prompt = format!(
        "{}{}{}",
        AEGIS_SYSTEM_PROMPT_BASE,
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
        // request time (400 wrong_api_format). The 0..=3 ceiling
        // is therefore enforced two other ways:
        //   * the system prompt explicitly says "produce 0..=3
        //     suggestions"
        //   * the route layer truncates to 3 at insert time
        //     (`run_chat_message`'s persistence block)
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
                                "required": ["kind", "severity", "text"],
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
                                    "text": { "type": "string" }
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
// voice / scope / intent is preserved -- this is a *revision*, not
// a complete rewrite by the assistant. Output is plain text (no
// JSON envelope) since the only consumer pastes it into the chat
// input box.
//
// Same model + soft-fail discipline as the analyzer; on any
// upstream failure the route returns 500 and the frontend keeps
// the student's original draft.

const AEGIS_REWRITE_SYSTEM_PROMPT: &str = r#"You are Aegis, the prompt-coaching assistant. The student has a draft prompt and you previously produced a small list of suggestions for it. Your job now is to rewrite the draft so it incorporates ALL the suggestions, then return the rewritten prompt.

Hard rules:
* Preserve the student's voice, intent, scope, level of formality, and what they actually want to know. You are revising their draft, not replacing it with your own question.
* Do NOT add information that is not implied by the original draft + the suggestions. If a suggestion says "name the version", do not invent which version they mean -- write a placeholder like "(I'm using Python 3.X)" so they can edit, OR rewrite as "(specify which Python version you're using)".
* Do NOT answer the question. The output is a PROMPT the student will then send to the chatbot, not an answer to the prompt.
* Do NOT include preamble, headings, "Here is the rewrite:", quote marks, or any wrapping. Output is the prompt and only the prompt, ready to drop into the chat input.
* Keep the prompt natural and concise. If the original was one sentence, the rewrite should usually still be one or two sentences -- not a multi-paragraph essay just because the suggestions added structure.
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
        // Nothing to incorporate -- the rewrite would just be the
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
    // user payload so it can't confuse one for the other.
    let user_payload = serde_json::json!({
        "original_draft": original,
        "suggestions": suggestions,
    });

    let body = serde_json::json!({
        "model": AEGIS_MODEL,
        "temperature": 0.2,
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
    record_cerebras_usage(db, course_id, CATEGORY_AEGIS, AEGIS_MODEL, &payload).await;

    let rewritten = payload["choices"][0]["message"]["content"]
        .as_str()
        .map(str::trim)
        .unwrap_or("");
    if rewritten.is_empty() {
        return Err("empty rewrite from model".to_string());
    }
    Ok(rewritten.to_string())
}
