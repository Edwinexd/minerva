//! Aegis: prompt-coaching analyzer.
//!
//! When the `aegis` feature flag is on for a course, every user
//! turn fires a single Cerebras call that scores the prompt along
//! the rubric drawn from the project description (Shen & Tamkin
//! 2026; Chen et al. 2024):
//!
//!   * clarity            -- specificity of the request, defined terms
//!   * context            -- background sufficient for the model
//!   * constraints        -- limits / goals stated
//!   * reasoning_demand   -- explanation/comparison vs. raw output
//!   * critical_thinking  -- justification, alternatives, uncertainty
//!
//! Plus three short feedback strings that mirror the figma mockup's
//! "Prompt Analysis" section: structural-clarity, terminology
//! specificity, and missing-constraint callouts. Each carries a
//! short label ("strong" / "weak" / "okay" / ...) and a
//! one-sentence rationale the panel renders verbatim.
//!
//! Soft-fail throughout. The chat hot path never waits on the
//! analyzer -- a transient Cerebras hiccup, malformed JSON, or DB
//! insert failure logs at warn and we move on without a row. The
//! Feedback panel just won't have content for that turn; the
//! assistant's reply still streams normally.
//!
//! Model selection mirrors the extraction-guard intent classifier:
//! `llama3.1-8b` is the cheapest in the catalog and per-turn latency
//! is the most important number here since the analyzer races the
//! generation strategy on every chat message.

use sqlx::PgPool;
use uuid::Uuid;

use crate::strategy::common::{cerebras_request_with_retry, record_cerebras_usage};
use minerva_db::queries::course_token_usage::CATEGORY_AEGIS;

/// Tiny model -- runs on every debounced keystroke when the flag
/// is on, so latency is the headline number. The schema-constrained
/// output keeps llama3.1-8b on rails for the JSON shape we want.
/// `pub` so the route layer can stamp `model_used` on persisted
/// rows from the same constant the analyzer actually called.
pub const AEGIS_MODEL: &str = "llama3.1-8b";

/// Cap on the analyzer's reply. The JSON has 6 ints + 3 short
/// label strings + 3 one-sentence feedback strings -- fits in 350
/// completion tokens with margin.
const AEGIS_MAX_TOKENS: usize = 512;

/// How many previous user turns we feed to the analyzer for
/// context. Five is enough that "explain that further" reads as
/// well-grounded follow-up rather than a context-free prompt with
/// missing constraints, while still being short enough to keep
/// the prompt cheap.
const HISTORY_TURNS: usize = 5;

const AEGIS_SYSTEM_PROMPT_BASE: &str = r#"You are Aegis, a prompt-coaching assistant for university students using a course-aware tutoring chatbot.

You read a student's prompt (and a short trail of their recent prior prompts in the same conversation, for context) and score the *prompt itself* on five dimensions. You DO NOT answer the prompt and you DO NOT critique the chatbot's reply. Your only job is to help the student learn to prompt more intentionally.

Score each dimension on an integer scale 0..=10, where 0 = absent / actively harmful and 10 = exemplary:

1. clarity              -- Is the request specific? Are key terms defined?
2. context              -- Does the model have enough background to answer well?
3. constraints          -- Are limits, goals, scope, or success criteria stated?
4. reasoning_demand     -- Does the prompt require explanation / derivation, or just a raw output?
5. critical_thinking    -- Does it ask for justification, compare alternatives, or acknowledge uncertainty?

Be generous on short follow-up questions whose context is obvious from the previous turns; the trail you receive carries that context. A two-word "explain that further" after a long substantive turn is not a 0/10 prompt.

Then produce three short feedback callouts, each with a short label and a single-sentence rationale (no leading bullet, no markdown, ≤ 25 words):

* structural_clarity  -- one of "strong", "okay", "weak". Look at sentence structure, ordering, whether the ask is at the front.
* terminology         -- one of "specific", "loose", "missing". Whether key technical / domain terms are present.
* missing_constraint  -- one of "well_constrained", "minor_gaps", "needs_constraints". What's the *one* most useful constraint or goal the student could add.

Finally produce overall_score on the same 0..=10 scale. It is YOUR aggregate, not an average -- weight the dimensions per-prompt."#;

