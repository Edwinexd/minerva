-- Figure description is a catalog model role, selected alongside the
-- course-chat and utility defaults. Preserve any prior admin selection.
ALTER TABLE chat_models
    ADD COLUMN is_figure_description_default BOOLEAN NOT NULL DEFAULT FALSE;

CREATE UNIQUE INDEX chat_models_single_figure_description_default
    ON chat_models ((is_figure_description_default))
    WHERE is_figure_description_default = TRUE;

UPDATE chat_models m
SET is_figure_description_default = TRUE
FROM system_defaults s
WHERE s.key = 'platform.figure_description_model'
  AND s.value #>> '{}' = m.model AND m.enabled
  AND m.provider IN ('openai', 'groq', 'gemini');

DELETE FROM system_defaults WHERE key = 'platform.figure_description_model';
