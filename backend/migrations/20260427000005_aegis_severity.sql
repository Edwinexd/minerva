-- Aegis: backfill `severity` on existing prompt_analyses suggestions.
-- The frontend now colours each suggestion card by severity (high/
-- medium/low), and the analyzer's response schema includes the field.
-- Old rows wrote suggestions without it; without a backfill, those
-- rows would render as "missing severity -> default colour" which
-- looks like a bug.
--
-- The `kind` enum widened in the analyzer's response schema too
-- (added clarity / rationale / audience / format / tasks /
-- instruction / examples; renamed context -> rationale,
-- specificity -> clarity, alternatives -> examples,
-- clarification -> clarity). The DB layer doesn't enforce kind
-- since the JSONB column is opaque, so the rename only matters on
-- read -- the frontend treats unknown kinds via an i18n
-- defaultValue fallback, so old rows render as their literal
-- string ("context") instead of breaking. We could rewrite old
-- rows to the new vocabulary, but the persisted rows are a
-- few-hours-old artifact at this point; not worth it.

UPDATE prompt_analyses
SET suggestions = (
    SELECT jsonb_agg(
        CASE
            WHEN s ? 'severity'
                THEN s
            ELSE s || jsonb_build_object('severity', 'medium')
        END
    )
    FROM jsonb_array_elements(suggestions) AS s
)
WHERE jsonb_typeof(suggestions) = 'array'
  AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(suggestions) AS s
    WHERE NOT (s ? 'severity')
  );
