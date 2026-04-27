//! Extraction guard: detect "student pasted an assignment and is
//! asking the model to do it" and intercept the response.
//!
//! Three pieces, each a thin wrapper around a Cerebras call:
//!
//! 1. `classify_intent`: per-turn pre-generation classifier. Looks
//!    at the last several user messages and decides whether the
//!    current turn is a literal pasted-assignment-extraction
//!    attempt. Strict by design; per the operational policy, the
//!    only cases we lock down are the ones that are already
//!    academic-dishonesty-by-the-rules; legitimate study questions
//!    should always pass through, even ones that look code-y.
//!
//! 2. `check_output_for_solution`: post-generation verdict on the
//!    assistant's reply. Asks "does this contain code that would
//!    constitute a complete solution to a graded programming
//!    exercise?". Used as the output-side guard when the input
//!    was flagged or multi-turn proximity is high.
//!
//! 3. `generate_socratic_rewrite`: when the output check trips,
//!    rewrite the assistant's reply into a Socratic question +
//!    visible policy note (per UX spec b: explicit note that we
//!    intercepted, not silent swap).
//!
//! All three run on the chat hot path. Intent + output check
//! keep latency bounded with tight `max_completion_tokens` and
//! `temperature: 0.0`. Soft-fail throughout: a transient
//! Cerebras hiccup never blocks a chat turn; worst case we
//! treat the verdict as "not extraction" / "not solution" and
//! continue. (We previously sent `reasoning_effort: "low"` here
//! too; Cerebras now hard-rejects that parameter on llama3.1-8b
//! with a 400, and it never did anything for non-reasoning
//! models anyway.)

use sqlx::PgPool;
use uuid::Uuid;

use crate::strategy::common::{cerebras_request_with_retry, record_cerebras_usage};
use minerva_db::queries::course_token_usage::CATEGORY_EXTRACTION_GUARD;

// ── Cerebras model selection ───────────────────────────────────────
//
// Three of the four guard calls are simple binary-classification
// tasks (intent / output / engagement) where a small fast model is
// the right tool. The intent classifier in particular runs on
// EVERY chat turn when the feature flag is on, so its latency is
// the single most important number in this whole module. Cerebras
// already hosts llama3.1-8b in the catalog (see health.rs); using
// it cuts ~10x off the prompt-cost and a meaningful chunk of
// latency vs. gpt-oss-120b without measurable quality loss for
// JSON-schema-constrained binary decisions.
//
// The rewrite call is different: it produces user-visible prose
// (the Socratic question + policy-note), runs only when the
// output check tripped (rare), and the model's writing quality
// directly affects whether the student's experience feels coherent.
// We keep gpt-oss-120b for that path.

/// Tiny model for the always-on / conditional binary classifiers.
/// Cheapest in the Cerebras catalog. JSON-schema-constrained
/// outputs + temperature 0 keep it on rails.
const GUARD_CLASSIFIER_MODEL: &str = "llama3.1-8b";

/// Larger model for the rewrite path only; runs rarely (only
/// when the output check trips) and produces text the student
/// reads, so quality matters.
const GUARD_REWRITE_MODEL: &str = "gpt-oss-120b";

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

const INTENT_SYSTEM_PROMPT: &str = r#"You are an academic-integrity check on a student's chat with a tutoring AI for a programming course.

You will read the last few turns of the student's side of the conversation and decide ONE thing: did the student literally paste an assignment problem statement and ask the model to produce the implementation / solution / code that answers it?

Reply YES only when ALL of these hold:
- The student's input includes verbatim or near-verbatim assignment text; numbered tasks, "your task is", "implement X that does Y", grading criteria, deadlines, structured problem statement.
- AND the student's actual ask is to produce the code / answer for that pasted problem (e.g. "do this", "solve this", "write the code", "implement this", "give me the solution", or implicit by absence of any other question).

Reply NO for everything else, including:
- Asking about a concept ("explain recursion", "what's a generic in Java")
- Asking for a small example to learn syntax
- Asking about a function from the standard library
- Pasting the student's OWN code and asking for help debugging
- Asking how to approach a problem in general terms (without pasting the problem)
- Even "implement bubble sort" alone; this is a textbook reference problem, not a pasted assignment unless the assignment text is also there.
- Multi-turn conversations that drift toward implementation but never include a pasted assignment.

