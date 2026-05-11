-- Per-task Aegis on/off for study mode.
--
-- The DM2731 / Aegis evaluation runs the same participant through
-- N rounds where some have Aegis support and some don't. Before this
-- column, `feature_flags::aegis_enabled` short-circuited to true under
-- study mode for the whole course, which made off-rounds impossible.
--
-- The flag is per-row on `study_tasks` rather than a side table because
-- it's intrinsic to the task: a researcher describing "round 2 with
-- support" wants it stored next to the task description, and the
-- replace-all `replace_tasks` write path already churns the whole row,
-- so a new column is the minimal join.
--
-- DEFAULT TRUE preserves the prior behaviour (study mode forces Aegis on)
-- for every existing row; the only way to get FALSE is an explicit admin
-- save, the seed preset's per-task value, or a future preset. New columns
-- in the JSONL export will carry the value alongside title/description.

ALTER TABLE study_tasks
    ADD COLUMN aegis_enabled BOOLEAN NOT NULL DEFAULT TRUE;
