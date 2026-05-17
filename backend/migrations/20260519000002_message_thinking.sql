-- Persist the research-phase output alongside each assistant message
-- so the "Thinking" disclosure survives a page refresh.
--
-- `thinking_transcript` is the concatenated `thinking_token` stream
-- the model emitted during research (Markdown-ish text). `tool_events`
-- is the ordered list of `{name, args, result_summary}` triples,
-- serialised as JSONB so the frontend can render it without re-parsing
-- prose. Both nullable; legacy (`tool_use_enabled = FALSE`) messages
-- leave them NULL and the UI renders no disclosure.
ALTER TABLE messages
    ADD COLUMN thinking_transcript TEXT,
    ADD COLUMN tool_events JSONB;
