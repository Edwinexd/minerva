-- Add daily token limit to courses (0 = unlimited)
ALTER TABLE courses ADD COLUMN daily_token_limit BIGINT NOT NULL DEFAULT 0;

-- Add suspended flag to users
ALTER TABLE users ADD COLUMN suspended BOOLEAN NOT NULL DEFAULT FALSE;
