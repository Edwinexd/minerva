//! Extraction guard: detect "student pasted a task and is asking the
//! model to do it" and, when the task is graded work, intercept the
//! response. Whether the task is graded is not decided here: the chat
//! strategy knows that from the course's document kinds.
//!
//! Three pieces, each a thin wrapper around a Cerebras call:
//!
//! 1. `classify_intent`: per-turn pre-generation classifier. Looks
//!    at the last several user messages and decides whether the
//!    current turn is a pasted task with a request for its answer
//!    and no attempt from the student. Strict by design: a study
//!    question in the student's own words always passes through.
//!
//! 2. `check_output_for_solution`: post-generation verdict on the
//!    assistant's reply. Asks "would this pass as the student's
//!    submission for the graded task?", whether that is code, a
//!    calculation or a written answer. Used as the output-side
//!    guard when graded work is in scope for the turn.
//!
//! 3. `generate_socratic_rewrite`: when the output check trips,
//!    rewrite the assistant's reply into a Socratic question +
//!    visible policy note (per UX spec b: explicit note that we
//!    intercepted, not silent swap).
//!
//! All three run on the chat hot path. Intent + output check
//! keep latency bounded with tight `max_completion_tokens`,
//! `temperature: 0.0`, and `reasoning_effort: "low"`. Soft-fail
//! throughout: a transient provider hiccup never blocks a chat
//! turn; worst case we treat the verdict as "not extraction" /
//! "not solution" and continue.

use sqlx::PgPool;
use uuid::Uuid;

use crate::llm::util_request;
use minerva_db::queries::course_token_usage::CATEGORY_EXTRACTION_GUARD;

// All three guard calls run on the admin-selected utility model
// (resolved per call via `AppState::utility_model`), at temperature 0 +
// `reasoning_effort: "low"` (set per call body) to keep the always-on
// intent classifier bounded.

/// How many recent user messages the intent classifier sees. Five
/// turns is enough to catch "drift" cases where the student didn't
/// paste in the most recent message but did earlier and is now
/// just asking for the next bit of code.
pub const INTENT_HISTORY_TURNS: usize = 5;

/// Per-call completion-token cap. The output is at most ~150 tokens
/// of JSON for intent / output check, and ~500 for the rewrite.
const INTENT_MAX_TOKENS: usize = 512;
const OUTPUT_CHECK_MAX_TOKENS: usize = 256;
const REWRITE_MAX_TOKENS: usize = 600;

/// What the intent classifier returned. `is_extraction` is the
/// only value the caller acts on; `rationale` is logged into
/// `conversation_flags.metadata` for the teacher dashboard.
#[derive(Debug, Clone)]
pub struct IntentVerdict {
    pub is_extraction: bool,
    pub rationale: String,
}

const INTENT_SYSTEM_PROMPT: &str = r#"You are a check on a student's chat with a tutoring AI for a university course. The course may be about anything: programming, theory, mathematics, essay writing.

You will read the last few turns of the student's side of the conversation and decide ONE thing: did the student paste a task from the course and ask the AI to produce its answer, without any attempt of their own?

Reply YES only when ALL of these hold:
- The student's input includes verbatim or near-verbatim task text: numbered tasks or sub-questions (a, b, c), "your task is", "implement X that does Y", "describe / explain / calculate ...", grading criteria, deadlines, a structured problem statement.
- AND the student's actual ask is for the answer to that pasted task (e.g. "do this", "solve this", "write the code", "answer these", "give me the solution", or implicit by absence of any other question).
- AND the student has shown no attempt of their own at it: no draft answer, no code, no reasoning to check.

Reply NO for everything else, including:
- Asking about a concept in their own words ("explain recursion", "what is a Turing machine")
- Asking for a small example
- Pasting a task together with their OWN answer, code or reasoning and asking for feedback or help with it
- Asking how to approach a problem in general terms (without pasting the problem)
- A single textbook-style question typed by the student ("implement bubble sort", "what does an operating system do")
- Multi-turn conversations that drift toward a task but never include the pasted task text.

