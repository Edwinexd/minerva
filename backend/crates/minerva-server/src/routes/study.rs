//! Study mode: research-evaluation pipeline endpoints.
//!
//! Two routers:
//!
//! * `router()` is participant-facing, mounted at
//!   `/api/courses/{course_id}/study`. Drives the linear pipeline:
//!   state, consent, pre-survey, N tasks, post-survey, done. All
//!   endpoints require course membership AND that study mode is
//!   enabled for the course; otherwise 404 (we don't want to leak
//!   that the routes exist for non-study courses).
//! * `admin_router()` is admin-only, mounted at `/api/admin`. Lets
//!   the researcher edit the per-course study config (consent copy,
//!   task list, surveys), view participant progress, and download
//!   the JSONL export.
//!
//! Lockout: `ensure_not_locked_out` is the helper called by other
//! routes (chat send-message in particular) to refuse a participant
//! who has already finished the post-survey. The participant-facing
//! `GET /state` route is the one mutation-free endpoint that survives
//! the lockout, so the frontend can render the thank-you screen.

use axum::body::{Body, Bytes};
use axum::extract::{Extension, Path, State};
use axum::http::{header, HeaderValue};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{stream, StreamExt};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::AppError;
use crate::feature_flags;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// Participant-facing routes, mounted at `/api/courses/{course_id}/study`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/state", get(get_state))
        .route("/consent", post(post_consent))
        .route("/survey/{kind}", get(get_survey).post(post_survey))
        .route("/task/{task_index}/start", post(start_task))
        .route("/task/{task_index}/done", post(finish_task))
}

/// Admin-only config + progress + export routes. Mounted at
/// `/api/admin`. Routes here all 403 for non-admins.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route(
            "/study/courses/{course_id}/config",
            get(admin_get_config).put(admin_put_config),
        )
        .route(
            "/study/courses/{course_id}/participants",
            get(admin_list_participants),
        )
        .route(
            "/study/courses/{course_id}/export.jsonl",
            get(admin_export_jsonl),
        )
        .route(
            "/study/courses/{course_id}/seed-dm2731",
            post(admin_seed_dm2731),
        )
        .route(
            "/study/courses/{course_id}/participants/{participant_number}/detail",
            get(admin_get_participant_detail),
        )
        .route(
            "/study/courses/{course_id}/participants/by-user/{user_id}",
            axum::routing::delete(admin_delete_participant_data),
        )
}

// ---------------------------------------------------------------------------
// Lockout helper (used by chat::send_message and elsewhere)
// ---------------------------------------------------------------------------

