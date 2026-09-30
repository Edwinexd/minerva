-- Course start / end dates from Daisy, per offering.
--
-- `semester_label` only says which term an offering belongs to; Daisy
-- also knows the exact period it runs (e.g. 2026-08-31 to 2026-09-30
-- for a first-period HT course). The daily sync now relays both dates.
-- They live on the offering, not on `courses`, because a merged course
-- can carry several offerings with different periods.
--
-- Nullable on both tables: Daisy leaves the period blank on some
-- offerings, and rows written before this migration have no dates until
-- the next sync fills them in.

ALTER TABLE course_daisy_offerings
    ADD COLUMN start_date DATE,
    ADD COLUMN end_date   DATE;

-- Staged snapshot of the same two fields, so an admin Apply writes the
-- dates the sync saw.
ALTER TABLE daisy_pending_imports
    ADD COLUMN daisy_start_date DATE,
    ADD COLUMN daisy_end_date   DATE;