The bar is HIGH and STRICT. False positives (calling a legitimate study question a pasted task) are worse than false negatives.

Output JSON only, matching this schema exactly:
{
  "is_extraction": true | false,
  "rationale": short specific string. If true, name the task-shaped phrasing you saw verbatim. If false, say briefly why this looks like a legitimate study question.
}

No prose."#;

/// Run the intent classifier. `recent_user_messages` is the trail
/// of the student's last few messages (oldest first); the last
/// element is the current turn's input. The classifier only sees
/// student messages; assistant content is irrelevant for "is
/// the student trying to extract".
pub async fn classify_intent(
    http: &reqwest::Client,
    util: &crate::llm::UtilityModel,
    db: &PgPool,
    course_id: Uuid,
    recent_user_messages: &[String],
) -> IntentVerdict {
    if util.provider.is_none() {
        // Dev / test path without CEREBRAS_API_KEY. Fail open.
        return IntentVerdict {
            is_extraction: false,
            rationale: "intent classifier skipped (no api key)".to_string(),
        };
    }
    if recent_user_messages.is_empty() {
        return IntentVerdict {
            is_extraction: false,
            rationale: "no user messages".to_string(),
        };
    }

    // Build a compact transcript: numbered, oldest first.
    let transcript = recent_user_messages
        .iter()
        .enumerate()
        .map(|(i, m)| format!("[{}] {}", i + 1, m))
        .collect::<Vec<_>>()
        .join("\n\n");
    let user_payload = serde_json::json!({
        "student_messages_oldest_first": transcript,
    });

    let body = serde_json::json!({
        "model": util.model,
        "temperature": 0.0,
        "reasoning_effort": "low",
        "max_completion_tokens": INTENT_MAX_TOKENS,
        "messages": [
            { "role": "system", "content": INTENT_SYSTEM_PROMPT },
            { "role": "user", "content": user_payload.to_string() },
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "extraction_intent_verdict",
                "strict": true,
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["is_extraction", "rationale"],
                    "properties": {
                        "is_extraction": { "type": "boolean" },
                        "rationale": { "type": "string" },
                    }
                }
            }
        }
    });

    let (content, usage) = match util_request(http, util, &body).await {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            tracing::warn!("extraction_guard: intent request failed (fail-open): {}", e);
            return IntentVerdict {
                is_extraction: false,
                rationale: format!("intent classifier failed: {e}"),
            };
        }
        None => {
            return IntentVerdict {
                is_extraction: false,
                rationale: "no utility model configured".to_string(),
            };
        }
    };
    crate::llm::record_pipeline_usage(
        db,
        course_id,
        CATEGORY_EXTRACTION_GUARD,
        &util.model,
        &usage,
    )
    .await;
    let raw = content.as_str();
    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "extraction_guard: intent verdict unparseable (fail-open): {}",
                e
            );
            return IntentVerdict {
                is_extraction: false,
                rationale: "intent verdict not valid JSON".to_string(),
            };
        }
    };
    IntentVerdict {
        is_extraction: parsed["is_extraction"].as_bool().unwrap_or(false),
        rationale: parsed["rationale"].as_str().unwrap_or_default().to_string(),
    }
}

/// What the output-side check decided about a finished assistant reply.
#[derive(Debug, Clone)]
pub struct OutputVerdict {
    pub is_complete_solution: bool,
    pub rationale: String,
}

const OUTPUT_CHECK_SYSTEM_PROMPT: &str = r#"You judge whether an AI tutor's response gave away the answer to a graded assignment.

Context: a student is working on graded work and MAY have pasted the assignment text and asked the tutor to solve it. The tutor has now produced its reply. Your job is to decide whether the reply, taken together, is a complete or near-complete answer the student could submit. The assignment may ask for code, a calculation, or a written answer.

