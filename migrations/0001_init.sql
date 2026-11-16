-- Initial schema. All timestamps are timestamptz (UTC). Usernames are stored lowercase.

CREATE TYPE exam_status AS ENUM ('draft', 'open', 'closed');
CREATE TYPE result_visibility AS ENUM ('none', 'score', 'score_and_answers');
CREATE TYPE answer_type AS ENUM ('single', 'multiple');

-- Generated secrets (session key, setup token).
CREATE TABLE app_settings (
    key   text PRIMARY KEY,
    value bytea NOT NULL
);

CREATE TABLE admins (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username      text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE exams (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    title             text NOT NULL,
    description       text NOT NULL DEFAULT '',
    duration_minutes  integer NOT NULL CHECK (duration_minutes BETWEEN 1 AND 1440),
    status            exam_status NOT NULL DEFAULT 'draft',
    result_visibility result_visibility NOT NULL DEFAULT 'score',
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE questions (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    exam_id     uuid NOT NULL REFERENCES exams(id) ON DELETE CASCADE,
    position    integer NOT NULL,
    text        text NOT NULL,
    image_path  text,
    points      integer NOT NULL DEFAULT 1 CHECK (points BETWEEN 0 AND 1000),
    answer_type answer_type NOT NULL DEFAULT 'single'
);
CREATE INDEX questions_exam_idx ON questions (exam_id, position);

CREATE TABLE options (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    question_id uuid NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    position    integer NOT NULL,
    text        text NOT NULL,
    is_correct  boolean NOT NULL DEFAULT false
);
CREATE INDEX options_question_idx ON options (question_id, position);

CREATE TABLE students (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    exam_id        uuid NOT NULL REFERENCES exams(id) ON DELETE CASCADE,
    full_name      text,
    username       text NOT NULL UNIQUE,
    password_hash  text NOT NULL,
    extra_minutes  integer NOT NULL DEFAULT 0 CHECK (extra_minutes >= 0),
    -- Random id written at each login; older sessions carrying another id are rejected.
    active_session uuid,
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX students_exam_idx ON students (exam_id);

CREATE TABLE attempts (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    student_id    uuid NOT NULL UNIQUE REFERENCES students(id) ON DELETE CASCADE,
    exam_id       uuid NOT NULL REFERENCES exams(id) ON DELETE CASCADE,
    started_at    timestamptz NOT NULL DEFAULT now(),
    deadline      timestamptz NOT NULL,
    submitted_at  timestamptz,
    earned_points integer,
    max_points    integer NOT NULL,
    percentage    double precision,
    -- Number of logins while this attempt was in progress (>1 is flagged on the dashboard).
    login_count   integer NOT NULL DEFAULT 1
);
CREATE INDEX attempts_exam_idx ON attempts (exam_id);
CREATE INDEX attempts_open_deadline_idx ON attempts (deadline) WHERE submitted_at IS NULL;

CREATE TABLE attempt_questions (
    attempt_id       uuid NOT NULL REFERENCES attempts(id) ON DELETE CASCADE,
    question_id      uuid NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    display_position integer NOT NULL,
    option_order     uuid[] NOT NULL,
    PRIMARY KEY (attempt_id, question_id),
    UNIQUE (attempt_id, display_position)
);
CREATE INDEX attempt_questions_question_idx ON attempt_questions (question_id);

CREATE TABLE answer_selections (
    attempt_id  uuid NOT NULL REFERENCES attempts(id) ON DELETE CASCADE,
    question_id uuid NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    option_id   uuid NOT NULL REFERENCES options(id) ON DELETE CASCADE,
    selected_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (attempt_id, question_id, option_id)
);
CREATE INDEX answer_selections_question_idx ON answer_selections (question_id);
CREATE INDEX answer_selections_option_idx ON answer_selections (option_id);
