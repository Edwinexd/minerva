// Wired into the chat strategies in a follow-up commit; the
// foundation lands first so the surface is testable in isolation.
#![allow(dead_code)]

//! Extraction guard: detect "student pasted an assignment and is
//! asking the model to do it" and intercept the response.
//!
//! Three pieces, each a thin wrapper around a Cerebras call:
//!
//! 1. `classify_intent`: per-turn pre-generation classifier. Looks
//!    at the last several user messages and decides whether the
//!    current turn is a literal pasted-assignment-extraction
//!    attempt. Strict by design -- per the operational policy, the
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
//! All three run on the chat hot path. Intent + output check use
//! `reasoning_effort: low` and tight `max_completion_tokens` to
//! keep latency bounded. Soft-fail throughout: a transient
//! Cerebras hiccup never blocks a chat turn -- worst case we
//! treat the verdict as "not extraction" / "not solution" and
//! continue.

use crate::strategy::common::cerebras_request_with_retry;

/// Cerebras model used for all three calls. Same as the rest of
/// the KG bundle so a single warmed cache services them all.
const GUARD_MODEL: &str = "gpt-oss-120b";

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
- The student's input includes verbatim or near-verbatim assignment text -- numbered tasks, "your task is", "implement X that does Y", grading criteria, deadlines, structured problem statement.
- AND the student's actual ask is to produce the code / answer for that pasted problem (e.g. "do this", "solve this", "write the code", "implement this", "give me the solution", or implicit by absence of any other question).

Reply NO for everything else, including:
- Asking about a concept ("explain recursion", "what's a generic in Java")
- Asking for a small example to learn syntax
- Asking about a function from the standard library
- Pasting the student's OWN code and asking for help debugging
- Asking how to approach a problem in general terms (without pasting the problem)
- Even "implement bubble sort" alone -- this is a textbook reference problem, not a pasted assignment unless the assignment text is also there.
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
/// student messages -- assistant content is irrelevant for "is
/// the student trying to extract".
pub async fn classify_intent(
    http: &reqwest::Client,
    api_key: &str,
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
        "model": GUARD_MODEL,
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
        "model": GUARD_MODEL,
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
- Stays in the same language as the student wrote (likely Swedish or English -- match it).

Output ONLY the message text. No JSON, no markdown headers, no explanation of what you did. Just the question."#;

/// Generate a Socratic-question rewrite when the output check trips.
/// Returns the prefix + Socratic question. On any failure returns
/// a stock fallback so the chat path always has something to show.
pub async fn generate_socratic_rewrite(
    http: &reqwest::Client,
    api_key: &str,
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
        "model": GUARD_MODEL,
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
    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    if raw.trim().is_empty() {
        return fallback;
    }
    format!("{}{}", REWRITE_PREFIX, raw.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_intent_fails_open_without_api_key() {
        // Sanity: no API key + dummy http client -> deterministic
        // not-extraction verdict, no panics.
        let http = reqwest::Client::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let v = rt.block_on(classify_intent(
            &http,
            "",
            &["implement my homework".to_string()],
        ));
        assert!(!v.is_extraction);
        assert!(v.rationale.contains("no api key"));
    }

    #[test]
    fn classify_intent_handles_empty_history() {
        let http = reqwest::Client::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let v = rt.block_on(classify_intent(&http, "fake-key", &[]));
        assert!(!v.is_extraction);
        assert!(v.rationale.contains("no user messages"));
    }

    #[test]
    fn rewrite_returns_fallback_without_api_key() {
        let http = reqwest::Client::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let s = rt.block_on(generate_socratic_rewrite(&http, "", "Q", "A"));
        assert!(s.starts_with(REWRITE_PREFIX));
        assert!(s.len() > REWRITE_PREFIX.len());
    }
}