The bar is HIGH and STRICT. False positives (calling a legit study question extraction) are worse than false negatives. We are enforcing what is already defined as academic dishonesty in the course rules; we are NOT making the AI more conservative than its baseline.

Output JSON only, matching this schema exactly:
{
  "is_extraction": true | false,
  "rationale": short specific string. If true, name the assignment-shaped phrasing you saw verbatim. If false, say briefly why this looks like a legitimate study question.
}

No prose."#;

/// Run the intent classifier. `recent_user_messages` is the trail
/// of the student's last few messages (oldest first); the last
/// element is the current turn's input. The classifier only sees
/// student messages; assistant content is irrelevant for "is
/// the student trying to extract".
pub async fn classify_intent(
    http: &reqwest::Client,
    api_key: &str,
    db: &PgPool,
    course_id: Uuid,
    recent_user_messages: &[String],
) -> IntentVerdict {
    if api_key.is_empty() {
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
        "model": GUARD_CLASSIFIER_MODEL,
        "temperature": 0.0,
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

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("extraction_guard: intent request failed (fail-open): {}", e);
            return IntentVerdict {
                is_extraction: false,
                rationale: format!("intent classifier failed: {e}"),
            };
        }
    };
    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("extraction_guard: intent JSON failed (fail-open): {}", e);
            return IntentVerdict {
                is_extraction: false,
                rationale: format!("intent response not JSON: {e}"),
            };
        }
    };
    record_cerebras_usage(
        db,
        course_id,
        CATEGORY_EXTRACTION_GUARD,
        GUARD_CLASSIFIER_MODEL,
        &payload,
    )
    .await;
    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
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

const OUTPUT_CHECK_SYSTEM_PROMPT: &str = r#"You judge whether an AI tutor's response gave away the answer to a graded programming assignment.

Context: a student MAY have just pasted assignment text and asked the model to solve it. The model has now produced its reply. Your job is to decide whether the reply contains code that, taken together, would constitute a complete or near-complete solution the student could submit.

Reply true ONLY when the assistant's reply contains code that:
- Implements the specific function / class / algorithm the assignment asks for, AND
- Could plausibly be submitted as the student's answer with little or no further work.

Reply false for:
- Code snippets that demonstrate a concept / library function in the abstract.
- Pseudo-code or skeletal sketches.
- Code paired with a question that requires the student to fill something in.
- Explanatory prose with no code.
- Code that addresses a different problem than the assignment.

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
    api_key: &str,
    db: &PgPool,
    course_id: Uuid,
    assistant_reply: &str,
    assignment_excerpts: &[String],
) -> OutputVerdict {
    if api_key.is_empty() || assistant_reply.is_empty() {
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
        "model": GUARD_CLASSIFIER_MODEL,
        "temperature": 0.0,
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

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("extraction_guard: output check failed (fail-open): {}", e);
            return OutputVerdict {
                is_complete_solution: false,
                rationale: format!("output check failed: {e}"),
            };
        }
    };
    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("extraction_guard: output JSON failed (fail-open): {}", e);
            return OutputVerdict {
                is_complete_solution: false,
                rationale: format!("output verdict not JSON: {e}"),
            };
        }
    };
    record_cerebras_usage(
        db,
        course_id,
        CATEGORY_EXTRACTION_GUARD,
        GUARD_CLASSIFIER_MODEL,
        &payload,
    )
    .await;
    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
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

const REWRITE_SYSTEM_PROMPT: &str = r#"The AI tutor was about to give a student the full code answer to a graded assignment. You are rewriting the reply so it helps the student work through the problem instead.

Output a single short message that:
- Asks ONE specific Socratic question that pushes the student to think about the next step.
- Does NOT include the original implementation, even partially.
- May reference the high-level concept involved without spelling out the algorithm.
- Stays in the same language as the student wrote (likely Swedish or English; match it).

Output ONLY the message text. No JSON, no markdown headers, no explanation of what you did. Just the question."#;