/// Calibration addendum appended after the base rubric. Tells the
/// analyzer how to weight the rubric against the student's
/// self-declared subject expertise -- the SAME prompt should score
/// differently for a beginner ("How to make Python faster?" is a
/// reasonable opening from someone who's still learning, expect
/// the model's reply to fill in the missing scaffolding) vs an
/// expert (the same prompt is too vague, they should know to name
/// the bottleneck, what they've tried, etc.). The mode is the
/// student's own choice in the panel; we don't second-guess them.
const AEGIS_BEGINNER_ADDENDUM: &str = r#"

The student has marked themselves as a BEGINNER in this subject.
Calibrate the rubric accordingly:
* Be lenient on `terminology` when standard domain terms are missing -- a beginner doesn't yet have the vocabulary to name what they don't know.
* Be lenient on `context` when the student doesn't pre-load background -- the chatbot is here to provide that scaffolding for them.
* Score `clarity` and `constraints` mostly on whether the *intent* is legible, not on professional polish.
* In `missing_constraint_feedback`, suggest the SIMPLEST one-sentence addition that would help them learn (e.g. "say what you've tried so the assistant can build on it"), not a checklist of sophisticated framing.
* Tone: encouraging. A beginner who tries to articulate at all is doing the cognitive work we want."#;

const AEGIS_EXPERT_ADDENDUM: &str = r#"

The student has marked themselves as an EXPERT in this subject.
Calibrate the rubric accordingly:
* Hold `terminology` to a high bar -- precise domain terms should be present; vague placeholders (\"this thing\", \"the issue\") cost points.
* Hold `context` to a high bar -- expect the student to pre-load what they've tried, what error they hit, what assumption they're checking. They have the vocabulary to do this.
* Hold `constraints` to a high bar -- expect explicit scope (which language version? which tool? what success criterion?).
* In `missing_constraint_feedback`, name the SHARPEST single addition (not the easiest) -- the kind of detail an expert peer would naturally include and would unlock a substantively better answer.
* Tone: direct partner-to-partner. No coddling, but no condescension either."#;

const AEGIS_OUTPUT_FOOTER: &str = r#"

Tone overall: constructive partner, not condescending teacher. Offer one concrete improvement, not a list. Never lecture, never refuse, never include moralising. The student decides whether to act on your feedback; this is non-blocking advice.

Output JSON only, matching the schema. No prose."#;

/// Student's self-declared subject expertise. Toggled in the
/// frontend's panel; passed verbatim from the analyze route. The
/// analyzer's only use of this is to pick which calibration
/// addendum to append after the base rubric -- the rubric itself
/// (5 scores + 3 callouts) is identical between modes.
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
}

/// Result of one analyzer run. The route layer wraps this in
/// `AegisAnalysisPayload` for the wire format and for DB insertion;
/// keeping the analyzer's own type field-for-field identical to that
/// payload would tangle the LLM-call concern with the wire/DB
/// concerns. The route conversion handles that mapping.
#[derive(Debug, Clone)]
pub struct AegisVerdict {
    pub overall_score: i32,
    pub clarity_score: i32,
    pub context_score: i32,
    pub constraints_score: i32,
    pub reasoning_demand_score: i32,
    pub critical_thinking_score: i32,
    pub structural_clarity_label: String,
    pub structural_clarity_feedback: String,
    pub terminology_label: String,
    pub terminology_feedback: String,
    pub missing_constraint_label: String,
    pub missing_constraint_feedback: String,
}

