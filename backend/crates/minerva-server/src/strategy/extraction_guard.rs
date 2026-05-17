//! Strategy-side extraction guard. Wraps the lower-level
//! `classification::extraction_guard` (which is just three
//! Cerebras-call wrappers) into the higher-level chat flow:
//! per-turn intent classification, multi-turn proximity tracking
//! via the KG, and post-generation output check + Socratic rewrite.
//!
//! Two entry points the strategies call:
//!
//! 1. `evaluate_for_turn`; runs after RAG retrieval, before
//!    generation. Resolves whether the extraction guard is enabled
//!    for this course, runs the intent classifier, computes
//!    "assignments near this turn" from RAG signals + KG
//!    `applied_in` partners, slides the recent-turns window in
//!    `kg_state`, and decides whether the constraint is active for
//!    this turn. Persists the updated `kg_state`.
//!
//! 2. `intercept_reply`; runs after generation, with the full
//!    assistant text. Idempotent no-op when the guard wasn't
//!    enabled or the constraint isn't active. When active: runs
//!    the output-side solution check; if it trips, generates a
//!    Socratic rewrite, sends a `rewrite` SSE event so the
//!    frontend can swap the displayed message, logs a
//!    `conversation_flag` row for the teacher dashboard, and
//!    returns the rewrite for downstream `finalize` to persist.
//!
//! Engagement detection (which would lift the constraint when the
//! student writes their own code or answers a Socratic question)
//! is NOT in this commit; it lands in the next one along with
//! the dashboard frontend.

use std::collections::HashSet;

use axum::response::sse::Event;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::classification::extraction_guard::{
    self, EngagementVerdict, IntentVerdict, OutputVerdict, INTENT_HISTORY_TURNS,
};
use crate::error::AppError;
use crate::feature_flags::extraction_guard_enabled;
use crate::strategy::common::RagChunk;

// ── flag kind constants ────────────────────────────────────────────
//
// We log an append-only event-stream of guard decisions to
// `conversation_flags`. Each row records ONE classifier verdict or
// state transition; the dashboard reconstructs the lifecycle by
// reading them oldest-first. Five kinds, all turn-indexed so the
// per-turn UI on the conversation detail page can align them.

/// Intent classifier returned `is_extraction = true` for this turn.
/// Independent of whether the constraint was already active --
/// gives the teacher the per-turn classifier signal even when the
/// guard was already locked on from a prior turn.
pub const INTENT_DETECTED_FLAG: &str = "extraction_intent_detected";

/// Constraint flipped from off to on this turn. Cause may be
/// intent OR proximity OR both; the metadata records which.
/// This is the "the guard is now constraining this conversation"
/// event the teacher dashboard primarily badges.
pub const CONSTRAINT_ACTIVATED_FLAG: &str = "extraction_constraint_activated";

/// Output check tripped during `intercept_reply` and we replaced
/// the streamed assistant text with a Socratic rewrite. Distinct
/// from `extraction_intent_detected` because the input-side and
/// output-side checks are independent: one can fire without the
/// other (e.g. the model produced a complete solution despite the
/// intent classifier saying no, or the intent classifier flagged
/// the input but the model handled it Socratically anyway).
pub const REWROTE_FLAG: &str = "extraction_rewrote";

/// Engagement classifier said `engaged = true` and we lifted the
/// constraint. Pairs with the `_activated` flag from earlier in
/// the conversation to bracket the lifecycle.
pub const CONSTRAINT_LIFTED_FLAG: &str = "extraction_constraint_lifted";

/// Engagement classifier said `engaged = false`; the student
/// didn't take the Socratic bait. Constraint stays on. Logged so
/// the teacher can see how many refusals it took before the
/// constraint either lifted or the conversation ended.
pub const ENGAGEMENT_REFUSED_FLAG: &str = "extraction_engagement_refused";

/// How many recent turns to keep in `kg_state.recent_turns` for the
/// multi-turn proximity check. 5 matches the spec
/// (Q3.2; sliding window).
const RECENT_TURNS_WINDOW: usize = 5;

