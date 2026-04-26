-- Track the last successful KG relink per course so the linker can
-- skip re-evaluating pairs whose endpoints haven't changed.
--
-- Without this, every ingest of a single new doc triggers a full
-- re-evaluation of every candidate pair in the course (potentially
-- ~50-100 Cerebras calls just to confirm relationships we already
-- know about). With it, we compute "dirty docs" as those with
-- `classified_at > last_linked_at` and only LLM-call pairs that
-- involve at least one dirty doc; clean<->clean pairs keep their
-- existing edges untouched.
--
-- NULL means "never linked" -- equivalent to "all docs dirty",
-- which gives the same behaviour as the previous full-wipe path
-- on the very first relink of a brand-new course.

ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS last_linked_at TIMESTAMPTZ;