/// Run the analyzer. `recent_user_messages` is the trail of the
/// student's last few prompts (oldest first); the LAST element is
/// the current turn -- the one we score. `mode` is the student's
/// self-declared subject expertise; calibrates the rubric (see
/// the addendum constants). Soft-fails to `None` on every error
/// path; callers treat that as "no panel content for this turn".
pub async fn analyze_prompt(
    http: &reqwest::Client,
    api_key: &str,
    db: &PgPool,
    course_id: Uuid,
    recent_user_messages: &[String],
    mode: AegisMode,
) -> Option<AegisVerdict> {
    if api_key.is_empty() {
        // Dev / test path without CEREBRAS_API_KEY. We could
        // synthesise a placeholder verdict here but that would
        // ship inert content to the panel; better to render nothing.
        return None;
    }
    let current = recent_user_messages.last()?;
    if current.trim().is_empty() {
        return None;
    }

    // Build the trail compactly. Numbered, oldest first; the
    // current turn is highlighted as `[current]` so the model
    // never confuses prior turns with the prompt under review.
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
                format!("[current] {}", m)
            } else {
                format!("[prior {}] {}", i + 1, m)
            }
        })
        .collect();
    let user_payload = serde_json::json!({
        "trail_oldest_first": trail.join("\n\n"),
    });

    // NOTE: llama3.1-8b on Cerebras returns 400 Bad Request when
    // `reasoning_effort` is in the body ("wrong_api_format" -- the
    // parameter is reserved for the gpt-oss reasoning models). It
    // used to be silently accepted; the API got stricter at some
    // point. We're already on the cheapest/fastest non-reasoning
    // model so the parameter is meaningless here regardless.
    // Compose the system prompt: base rubric + per-mode calibration
    // + output-format footer. Concatenation here (not in const) so
    // the addendums stay isolated -- easier to tune one without
    // accidentally drifting the other.
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
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "aegis_prompt_analysis",
                "strict": true,
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "overall_score",
                        "clarity_score",
                        "context_score",
                        "constraints_score",
                        "reasoning_demand_score",
                        "critical_thinking_score",
                        "structural_clarity_label",
                        "structural_clarity_feedback",
                        "terminology_label",
                        "terminology_feedback",
                        "missing_constraint_label",
                        "missing_constraint_feedback",
                    ],
                    "properties": {
                        "overall_score":           { "type": "integer", "minimum": 0, "maximum": 10 },
                        "clarity_score":           { "type": "integer", "minimum": 0, "maximum": 10 },
                        "context_score":           { "type": "integer", "minimum": 0, "maximum": 10 },
                        "constraints_score":       { "type": "integer", "minimum": 0, "maximum": 10 },
                        "reasoning_demand_score":  { "type": "integer", "minimum": 0, "maximum": 10 },
                        "critical_thinking_score": { "type": "integer", "minimum": 0, "maximum": 10 },
                        "structural_clarity_label":    { "type": "string" },
                        "structural_clarity_feedback": { "type": "string" },
                        "terminology_label":           { "type": "string" },
                        "terminology_feedback":        { "type": "string" },
                        "missing_constraint_label":    { "type": "string" },
                        "missing_constraint_feedback": { "type": "string" },
                    }
                }
            }
        }
    });

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("aegis: request failed (soft-fail): {}", e);
            return None;
        }
    };
    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("aegis: response not JSON (soft-fail): {}", e);
            return None;
        }
    };
    record_cerebras_usage(db, course_id, CATEGORY_AEGIS, AEGIS_MODEL, &payload).await;

    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("aegis: verdict not parseable JSON (soft-fail): {}", e);
            return None;
        }
    };

    // Each score is structured-output-validated to 0..=10, but be
    // defensive on the read path: a missing field means the schema
    // engine slipped, and we'd rather render nothing than a row of
    // zeros that look like real scores.
    fn read_score(v: &serde_json::Value, key: &str) -> Option<i32> {
        let n = v.get(key)?.as_i64()?;
        if (0..=10).contains(&n) {
            Some(n as i32)
        } else {
            None
        }
    }
    let verdict = AegisVerdict {
        overall_score: read_score(&parsed, "overall_score")?,
        clarity_score: read_score(&parsed, "clarity_score")?,
        context_score: read_score(&parsed, "context_score")?,
        constraints_score: read_score(&parsed, "constraints_score")?,
        reasoning_demand_score: read_score(&parsed, "reasoning_demand_score")?,
        critical_thinking_score: read_score(&parsed, "critical_thinking_score")?,
        structural_clarity_label: parsed["structural_clarity_label"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        structural_clarity_feedback: parsed["structural_clarity_feedback"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        terminology_label: parsed["terminology_label"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        terminology_feedback: parsed["terminology_feedback"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        missing_constraint_label: parsed["missing_constraint_label"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        missing_constraint_feedback: parsed["missing_constraint_feedback"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    };
    Some(verdict)
}