/// Multi-turn proximity threshold: if the same assignment appears
/// in this many of the last `RECENT_TURNS_WINDOW` turns, the
/// constraint flips on even without a direct intent-classifier
/// trigger this turn.
const PROXIMITY_THRESHOLD: usize = 2;

// ── kg_state shape (matches the JSONB column) ──────────────────────

/// In-memory mirror of the JSONB blob we keep on
/// `conversations.kg_state`. Fields are all `serde(default)` so
/// older empty rows deserialise cleanly into the default state.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct KgState {
    /// True iff the extraction guard is currently constraining the
    /// conversation; next turn's generation will be subject to
    /// the output-side check unless engagement lifts it.
    #[serde(default)]
    pub constraint_active: bool,
    /// Which assignment doc ids the constraint is tracking. Used
    /// when the output check needs to know which assignments to
    /// reference, and when the dashboard shows what triggered.
    #[serde(default)]
    pub constraint_assignment_doc_ids: Vec<Uuid>,
    /// 1-based turn index at which the constraint was last lifted
    /// (engagement detected). Lets the dashboard show the lifecycle
    /// of an extraction attempt over time. None until first lift.
    #[serde(default)]
    pub constraint_lifted_at_turn: Option<i32>,
    /// Sliding-window log of the last few turns: which assignment
    /// doc ids were "near" each turn (direct retrieval signal +
    /// `applied_in` partners of the lectures in context).
    #[serde(default)]
    pub recent_turns: Vec<RecentTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentTurn {
    pub turn_idx: i32,
    pub assignments_near: Vec<Uuid>,
}

// ── per-turn evaluation result ─────────────────────────────────────

/// Lives across the strategy run: produced by `evaluate_for_turn`,
/// consumed by `intercept_reply`. Carries both the verdict and the
/// data the post-generation check needs (assignment excerpts so the
/// output check has context for its judgement).
pub struct GuardDecision {
    /// 1-based index of this turn within the conversation. Stamped
    /// onto any `conversation_flags` we emit so the dashboard can
    /// align flags to messages.
    pub turn_index: i32,
    /// Pre-generation classifier verdict. Always populated when the
    /// guard ran; soft-fail elsewhere returns
    /// `is_extraction = false`.
    pub intent: IntentVerdict,
    /// Whether the constraint applies to *this* turn's generation.
    /// True iff the intent classifier said is_extraction OR the
    /// multi-turn proximity threshold tripped OR the prior turn
    /// was active and engagement hasn't lifted it.
    pub constraint_active: bool,
    /// Excerpts the output check feeds the model so it can compare
    /// the assistant's reply against what the assignment actually
    /// asked. Drawn from `rag_signals` + the in-scope assignment
    /// docs' representative text.
    pub assignment_excerpts: Vec<String>,
    /// Assignment doc ids relevant to this turn; used by the
    /// flag-row metadata so the dashboard knows which assignments
    /// the guard tagged.
    pub in_scope_assignment_doc_ids: Vec<Uuid>,
}

// ── public API ─────────────────────────────────────────────────────