/// Refuses with `StudyLockedOut` (423) iff (course, user) is in a
/// study course AND the participant has finished the post-survey.
/// No-op for non-study courses, non-members, and admins not enrolled
/// as participants. Cheap enough to call on every chat send: one
/// flag lookup + one participant_state lookup, both indexed.
pub(crate) async fn ensure_not_locked_out(
    state: &AppState,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<(), AppError> {
    if !feature_flags::study_mode_enabled(&state.db, course_id).await {
        return Ok(());
    }
    let Some(participant) =
        minerva_db::queries::study::get_state(&state.db, course_id, user_id).await?
    else {
        return Ok(());
    };
    if participant.stage == "done" {
        return Err(AppError::StudyLockedOut);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Common gating
// ---------------------------------------------------------------------------

/// Resolve the per-course study config OR refuse with 404 if the
/// course isn't actually in study mode. We treat "flag off" the same
/// as "no config row" because either alone is enough to make the
/// pipeline inert; the participant routes shouldn't behave
/// differently between them.
async fn require_study_course(
    state: &AppState,
    course_id: Uuid,
) -> Result<minerva_db::queries::study::StudyCourseRow, AppError> {
    if !feature_flags::study_mode_enabled(&state.db, course_id).await {
        return Err(AppError::NotFound);
    }
    minerva_db::queries::study::get_study_course(&state.db, course_id)
        .await?
        .ok_or_else(|| {
            // Flag on but no config row: misconfiguration. Surface as
            // 500 so the admin notices, rather than 404 which would
            // hide the inconsistency.
            AppError::Internal(format!(
                "study mode enabled for course {} but no study_courses row exists",
                course_id
            ))
        })
}

/// Reject if the caller is not a member of the course. Admins who
/// aren't enrolled as participants get the same treatment because
/// participant-facing routes mutate participant state; admins use
/// the admin_router for everything they need.
async fn require_course_member(
    state: &AppState,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<(), AppError> {
    let role = minerva_db::queries::courses::get_member_role(&state.db, course_id, user_id).await?;
    if role.is_none() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Owner / strict-teacher / platform-admin gate for the per-course
/// study management endpoints (config GET/PUT, participants list,
/// JSONL export). TAs are deliberately excluded: surveys + per-
/// participant transcripts are sensitive enough that "teacher" is
/// the right floor. The seed-preset endpoint stays admin-only
/// (one-shot destructive overwrite, easier to audit centrally).
async fn require_course_owner_teacher_or_admin(
    state: &AppState,
    course_id: Uuid,
    user: &User,
) -> Result<(), AppError> {
    if user.role.is_admin() {
        return Ok(());
    }
    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if course.owner_id == user.id {
        return Ok(());
    }
    if minerva_db::queries::courses::is_course_teacher_strict(&state.db, course_id, user.id).await?
    {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

// ---------------------------------------------------------------------------
// GET /state
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StateResponse {
    stage: String,
    current_task_index: i32,
    number_of_tasks: i32,
    completion_gate_kind: String,
    consent_html: String,
    thank_you_html: String,
    consented_at: Option<chrono::DateTime<chrono::Utc>>,
    pre_survey_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    post_survey_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    locked_out_at: Option<chrono::DateTime<chrono::Utc>>,
    /// If `stage == "task"`, the task body to render. Null at every
    /// other stage so the frontend doesn't have to special-case the
    /// transition windows.
    current_task: Option<TaskView>,
    /// If `stage == "task"`, the per-task conversation_id (created on
    /// first /task/{i}/start hit and persisted thereafter), else null.
    current_task_conversation_id: Option<Uuid>,
}

#[derive(Serialize)]
struct TaskView {
    task_index: i32,
    title: String,
    description: String,
}

async fn get_state(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<StateResponse>, AppError> {
    let study = require_study_course(&state, course_id).await?;
    require_course_member(&state, course_id, user.id).await?;

    let participant =
        minerva_db::queries::study::get_or_init_state(&state.db, course_id, user.id).await?;

    let (current_task, current_task_conversation_id) = if participant.stage == "task" {
        let task = minerva_db::queries::study::get_task(
            &state.db,
            course_id,
            participant.current_task_index,
        )
        .await?;
        let conv = minerva_db::queries::study::list_task_conversations_for_user(
            &state.db, course_id, user.id,
        )
        .await?
        .into_iter()
        .find(|r| r.task_index == participant.current_task_index)
        .map(|r| r.conversation_id);
        (
            task.map(|t| TaskView {
                task_index: t.task_index,
                title: t.title,
                description: t.description,
            }),
            conv,
        )
    } else {
        (None, None)
    };

    Ok(Json(StateResponse {
        stage: participant.stage,
        current_task_index: participant.current_task_index,
        number_of_tasks: study.number_of_tasks,
        completion_gate_kind: study.completion_gate_kind,
        consent_html: study.consent_html,
        thank_you_html: study.thank_you_html,
        consented_at: participant.consented_at,
        pre_survey_completed_at: participant.pre_survey_completed_at,
        post_survey_completed_at: participant.post_survey_completed_at,
        locked_out_at: participant.locked_out_at,
        current_task,
        current_task_conversation_id,
    }))
}

// ---------------------------------------------------------------------------
// POST /consent
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ConsentRequest {
    /// Must be `true`; sent as an explicit body field so a stray POST
    /// with an empty body can never bypass the consent screen.
    consent_given: bool,
}

async fn post_consent(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<ConsentRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_study_course(&state, course_id).await?;
    require_course_member(&state, course_id, user.id).await?;
    ensure_not_locked_out(&state, course_id, user.id).await?;

    if !body.consent_given {
        return Err(AppError::bad_request("study.consent_required"));
    }
    minerva_db::queries::study::get_or_init_state(&state.db, course_id, user.id).await?;
    let row = minerva_db::queries::study::record_consent(&state.db, course_id, user.id).await?;
    Ok(Json(serde_json::json!({ "stage": row.stage })))
}

// ---------------------------------------------------------------------------
// GET /survey/{kind}, POST /survey/{kind}
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SurveyResponse {
    kind: String,
    questions: Vec<SurveyQuestionView>,
    /// Existing answers, keyed by question_id, so the participant can
    /// resume a half-filled survey after a tab close. Empty on first
    /// load. The frontend matches by id rather than position so a
    /// future re-order doesn't smear answers.
    existing: Vec<SurveyAnswerView>,
}

#[derive(Serialize)]
struct SurveyQuestionView {
    id: Uuid,
    ord: i32,
    /// One of: `likert`, `free_text`, `section_heading`. The frontend
    /// renders `section_heading` as a divider/heading and never
    /// requests an answer for it.
    kind: String,
    prompt: String,
    likert_min: Option<i32>,
    likert_max: Option<i32>,
    likert_min_label: Option<String>,
    likert_max_label: Option<String>,
    /// FALSE -> participant may submit without answering. Always
    /// FALSE for `section_heading`.
    is_required: bool,
    /// Likert-only: when this value is answered, the route
    /// short-circuits to `done` (lockout) instead of advancing to
    /// the next stage. NULL means no kill switch. The frontend
    /// renders the question normally; the lockout happens
    /// server-side after submission.
    kill_on_value: Option<i32>,
}

#[derive(Serialize)]
struct SurveyAnswerView {
    question_id: Uuid,
    likert_value: Option<i32>,
    free_text_value: Option<String>,
}

fn validate_survey_kind(kind: &str) -> Result<(), AppError> {
    match kind {
        "pre" | "post" => Ok(()),
        _ => Err(AppError::bad_request("study.survey_kind_invalid")),
    }
}

async fn get_survey(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, kind)): Path<(Uuid, String)>,
) -> Result<Json<SurveyResponse>, AppError> {
    require_study_course(&state, course_id).await?;
    require_course_member(&state, course_id, user.id).await?;
    validate_survey_kind(&kind)?;

    let bundle = minerva_db::queries::study::get_survey_with_questions(&state.db, course_id, &kind)
        .await?
        .ok_or_else(|| AppError::bad_request("study.survey_not_configured"))?;

    let existing =
        minerva_db::queries::study::list_user_responses(&state.db, bundle.survey.id, user.id)
            .await?;

    Ok(Json(SurveyResponse {
        kind,
        questions: bundle
            .questions
            .into_iter()
            .map(|q| SurveyQuestionView {
                id: q.id,
                ord: q.ord,
                kind: q.kind,
                prompt: q.prompt,
                likert_min: q.likert_min,
                likert_max: q.likert_max,
                likert_min_label: q.likert_min_label,
                likert_max_label: q.likert_max_label,
                is_required: q.is_required,
                kill_on_value: q.kill_on_value,
            })
            .collect(),
        existing: existing
            .into_iter()
            .map(|r| SurveyAnswerView {
                question_id: r.question_id,
                likert_value: r.likert_value,
                free_text_value: r.free_text_value,
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
struct SubmitSurveyRequest {
    answers: Vec<SubmitSurveyAnswer>,
}

#[derive(Deserialize)]
struct SubmitSurveyAnswer {
    question_id: Uuid,
    likert_value: Option<i32>,
    free_text_value: Option<String>,
}

async fn post_survey(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, kind)): Path<(Uuid, String)>,
    Json(body): Json<SubmitSurveyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_study_course(&state, course_id).await?;
    require_course_member(&state, course_id, user.id).await?;
    ensure_not_locked_out(&state, course_id, user.id).await?;
    validate_survey_kind(&kind)?;

    // Stage check: pre-survey only valid in `pre_survey` stage; post-
    // survey only valid in `post_survey` stage. A late re-submit (e.g.
    // user clicks back) is rejected so we don't accidentally rewind
    // the pipeline state by re-running the advance.
    let participant = minerva_db::queries::study::get_state(&state.db, course_id, user.id)
        .await?
        .ok_or_else(|| AppError::bad_request("study.no_participant_state"))?;
    let expected_stage = match kind.as_str() {
        "pre" => "pre_survey",
        "post" => "post_survey",
        _ => unreachable!("validate_survey_kind already checked"),
    };
    if participant.stage != expected_stage {
        return Err(AppError::bad_request("study.invalid_stage"));
    }

    let bundle = minerva_db::queries::study::get_survey_with_questions(&state.db, course_id, &kind)
        .await?
        .ok_or_else(|| AppError::bad_request("study.survey_not_configured"))?;

    // Validate every answer matches a known question of the right
    // kind, with the right value column populated. Cheap to do up
    // front; the DB CHECK constraints would catch mistakes too but
    // would surface as a generic 500.
    let mut question_lookup: std::collections::HashMap<
        Uuid,
        &minerva_db::queries::study::StudySurveyQuestionRow,
    > = std::collections::HashMap::with_capacity(bundle.questions.len());
    for q in &bundle.questions {
        question_lookup.insert(q.id, q);
    }

    let mut inputs = Vec::with_capacity(body.answers.len());
    for a in body.answers {
        let q = question_lookup
            .get(&a.question_id)
            .ok_or_else(|| AppError::bad_request("study.unknown_question"))?;
        match q.kind.as_str() {
            "likert" => {
                // Optional likert can be omitted; treat
                // `likert_value: None` as "no answer" rather than an
                // error so the frontend doesn't have to filter
                // unanswered optionals out of the body.
                let Some(v) = a.likert_value else {
                    if q.is_required {
                        return Err(AppError::bad_request("study.likert_value_required"));
                    }
                    continue;
                };
                let (min, max) = match (q.likert_min, q.likert_max) {
                    (Some(min), Some(max)) => (min, max),
                    _ => {
                        return Err(AppError::Internal(
                            "likert question missing min/max bounds".into(),
                        ))
                    }
                };
                if v < min || v > max {
                    return Err(AppError::bad_request("study.likert_out_of_range"));
                }
                inputs.push(minerva_db::queries::study::SurveyResponseInput {
                    question_id: a.question_id,
                    likert_value: Some(v),
                    free_text_value: None,
                });
            }
            "free_text" => {
                let trimmed = a
                    .free_text_value
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let Some(txt) = trimmed else {
                    if q.is_required {
                        return Err(AppError::bad_request("study.free_text_required"));
                    }
                    continue;
                };
                inputs.push(minerva_db::queries::study::SurveyResponseInput {
                    question_id: a.question_id,
                    likert_value: None,
                    free_text_value: Some(txt),
                });
            }
            "section_heading" => {
                // Display-only; client should never POST an answer
                // for a heading. Silently ignore if it does.
                continue;
            }
            _ => return Err(AppError::Internal("unknown survey question kind".into())),
        }
    }

    // Required-question completeness check. We can't infer this from
    // the body alone (clients should send `null`s for omitted
    // optionals, but a client bug could just elide them); walk every
    // required question and confirm we either staged an input or the
    // matching input was an explicit-null optional.
    let staged_question_ids: std::collections::HashSet<Uuid> =
        inputs.iter().map(|i| i.question_id).collect();
    for q in &bundle.questions {
        if !q.is_required {
            continue;
        }
        if q.kind == "section_heading" {
            continue;
        }
        if !staged_question_ids.contains(&q.id) {
            return Err(AppError::bad_request(match q.kind.as_str() {
                "likert" => "study.likert_value_required",
                "free_text" => "study.free_text_required",
                _ => "study.unknown_question",
            }));
        }
    }

    minerva_db::queries::study::submit_survey_responses(
        &state.db,
        bundle.survey.id,
        user.id,
        &inputs,
    )
    .await?;

    // Kill-switch evaluation. If any answered likert question
    // matches its `kill_on_value`, the participant is short-
    // circuited straight to `done` regardless of which survey
    // they were on. This is the GDPR-withdraw path: the participant
    // selected "no, do not save my data", so we stop the pipeline
    // and the lockout middleware blocks any further interaction
    // with the chat path.
    let killed = inputs.iter().any(|input| {
        let Some(v) = input.likert_value else {
            return false;
        };
        let Some(q) = question_lookup.get(&input.question_id) else {
            return false;
        };
        q.kill_on_value == Some(v)
    });

    let advanced = if killed {
        // We have to walk through the existing stage-machine path
        // because `advance_to_done` only accepts `post_survey ->
        // done` transitions. For the pre-survey kill case we need
        // to nudge the participant through the intermediate stages
        // so the timestamps make sense in the export. Pragmatic
        // workaround: bump them stage-by-stage. Two extra UPDATEs
        // is fine for a kill-switch path.
        if matches!(kind.as_str(), "pre") {
            // pre_survey -> task -> post_survey -> done. Advance
            // through the stages so the FSM stays consistent;
            // `current_task_index` ends up at 0 which is harmless
            // because `stage = 'done'` is what the lockout
            // middleware reads.
            let _ =
                minerva_db::queries::study::advance_to_first_task(&state.db, course_id, user.id)
                    .await?;
            let _ = minerva_db::queries::study::advance_after_task(
                &state.db,
                course_id,
                user.id,
                0,
                study_for_kill_switch_total_tasks(&state, course_id).await?,
            )
            .await?;
        }
        minerva_db::queries::study::advance_to_done(&state.db, course_id, user.id).await?
    } else {
        match kind.as_str() {
            "pre" => {
                minerva_db::queries::study::advance_to_first_task(&state.db, course_id, user.id)
                    .await?
            }
            "post" => {
                minerva_db::queries::study::advance_to_done(&state.db, course_id, user.id).await?
            }
            _ => unreachable!(),
        }
    };

    Ok(Json(
        serde_json::json!({ "stage": advanced.stage, "current_task_index": advanced.current_task_index }),
    ))
}

/// Helper used only by the kill-switch path: returns the configured
/// `number_of_tasks` for a course. We need it to walk the stage
/// FSM through `advance_after_task` (which expects the task total
/// so it can decide whether to land at `task` or `post_survey`).
async fn study_for_kill_switch_total_tasks(
    state: &AppState,
    course_id: Uuid,
) -> Result<i32, AppError> {
    let study = minerva_db::queries::study::get_study_course(&state.db, course_id)
        .await?
        .ok_or_else(|| {
            AppError::Internal(format!("study config missing for course {}", course_id))
        })?;
    Ok(study.number_of_tasks)
}

// ---------------------------------------------------------------------------
// POST /task/{i}/start
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StartTaskResponse {
    task_index: i32,
    conversation_id: Uuid,
}

async fn start_task(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, task_index)): Path<(Uuid, i32)>,
) -> Result<Json<StartTaskResponse>, AppError> {
    let study = require_study_course(&state, course_id).await?;
    require_course_member(&state, course_id, user.id).await?;
    ensure_not_locked_out(&state, course_id, user.id).await?;

    if task_index < 0 || task_index >= study.number_of_tasks {
        return Err(AppError::bad_request("study.task_index_invalid"));
    }

    let participant = minerva_db::queries::study::get_state(&state.db, course_id, user.id)
        .await?
        .ok_or_else(|| AppError::bad_request("study.no_participant_state"))?;

    // Only the current task slot is startable. We don't let participants
    // jump ahead or revisit completed tasks; the resume use case (tab
    // close mid-task) is handled by re-fetching the SAME task_index,
    // which works because get_or_create_task_conversation is idempotent.
    if participant.stage != "task" || participant.current_task_index != task_index {
        return Err(AppError::bad_request("study.invalid_stage"));
    }

    let row = minerva_db::queries::study::get_or_create_task_conversation(
        &state.db, course_id, user.id, task_index,
    )
    .await?;

    Ok(Json(StartTaskResponse {
        task_index: row.task_index,
        conversation_id: row.conversation_id,
    }))
}

// ---------------------------------------------------------------------------
// POST /task/{i}/done
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct FinishTaskResponse {
    stage: String,
    current_task_index: i32,
    /// Convenience: if the participant just finished the last task,
    /// this is true; the frontend uses it to switch immediately to
    /// the post-survey screen rather than waiting for the next /state
    /// poll.
    is_last_task: bool,
}

async fn finish_task(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, task_index)): Path<(Uuid, i32)>,
) -> Result<Json<FinishTaskResponse>, AppError> {
    let study = require_study_course(&state, course_id).await?;
    require_course_member(&state, course_id, user.id).await?;
    ensure_not_locked_out(&state, course_id, user.id).await?;

    if task_index < 0 || task_index >= study.number_of_tasks {
        return Err(AppError::bad_request("study.task_index_invalid"));
    }

    let participant = minerva_db::queries::study::get_state(&state.db, course_id, user.id)
        .await?
        .ok_or_else(|| AppError::bad_request("study.no_participant_state"))?;
    if participant.stage != "task" || participant.current_task_index != task_index {
        return Err(AppError::bad_request("study.invalid_stage"));
    }

    // Gate evaluation. Currently only `messages_only` is supported;
    // the column is kept on `study_courses` so other gates can be
    // added later without a migration.
    let task_conv = minerva_db::queries::study::get_or_create_task_conversation(
        &state.db, course_id, user.id, task_index,
    )
    .await?;
    match study.completion_gate_kind.as_str() {
        "messages_only" => {
            let n = minerva_db::queries::study::count_user_messages_in_conversation(
                &state.db,
                task_conv.conversation_id,
            )
            .await?;
            if n < 1 {
                return Err(AppError::bad_request("study.gate_not_met"));
            }
        }
        other => {
            return Err(AppError::Internal(format!(
                "unsupported completion_gate_kind {other:?}",
            )));
        }
    }

    minerva_db::queries::study::mark_task_done(&state.db, course_id, user.id, task_index).await?;
    let advanced = minerva_db::queries::study::advance_after_task(
        &state.db,
        course_id,
        user.id,
        task_index,
        study.number_of_tasks,
    )
    .await?
    .ok_or_else(|| {
        // The state changed under us between the gate check and the
        // advance; ask the client to refetch /state and try again.
        AppError::bad_request("study.invalid_stage")
    })?;

    Ok(Json(FinishTaskResponse {
        stage: advanced.stage,
        current_task_index: advanced.current_task_index,
        is_last_task: task_index + 1 >= study.number_of_tasks,
    }))
}

// ---------------------------------------------------------------------------
// Admin: GET / PUT /admin/study/courses/{course_id}/config
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AdminConfigResponse {
    course_id: Uuid,
    number_of_tasks: i32,
    completion_gate_kind: String,
    consent_html: String,
    thank_you_html: String,
    tasks: Vec<AdminTaskView>,
    pre_survey: Option<AdminSurveyView>,
    post_survey: Option<AdminSurveyView>,
    /// True iff any participant is past `consent` stage. The admin UI
    /// uses this to warn before destructive task / question edits;
    /// editing config mid-study is allowed (sometimes you have to fix
    /// a typo) but the warning makes the trade-off explicit.
    has_in_flight_participants: bool,
}

#[derive(Serialize, Deserialize)]
struct AdminTaskView {
    task_index: i32,
    title: String,
    description: String,
}

#[derive(Serialize)]
struct AdminSurveyView {
    kind: String,
    questions: Vec<AdminSurveyQuestionView>,
    response_count: i64,
}

#[derive(Serialize, Deserialize)]
struct AdminSurveyQuestionView {
    /// `likert`, `free_text`, or `section_heading`.
    kind: String,
    prompt: String,
    likert_min: Option<i32>,
    likert_max: Option<i32>,
    likert_min_label: Option<String>,
    likert_max_label: Option<String>,
    /// Defaults to TRUE for backwards compatibility when the admin
    /// UI omits the field on existing rows. Section-headings must
    /// always be FALSE.
    #[serde(default = "default_true")]
    is_required: bool,
    /// Likert-only kill switch; see SurveyQuestionView. NULL means
    /// no kill switch. Admin UI doesn't surface this for the
    /// current eval; the seed script sets it directly for the
    /// GDPR consent question.
    #[serde(default)]
    kill_on_value: Option<i32>,
}

fn default_true() -> bool {
    true
}

async fn admin_get_config(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<AdminConfigResponse>, AppError> {
    require_course_owner_teacher_or_admin(&state, course_id, &user).await?;

    // Bootstrap-tolerant: if the admin enabled the `study_mode` flag
    // for a course that doesn't yet have a `study_courses` row (the
    // common path when setting up a new study), we synthesise an
    // empty default config so the editor + "Load preset" button can
    // render. The row is created on first save (PUT /config) or on
    // first preset seed (POST /seed-dm2731), both of which use
    // upsert_study_course. We never write here so a stray GET can't
    // leave junk rows behind.
    let study = minerva_db::queries::study::get_study_course(&state.db, course_id)
        .await?
        .unwrap_or_else(|| minerva_db::queries::study::StudyCourseRow {
            course_id,
            // 0 here is a wire-only default; the DB CHECK requires
            // `> 0`. The frontend's task editor keeps `number_of_tasks`
            // in lockstep with `tasks.length` as the admin adds rows,
            // so the first PUT after they add at least one task will
            // satisfy the CHECK. Saving with zero tasks gets caught
            // upstream by `study.number_of_tasks_invalid`.
            number_of_tasks: 0,
            completion_gate_kind: "messages_only".into(),
            consent_html: String::new(),
            thank_you_html: String::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        });
    let tasks = minerva_db::queries::study::list_tasks(&state.db, course_id).await?;
    let pre =
        minerva_db::queries::study::get_survey_with_questions(&state.db, course_id, "pre").await?;
    let post =
        minerva_db::queries::study::get_survey_with_questions(&state.db, course_id, "post").await?;

    // "In flight" = any participant past consent stage. The
    // consent-only count is fine to edit through; once someone has
    // started the pre-survey a config change risks silently
    // invalidating their data.
    let participants =
        minerva_db::queries::study::list_participants_with_stages(&state.db, course_id).await?;
    let has_in_flight_participants = participants.iter().any(|p| p.stage != "consent");

    let map_survey =
        |sw: minerva_db::queries::study::SurveyWithQuestions, count: i64| AdminSurveyView {
            kind: sw.survey.kind,
            questions: sw
                .questions
                .into_iter()
                .map(|q| AdminSurveyQuestionView {
                    kind: q.kind,
                    prompt: q.prompt,
                    likert_min: q.likert_min,
                    likert_max: q.likert_max,
                    likert_min_label: q.likert_min_label,
                    likert_max_label: q.likert_max_label,
                    is_required: q.is_required,
                    kill_on_value: q.kill_on_value,
                })
                .collect(),
            response_count: count,
        };

    let pre_view = if let Some(sw) = pre {
        let n = minerva_db::queries::study::count_survey_responses(&state.db, sw.survey.id).await?;
        Some(map_survey(sw, n))
    } else {
        None
    };
    let post_view = if let Some(sw) = post {
        let n = minerva_db::queries::study::count_survey_responses(&state.db, sw.survey.id).await?;
        Some(map_survey(sw, n))
    } else {
        None
    };

    Ok(Json(AdminConfigResponse {
        course_id: study.course_id,
        number_of_tasks: study.number_of_tasks,
        completion_gate_kind: study.completion_gate_kind,
        consent_html: study.consent_html,
        thank_you_html: study.thank_you_html,
        tasks: tasks
            .into_iter()
            .map(|t| AdminTaskView {
                task_index: t.task_index,
                title: t.title,
                description: t.description,
            })
            .collect(),
        pre_survey: pre_view,
        post_survey: post_view,
        has_in_flight_participants,
    }))
}

#[derive(Deserialize)]
struct AdminPutConfigRequest {
    number_of_tasks: i32,
    completion_gate_kind: String,
    consent_html: String,
    thank_you_html: String,
    tasks: Vec<AdminTaskView>,
    pre_survey: Vec<AdminSurveyQuestionView>,
    post_survey: Vec<AdminSurveyQuestionView>,
}

async fn admin_put_config(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
    Json(body): Json<AdminPutConfigRequest>,
) -> Result<Json<AdminConfigResponse>, AppError> {
    require_course_owner_teacher_or_admin(&state, course_id, &user).await?;

    if body.number_of_tasks <= 0 {
        return Err(AppError::bad_request("study.number_of_tasks_invalid"));
    }
    if body.tasks.len() != body.number_of_tasks as usize {
        return Err(AppError::bad_request("study.tasks_count_mismatch"));
    }
    // Tasks must be supplied in 0..N order without gaps; the DB has a
    // UNIQUE on (course_id, task_index) but doesn't enforce density.
    for (i, t) in body.tasks.iter().enumerate() {
        if t.task_index != i as i32 {
            return Err(AppError::bad_request("study.task_index_must_be_dense"));
        }
        if t.title.trim().is_empty() || t.description.trim().is_empty() {
            return Err(AppError::bad_request("study.task_fields_required"));
        }
    }
    if body.completion_gate_kind != "messages_only" {
        return Err(AppError::bad_request("study.completion_gate_kind_invalid"));
    }
    validate_survey_question_inputs(&body.pre_survey)?;
    validate_survey_question_inputs(&body.post_survey)?;

    minerva_db::queries::study::upsert_study_course(
        &state.db,
        course_id,
        body.number_of_tasks,
        &body.completion_gate_kind,
        &body.consent_html,
        &body.thank_you_html,
    )
    .await?;

    let task_inputs: Vec<(i32, String, String)> = body
        .tasks
        .into_iter()
        .map(|t| (t.task_index, t.title, t.description))
        .collect();
    minerva_db::queries::study::replace_tasks(&state.db, course_id, &task_inputs).await?;

    minerva_db::queries::study::replace_survey(
        &state.db,
        course_id,
        "pre",
        &to_survey_inputs(body.pre_survey),
    )
    .await?;
    minerva_db::queries::study::replace_survey(
        &state.db,
        course_id,
        "post",
        &to_survey_inputs(body.post_survey),
    )
    .await?;

    admin_get_config(State(state), Extension(user), Path(course_id)).await
}

fn validate_survey_question_inputs(qs: &[AdminSurveyQuestionView]) -> Result<(), AppError> {
    for q in qs {
        if q.prompt.trim().is_empty() {
            return Err(AppError::bad_request("study.question_prompt_required"));
        }
        match q.kind.as_str() {
            "likert" => {
                let (min, max) = match (q.likert_min, q.likert_max) {
                    (Some(min), Some(max)) => (min, max),
                    _ => return Err(AppError::bad_request("study.likert_bounds_required")),
                };
                if max <= min {
                    return Err(AppError::bad_request("study.likert_bounds_invalid"));
                }
            }
            "free_text" => {
                if q.likert_min.is_some()
                    || q.likert_max.is_some()
                    || q.likert_min_label.is_some()
                    || q.likert_max_label.is_some()
                {
                    return Err(AppError::bad_request(
                        "study.free_text_must_not_have_likert_fields",
                    ));
                }
            }
            "section_heading" => {
                if q.likert_min.is_some()
                    || q.likert_max.is_some()
                    || q.likert_min_label.is_some()
                    || q.likert_max_label.is_some()
                {
                    return Err(AppError::bad_request(
                        "study.free_text_must_not_have_likert_fields",
                    ));
                }
                if q.is_required {
                    // The DB CHECK enforces this anyway; reject with
                    // a clearer error code rather than letting the
                    // 500-Database surface.
                    return Err(AppError::bad_request(
                        "study.section_heading_must_be_optional",
                    ));
                }
            }
            _ => return Err(AppError::bad_request("study.question_kind_invalid")),
        }
    }
    Ok(())
}

fn to_survey_inputs(
    qs: Vec<AdminSurveyQuestionView>,
) -> Vec<minerva_db::queries::study::SurveyQuestionInput> {
    qs.into_iter()
        .map(|q| minerva_db::queries::study::SurveyQuestionInput {
            kind: q.kind,
            prompt: q.prompt,
            likert_min: q.likert_min,
            likert_max: q.likert_max,
            likert_min_label: q.likert_min_label,
            likert_max_label: q.likert_max_label,
            is_required: q.is_required,
            kill_on_value: q.kill_on_value,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Admin: GET /admin/study/courses/{course_id}/participants
// ---------------------------------------------------------------------------

/// Anonymous participant row for the Study Mode admin UI. No
/// `user_id`, `eppn`, or `display_name` here on purpose: the
/// researcher's analysis view should never need (or be able to)
/// link a row back to a specific person. The "who is participant
/// 5?" lookup happens via the regular course members tab, where
/// names live alongside a `study_stage` field for matching, and the
/// "delete participant data on request" operation goes via the
/// members tab too. Strict separation of identified-roster from
/// anonymous-analysis views.
#[derive(Serialize)]
struct AdminParticipantRow {
    /// Sequential per-course participant identifier, assigned at
    /// consent time. NULL for rows that landed on the consent
    /// screen but never consented (still useful to count drop-off).
    participant_number: Option<i32>,
    stage: String,
    current_task_index: i32,
    consented_at: Option<chrono::DateTime<chrono::Utc>>,
    pre_survey_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    post_survey_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    locked_out_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn admin_list_participants(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<Vec<AdminParticipantRow>>, AppError> {
    require_course_owner_teacher_or_admin(&state, course_id, &user).await?;
    let rows =
        minerva_db::queries::study::list_participants_with_stages(&state.db, course_id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| AdminParticipantRow {
                participant_number: r.participant_number,
                stage: r.stage,
                current_task_index: r.current_task_index,
                consented_at: r.consented_at,
                pre_survey_completed_at: r.pre_survey_completed_at,
                post_survey_completed_at: r.post_survey_completed_at,
                locked_out_at: r.locked_out_at,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Admin: GET /admin/study/courses/{id}/participants/{n}/detail
// ---------------------------------------------------------------------------

/// Per-participant detail dump for the researcher's UI drill-in.
/// Keyed by `participant_number`, never by user_id, so the admin
/// page never has to surface a person-identifying token. Returns
/// the same data shape as one line of the JSONL export (surveys +
/// tasks + messages + Aegis analyses + iteration history) but as a
/// regular JSON response for the frontend to render.
async fn admin_get_participant_detail(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, participant_number)): Path<(Uuid, i32)>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_course_owner_teacher_or_admin(&state, course_id, &user).await?;

    let participant = minerva_db::queries::study::find_by_participant_number(
        &state.db,
        course_id,
        participant_number,
    )
    .await?
    .ok_or(AppError::NotFound)?;

    let pre_survey =
        minerva_db::queries::study::get_survey_with_questions(&state.db, course_id, "pre").await?;
    let post_survey =
        minerva_db::queries::study::get_survey_with_questions(&state.db, course_id, "post").await?;

    let pre_responses = if let Some(s) = &pre_survey {
        minerva_db::queries::study::export_responses_for_user_in_survey(
            &state.db,
            s.survey.id,
            participant.user_id,
        )
        .await?
    } else {
        vec![]
    };
    let post_responses = if let Some(s) = &post_survey {
        minerva_db::queries::study::export_responses_for_user_in_survey(
            &state.db,
            s.survey.id,
            participant.user_id,
        )
        .await?
    } else {
        vec![]
    };

    let task_conversations = minerva_db::queries::study::list_task_conversations_for_user(
        &state.db,
        course_id,
        participant.user_id,
    )
    .await?;

    let mut tasks_json = Vec::with_capacity(task_conversations.len());
    for tc in &task_conversations {
        let task_meta =
            minerva_db::queries::study::get_task(&state.db, course_id, tc.task_index).await?;
        let messages = minerva_db::queries::study::export_messages_for_conversation(
            &state.db,
            tc.conversation_id,
        )
        .await?;
        let analyses = minerva_db::queries::study::export_prompt_analyses_for_conversation(
            &state.db,
            tc.conversation_id,
        )
        .await?;
        let iterations = minerva_db::queries::aegis_iterations::list_for_conversation(
            &state.db,
            tc.conversation_id,
        )
        .await?;
        tasks_json.push(json!({
            "task_index": tc.task_index,
            "task_title": task_meta.as_ref().map(|t| t.title.clone()),
            "task_description": task_meta.as_ref().map(|t| t.description.clone()),
            "conversation_id": tc.conversation_id,
            "started_at": tc.started_at,
            "marked_done_at": tc.marked_done_at,
            "messages": messages.into_iter().map(|m| json!({
                "id": m.id,
                "role": m.role,
                "content": m.content,
                "model_used": m.model_used,
                "tokens_prompt": m.tokens_prompt,
                "tokens_completion": m.tokens_completion,
                "generation_ms": m.generation_ms,
                "retrieval_count": m.retrieval_count,
                "created_at": m.created_at,
            })).collect::<Vec<_>>(),
            "aegis_prompt_analyses": analyses.into_iter().map(|a| json!({
                "message_id": a.message_id,
                "suggestions": a.suggestions,
                "mode": a.mode,
                "model_used": a.model_used,
                "created_at": a.created_at,
            })).collect::<Vec<_>>(),
            "aegis_live_iterations": iterations.into_iter().map(|it| json!({
                "id": it.id,
                "draft_text": it.draft_text,
                "suggestions": it.suggestions,
                "mode": it.mode,
                "model_used": it.model_used,
                "created_at": it.created_at,
            })).collect::<Vec<_>>(),
        }));
    }

    Ok(Json(json!({
        "participant_number": participant.participant_number,
        "stage": participant.stage,
        "consented_at": participant.consented_at,
        "pre_survey_completed_at": participant.pre_survey_completed_at,
        "post_survey_completed_at": participant.post_survey_completed_at,
        "locked_out_at": participant.locked_out_at,
        "pre_survey_responses": pre_responses.iter().cloned().map(serialize_response).collect::<Vec<_>>(),
        "post_survey_responses": post_responses.iter().cloned().map(serialize_response).collect::<Vec<_>>(),
        "tasks": tasks_json,
    })))
}

// ---------------------------------------------------------------------------
// Admin: DELETE /admin/study/courses/{id}/participants/by-user/{user_id}
// ---------------------------------------------------------------------------

/// GDPR-style erasure of one participant's study data. Triggered
/// from the course members tab (where names ARE shown so the
/// researcher can pick "Alice"), NOT from the anonymous Study Mode
/// participants table. Wipes per-task conversations (and through
/// CASCADE: messages, prompt_analyses, aegis_iterations, the
/// study_task_conversations mapping), survey responses, and the
/// participant_state row. Course membership stays put; that's a
/// separate "remove from course" operation.
///
/// The deleted participant's `participant_number` is NOT reused;
/// remaining participants keep their stable numbers so any prior
/// analyses referring to "participant 5" still mean the same row.
async fn admin_delete_participant_data(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((course_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_course_owner_teacher_or_admin(&state, course_id, &user).await?;
    minerva_db::queries::study::delete_participant_data(&state.db, course_id, target_user_id)
        .await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---------------------------------------------------------------------------
// Admin: GET /admin/study/courses/{course_id}/export.jsonl  (Phase 5)
// ---------------------------------------------------------------------------

/// Streaming NDJSON export. One JSON object per line, one line per
/// participant who has at least consented. `participant_id` is
/// assigned at export time as the index in `consented_at ASC` order
/// (1-based for human-readability) and is NOT persisted;
/// re-exporting after a new participant consents is stable for the
/// existing rows because order doesn't change for already-consented
/// participants.
///
/// Bypasses `ext_obfuscate` deliberately: researchers need real
/// eppns + display names so they can reconcile the JSONL against
/// their participant roster. Admins-only.
///
/// Streamed line-by-line (one per participant) so a course with
/// hundreds of participants doesn't OOM the backend by buffering
/// the whole array in memory.
async fn admin_export_jsonl(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Response, AppError> {
    require_course_owner_teacher_or_admin(&state, course_id, &user).await?;

    // We pre-fetch the participant list (ordered) so we can assign
    // participant_id deterministically. The per-participant fan-out
    // queries happen inside the stream, one participant at a time.
    let participants =
        minerva_db::queries::study::list_participants_for_export(&state.db, course_id).await?;

    let study = minerva_db::queries::study::get_study_course(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let pre_survey =
        minerva_db::queries::study::get_survey_with_questions(&state.db, course_id, "pre").await?;
    let post_survey =
        minerva_db::queries::study::get_survey_with_questions(&state.db, course_id, "post").await?;

    let db = state.db.clone();
    let line_stream = stream::iter(participants).then(move |participant| {
        let db = db.clone();
        let pre_survey_id = pre_survey.as_ref().map(|s| s.survey.id);
        let post_survey_id = post_survey.as_ref().map(|s| s.survey.id);
        let study_course_meta = json!({
            "course_id": study.course_id,
            "number_of_tasks": study.number_of_tasks,
            "completion_gate_kind": study.completion_gate_kind.clone(),
        });
        async move {
            // The list query only returns rows with a non-null
            // participant_number, so unwrap is safe; fall back
            // to 0 defensively in the impossible-but-explicit case.
            let pid = participant.participant_number.unwrap_or(0);
            build_participant_line(
                &db,
                pid,
                &participant,
                pre_survey_id,
                post_survey_id,
                &study_course_meta,
            )
            .await
        }
    });

    let body = Body::from_stream(line_stream);
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    let filename = format!("study-{course_id}.jsonl");
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|_| AppError::Internal("bad filename header".into()))?,
    );
    Ok(response)
}

/// Builds one NDJSON line for one participant. Fan-out queries
/// happen here (per-participant tasks + their messages + their
/// aegis analyses + survey responses).
///
/// Returns `Result<String, std::io::Error>` because that's what
/// `Body::from_stream` expects; on a per-row DB error we emit a
/// `{"error": "..."}` line so the export doesn't abort midway and
/// the researcher can see which participant failed.
async fn build_participant_line(
    db: &sqlx::PgPool,
    participant_id: i32,
    participant: &minerva_db::queries::study::StudyParticipantStateRow,
    pre_survey_id: Option<Uuid>,
    post_survey_id: Option<Uuid>,
    study_course_meta: &serde_json::Value,
) -> Result<Bytes, std::io::Error> {
    let user_id = participant.user_id;

    // Pseudonymised export: the only identifier we emit is the
    // sequential `participant_id` (assigned at export time, ordered by
    // `consented_at`). NEVER emit the participant's eppn or display
    // name in the JSONL; the consent screen promises anonymisation
    // and the live admin participants table is the canonical place
    // to look up which person corresponds to which participant_id
    // (admins-only, behind the same auth as the export itself).
    //
    // The internal `user_id` UUID is used below to fetch responses
    // and conversations, but never serialised into the line.

    // Pre + post survey responses, joined to question prompts.
    let pre_responses = match pre_survey_id {
        Some(sid) => {
            match minerva_db::queries::study::export_responses_for_user_in_survey(db, sid, user_id)
                .await
            {
                Ok(rs) => rs,
                Err(e) => return Ok(json_err_line(participant_id, &format!("pre survey: {e}"))),
            }
        }
        None => vec![],
    };
    let post_responses = match post_survey_id {
        Some(sid) => {
            match minerva_db::queries::study::export_responses_for_user_in_survey(db, sid, user_id)
                .await
            {
                Ok(rs) => rs,
                Err(e) => return Ok(json_err_line(participant_id, &format!("post survey: {e}"))),
            }
        }
        None => vec![],
    };

    // Per-task conversations + their messages + Aegis analyses.
    let task_conversations = match minerva_db::queries::study::list_task_conversations_for_user(
        db,
        participant.course_id,
        user_id,
    )
    .await
    {
        Ok(tcs) => tcs,
        Err(e) => return Ok(json_err_line(participant_id, &format!("task convs: {e}"))),
    };

    let mut tasks_json = Vec::with_capacity(task_conversations.len());
    for tc in &task_conversations {
        let messages = match minerva_db::queries::study::export_messages_for_conversation(
            db,
            tc.conversation_id,
        )
        .await
        {
            Ok(m) => m,
            Err(e) => {
                return Ok(json_err_line(
                    participant_id,
                    &format!("messages for task {}: {e}", tc.task_index),
                ))
            }
        };
        let iterations = match minerva_db::queries::aegis_iterations::list_for_conversation(
            db,
            tc.conversation_id,
        )
        .await
        {
            Ok(rs) => rs,
            Err(e) => {
                return Ok(json_err_line(
                    participant_id,
                    &format!("aegis iterations for task {}: {e}", tc.task_index),
                ))
            }
        };
        let analyses = match minerva_db::queries::study::export_prompt_analyses_for_conversation(
            db,
            tc.conversation_id,
        )
        .await
        {
            Ok(a) => a,
            Err(e) => {
                return Ok(json_err_line(
                    participant_id,
                    &format!("analyses for task {}: {e}", tc.task_index),
                ))
            }
        };
        tasks_json.push(json!({
            "task_index": tc.task_index,
            "conversation_id": tc.conversation_id,
            "started_at": tc.started_at,
            "marked_done_at": tc.marked_done_at,
            "messages": messages
                .into_iter()
                .map(|m| json!({
                    "id": m.id,
                    "role": m.role,
                    "content": m.content,
                    "model_used": m.model_used,
                    "tokens_prompt": m.tokens_prompt,
                    "tokens_completion": m.tokens_completion,
                    "generation_ms": m.generation_ms,
                    "retrieval_count": m.retrieval_count,
                    "created_at": m.created_at,
                }))
                .collect::<Vec<_>>(),
            "aegis_prompt_analyses": analyses
                .into_iter()
                .map(|a| json!({
                    "message_id": a.message_id,
                    "suggestions": a.suggestions,
                    "mode": a.mode,
                    "model_used": a.model_used,
                    "created_at": a.created_at,
                }))
                .collect::<Vec<_>>(),
            // Live iteration history: every debounced analyze call
            // the participant's drafting triggered. Ordered oldest
            // first; the participant's at-send draft is the
            // last user message in `messages`, and the corresponding
            // at-send verdict is in `aegis_prompt_analyses` (joined
            // by message_id). Iterations BETWEEN are visible only
            // here, which is the whole point of this stream for an
            // Aegis evaluation.
            "aegis_live_iterations": iterations
                .into_iter()
                .map(|it| json!({
                    "id": it.id,
                    "draft_text": it.draft_text,
                    "suggestions": it.suggestions,
                    "mode": it.mode,
                    "model_used": it.model_used,
                    "created_at": it.created_at,
                }))
                .collect::<Vec<_>>(),
        }));
    }

    let line = json!({
        "participant_id": participant_id,
        "study_course": study_course_meta,
        "stage": participant.stage,
        "consented_at": participant.consented_at,
        "pre_survey_completed_at": participant.pre_survey_completed_at,
        "post_survey_completed_at": participant.post_survey_completed_at,
        "locked_out_at": participant.locked_out_at,
        "pre_survey_responses": pre_responses
            .into_iter()
            .map(serialize_response)
            .collect::<Vec<_>>(),
        "post_survey_responses": post_responses
            .into_iter()
            .map(serialize_response)
            .collect::<Vec<_>>(),
        "tasks": tasks_json,
    });

    let mut s = serde_json::to_string(&line).unwrap_or_else(|e| {
        format!("{{\"participant_id\":{participant_id},\"error\":\"serialize: {e}\"}}")
    });
    s.push('\n');
    Ok(Bytes::from(s))
}

fn serialize_response(r: minerva_db::queries::study::ExportSurveyResponseRow) -> serde_json::Value {
    json!({
        "question_id": r.question_id,
        "question_ord": r.question_ord,
        "question_prompt": r.question_prompt,
        "question_kind": r.question_kind,
        "likert_value": r.likert_value,
        "free_text_value": r.free_text_value,
        "submitted_at": r.submitted_at,
    })
}

fn json_err_line(participant_id: i32, msg: &str) -> Bytes {
    let mut s = serde_json::to_string(&json!({
        "participant_id": participant_id,
        "error": msg,
    }))
    .unwrap_or_else(|_| {
        format!("{{\"participant_id\":{participant_id},\"error\":\"unprintable\"}}")
    });
    s.push('\n');
    Bytes::from(s)
}

// ---------------------------------------------------------------------------
// Admin: POST /admin/study/courses/{course_id}/seed-dm2731
// ---------------------------------------------------------------------------

/// One-shot seed for the AI for Learning DM2731 / Aegis evaluation.
///
/// Idempotent: each call is the same delete-then-insert that
/// `admin_put_config` performs (transactional per-survey + per-task
/// list), so re-running just brings the course back to the canonical
/// content. Useful for pre-launch dry runs and for resetting a test
/// course mid-development. Refuses if any participant has progressed
/// past `consent` so a careless click during a live study can't blow
/// away participants' in-flight surveys (responses CASCADE-delete
/// when their question rows are replaced).
///
/// Researcher can edit any field via the regular admin editor after
/// seeding; this just removes the "type 17 questions in by hand"
/// step. The content lives in Rust rather than a SQL/JSON file so
/// it ships with every deploy and survives a fresh DB reset without
/// needing a separate apply step.
async fn admin_seed_dm2731(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(course_id): Path<Uuid>,
) -> Result<Json<AdminConfigResponse>, AppError> {
    require_admin(&user)?;

    // Refuse if anyone is mid-study; replacing the question rows
    // would CASCADE-delete their responses.
    let participants =
        minerva_db::queries::study::list_participants_with_stages(&state.db, course_id).await?;
    if participants.iter().any(|p| p.stage != "consent") {
        return Err(AppError::bad_request("study.seed_blocked_in_flight"));
    }

    let body = dm2731_preset();

    // Delegate to the same path the editor's Save button uses, so
    // the validation + transaction story is identical (no parallel
    // code path that could drift).
    minerva_db::queries::study::upsert_study_course(
        &state.db,
        course_id,
        body.number_of_tasks,
        &body.completion_gate_kind,
        &body.consent_html,
        &body.thank_you_html,
    )
    .await?;

    let task_inputs: Vec<(i32, String, String)> = body
        .tasks
        .into_iter()
        .map(|t| (t.task_index, t.title, t.description))
        .collect();
    minerva_db::queries::study::replace_tasks(&state.db, course_id, &task_inputs).await?;

    minerva_db::queries::study::replace_survey(
        &state.db,
        course_id,
        "pre",
        &to_survey_inputs(body.pre_survey),
    )
    .await?;
    minerva_db::queries::study::replace_survey(
        &state.db,
        course_id,
        "post",
        &to_survey_inputs(body.post_survey),
    )
    .await?;

    admin_get_config(State(state), Extension(user), Path(course_id)).await
}

/// AdminPutConfigRequest payload for the DM2731 / Aegis evaluation.
/// Survey content is from the researcher; the typo on the "I felt
/// engaged when using Aegis" question (both endpoints labelled
/// "Strongly agree" in the source) is corrected here to disagree -> agree.
fn dm2731_preset() -> AdminPutConfigRequest {
    AdminPutConfigRequest {
        number_of_tasks: 1,
        completion_gate_kind: "messages_only".into(),
        consent_html: DM2731_CONSENT.into(),
        thank_you_html: DM2731_THANKS.into(),
        tasks: vec![AdminTaskView {
            task_index: 0,
            title: "Mars habitability prompt".into(),
            description: "Your task is to create a prompt that helps you understand what environmental and technological challenges must be addressed for humans to live on Mars.".into(),
        }],
        pre_survey: dm2731_pre_survey(),
        post_survey: dm2731_post_survey(),
    }
}

fn dm2731_pre_survey() -> Vec<AdminSurveyQuestionView> {
    // GDPR consent is collected via the consent screen that
    // precedes the pre-survey, not as a survey question. The
    // `kill_on_value` infrastructure is still present in the
    // schema + admin editor for any future survey that wants a
    // withdraw-on-answer kill switch; the DM2731 preset just
    // doesn't use it.
    vec![
        free_text("How old are you? (e.g. 23)", true),
        likert(
            "How often do you use Generative AI?",
            1,
            10,
            "Never",
            "Every hour awake",
            true,
            None,
        ),
        free_text("How do you use Generative AI?", true),
    ]
}

fn dm2731_post_survey() -> Vec<AdminSurveyQuestionView> {
    let sus_endpoints = ("Strongly disagree", "Strongly agree");
    let sus = |prompt: &str| likert(prompt, 1, 5, sus_endpoints.0, sus_endpoints.1, true, None);
    vec![
        section_heading(
            "Please rank these according to your experience with the User Interface.",
        ),
        sus("I think that I would like to use this system frequently."),
        sus("I found the system unnecessarily complex."),
        sus("I thought the system was easy to use."),
        sus("I think that I would need the support of a technical person to be able to use this system."),
        sus("I found the various functions in this system were well integrated."),
        sus("I thought there was too much inconsistency in this system."),
        sus("I would imagine that most people would learn to use this system very quickly."),
        sus("I found the system very cumbersome to use."),
        sus("I felt very confident using the system."),
        sus("I needed to learn a lot of things before I could get going with this system."),
        section_heading(
            "USER INTERFACE - Please rank these according to your experience with the User Interface of Aegis (not Minerva).",
        ),
        sus("The interface was easy to understand."),
        sus("Aegis motivated me to achieve my goal."),
        // The user's transcription of this one had both endpoints
        // labelled "Strongly agree" (obvious typo); restored to
        // disagree -> agree to match the rest.
        sus("I felt engaged when using Aegis."),
        free_text("How did using Aegis affect your prompting?", true),
        // Final question explicitly optional (no asterisk in source).
        free_text("Anything you would like to add about your experience?", false),
    ]
}

fn likert(
    prompt: &str,
    min: i32,
    max: i32,
    min_label: &str,
    max_label: &str,
    is_required: bool,
    kill_on_value: Option<i32>,
) -> AdminSurveyQuestionView {
    AdminSurveyQuestionView {
        kind: "likert".into(),
        prompt: prompt.into(),
        likert_min: Some(min),
        likert_max: Some(max),
        likert_min_label: Some(min_label.into()),
        likert_max_label: Some(max_label.into()),
        is_required,
        kill_on_value,
    }
}

fn free_text(prompt: &str, is_required: bool) -> AdminSurveyQuestionView {
    AdminSurveyQuestionView {
        kind: "free_text".into(),
        prompt: prompt.into(),
        likert_min: None,
        likert_max: None,
        likert_min_label: None,
        likert_max_label: None,
        is_required,
        kill_on_value: None,
    }
}

fn section_heading(prompt: &str) -> AdminSurveyQuestionView {
    AdminSurveyQuestionView {
        kind: "section_heading".into(),
        prompt: prompt.into(),
        likert_min: None,
        likert_max: None,
        likert_min_label: None,
        likert_max_label: None,
        // DB CHECK enforces section_heading => is_required = false.
        is_required: false,
        kill_on_value: None,
    }
}

const DM2731_CONSENT: &str = r#"# Consent to Participate in a User Study

You are invited to take part in a research study conducted as part of the
course **AI for Learning DM2731**. The purpose of this study is to explore
how prompt coaching affects student prompting behaviour. Please read the
following information carefully. Feel free to ask any questions you have
before agreeing to take part in the study.

In this study, you will complete a task in a chat interface. Before we
start we will collect basic demographic information and your previous
experience with LLM. The conversation you have with the LLM will be
recorded as well as your responses to a short questionnaire.

All collected data will be anonymized, securely stored, and used only for
the purposes of this study. The data will be deleted upon completion of
the project.

Participation is entirely voluntary. You have the right to withdraw
whenever you want, without providing a reason. The full study is expected
to take approximately 30 minutes.

If you have any questions before or during the study, feel free to ask
the researchers. You can always contact us at:
**edwinsu@dsv.su.se**, **edwinsu@kth.se**, **khogb@kth.se**

## Consent

By ticking the box below and pressing "I consent", I confirm that I have
read and understood the information above and voluntarily agree to
participate in this study.
"#;

const DM2731_THANKS: &str = r#"# Thank you for your participation!

Your responses and conversation log have been recorded.

You can close this tab.
"#;
