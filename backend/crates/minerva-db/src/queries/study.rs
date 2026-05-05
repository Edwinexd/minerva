//! Study mode: per-course research-evaluation pipeline.
//!
//! Schema lives in migrations 20260505000001 through 20260505000005.
//! The application gate is `feature_flags::FLAG_STUDY_MODE`; the rows
//! in `study_courses` / `study_tasks` / `study_surveys` are the
//! configuration the gate activates.
//!
//! Layout of this module:
//!   * Row structs: one per table.
//!   * Config CRUD (admin-side): `get_study_course`, `upsert_study_course`,
//!     `replace_tasks`, `replace_survey`, etc.
//!   * Participant state machine: `get_or_init_state`, `record_consent`,
//!     `advance_to_*`, `mark_locked_out`.
//!   * Per-task conversation mapping: `get_or_create_task_conversation`,
//!     `mark_task_done`.
//!   * Survey responses: `submit_survey_responses` (transactional UPSERT).
//!   * Gate evaluation: `count_user_messages_in_conversation` (the only
//!     gate kind currently implemented is "messages_only").
//!   * Admin views: `list_participants_with_stages`.
//!   * Export: `export_iter_participants` and helpers used by the
//!     NDJSON streaming export route in the server crate.

use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StudyCourseRow {
    pub course_id: Uuid,
    pub number_of_tasks: i32,
    pub completion_gate_kind: String,
    pub consent_html: String,
    pub thank_you_html: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct StudyTaskRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub task_index: i32,
    pub title: String,
    pub description: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct StudySurveyRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub kind: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct StudySurveyQuestionRow {
    pub id: Uuid,
    pub survey_id: Uuid,
    pub ord: i32,
    /// One of: `likert`, `free_text`, `section_heading`. Section
    /// headings are display-only (no answer); the route layer skips
    /// them in submission validation. Boolean / single-choice
    /// questions are out of scope for now; researchers can model
    /// 2-button yes/no as `likert` with `min=1, max=2,
    /// min_label="No", max_label="Yes"`.
    pub kind: String,
    pub prompt: String,
    pub likert_min: Option<i32>,
    pub likert_max: Option<i32>,
    pub likert_min_label: Option<String>,
    pub likert_max_label: Option<String>,
    /// FALSE -> participant may submit the survey without answering
    /// this question. Section-heading questions are always FALSE
    /// (DB CHECK enforces it); answer-bearing questions default to
    /// TRUE for backwards compatibility with the original schema.
    pub is_required: bool,
    /// Withdraw-on-answer kill switch (likert questions only).
    /// When the participant answers with this value, the route
    /// handler short-circuits the normal stage advance and jumps
    /// straight to `done` (lockout). Used for GDPR consent
    /// questions: if "No, do not save my data", the study ends
    /// without further data collection.
    pub kill_on_value: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct StudySurveyResponseRow {
    pub id: Uuid,
    pub survey_id: Uuid,
    pub user_id: Uuid,
    pub question_id: Uuid,
    pub likert_value: Option<i32>,
    pub free_text_value: Option<String>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct StudyParticipantStateRow {
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub stage: String,
    pub current_task_index: i32,
    pub consented_at: Option<chrono::DateTime<chrono::Utc>>,
    pub pre_survey_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub post_survey_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub locked_out_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct StudyTaskConversationRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub task_index: i32,
    pub conversation_id: Uuid,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub marked_done_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ---------------------------------------------------------------------------
// Config CRUD: study_courses
// ---------------------------------------------------------------------------

pub async fn get_study_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Option<StudyCourseRow>, sqlx::Error> {
    sqlx::query_as!(
        StudyCourseRow,
        r#"SELECT course_id, number_of_tasks, completion_gate_kind, consent_html, thank_you_html, created_at, updated_at
        FROM study_courses
        WHERE course_id = $1"#,
        course_id,
    )
    .fetch_optional(db)
    .await
}

/// Insert or update the per-course study config. Idempotent on
/// `course_id`. The application validates `completion_gate_kind`
/// against the supported set before calling this; the DB CHECK is
/// the backstop.
pub async fn upsert_study_course(
    db: &PgPool,
    course_id: Uuid,
    number_of_tasks: i32,
    completion_gate_kind: &str,
    consent_html: &str,
    thank_you_html: &str,
) -> Result<StudyCourseRow, sqlx::Error> {
    sqlx::query_as!(
        StudyCourseRow,
        r#"INSERT INTO study_courses (course_id, number_of_tasks, completion_gate_kind, consent_html, thank_you_html)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (course_id) DO UPDATE SET
            number_of_tasks = EXCLUDED.number_of_tasks,
            completion_gate_kind = EXCLUDED.completion_gate_kind,
            consent_html = EXCLUDED.consent_html,
            thank_you_html = EXCLUDED.thank_you_html,
            updated_at = NOW()
        RETURNING course_id, number_of_tasks, completion_gate_kind, consent_html, thank_you_html, created_at, updated_at"#,
        course_id,
        number_of_tasks,
        completion_gate_kind,
        consent_html,
        thank_you_html,
    )
    .fetch_one(db)
    .await
}

// ---------------------------------------------------------------------------
// Config CRUD: study_tasks
// ---------------------------------------------------------------------------

pub async fn list_tasks(db: &PgPool, course_id: Uuid) -> Result<Vec<StudyTaskRow>, sqlx::Error> {
    sqlx::query_as!(
        StudyTaskRow,
        r#"SELECT id, course_id, task_index, title, description, created_at, updated_at
        FROM study_tasks
        WHERE course_id = $1
        ORDER BY task_index ASC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

pub async fn get_task(
    db: &PgPool,
    course_id: Uuid,
    task_index: i32,
) -> Result<Option<StudyTaskRow>, sqlx::Error> {
    sqlx::query_as!(
        StudyTaskRow,
        r#"SELECT id, course_id, task_index, title, description, created_at, updated_at
        FROM study_tasks
        WHERE course_id = $1 AND task_index = $2"#,
        course_id,
        task_index,
    )
    .fetch_optional(db)
    .await
}

/// Replaces the task list for a course atomically: deletes existing
/// rows and inserts the supplied ones in a single transaction. This
/// is the admin "save" path; it's destructive to existing rows but
/// has to be, because task order is part of the experimental design
/// and partial edits would leave the dataset ambiguous. Tasks that
/// have already been answered (i.e. have `study_task_conversations`
/// rows pointing at the deleted task slot) are preserved on the
/// conversation side; the route handler is responsible for refusing
/// the save if any participant is past `consent` stage. The check
/// belongs upstream because it needs richer error reporting than a
/// transaction abort can give.
pub async fn replace_tasks(
    db: &PgPool,
    course_id: Uuid,
    tasks: &[(i32, String, String)],
) -> Result<Vec<StudyTaskRow>, sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query!("DELETE FROM study_tasks WHERE course_id = $1", course_id,)
        .execute(&mut *tx)
        .await?;

    let mut out = Vec::with_capacity(tasks.len());
    for (idx, title, description) in tasks {
        let row = sqlx::query_as!(
            StudyTaskRow,
            r#"INSERT INTO study_tasks (course_id, task_index, title, description)
            VALUES ($1, $2, $3, $4)
            RETURNING id, course_id, task_index, title, description, created_at, updated_at"#,
            course_id,
            idx,
            title,
            description,
        )
        .fetch_one(&mut *tx)
        .await?;
        out.push(row);
    }

    tx.commit().await?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Config CRUD: study_surveys + questions
// ---------------------------------------------------------------------------

pub struct SurveyQuestionInput {
    pub kind: String,
    pub prompt: String,
    pub likert_min: Option<i32>,
    pub likert_max: Option<i32>,
    pub likert_min_label: Option<String>,
    pub likert_max_label: Option<String>,
    pub is_required: bool,
    pub kill_on_value: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct SurveyWithQuestions {
    pub survey: StudySurveyRow,
    pub questions: Vec<StudySurveyQuestionRow>,
}

pub async fn get_survey_with_questions(
    db: &PgPool,
    course_id: Uuid,
    kind: &str,
) -> Result<Option<SurveyWithQuestions>, sqlx::Error> {
    let Some(survey) = sqlx::query_as!(
        StudySurveyRow,
        r#"SELECT id, course_id, kind, created_at, updated_at
        FROM study_surveys
        WHERE course_id = $1 AND kind = $2"#,
        course_id,
        kind,
    )
    .fetch_optional(db)
    .await?
    else {
        return Ok(None);
    };

    let questions = sqlx::query_as!(
        StudySurveyQuestionRow,
        r#"SELECT id, survey_id, ord, kind, prompt, likert_min, likert_max, likert_min_label, likert_max_label, is_required, kill_on_value, created_at, updated_at
        FROM study_survey_questions
        WHERE survey_id = $1
        ORDER BY ord ASC"#,
        survey.id,
    )
    .fetch_all(db)
    .await?;

    Ok(Some(SurveyWithQuestions { survey, questions }))
}

/// Replace a survey's question list in a single transaction. Same
/// destructive-replace shape as `replace_tasks`; route handler must
/// gate the save when responses already exist (mid-study question
/// edits are a research-design hazard). Creates the survey row on
/// first save.
pub async fn replace_survey(
    db: &PgPool,
    course_id: Uuid,
    kind: &str,
    questions: &[SurveyQuestionInput],
) -> Result<SurveyWithQuestions, sqlx::Error> {
    let mut tx = db.begin().await?;

    let survey = sqlx::query_as!(
        StudySurveyRow,
        r#"INSERT INTO study_surveys (course_id, kind)
        VALUES ($1, $2)
        ON CONFLICT (course_id, kind) DO UPDATE SET updated_at = NOW()
        RETURNING id, course_id, kind, created_at, updated_at"#,
        course_id,
        kind,
    )
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query!(
        "DELETE FROM study_survey_questions WHERE survey_id = $1",
        survey.id,
    )
    .execute(&mut *tx)
    .await?;

    let mut out_questions = Vec::with_capacity(questions.len());
    for (idx, q) in questions.iter().enumerate() {
        let row = sqlx::query_as!(
            StudySurveyQuestionRow,
            r#"INSERT INTO study_survey_questions (survey_id, ord, kind, prompt, likert_min, likert_max, likert_min_label, likert_max_label, is_required, kill_on_value)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING id, survey_id, ord, kind, prompt, likert_min, likert_max, likert_min_label, likert_max_label, is_required, kill_on_value, created_at, updated_at"#,
            survey.id,
            idx as i32,
            q.kind,
            q.prompt,
            q.likert_min,
            q.likert_max,
            q.likert_min_label,
            q.likert_max_label,
            q.is_required,
            q.kill_on_value,
        )
        .fetch_one(&mut *tx)
        .await?;
        out_questions.push(row);
    }

    tx.commit().await?;
    Ok(SurveyWithQuestions {
        survey,
        questions: out_questions,
    })
}

pub async fn count_survey_responses(db: &PgPool, survey_id: Uuid) -> Result<i64, sqlx::Error> {
    let n: Option<i64> = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM study_survey_responses WHERE survey_id = $1",
        survey_id,
    )
    .fetch_one(db)
    .await?;
    Ok(n.unwrap_or(0))
}

// ---------------------------------------------------------------------------
// Survey responses
// ---------------------------------------------------------------------------

pub struct SurveyResponseInput {
    pub question_id: Uuid,
    pub likert_value: Option<i32>,
    pub free_text_value: Option<String>,
}

/// Submit a participant's responses to one survey. UPSERTs each
/// answer keyed on (survey_id, user_id, question_id), so re-submission
/// of the same survey overwrites prior answers rather than appending.
/// All responses go in one transaction; partial submissions are not
/// observable.
pub async fn submit_survey_responses(
    db: &PgPool,
    survey_id: Uuid,
    user_id: Uuid,
    answers: &[SurveyResponseInput],
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    for a in answers {
        sqlx::query!(
            r#"INSERT INTO study_survey_responses (survey_id, user_id, question_id, likert_value, free_text_value)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (survey_id, user_id, question_id) DO UPDATE SET
                likert_value = EXCLUDED.likert_value,
                free_text_value = EXCLUDED.free_text_value,
                submitted_at = NOW()"#,
            survey_id,
            user_id,
            a.question_id,
            a.likert_value,
            a.free_text_value,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn list_user_responses(
    db: &PgPool,
    survey_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<StudySurveyResponseRow>, sqlx::Error> {
    sqlx::query_as!(
        StudySurveyResponseRow,
        r#"SELECT id, survey_id, user_id, question_id, likert_value, free_text_value, submitted_at
        FROM study_survey_responses
        WHERE survey_id = $1 AND user_id = $2
        ORDER BY submitted_at ASC"#,
        survey_id,
        user_id,
    )
    .fetch_all(db)
    .await
}

// ---------------------------------------------------------------------------
// Participant state machine
// ---------------------------------------------------------------------------

pub async fn get_state(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<Option<StudyParticipantStateRow>, sqlx::Error> {
    sqlx::query_as!(
        StudyParticipantStateRow,
        r#"SELECT course_id, user_id, stage, current_task_index, consented_at, pre_survey_completed_at, post_survey_completed_at, locked_out_at, created_at, updated_at
        FROM study_participant_state
        WHERE course_id = $1 AND user_id = $2"#,
        course_id,
        user_id,
    )
    .fetch_optional(db)
    .await
}

/// Materialise the participant's row on first contact, in `consent`
/// stage. Idempotent on (course_id, user_id) so repeated calls during
/// the consent screen don't churn the row.
pub async fn get_or_init_state(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<StudyParticipantStateRow, sqlx::Error> {
    sqlx::query_as!(
        StudyParticipantStateRow,
        r#"INSERT INTO study_participant_state (course_id, user_id)
        VALUES ($1, $2)
        ON CONFLICT (course_id, user_id) DO UPDATE SET updated_at = study_participant_state.updated_at
        RETURNING course_id, user_id, stage, current_task_index, consented_at, pre_survey_completed_at, post_survey_completed_at, locked_out_at, created_at, updated_at"#,
        course_id,
        user_id,
    )
    .fetch_one(db)
    .await
}

/// Record consent and transition to `pre_survey`. Idempotent on the
/// stage transition (a second click stays in `pre_survey`).
pub async fn record_consent(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<StudyParticipantStateRow, sqlx::Error> {
    sqlx::query_as!(
        StudyParticipantStateRow,
        r#"UPDATE study_participant_state
        SET stage = 'pre_survey',
            consented_at = COALESCE(consented_at, NOW()),
            updated_at = NOW()
        WHERE course_id = $1 AND user_id = $2 AND stage IN ('consent', 'pre_survey')
        RETURNING course_id, user_id, stage, current_task_index, consented_at, pre_survey_completed_at, post_survey_completed_at, locked_out_at, created_at, updated_at"#,
        course_id,
        user_id,
    )
    .fetch_one(db)
    .await
}

/// Pre-survey complete -> task 0. Idempotent if already past.
pub async fn advance_to_first_task(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<StudyParticipantStateRow, sqlx::Error> {
    sqlx::query_as!(
        StudyParticipantStateRow,
        r#"UPDATE study_participant_state
        SET stage = 'task',
            current_task_index = 0,
            pre_survey_completed_at = COALESCE(pre_survey_completed_at, NOW()),
            updated_at = NOW()
        WHERE course_id = $1 AND user_id = $2 AND stage IN ('pre_survey', 'task')
        RETURNING course_id, user_id, stage, current_task_index, consented_at, pre_survey_completed_at, post_survey_completed_at, locked_out_at, created_at, updated_at"#,
        course_id,
        user_id,
    )
    .fetch_one(db)
    .await
}

/// Advance from task `i` to task `i+1` if there is one, or to
/// `post_survey` if the participant just finished the last task. The
/// caller passes `total_tasks` so this query doesn't have to re-read
/// `study_courses.number_of_tasks`. Stage gating is in the WHERE
/// clause: a participant who is not in `task` stage at the right
/// index gets a no-op (returns Ok(None)); the route handler interprets
/// that as "request didn't change anything; refresh from /state".
pub async fn advance_after_task(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
    finished_task_index: i32,
    total_tasks: i32,
) -> Result<Option<StudyParticipantStateRow>, sqlx::Error> {
    let next_index = finished_task_index + 1;
    let (next_stage, next_task_index) = if next_index >= total_tasks {
        ("post_survey", finished_task_index) // index becomes inert past last task
    } else {
        ("task", next_index)
    };

    sqlx::query_as!(
        StudyParticipantStateRow,
        r#"UPDATE study_participant_state
        SET stage = $3,
            current_task_index = $4,
            updated_at = NOW()
        WHERE course_id = $1
          AND user_id = $2
          AND stage = 'task'
          AND current_task_index = $5
        RETURNING course_id, user_id, stage, current_task_index, consented_at, pre_survey_completed_at, post_survey_completed_at, locked_out_at, created_at, updated_at"#,
        course_id,
        user_id,
        next_stage,
        next_task_index,
        finished_task_index,
    )
    .fetch_optional(db)
    .await
}

/// Post-survey complete -> done + locked_out_at set. Idempotent.
pub async fn advance_to_done(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<StudyParticipantStateRow, sqlx::Error> {
    sqlx::query_as!(
        StudyParticipantStateRow,
        r#"UPDATE study_participant_state
        SET stage = 'done',
            post_survey_completed_at = COALESCE(post_survey_completed_at, NOW()),
            locked_out_at = COALESCE(locked_out_at, NOW()),
            updated_at = NOW()
        WHERE course_id = $1 AND user_id = $2 AND stage IN ('post_survey', 'done')
        RETURNING course_id, user_id, stage, current_task_index, consented_at, pre_survey_completed_at, post_survey_completed_at, locked_out_at, created_at, updated_at"#,
        course_id,
        user_id,
    )
    .fetch_one(db)
    .await
}

// ---------------------------------------------------------------------------
// Per-task conversations
// ---------------------------------------------------------------------------

/// Returns the existing per-task conversation if one exists, else
/// creates a fresh `conversations` row, registers the mapping, and
/// returns the new conversation_id. Two-step (no single SQL CTE)
/// because `conversations` lives in the foreign schema and we want
/// the application's `conversations::create` semantics (e.g.
/// future-default columns) to remain the single source of truth.
pub async fn get_or_create_task_conversation(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
    task_index: i32,
) -> Result<StudyTaskConversationRow, sqlx::Error> {
    if let Some(row) = sqlx::query_as!(
        StudyTaskConversationRow,
        r#"SELECT id, course_id, user_id, task_index, conversation_id, started_at, marked_done_at
        FROM study_task_conversations
        WHERE course_id = $1 AND user_id = $2 AND task_index = $3"#,
        course_id,
        user_id,
        task_index,
    )
    .fetch_optional(db)
    .await?
    {
        return Ok(row);
    }

    let mut tx = db.begin().await?;
    let conversation_id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO conversations (id, course_id, user_id) VALUES ($1, $2, $3)",
        conversation_id,
        course_id,
        user_id,
    )
    .execute(&mut *tx)
    .await?;

    let row = sqlx::query_as!(
        StudyTaskConversationRow,
        r#"INSERT INTO study_task_conversations (course_id, user_id, task_index, conversation_id)
        VALUES ($1, $2, $3, $4)
        RETURNING id, course_id, user_id, task_index, conversation_id, started_at, marked_done_at"#,
        course_id,
        user_id,
        task_index,
        conversation_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(row)
}

pub async fn list_task_conversations_for_user(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<StudyTaskConversationRow>, sqlx::Error> {
    sqlx::query_as!(
        StudyTaskConversationRow,
        r#"SELECT id, course_id, user_id, task_index, conversation_id, started_at, marked_done_at
        FROM study_task_conversations
        WHERE course_id = $1 AND user_id = $2
        ORDER BY task_index ASC"#,
        course_id,
        user_id,
    )
    .fetch_all(db)
    .await
}

pub async fn mark_task_done(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
    task_index: i32,
) -> Result<Option<StudyTaskConversationRow>, sqlx::Error> {
    sqlx::query_as!(
        StudyTaskConversationRow,
        r#"UPDATE study_task_conversations
        SET marked_done_at = COALESCE(marked_done_at, NOW())
        WHERE course_id = $1 AND user_id = $2 AND task_index = $3
        RETURNING id, course_id, user_id, task_index, conversation_id, started_at, marked_done_at"#,
        course_id,
        user_id,
        task_index,
    )
    .fetch_optional(db)
    .await
}

// ---------------------------------------------------------------------------
// Gate evaluation
// ---------------------------------------------------------------------------

/// "messages_only" gate: at least one user-role message exists in the
/// task's conversation. Under forced-on Aegis (study mode short-
/// circuits `aegis_enabled` to true), every user turn also produces an
/// `aegis_prompt_analyses` row, so this implicitly proves Aegis ran;
/// no separate engagement table is needed for the eval.
pub async fn count_user_messages_in_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let n: Option<i64> = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM messages WHERE conversation_id = $1 AND role = 'user'",
        conversation_id,
    )
    .fetch_one(db)
    .await?;
    Ok(n.unwrap_or(0))
}

// ---------------------------------------------------------------------------
// Admin views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ParticipantProgressRow {
    pub user_id: Uuid,
    pub eppn: Option<String>,
    pub display_name: Option<String>,
    pub stage: String,
    pub current_task_index: i32,
    pub consented_at: Option<chrono::DateTime<chrono::Utc>>,
    pub pre_survey_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub post_survey_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub locked_out_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_participants_with_stages(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<ParticipantProgressRow>, sqlx::Error> {
    sqlx::query_as!(
        ParticipantProgressRow,
        r#"SELECT s.user_id, u.eppn AS "eppn?", u.display_name, s.stage, s.current_task_index,
            s.consented_at, s.pre_survey_completed_at, s.post_survey_completed_at, s.locked_out_at
        FROM study_participant_state s
        JOIN users u ON u.id = s.user_id
        WHERE s.course_id = $1
        ORDER BY s.consented_at ASC NULLS LAST, s.created_at ASC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

// ---------------------------------------------------------------------------
// Export support
// ---------------------------------------------------------------------------

/// One row per participant who has at least consented; ordered by
/// `consented_at ASC` so the export route can assign sequential
/// participant IDs deterministically.
pub async fn list_participants_for_export(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<StudyParticipantStateRow>, sqlx::Error> {
    sqlx::query_as!(
        StudyParticipantStateRow,
        r#"SELECT course_id, user_id, stage, current_task_index, consented_at, pre_survey_completed_at, post_survey_completed_at, locked_out_at, created_at, updated_at
        FROM study_participant_state
        WHERE course_id = $1 AND consented_at IS NOT NULL
        ORDER BY consented_at ASC, user_id ASC"#,
        course_id,
    )
    .fetch_all(db)
    .await
}

/// All messages for one conversation, raw shape (no pseudonym
/// rewriting); the export bypasses pseudonymisation by design so
/// researchers see real eppns + content.
#[derive(Debug, Clone)]
pub struct ExportMessageRow {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub model_used: Option<String>,
    pub tokens_prompt: Option<i32>,
    pub tokens_completion: Option<i32>,
    pub generation_ms: Option<i32>,
    pub retrieval_count: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn export_messages_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<ExportMessageRow>, sqlx::Error> {
    sqlx::query_as!(
        ExportMessageRow,
        r#"SELECT id, role, content, model_used, tokens_prompt, tokens_completion, generation_ms, retrieval_count, created_at
        FROM messages
        WHERE conversation_id = $1
        ORDER BY created_at ASC"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}

#[derive(Debug, Clone)]
pub struct ExportPromptAnalysisRow {
    pub message_id: Uuid,
    pub suggestions: serde_json::Value,
    pub mode: String,
    pub model_used: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Aegis prompt-analyses for one conversation, joined through the
/// conversation's messages so the export ships everything Aegis
/// produced for the participant's task. Each row corresponds to one
/// user turn (analyzer is best-effort; some rows may be missing if
/// the analyzer soft-failed for that turn).
pub async fn export_prompt_analyses_for_conversation(
    db: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<ExportPromptAnalysisRow>, sqlx::Error> {
    sqlx::query_as!(
        ExportPromptAnalysisRow,
        r#"SELECT pa.message_id, pa.suggestions, pa.mode, pa.model_used, pa.created_at
        FROM prompt_analyses pa
        JOIN messages m ON m.id = pa.message_id
        WHERE m.conversation_id = $1
        ORDER BY pa.created_at ASC"#,
        conversation_id,
    )
    .fetch_all(db)
    .await
}

/// All survey responses one participant submitted to one survey.
/// The export joins these against the survey's questions so the
/// JSONL line carries `(question_prompt, kind, value)` tuples
/// rather than naked `question_id`s the researcher would have to
/// re-join.
#[derive(Debug, Clone)]
pub struct ExportSurveyResponseRow {
    pub question_id: Uuid,
    pub question_prompt: String,
    pub question_kind: String,
    pub question_ord: i32,
    pub likert_value: Option<i32>,
    pub free_text_value: Option<String>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

pub async fn export_responses_for_user_in_survey(
    db: &PgPool,
    survey_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<ExportSurveyResponseRow>, sqlx::Error> {
    sqlx::query_as!(
        ExportSurveyResponseRow,
        r#"SELECT
            r.question_id,
            q.prompt AS question_prompt,
            q.kind   AS question_kind,
            q.ord    AS question_ord,
            r.likert_value,
            r.free_text_value,
            r.submitted_at
        FROM study_survey_responses r
        JOIN study_survey_questions q ON q.id = r.question_id
        WHERE r.survey_id = $1 AND r.user_id = $2
        ORDER BY q.ord ASC"#,
        survey_id,
        user_id,
    )
    .fetch_all(db)
    .await
}
