//! Strategy-side extraction guard. Wraps the lower-level
//! `classification::extraction_guard` (thin utility-model call
//! wrappers) into the chat flow and applies the course's answer
//! policy, which turns on what kind of material the turn is about:
//!
//! * **Examining work** (a graded assignment, lab, or take-home exam):
//!   never a full solution. The constraint goes on when the student
//!   pastes the task and retrieval matches it, or when the same graded
//!   doc keeps turning up across turns; while it is on, every reply is
//!   checked and a complete solution is swapped for a Socratic rewrite.
//!   An attempt does not lift it. It comes off once the conversation
//!   has left that work behind.
//! * **Practice material** (exercises, old exams): a published answer
//!   is given; otherwise the answer follows an honest attempt. No
//!   constraint and no rewrite: the turn gets a prompt addendum
//!   (`prompts::PRACTICE_ATTEMPT_ADDENDUM`) and the model, which can
//!   see both the retrieved materials and the student's attempts,
//!   applies it.
//!
//! Two entry points the strategies call:
//!
//! 1. `evaluate_for_turn`; runs after RAG retrieval, before
//!    generation. Runs the intent classifier, computes "graded work
//!    near this turn" from RAG signals + KG `applied_in` partners,
//!    slides the recent-turns window in `kg_state`, and decides which
//!    of the two policies (if either) applies. Persists the updated
//!    `kg_state`.
//!
//! 2. `intercept_reply`; runs after generation, with the full
//!    assistant text. Idempotent no-op when the guard wasn't
//!    enabled or the constraint isn't active. When active: runs
//!    the output-side solution check; if it trips, generates a
//!    Socratic rewrite, sends a `rewrite` SSE event so the
//!    frontend can swap the displayed message, logs a
//!    `conversation_flag` row for the teacher dashboard, and
//!    returns the rewrite for downstream `finalize` to persist.

use std::collections::HashSet;

use axum::response::sse::Event;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::classification::extraction_guard::{
    self, IntentVerdict, OutputVerdict, INTENT_HISTORY_TURNS,
};
use crate::error::AppError;
use crate::feature_flags::extraction_guard_enabled;
use crate::strategy::common::RagChunk;

// ── flag kind constants ────────────────────────────────────────────
//
// We log an append-only event-stream of guard decisions to
// `conversation_flags`. Each row records ONE classifier verdict or
// state transition; the dashboard reconstructs the lifecycle by
// reading them oldest-first. Four kinds, all turn-indexed so the
// per-turn UI on the conversation detail page can align them.

/// Intent classifier returned `is_extraction = true` for this turn:
/// a task pasted with a request for its answer and no attempt. Logged
/// for graded and practice material alike (the metadata says which),
/// and independent of whether the constraint was already active.
pub const INTENT_DETECTED_FLAG: &str = "extraction_intent_detected";

/// Constraint flipped from off to on this turn: graded work is in
/// scope. Cause may be intent OR proximity OR both; the metadata
/// records which.
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

/// The constraint came off because the conversation moved away from
/// the graded work that set it. Pairs with the `_activated` flag from
/// earlier in the conversation to bracket the lifecycle.
pub const CONSTRAINT_LIFTED_FLAG: &str = "extraction_constraint_lifted";