/// Generate a Socratic-question rewrite when the output check trips.
/// Returns the prefix + Socratic question. On any failure returns
/// a stock fallback so the chat path always has something to show.
pub async fn generate_socratic_rewrite(
    http: &reqwest::Client,
    api_key: &str,
    db: &PgPool,
    course_id: Uuid,
    student_message: &str,
    original_reply: &str,
) -> String {
    let fallback = format!(
        "{}What's the first concrete step you'd take to solve this on your own? Walk me through it and I'll help you think it through.",
        REWRITE_PREFIX
    );
    if api_key.is_empty() {
        return fallback;
    }
    let user_payload = serde_json::json!({
        "student_message": student_message,
        "original_reply_we_blocked": original_reply,
    });
    let body = serde_json::json!({
        "model": GUARD_REWRITE_MODEL,
        "temperature": 0.3,
        "reasoning_effort": "low",
        "max_completion_tokens": REWRITE_MAX_TOKENS,
        "messages": [
            { "role": "system", "content": REWRITE_SYSTEM_PROMPT },
            { "role": "user", "content": user_payload.to_string() },
        ],
    });
    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("extraction_guard: rewrite request failed: {}", e);
            return fallback;
        }
    };
    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(_) => return fallback,
    };
    record_cerebras_usage(
        db,
        course_id,
        CATEGORY_EXTRACTION_GUARD,
        GUARD_REWRITE_MODEL,
        &payload,
    )
    .await;
    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    if raw.trim().is_empty() {
        return fallback;
    }
    format!("{}{}", REWRITE_PREFIX, raw.trim())
}

/// Verdict from `classify_engagement`. `engaged` is the only value
/// the caller acts on (it lifts the constraint). Rationale is logged
/// onto the `constraint_lifted` flag for the dashboard.
#[derive(Debug, Clone)]
pub struct EngagementVerdict {
    pub engaged: bool,
    pub rationale: String,
}

const ENGAGEMENT_SYSTEM_PROMPT: &str = r#"You judge whether a student is actively engaging with a programming tutor's Socratic guidance, or still trying to extract a ready-made solution.

Context: the tutor previously refused to give a complete answer to what looked like a pasted assignment, and instead asked the student a Socratic question or pushed them to think. You are reading the student's NEXT message and deciding whether they took the bait.

Reply true (engaged) when the student's message does ANY of:
- Includes their own code attempt (even if buggy / partial / wrong); a fenced code block, a function definition, a snippet they wrote.
- Describes their own approach in their own words (pseudo-code, plan, reasoning).
- Answers the tutor's Socratic question with a substantive opinion / guess / reasoning, not a deflection.
- Asks a focused conceptual follow-up that shows they tried to understand ("why does X happen here", "is the pattern Y the right one for this", "I think we should do Z, is that right").
- Shares an error / output / observation from running something themselves.

Reply false (not engaged, still extracting) when the student:
- Repeats the original request ("just give me the code", "but I need the answer", "stop with the questions").
- Pastes more assignment text or another sub-task and asks for that to be solved.
- Says they don't know / asks the tutor to do it for them with no attempt of their own.
- Sends a one-word "yes" / "ok" / "do it" with no substance.

The default when uncertain is true (engaged): we'd rather lift the constraint and risk a slip than keep pestering a student who is actually working. The output check still runs every turn; if they slip back into extraction, the constraint will re-trip on its own.

Output JSON only:
{
  "engaged": true | false,
  "rationale": short specific string. Name the engagement signal (or its absence).
}

No prose."#;

