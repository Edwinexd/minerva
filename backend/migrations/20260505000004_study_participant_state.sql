-- Per-participant study progress. (course_id, user_id) is the natural
-- key because participation reuses the existing course_members flow:
-- joining the course via invite link enrolls them, the first hit on
-- /api/courses/.../study/state lazily inserts the row in 'consent'
-- stage. There's no separate study_participants table -- everyone with
-- a course_members row in a study course IS a potential participant,
-- and this row materialises only once they hit the study landing.
--
-- `stage` is the linear pipeline state machine; the application
-- enforces the legal transitions. `current_task_index` is only
-- meaningful while stage = 'task' and is otherwise read-as-zero.
-- The four `*_at` timestamps are completion markers for downstream
-- analysis (and `consented_at` is the deterministic ordering key for
-- export-time participant_id assignment).
--
-- ON DELETE CASCADE on both FKs follows the feature_flags pattern:
-- removing a course or user cleans up their study state automatically.

CREATE TABLE study_participant_state (
    course_id                  UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    user_id                    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    stage                      TEXT NOT NULL DEFAULT 'consent'
        CHECK (stage IN ('consent', 'pre_survey', 'task', 'post_survey', 'done')),
    current_task_index         INTEGER NOT NULL DEFAULT 0 CHECK (current_task_index >= 0),
    consented_at               TIMESTAMPTZ,
    pre_survey_completed_at    TIMESTAMPTZ,
    post_survey_completed_at   TIMESTAMPTZ,
    locked_out_at              TIMESTAMPTZ,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (course_id, user_id)
);

CREATE INDEX idx_study_participant_state_course_stage
    ON study_participant_state(course_id, stage);

CREATE INDEX idx_study_participant_state_consented
    ON study_participant_state(course_id, consented_at)
    WHERE consented_at IS NOT NULL;
