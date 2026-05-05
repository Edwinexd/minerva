-- Study surveys: pre- and post-task questionnaires. Each course has at
-- most one survey of each kind (UNIQUE(course_id, kind)); editing the
-- questions of an active survey is allowed but the admin UI warns when
-- responses already exist, since responses reference question_id and
-- deleting a question would orphan its responses (FK CASCADE deletes
-- them on purpose -- if the researcher removed the question the data
-- isn't usable anyway).
--
-- Two question kinds for now: 'likert' (integer scale, configurable
-- min/max + endpoint labels) and 'free_text' (unbounded TEXT). Other
-- kinds (multiple choice, ranking) can be added by extending the CHECK
-- list and the response columns; out of scope for the current eval.
--
-- Responses are keyed UNIQUE(survey_id, user_id, question_id) so a
-- repeat submission UPSERTs rather than appends. The CHECK ensures the
-- response value matches the question kind (exactly one of the two
-- value columns is set) -- belt-and-braces alongside the application's
-- own validation.

CREATE TABLE study_surveys (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    course_id  UUID NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('pre', 'post')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (course_id, kind)
);

CREATE TABLE study_survey_questions (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    survey_id        UUID NOT NULL REFERENCES study_surveys(id) ON DELETE CASCADE,
    ord              INTEGER NOT NULL CHECK (ord >= 0),
    kind             TEXT NOT NULL CHECK (kind IN ('likert', 'free_text')),
    prompt           TEXT NOT NULL,
    likert_min       INTEGER,
    likert_max       INTEGER,
    likert_min_label TEXT,
    likert_max_label TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (survey_id, ord),
    CHECK (
        (kind = 'likert'
            AND likert_min IS NOT NULL
            AND likert_max IS NOT NULL
            AND likert_max > likert_min)
        OR
        (kind = 'free_text'
            AND likert_min IS NULL
            AND likert_max IS NULL
            AND likert_min_label IS NULL
            AND likert_max_label IS NULL)
    )
);

CREATE INDEX idx_study_survey_questions_survey ON study_survey_questions(survey_id);

CREATE TABLE study_survey_responses (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    survey_id       UUID NOT NULL REFERENCES study_surveys(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    question_id     UUID NOT NULL REFERENCES study_survey_questions(id) ON DELETE CASCADE,
    likert_value    INTEGER,
    free_text_value TEXT,
    submitted_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (survey_id, user_id, question_id),
    CHECK (
        (likert_value IS NOT NULL AND free_text_value IS NULL)
        OR
        (likert_value IS NULL AND free_text_value IS NOT NULL)
    )
);

CREATE INDEX idx_study_survey_responses_survey_user
    ON study_survey_responses(survey_id, user_id);
