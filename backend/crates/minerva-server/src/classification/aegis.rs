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

Hard rules:
* Do NOT answer the prompt. Do NOT critique the chatbot's reply. Your only output is suggestions about the prompt itself.
* Do NOT score, rank, or grade the prompt. No numbers, no rubric, no "your prompt is X/10".
* If the prompt is already clear and the student has been thoughtful, return an EMPTY suggestion list. Don't invent suggestions for the sake of having something to say -- that's the condescending failure mode we are explicitly avoiding.
* Each suggestion is a single sentence in the second person ("you could...", "consider..."). No leading bullets, no markdown, ≤ 30 words.
* Each suggestion includes a `kind` tag from this list: "context", "constraints", "specificity", "alternatives", "clarification". Pick the closest match; one suggestion can only have one kind.
* Order suggestions most-impactful first. Prefer ONE strong suggestion over three weak ones.

Tone: constructive partner, not condescending teacher. Encouraging, never moralising. Never lecture, never refuse. The student decides whether to act on your feedback; this is non-blocking advice.

Be generous on short follow-up questions whose context is obvious from the previous turns. A two-word "explain that further" after a long substantive turn doesn't need any suggestions."#;

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

Default behaviour: RETURN AN EMPTY SUGGESTION LIST. A beginner asking a question -- any question, even a vague one -- is doing the work we want. The tutoring chatbot will fill in the missing context for them. Your job is NOT to teach them how to prompt; it's to occasionally point out one tiny low-effort improvement when one is genuinely useful.

Suggest at most ONE thing, and only when the draft is so vague the chatbot would have to guess wildly to even start (e.g. literally one or two words with no verb, or asking about "this" with no antecedent in the trail). For everything else, return [].

NEVER suggest:
* Adding domain terminology a beginner wouldn't know yet.
* Adding "background", "context", "scope", "constraints", "success criteria", or any prompt-engineering jargon.
* Specifying versions, tools, frameworks, or technical details.
* Restructuring the sentence.

When you DO suggest something, write it warmly and concretely as a single short sentence the student could add verbatim. Example: "You could add what you've already tried, like 'I read about lists but the docs confused me.'"

Examples that should return []:
- "How does recursion work?"
- "What is a generic in Java?"
- "Can you explain that more simply?"
- "How to make Python faster?"
- "I'm stuck on the sorting assignment"

Examples that warrant ONE suggestion:
- "this" (kind: clarification, text: "Could you describe what 'this' refers to? For example: 'this code I just wrote' or 'the topic from the last lecture'.")
- "help" (kind: clarification, text: "What part are you stuck on? Even a few words helps -- like 'I don't get how loops work' or 'my code throws an error'.")"#;

const AEGIS_EXPERT_ADDENDUM: &str = r#"

THE STUDENT IS AN EXPERT. This changes your behaviour substantially.

Default behaviour: RETURN AT LEAST ONE SUGGESTION unless the draft is genuinely complete (named scope, named tool/version where relevant, stated what they've tried OR a clear conceptual question, no vague placeholders). Most expert drafts have at least one fixable gap; find the sharpest one and surface it.

Hold the bar peer-to-peer high:
* Vague nouns ("this thing", "the issue", "it's broken") almost always warrant a `clarification` suggestion naming the specific concept/symbol.
* Missing version / tool / framework on a technical question almost always warrants a `constraints` suggestion.
* "How to X" without naming what X is FOR (the actual task) almost always warrants a `context` suggestion to add the goal.
* Expecting a code answer without sharing what they've already tried almost always warrants a `context` suggestion.
* Asking for a recommendation without listing the alternatives they're choosing between almost always warrants an `alternatives` suggestion.

When you DO suggest something, write it directly and tersely -- terminology IS expected here. Don't soften with "you could maybe" or "perhaps consider". Use the imperative or near-imperative ("Name the specific X.", "Add what you tried.", "Specify which Y.").

Examples that should return [] (rare):
- "Why does Python's GIL prevent CPU-bound multithreading from scaling, and how does multiprocessing sidestep it for tasks that release the GIL inside C extensions?"

Examples that warrant suggestions:
- "How to make Python faster?" -> [{kind: "context", text: "Name what's slow and how you measured it. CPU-bound vs I/O-bound has completely different fixes."}, {kind: "constraints", text: "Pin the Python version -- 3.11+ has substantial perf changes that change the right answer."}]
- "Tell me about decorators" -> [{kind: "context", text: "Say what you already know -- syntax-level vs semantics vs typical use cases dictates a very different answer."}]"#;

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
    /// Short kind tag. Free-form string by design (so adding a
    /// category doesn't force a server change), but the system
    /// prompt restricts the model to a documented set.
    pub kind: String,
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
                                "required": ["kind", "text"],
                                "properties": {
                                    "kind": {
                                        "type": "string",
                                        "enum": [
                                            "context",
                                            "constraints",
                                            "specificity",
                                            "alternatives",
                                            "clarification",
                                        ]
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