/// Phase 1: evaluate the guard for this turn, after retrieval
/// finishes and before generation starts. Returns `None` when the
/// extraction_guard feature flag is OFF for this course (the chat
/// path skips the rest of the integration in that case). Returns
/// `Some(decision)` otherwise; the decision encodes whether the
/// constraint is active for this turn.
///
/// Side effects: persists the updated `kg_state` (sliding-window
/// recent_turns, constraint_active flag) to the conversations row.
#[allow(clippy::too_many_arguments)]
pub async fn evaluate_for_turn(
    db: &PgPool,
    http: &reqwest::Client,
    api_key: &str,
    course_id: Uuid,
    conversation_id: Uuid,
    history: &[minerva_db::queries::conversations::MessageRow],
    user_content: &str,
    rag_signals: &[RagChunk],
    rag_context: &[RagChunk],
) -> Option<GuardDecision> {
    if !extraction_guard_enabled(db, course_id).await {
        tracing::debug!(
            "extraction_guard: feature flag off for course {}, skipping",
            course_id
        );
        return None;
    }

    let turn_index = compute_turn_index(history);

    // Intent classifier sees the last N user messages, oldest
    // first, with the current turn's input as the most recent.
    let recent_user_messages = recent_user_messages(history, user_content, INTENT_HISTORY_TURNS);
    let intent =
        extraction_guard::classify_intent(http, api_key, db, course_id, &recent_user_messages)
            .await;
    tracing::info!(
        "extraction_guard: turn={} conversation={} intent.is_extraction={} intent.rationale={:?}",
        turn_index,
        conversation_id,
        intent.is_extraction,
        intent.rationale
    );

    // Per-turn intent classifier flag. Append-only event log:
    // recorded whenever the classifier returns yes, *independent*
    // of whether the constraint was already active. Lets the
    // teacher see every turn the classifier flagged, not just the
    // first one in a streak.
    if intent.is_extraction {
        let metadata = serde_json::json!({
            "intent": {
                "is_extraction": true,
                "rationale": intent.rationale,
            },
        });
        if let Err(e) = minerva_db::queries::conversation_flags::insert(
            db,
            conversation_id,
            INTENT_DETECTED_FLAG,
            Some(turn_index),
            Some(intent.rationale.as_str()),
            Some(&metadata),
        )
        .await
        {
            tracing::warn!(
                "extraction_guard: failed to insert {} flag for {}: {}",
                INTENT_DETECTED_FLAG,
                conversation_id,
                e
            );
        }
    }
    tracing::info!(
        target: "extraction_guard",
        conversation_id = %conversation_id,
        turn = turn_index,
        is_extraction = intent.is_extraction,
        rationale = %intent.rationale,
        "intent verdict",
    );

    // Compute "assignments near this turn":
    //   * direct: signals are assignment-kind chunks above the
    //     similarity floor.
    //   * graph-derived: lectures / readings / transcripts in
    //     context, mapped via `applied_in` to assignment dst docs.
    let mut assignments_near: HashSet<Uuid> = HashSet::new();
    for s in rag_signals {
        if let Ok(uuid) = Uuid::parse_str(&s.document_id) {
            assignments_near.insert(uuid);
        }
    }
    let context_lecture_doc_ids: Vec<Uuid> = rag_context
        .iter()
        .filter(|c| {
            matches!(
                c.kind.as_deref(),
                Some("lecture") | Some("lecture_transcript") | Some("reading")
            )
        })
        .filter_map(|c| Uuid::parse_str(&c.document_id).ok())
        .collect();
    if !context_lecture_doc_ids.is_empty() {
        match minerva_db::queries::document_relations::applied_in_assignments_for_lectures(
            db,
            course_id,
            &context_lecture_doc_ids,
        )
        .await
        {
            Ok(extra) => assignments_near.extend(extra),
            Err(e) => tracing::warn!("extraction_guard: applied_in lookup failed: {}", e),
        }
    }
    let assignments_near_vec: Vec<Uuid> = assignments_near.iter().copied().collect();

    // Read kg_state, slide the window. Engagement check runs
    // BEFORE the constraint-active decision: if the prior turn
    // left the constraint on, we look at this turn's student
    // message to see whether the student engaged with whatever
    // Socratic prompt they were given. If so, the constraint
    // lifts for *this* turn and the conversation resumes normal
    // generation (the output check still runs every turn that's
    // active, so a relapse re-trips on its own).
    let mut state = load_kg_state(db, conversation_id).await;
    push_turn(&mut state, turn_index, &assignments_near_vec);
    let proximity_active = proximity_threshold_tripped(&state);
    let prev_active_before_lift = state.constraint_active;

    let mut engagement_verdict: Option<EngagementVerdict> = None;
    if state.constraint_active {
        let prior_assistant = history
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let v = extraction_guard::classify_engagement(
            http,
            api_key,
            db,
            course_id,
            prior_assistant,
            user_content,
        )
        .await;
        tracing::info!(
            "extraction_guard: turn={} conversation={} engagement.engaged={} engagement.rationale={:?}",
            turn_index,
            conversation_id,
            v.engaged,
            v.rationale
        );
        if v.engaged {
            state.constraint_active = false;
            state.constraint_lifted_at_turn = Some(turn_index);
            // Log the lift so the dashboard can show the
            // lifecycle (activated at turn X, lifted at turn Y).
            // Best-effort; log and move on if it fails.
            let metadata = serde_json::json!({
                "engagement": {
                    "engaged": true,
                    "rationale": v.rationale,
                },
                "lifted_assignment_doc_ids": state.constraint_assignment_doc_ids,
            });
            if let Err(e) = minerva_db::queries::conversation_flags::insert(
                db,
                conversation_id,
                CONSTRAINT_LIFTED_FLAG,
                Some(turn_index),
                Some(v.rationale.as_str()),
                Some(&metadata),
            )
            .await
            {
                tracing::warn!(
                    "extraction_guard: failed to log lift flag for {}: {}",
                    conversation_id,
                    e
                );
            }
        } else {
            // Refusal event: student didn't take the Socratic
            // bait. Constraint stays on. Logged so the teacher
            // can see how many refusals it took before either a
            // lift or the conversation ending. Per the on-
            // transitions policy: only logged when constraint
            // was active going in; we don't log "engaged" for
            // turns where the constraint was off (those would be
            // noise).
            let metadata = serde_json::json!({
                "engagement": {
                    "engaged": false,
                    "rationale": v.rationale,
                },
                "active_assignment_doc_ids": state.constraint_assignment_doc_ids,
            });
            if let Err(e) = minerva_db::queries::conversation_flags::insert(
                db,
                conversation_id,
                ENGAGEMENT_REFUSED_FLAG,
                Some(turn_index),
                Some(v.rationale.as_str()),
                Some(&metadata),
            )
            .await
            {
                tracing::warn!(
                    "extraction_guard: failed to log refused flag for {}: {}",
                    conversation_id,
                    e
                );
            }
        }
        engagement_verdict = Some(v);
    }

    // After the optional lift, prev_active reflects whether the
    // constraint is *still* on entering this turn's decision.
    let prev_active = state.constraint_active;
    let constraint_active = intent.is_extraction || proximity_active || prev_active;

    // When this turn newly trips the constraint (was off coming
    // in, but intent or proximity flips it), record which
    // assignments are responsible so the dashboard can show them.
    // `prev_active_before_lift` is the *before-lift* state: a
    // student who engaged AND immediately pasted another assignment
    // counts as a fresh trip, with the lift flag recording the
    // brief gap between the two attempts.
    let mut newly_activated = false;
    if constraint_active && !prev_active && !prev_active_before_lift {
        state.constraint_active = true;
        // Pick the assignments that justify the activation:
        // prefer the ones in the proximity window if that's why
        // we tripped; else the ones near *this* turn.
        state.constraint_assignment_doc_ids = if proximity_active {
            proximity_winners(&state)
        } else {
            assignments_near_vec.clone()
        };
        state.constraint_lifted_at_turn = None;
        newly_activated = true;
    } else if constraint_active && !prev_active {
        // Was active before lift, lifted, then re-tripped within
        // the same turn (intent classifier said extraction). Keep
        // the prior assignment scope but turn the flag back on.
        state.constraint_active = true;
        newly_activated = true;
    }
    // If constraint was active and neither intent nor proximity
    // re-fired but engagement didn't lift either, we keep it on.

    // Per-turn decision summary: one INFO line that shows the
    // full reasoning trace at a glance. The intent + engagement
    // verdicts have their own lines above; this one is the
    // post-decision state.
    tracing::info!(
        "extraction_guard: turn={} conversation={} decision: intent.is_extraction={} proximity_active={} prev_active_before_lift={} newly_activated={} constraint_active={} assignment_scope={:?}",
        turn_index,
        conversation_id,
        intent.is_extraction,
        proximity_active,
        prev_active_before_lift,
        newly_activated,
        constraint_active,
        state.constraint_assignment_doc_ids
    );

    // Append-only activation event. Recorded whenever the
    // constraint flips from off to on this turn (covering the
    // first-trip case AND the same-turn lift-then-retrip case).
    // Lets the dashboard render an "extraction guard activated"
    // badge tied to the specific turn that started the streak.
    if newly_activated {
        let cause = if intent.is_extraction && proximity_active {
            "intent_and_proximity"
        } else if intent.is_extraction {
            "intent"
        } else {
            "proximity"
        };
        let rationale = if intent.is_extraction {
            intent.rationale.clone()
        } else {
            format!(
                "proximity threshold tripped; assignment(s) recurred in recent turns: {:?}",
                state.constraint_assignment_doc_ids
            )
        };
        let metadata = serde_json::json!({
            "cause": cause,
            "intent": {
                "is_extraction": intent.is_extraction,
                "rationale": intent.rationale,
            },
            "proximity_active": proximity_active,
            "constraint_assignment_doc_ids": state.constraint_assignment_doc_ids,
            "recent_turns": state.recent_turns,
        });
        if let Err(e) = minerva_db::queries::conversation_flags::insert(
            db,
            conversation_id,
            CONSTRAINT_ACTIVATED_FLAG,
            Some(turn_index),
            Some(rationale.as_str()),
            Some(&metadata),
        )
        .await
        {
            tracing::warn!(
                "extraction_guard: failed to insert {} flag for {}: {}",
                CONSTRAINT_ACTIVATED_FLAG,
                conversation_id,
                e
            );
        }
    }

    save_kg_state(db, conversation_id, &state).await;
    // Suppress unused lint when the verdict is held purely for
    // diagnostic side effects above; the value isn't returned to
    // the caller.
    let _ = engagement_verdict;

    Some(GuardDecision {
        turn_index,
        intent,
        constraint_active,
        assignment_excerpts: rag_signals.iter().map(|c| c.text.clone()).collect(),
        in_scope_assignment_doc_ids: state.constraint_assignment_doc_ids.clone(),
    })
}