/// Engagement classifier. Decides whether the student's *new*
/// message represents genuine engagement with the prior Socratic
/// guidance, in which case the chat path lifts the extraction
/// constraint and resumes normal generation. The classifier sees
/// both the prior assistant reply (so it knows what was asked) and
/// the new student message.
///
/// Soft-fails to "engaged = true" on transport errors, matching the
/// "default to engaged when uncertain" policy in the prompt: false
/// negatives here would keep a working student stuck under the
/// constraint, which is the harm we want to avoid.
pub async fn classify_engagement(
    http: &reqwest::Client,
    api_key: &str,
    db: &PgPool,
    course_id: Uuid,
    prior_assistant_reply: &str,
    new_student_message: &str,
) -> EngagementVerdict {
    // Cheap heuristic: a fenced code block in the new message is a
    // strong "engaged" signal; the student is showing their own
    // work. Skip the LLM call entirely in that case to save latency.
    if new_student_message.contains("```") {
        return EngagementVerdict {
            engaged: true,
            rationale: "student included a code block".to_string(),
        };
    }
    if api_key.is_empty() {
        // Dev / test path. Per the "default engaged" policy, lift.
        return EngagementVerdict {
            engaged: true,
            rationale: "engagement check skipped (no api key)".to_string(),
        };
    }
    if new_student_message.trim().is_empty() {
        return EngagementVerdict {
            engaged: false,
            rationale: "empty student message".to_string(),
        };
    }
    let user_payload = serde_json::json!({
        "prior_assistant_reply": prior_assistant_reply,
        "new_student_message": new_student_message,
    });
    let body = serde_json::json!({
        "model": GUARD_CLASSIFIER_MODEL,
        "temperature": 0.0,
        "max_completion_tokens": OUTPUT_CHECK_MAX_TOKENS,
        "messages": [
            { "role": "system", "content": ENGAGEMENT_SYSTEM_PROMPT },
            { "role": "user", "content": user_payload.to_string() },
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "extraction_engagement_verdict",
                "strict": true,
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["engaged", "rationale"],
                    "properties": {
                        "engaged": { "type": "boolean" },
                        "rationale": { "type": "string" },
                    }
                }
            }
        }
    });

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                "extraction_guard: engagement request failed (defaulting to engaged): {}",
                e
            );
            return EngagementVerdict {
                engaged: true,
                rationale: format!("engagement classifier failed: {e}"),
            };
        }
    };
    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "extraction_guard: engagement JSON failed (defaulting to engaged): {}",
                e
            );
            return EngagementVerdict {
                engaged: true,
                rationale: format!("engagement response not JSON: {e}"),
            };
        }
    };
    record_cerebras_usage(
        db,
        course_id,
        CATEGORY_EXTRACTION_GUARD,
        GUARD_CLASSIFIER_MODEL,
        &payload,
    )
    .await;
    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => {
            return EngagementVerdict {
                engaged: true,
                rationale: "engagement verdict not valid JSON".to_string(),
            };
        }
    };
    EngagementVerdict {
        // Default-engaged is the safe fallback when the field is
        // missing; matches the "default engaged" policy.
        engaged: parsed["engaged"].as_bool().unwrap_or(true),
        rationale: parsed["rationale"].as_str().unwrap_or_default().to_string(),
    }
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

    #[tokio::test]
    async fn classify_intent_fails_open_without_api_key() {
        // Sanity: no API key + dummy http client -> deterministic
        // not-extraction verdict, no panics.
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let v = classify_intent(
            &http,
            "",
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
        let v = classify_intent(&http, "fake-key", &db, Uuid::nil(), &[]).await;
        assert!(!v.is_extraction);
        assert!(v.rationale.contains("no user messages"));
    }

    #[tokio::test]
    async fn rewrite_returns_fallback_without_api_key() {
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let s = generate_socratic_rewrite(&http, "", &db, Uuid::nil(), "Q", "A").await;
        assert!(s.starts_with(REWRITE_PREFIX));
        assert!(s.len() > REWRITE_PREFIX.len());
    }

    #[tokio::test]
    async fn engagement_short_circuits_on_code_block() {
        // A fenced code block is the cheap "engaged" signal; we
        // never even hit the LLM. Verifies that path returns true
        // without an API key.
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let v = classify_engagement(
            &http,
            "",
            &db,
            Uuid::nil(),
            "What's your first step?",
            "Here's what I tried:\n```python\ndef f(x):\n    return x*2\n```\nIs that right?",
        )
        .await;
        assert!(v.engaged);
        assert!(v.rationale.contains("code block"));
    }

    #[tokio::test]
    async fn engagement_defaults_engaged_without_api_key() {
        // Dev/test path: no API key -> default to engaged so the
        // constraint lifts. Matches the "lift on uncertainty" policy.
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let v = classify_engagement(
            &http,
            "",
            &db,
            Uuid::nil(),
            "ask",
            "I think we should sort first",
        )
        .await;
        assert!(v.engaged);
        assert!(v.rationale.contains("no api key"));
    }

    #[tokio::test]
    async fn engagement_returns_not_engaged_for_empty_message() {
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let v = classify_engagement(&http, "fake-key", &db, Uuid::nil(), "ask", "   ").await;
        assert!(!v.engaged);
    }
}