/// How many recent turns to keep in `kg_state.recent_turns` for the
/// multi-turn proximity check, and how many turns away from the graded
/// work it takes for an active constraint to come off.
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
    /// the output-side check unless the conversation has moved on.
    #[serde(default)]
    pub constraint_active: bool,
    /// Which assignment doc ids the constraint is tracking. Used
    /// when the output check needs to know which assignments to
    /// reference, and when the dashboard shows what triggered.
    #[serde(default)]
    pub constraint_assignment_doc_ids: Vec<Uuid>,
    /// 1-based turn index at which the constraint was last lifted.
    /// Lets the dashboard show the lifecycle
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
    /// Whether the examining constraint applies to *this* turn's
    /// generation. True iff this turn is flagged (see
    /// `flagged_this_turn`) OR an earlier turn was and the graded work
    /// it matched is still inside the recent-turns window.
    ///
    /// Controls the post-generation `intercept_reply` output check.
    /// NOT the right signal for "should the thinking stream + sources
    /// panel be hidden on this turn": once tripped this stays sticky
    /// across innocent follow-ups, which would force every
    /// subsequent benign turn into the placeholder UX. Use
    /// `flagged_this_turn` for the live-suppression decision instead.
    pub constraint_active: bool,
    /// Whether THIS specific turn put graded work in scope: a pasted
    /// task that retrieval matched to an examining doc, OR the
    /// multi-turn proximity threshold tripping on this turn's RAG.
    /// Excludes the sticky `prev_active` carry-over.
    ///
    /// The right gate for hiding the research transcript / sources
    /// panel from the student on a guarded turn: a single past
    /// paste shouldn't keep blanking sources across innocent
    /// follow-up questions. The sticky `constraint_active` still
    /// governs the writeup-time output check via `intercept_reply`,
    /// which is the right place for stickiness (the student might
    /// still be drifting toward an extraction even on an innocent-
    /// looking turn).
    pub flagged_this_turn: bool,
    /// The student pasted a task and asked for its answer without an
    /// attempt, and nothing graded is in scope: practice material.
    /// Drives the attempt-first prompt addendum; never a rewrite.
    pub practice_attempt_first: bool,
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
    util: &crate::llm::UtilityModel,
    course_id: Uuid,
    conversation_id: Uuid,
    history: &[minerva_db::queries::conversations::MessageRow],
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

    // Intent classifier sees the last N user messages from history,
    // oldest first. `history` already contains the current turn's
    // user message (run_chat_message persists the user row before
    // loading history; see compute_turn_index for the same lifecycle
    // assumption), so there is nothing to append; doing so would
    // duplicate the latest prompt in the classifier window.
    let recent_user_messages = recent_user_messages(history, INTENT_HISTORY_TURNS);
    let intent =
        extraction_guard::classify_intent(http, util, db, course_id, &recent_user_messages).await;
    tracing::info!(
        "extraction_guard: turn={} conversation={} intent.is_extraction={} intent.rationale={:?}",
        turn_index,
        conversation_id,
        intent.is_extraction,
        intent.rationale
    );

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

    // A direct match: retrieval put an examining doc's own text next
    // to the student's message with a score high enough to mean the
    // message is that task, not merely a question on the same topic.
    let matched_examining: Vec<Uuid> = rag_signals
        .iter()
        .filter(|s| s.score >= super::common::ASSIGNMENT_SIGNAL_MIN_SCORE)
        .filter_map(|s| Uuid::parse_str(&s.document_id).ok())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let mut state = load_kg_state(db, conversation_id).await;
    push_turn(&mut state, turn_index, &assignments_near_vec);
    let proximity_active = proximity_threshold_tripped(&state);
    let prev_active = state.constraint_active;

    // Graded work is in scope this turn. A pasted task with no
    // examining match is practice material and takes the other path.
    let flagged_this_turn =
        (intent.is_extraction && !matched_examining.is_empty()) || proximity_active;
    let practice_attempt_first = intent.is_extraction && !flagged_this_turn;

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
            "examining": flagged_this_turn,
        });
        log_flag(
            db,
            conversation_id,
            INTENT_DETECTED_FLAG,
            turn_index,
            &intent.rationale,
            &metadata,
        )
        .await;
    }

    // An attempt does not lift the constraint: graded work is never
    // solved in full. What ends it is distance, i.e. none of the docs
    // that set it appearing anywhere in the recent-turns window.
    let constraint_active = flagged_this_turn || (prev_active && scope_in_window(&state));

    if constraint_active && !prev_active {
        // Prefer the assignments in the proximity window if that's
        // why we tripped; else the ones this turn matched.
        state.constraint_active = true;
        state.constraint_assignment_doc_ids = if proximity_active {
            proximity_winners(&state)
        } else {
            matched_examining
        };
        state.constraint_lifted_at_turn = None;

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
        log_flag(
            db,
            conversation_id,
            CONSTRAINT_ACTIVATED_FLAG,
            turn_index,
            &rationale,
            &metadata,
        )
        .await;
    } else if prev_active && !constraint_active {
        state.constraint_active = false;
        state.constraint_lifted_at_turn = Some(turn_index);
        let metadata = serde_json::json!({
            "lifted_assignment_doc_ids": state.constraint_assignment_doc_ids,
        });
        log_flag(
            db,
            conversation_id,
            CONSTRAINT_LIFTED_FLAG,
            turn_index,
            "conversation moved away from the graded work",
            &metadata,
        )
        .await;
    }

    tracing::info!(
        "extraction_guard: turn={} conversation={} decision: intent.is_extraction={} proximity_active={} flagged_this_turn={} practice_attempt_first={} constraint_active={} assignment_scope={:?}",
        turn_index,
        conversation_id,
        intent.is_extraction,
        proximity_active,
        flagged_this_turn,
        practice_attempt_first,
        constraint_active,
        state.constraint_assignment_doc_ids
    );

    save_kg_state(db, conversation_id, &state).await;

    Some(GuardDecision {
        turn_index,
        intent,
        constraint_active,
        flagged_this_turn,
        practice_attempt_first,
        assignment_excerpts: rag_signals.iter().map(|c| c.text.clone()).collect(),
        in_scope_assignment_doc_ids: state.constraint_assignment_doc_ids.clone(),
    })
}

/// Whether the turn should carry the attempt-first practice addendum.
/// False when the guard is off for the course.
pub fn practice_attempt_first(decision: &Option<GuardDecision>) -> bool {
    decision.as_ref().is_some_and(|d| d.practice_attempt_first)
}