Reply true ONLY when the tutor's reply:
- Does the specific thing the assignment asks for (implements the function / class / program, works the calculation through to its result, or writes out the requested answer or text), AND
- Could plausibly be submitted as the student's answer with little or no further work.

Reply false for:
- Explaining a concept, or showing a snippet or example that demonstrates it in the abstract.
- Pseudo-code, outlines, hints, or skeletal sketches.
- Material paired with a question that requires the student to fill something in.
- A worked example on a different problem than the assignment.
- Feedback on an attempt the student wrote themselves that stops short of rewriting it into a finished answer.

Output JSON only:
{
  "is_complete_solution": true | false,
  "rationale": short specific string.
}

No prose."#;

/// Output-side check. Caller passes the assistant's full reply +
/// the assignment context (excerpt of the matched assignment_brief
/// chunk(s), so the model can compare). Soft-fail to "false" on
/// transport or parsing errors.
pub async fn check_output_for_solution(
    http: &reqwest::Client,
    util: &crate::llm::UtilityModel,
    db: &PgPool,
    course_id: Uuid,
    assistant_reply: &str,
    assignment_excerpts: &[String],
) -> OutputVerdict {
    if util.provider.is_none() || assistant_reply.is_empty() {
        return OutputVerdict {
            is_complete_solution: false,
            rationale: "output check skipped".to_string(),
        };
    }
    let user_payload = serde_json::json!({
        "assignment_excerpts": assignment_excerpts,
        "assistant_reply": assistant_reply,
    });
    let body = serde_json::json!({
        "model": util.model,
        "temperature": 0.0,
        "reasoning_effort": "low",
        "max_completion_tokens": OUTPUT_CHECK_MAX_TOKENS,
        "messages": [
            { "role": "system", "content": OUTPUT_CHECK_SYSTEM_PROMPT },
            { "role": "user", "content": user_payload.to_string() },
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "extraction_output_verdict",
                "strict": true,
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["is_complete_solution", "rationale"],
                    "properties": {
                        "is_complete_solution": { "type": "boolean" },
                        "rationale": { "type": "string" },
                    }
                }
            }
        }
    });

    let (content, usage) = match util_request(http, util, &body).await {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            tracing::warn!("extraction_guard: output check failed (fail-open): {}", e);
            return OutputVerdict {
                is_complete_solution: false,
                rationale: format!("output check failed: {e}"),
            };
        }
        None => {
            return OutputVerdict {
                is_complete_solution: false,
                rationale: "no utility model configured".to_string(),
            };
        }
    };
    crate::llm::record_pipeline_usage(
        db,
        course_id,
        CATEGORY_EXTRACTION_GUARD,
        &util.model,
        &usage,
    )
    .await;
    let raw = content.as_str();
    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => {
            return OutputVerdict {
                is_complete_solution: false,
                rationale: "output verdict not valid JSON".to_string(),
            };
        }
    };
    OutputVerdict {
        is_complete_solution: parsed["is_complete_solution"].as_bool().unwrap_or(false),
        rationale: parsed["rationale"].as_str().unwrap_or_default().to_string(),
    }
}

/// Visible prefix added to the rewritten reply per UX spec (option b
/// in the design discussion). The student sees that the system
/// caught itself rather than getting a silent swap.
pub const REWRITE_PREFIX: &str = "_(I started to give you the full solution; per course policy I should help you work through it instead.)_\n\n";

const REWRITE_SYSTEM_PROMPT: &str = r#"The AI tutor was about to give a student the full answer to a graded assignment. You are rewriting the reply so it helps the student work through the problem instead.

Output a single short message that:
- Asks ONE specific Socratic question that pushes the student to think about the next step.
- Does NOT include the original answer, even partially.
- May reference the high-level concept involved without spelling out the solution.
- Stays in the same language as the student wrote (likely Swedish or English; match it).

Output ONLY the message text. No JSON, no markdown headers, no explanation of what you did. Just the question."#;

