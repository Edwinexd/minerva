-- Course code as a first-class column.
--
-- Previously this lived only as the `play_designations.designation`
-- row the Daisy auto-import seeded, so the course itself carried no
-- stable short identifier; the human-facing course `name` (e.g.
-- "Programmering 2") isn't unique across years, and a Daisy-side
-- rename would break any UI / query keyed on it.
--
-- New shape: `courses.course_code` holds the canonical code
-- (`PROG2`, `IDSV`, `INFOC`, ...). Daisy refers to this as
-- *beteckning*; we keep that name only in the `DaisyCourseInput`
-- bridge that mirrors dsv-wrapper, and translate to English at the
-- DB / API boundary. Nullable so historical / manually-created
-- courses without a Daisy linkage stay valid; backfilled by the
-- auto-import for every offering it touches.

ALTER TABLE courses
    ADD COLUMN course_code TEXT;

-- Lookup helper for the "show course code on the My Courses tile"
-- use case + admin filters by code. Partial so the index only
-- covers the rows that actually carry a value.
CREATE INDEX idx_courses_course_code
    ON courses (course_code)
    WHERE course_code IS NOT NULL;