/// Phase 2: post-generation interception. Returns the text that
/// should ultimately land in `conversations.messages`; either the
/// original assistant reply (when the guard wasn't enabled, the
/// constraint wasn't active, or the output check passed) or a
/// Socratic rewrite (when the output check tripped).
///
/// Side effects: when the rewrite happens, sends a `rewrite` SSE
/// event so the frontend can swap the displayed message, and
/// inserts a `conversation_flags` row tagged
/// `EXTRACTION_FLAG_NAME` with the verdict rationale.
#[allow(clippy::too_many_arguments)]
pub async fn intercept_reply(
    db: &PgPool,
    http: &reqwest::Client,
    api_key: &str,
    course_id: Uuid,
    conversation_id: Uuid,
    decision: &Option<GuardDecision>,
    student_message: &str,
    assistant_reply: &str,
    tx: &mpsc::Sender<Result<Event, AppError>>,
) -> String {
    let Some(decision) = decision.as_ref() else {
        return assistant_reply.to_string();
    };
    if !decision.constraint_active {
        return assistant_reply.to_string();
    }
    if assistant_reply.is_empty() {
        return assistant_reply.to_string();
    }

    let verdict: OutputVerdict = extraction_guard::check_output_for_solution(
        http,
        api_key,
        db,
        course_id,
        assistant_reply,
        &decision.assignment_excerpts,
    )
    .await;
    tracing::info!(
        "extraction_guard: turn={} conversation={} output.is_complete_solution={} output.rationale={:?}",
        decision.turn_index,
        conversation_id,
        verdict.is_complete_solution,
        verdict.rationale
    );
    if !verdict.is_complete_solution {
        // Constraint was active (e.g. previously flagged) but this
        // turn's output is fine. Return original; no flag emitted.
        return assistant_reply.to_string();
    }

    // Output tripped. Build the Socratic rewrite, log the flag,
    // signal the frontend to swap.
    let rewrite = extraction_guard::generate_socratic_rewrite(
        http,
        api_key,
        db,
        course_id,
        student_message,
        assistant_reply,
    )
    .await;

    let metadata = serde_json::json!({
        "intent": {
            "is_extraction": decision.intent.is_extraction,
            "rationale": decision.intent.rationale,
        },
        "output_check": {
            "is_complete_solution": verdict.is_complete_solution,
            "rationale": verdict.rationale,
        },
        "matched_assignment_doc_ids": decision.in_scope_assignment_doc_ids,
    });
    if let Err(e) = minerva_db::queries::conversation_flags::insert(
        db,
        conversation_id,
        REWROTE_FLAG,
        Some(decision.turn_index),
        Some(verdict.rationale.as_str()),
        Some(&metadata),
    )
    .await
    {
        tracing::warn!(
            "extraction_guard: failed to insert {} flag: {}",
            REWROTE_FLAG,
            e
        );
    }

    // Signal the frontend that the streamed text should be
    // replaced. The frontend chat handler listens for `rewrite`
    // and swaps the displayed assistant message in place.
    let payload = serde_json::json!({
        "type": "rewrite",
        "content": rewrite,
    });
    let _ = tx
        .send(Ok(Event::default().data(payload.to_string())))
        .await;

    rewrite
}