/// Generate a Socratic-question rewrite when the output check trips.
/// Returns the prefix + Socratic question. On any failure returns
/// a stock fallback so the chat path always has something to show.
pub async fn generate_socratic_rewrite(
    http: &reqwest::Client,
    util: &crate::llm::UtilityModel,
    db: &PgPool,
    course_id: Uuid,
    student_message: &str,
    original_reply: &str,
) -> String {
    let fallback = format!(
        "{}What's the first concrete step you'd take to solve this on your own? Walk me through it and I'll help you think it through.",
        REWRITE_PREFIX
    );
    if util.provider.is_none() {
        return fallback;
    }
    let user_payload = serde_json::json!({
        "student_message": student_message,
        "original_reply_we_blocked": original_reply,
    });
    let body = serde_json::json!({
        "model": util.model,
        "temperature": 0.3,
        "reasoning_effort": "low",
        "max_completion_tokens": REWRITE_MAX_TOKENS,
        "messages": [
            { "role": "system", "content": REWRITE_SYSTEM_PROMPT },
            { "role": "user", "content": user_payload.to_string() },
        ],
    });
    let (content, usage) = match util_request(http, util, &body).await {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            tracing::warn!("extraction_guard: rewrite request failed: {}", e);
            return fallback;
        }
        None => return fallback,
    };
    crate::llm::record_pipeline_usage(
        db,
        course_id,
        CATEGORY_EXTRACTION_GUARD,
        &util.model,
        &usage,
    )
    .await;
    let raw = content.as_str();
    if raw.trim().is_empty() {
        return fallback;
    }
    format!("{}{}", REWRITE_PREFIX, raw.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lazy pool: connect_lazy doesn't open a connection until used.
    /// All tests below take early-return paths that never touch the
    /// db, so a bogus URL is fine and keeps these tests free of a
    /// Postgres dependency.
    fn lazy_pool() -> PgPool {
        PgPool::connect_lazy("postgres://test:test@127.0.0.1:1/test").unwrap()
    }

    // Tests use `#[tokio::test]` so the lazy PgPool can be
    // constructed inside a Tokio context (sqlx::PgPool::connect_lazy
    // requires it). All tests still take early-return paths that
    // never actually open a connection.

    fn util(api_key: &str) -> crate::llm::UtilityModel {
        // Empty key -> no provider (the "no utility model" path). A
        // non-empty key wires a real provider pointed at an unreachable
        // endpoint: tests that pass one only exercise the early input
        // guards (empty history / empty message / code block), which
        // return before any network call is made.
        let provider: Option<std::sync::Arc<dyn crate::llm::ChatProvider>> = if api_key.is_empty() {
            None
        } else {
            Some(std::sync::Arc::new(
                crate::llm::OpenAiCompatibleProvider::new(
                    "test",
                    "http://127.0.0.1:1/v1",
                    api_key,
                    reqwest::Client::new(),
                ),
            ))
        };
        crate::llm::UtilityModel {
            provider,
            model: "gpt-oss-120b".to_string(),
        }
    }

    #[tokio::test]
    async fn classify_intent_fails_open_without_api_key() {
        // Sanity: no API key + dummy http client -> deterministic
        // not-extraction verdict, no panics.
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let v = classify_intent(
            &http,
            &util(""),
            &db,
            Uuid::nil(),
            &["implement my homework".to_string()],
        )
        .await;
        assert!(!v.is_extraction);
        assert!(v.rationale.contains("no api key"));
    }

    #[tokio::test]
    async fn classify_intent_handles_empty_history() {
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let v = classify_intent(&http, &util("fake-key"), &db, Uuid::nil(), &[]).await;
        assert!(!v.is_extraction);
        assert!(v.rationale.contains("no user messages"));
    }

    #[tokio::test]
    async fn rewrite_returns_fallback_without_api_key() {
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let s = generate_socratic_rewrite(&http, &util(""), &db, Uuid::nil(), "Q", "A").await;
        assert!(s.starts_with(REWRITE_PREFIX));
        assert!(s.len() > REWRITE_PREFIX.len());
    }
}