/// Append one guard event to `conversation_flags`. Best-effort: the
/// log is for the teacher dashboard and must never fail a chat turn.
async fn log_flag(
    db: &PgPool,
    conversation_id: Uuid,
    flag: &str,
    turn_index: i32,
    rationale: &str,
    metadata: &serde_json::Value,
) {
    if let Err(e) = minerva_db::queries::conversation_flags::insert(
        db,
        conversation_id,
        flag,
        Some(turn_index),
        Some(rationale),
        Some(metadata),
    )
    .await
    {
        tracing::warn!(
            "extraction_guard: failed to insert {} flag for {}: {}",
            flag,
            conversation_id,
            e
        );
    }
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
    util: &crate::llm::UtilityModel,
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
        util,
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
        util,
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
    log_flag(
        db,
        conversation_id,
        REWROTE_FLAG,
        decision.turn_index,
        &verdict.rationale,
        &metadata,
    )
    .await;

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
    // reply. `history` is loaded AFTER the current user message has
    // already been persisted (run_chat_message in routes/chat.rs
    // inserts the user row before passing history into the strategy),
    // so the turn we're evaluating is the Nth user message inside
    // history; just count them.
    //
    // Must match the convention `assistant_ids_on_turns` (chat.rs)
    // and the frontend teacher dashboard walker
    // (conversations-page.tsx) use when joining flags to messages:
    // walk in order, increment on each user, the assistant that
    // follows user N has turn N. An off-by-one here breaks the
    // read-time owner-suppression gate's REWROTE_FLAG fallback AND
    // the teacher dashboard's per-turn badges.
    history.iter().filter(|m| m.role == "user").count() as i32
}

fn recent_user_messages(
    history: &[minerva_db::queries::conversations::MessageRow],
    last_n: usize,
) -> Vec<String> {
    // `history` already contains the current turn's user message
    // (see compute_turn_index), so there's no separate
    // `current_user_content` to append. Doing so would duplicate
    // the latest prompt adjacent to itself in the classifier window
    // and shrink the effective lookback by one turn.
    let mut out: Vec<String> = history
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.clone())
        .collect();
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

/// True while any doc the constraint was set for still appears in the
/// recent-turns window. Once none does, the conversation has left that
/// graded work behind and the constraint comes off.
fn scope_in_window(state: &KgState) -> bool {
    state.recent_turns.iter().any(|t| {
        t.assignments_near
            .iter()
            .any(|a| state.constraint_assignment_doc_ids.contains(a))
    })
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
    fn constraint_scope_holds_while_its_assignment_is_in_the_window() {
        let scope = vec![Uuid::from_bytes([1; 16])];
        let mut s = KgState {
            constraint_active: true,
            constraint_assignment_doc_ids: scope,
            ..Default::default()
        };
        push_turn(&mut s, 1, &[Uuid::from_bytes([1; 16])]);
        // Four turns on something else: the graded work is still in
        // the five-turn window, so the constraint holds.
        for i in 2..=5 {
            push_turn(&mut s, i, &[Uuid::from_bytes([2; 16])]);
            assert!(scope_in_window(&s), "turn {i}");
        }
        // A fifth pushes it out.
        push_turn(&mut s, 6, &[]);
        assert!(!scope_in_window(&s));
    }

    #[test]
    fn practice_attempt_first_is_false_without_a_decision() {
        assert!(!practice_attempt_first(&None));
    }

    #[test]
    fn recent_user_messages_takes_last_n_from_history() {
        // `history` already contains the current turn's user message
        // when `evaluate_for_turn` is called (the chat route persists
        // the user row before loading history), so the function just
        // takes the last N user messages from it.
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
            research_prompt_tokens: None,
            research_completion_tokens: None,
            thinking_hidden: false,
            created_at: chrono::Utc::now(),
        };
        let h = vec![
            mk("user", "u1"),
            mk("assistant", "a1"),
            mk("user", "u2"),
            mk("assistant", "a2"),
            mk("user", "u3"),
        ];
        let v = recent_user_messages(&h, 5);
        assert_eq!(
            v,
            vec!["u1".to_string(), "u2".to_string(), "u3".to_string()]
        );
    }

    #[test]
    fn recent_user_messages_caps_at_last_n() {
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
            research_prompt_tokens: None,
            research_completion_tokens: None,
            thinking_hidden: false,
            created_at: chrono::Utc::now(),
        };
        let h = vec![
            mk("user", "u1"),
            mk("user", "u2"),
            mk("user", "u3"),
            mk("user", "u4"),
        ];
        let v = recent_user_messages(&h, 2);
        // Oldest two dropped; preserves chronological order.
        assert_eq!(v, vec!["u3".to_string(), "u4".to_string()]);
    }

    #[test]
    fn turn_index_counts_user_messages_in_history() {
        // `history` already contains the just-persisted current user
        // message, so the count IS the turn index (1-based).
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
            research_prompt_tokens: None,
            research_completion_tokens: None,
            thinking_hidden: false,
            created_at: chrono::Utc::now(),
        };
        // History at the time of evaluating turn 3: u1,a1,u2,a2,u3.
        // Three user messages -> this is turn 3.
        let h = vec![
            mk("user"),
            mk("assistant"),
            mk("user"),
            mk("assistant"),
            mk("user"),
        ];
        assert_eq!(compute_turn_index(&h), 3);

        // First turn: just the freshly-inserted u1.
        let first = vec![mk("user")];
        assert_eq!(compute_turn_index(&first), 1);
    }
}