// ── helpers ────────────────────────────────────────────────────────

fn compute_turn_index(history: &[minerva_db::queries::conversations::MessageRow]) -> i32 {
    // Each conversational turn = one user message + one assistant
    // reply. The current turn (the one we're evaluating) hasn't
    // been persisted yet, so count user messages already in
    // history and add 1 for "this one".
    let prior_user_count = history.iter().filter(|m| m.role == "user").count();
    (prior_user_count + 1) as i32
}

fn recent_user_messages(
    history: &[minerva_db::queries::conversations::MessageRow],
    current_user_content: &str,
    last_n: usize,
) -> Vec<String> {
    let mut out: Vec<String> = history
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.clone())
        .collect();
    out.push(current_user_content.to_string());
    if out.len() > last_n {
        let drop = out.len() - last_n;
        out.drain(0..drop);
    }
    out
}

async fn load_kg_state(db: &PgPool, conversation_id: Uuid) -> KgState {
    match minerva_db::queries::conversation_flags::get_kg_state(db, conversation_id).await {
        Ok(value) => serde_json::from_value(value).unwrap_or_default(),
        Err(e) => {
            tracing::warn!(
                "extraction_guard: kg_state load failed for {}: {}",
                conversation_id,
                e
            );
            KgState::default()
        }
    }
}

async fn save_kg_state(db: &PgPool, conversation_id: Uuid, state: &KgState) {
    let value = match serde_json::to_value(state) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("extraction_guard: kg_state serialise failed: {}", e);
            return;
        }
    };
    if let Err(e) =
        minerva_db::queries::conversation_flags::set_kg_state(db, conversation_id, &value).await
    {
        tracing::warn!(
            "extraction_guard: kg_state persist failed for {}: {}",
            conversation_id,
            e
        );
    }
}

fn push_turn(state: &mut KgState, turn_idx: i32, assignments_near: &[Uuid]) {
    state.recent_turns.push(RecentTurn {
        turn_idx,
        assignments_near: assignments_near.to_vec(),
    });
    if state.recent_turns.len() > RECENT_TURNS_WINDOW {
        let drop = state.recent_turns.len() - RECENT_TURNS_WINDOW;
        state.recent_turns.drain(0..drop);
    }
}

/// True when any single assignment doc id appears in at least
/// `PROXIMITY_THRESHOLD` of the last `RECENT_TURNS_WINDOW` turns.
fn proximity_threshold_tripped(state: &KgState) -> bool {
    let mut counts: std::collections::HashMap<Uuid, usize> = std::collections::HashMap::new();
    for t in &state.recent_turns {
        // Distinct per turn; a single turn can't count twice.
        let mut seen: HashSet<Uuid> = HashSet::new();
        for a in &t.assignments_near {
            if seen.insert(*a) {
                *counts.entry(*a).or_insert(0) += 1;
            }
        }
    }
    counts.values().any(|&n| n >= PROXIMITY_THRESHOLD)
}

/// Which assignment(s) tripped the proximity threshold. Used to
/// populate `kg_state.constraint_assignment_doc_ids` when the
/// constraint flips on via proximity.
fn proximity_winners(state: &KgState) -> Vec<Uuid> {
    let mut counts: std::collections::HashMap<Uuid, usize> = std::collections::HashMap::new();
    for t in &state.recent_turns {
        let mut seen: HashSet<Uuid> = HashSet::new();
        for a in &t.assignments_near {
            if seen.insert(*a) {
                *counts.entry(*a).or_insert(0) += 1;
            }
        }
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n >= PROXIMITY_THRESHOLD)
        .map(|(id, _)| id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(idx: i32, ids: &[u8]) -> RecentTurn {
        RecentTurn {
            turn_idx: idx,
            assignments_near: ids.iter().map(|i| Uuid::from_bytes([*i; 16])).collect(),
        }
    }

    #[test]
    fn push_turn_caps_at_window_size() {
        let mut s = KgState::default();
        for i in 1..=8 {
            push_turn(&mut s, i, &[]);
        }
        assert_eq!(s.recent_turns.len(), RECENT_TURNS_WINDOW);
        assert_eq!(s.recent_turns.first().unwrap().turn_idx, 4);
        assert_eq!(s.recent_turns.last().unwrap().turn_idx, 8);
    }

    #[test]
    fn proximity_trips_when_same_assignment_in_two_turns() {
        let s = KgState {
            recent_turns: vec![turn(1, &[1]), turn(2, &[]), turn(3, &[1])],
            ..Default::default()
        };
        assert!(proximity_threshold_tripped(&s));
    }

    #[test]
    fn proximity_does_not_trip_with_distinct_assignments() {
        let s = KgState {
            recent_turns: vec![turn(1, &[1]), turn(2, &[2]), turn(3, &[3])],
            ..Default::default()
        };
        assert!(!proximity_threshold_tripped(&s));
    }

    #[test]
    fn proximity_dedups_within_a_single_turn() {
        // A single turn listing the same assignment twice (paranoia
        //; the producer doesn't actually do this, but the counter
        // shouldn't be fooled).
        let s = KgState {
            recent_turns: vec![turn(1, &[1, 1])],
            ..Default::default()
        };
        assert!(!proximity_threshold_tripped(&s));
    }

    #[test]
    fn proximity_winners_returns_only_threshold_meeters() {
        let s = KgState {
            recent_turns: vec![turn(1, &[1, 2]), turn(2, &[1]), turn(3, &[2])],
            ..Default::default()
        };
        let winners = proximity_winners(&s);
        // 1 and 2 both appear twice -> both win.
        assert_eq!(winners.len(), 2);
    }

    #[test]
    fn recent_user_messages_takes_last_n_with_current() {
        use minerva_db::queries::conversations::MessageRow;
        let mk = |role: &str, content: &str| MessageRow {
            id: Uuid::nil(),
            conversation_id: Uuid::nil(),
            role: role.to_string(),
            content: content.to_string(),
            chunks_used: None,
            model_used: None,
            tokens_prompt: None,
            tokens_completion: None,
            generation_ms: None,
            retrieval_count: None,
            thinking_transcript: None,
            tool_events: None,
            thinking_ms: None,
            research_tokens: None,
            created_at: chrono::Utc::now(),
        };
        let h = vec![mk("user", "u1"), mk("assistant", "a1"), mk("user", "u2")];
        let v = recent_user_messages(&h, "u3", 5);
        assert_eq!(
            v,
            vec!["u1".to_string(), "u2".to_string(), "u3".to_string()]
        );
    }

    #[test]
    fn turn_index_counts_user_messages_plus_one() {
        use minerva_db::queries::conversations::MessageRow;
        let mk = |role: &str| MessageRow {
            id: Uuid::nil(),
            conversation_id: Uuid::nil(),
            role: role.to_string(),
            content: String::new(),
            chunks_used: None,
            model_used: None,
            tokens_prompt: None,
            tokens_completion: None,
            generation_ms: None,
            retrieval_count: None,
            thinking_transcript: None,
            tool_events: None,
            thinking_ms: None,
            research_tokens: None,
            created_at: chrono::Utc::now(),
        };
        let h = vec![mk("user"), mk("assistant"), mk("user"), mk("assistant")];
        // Two prior user messages -> the next turn is turn 3.
        assert_eq!(compute_turn_index(&h), 3);
    }
}
