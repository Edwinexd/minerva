-- Duration of the research phase in milliseconds (wall-clock from
-- entering `research_phase::run` until it returns). Persisted so the
-- frontend's "Thinking" disclosure can show "Thought for Ns" on each
-- past message, not just on the one currently streaming. NULL for
-- legacy single-pass messages and for tool-use messages where the
-- model didn't actually do any research (no tool calls, no FLARE
-- injections) ; matches the `thinking_transcript` / `tool_events`
-- gating in `tool_use::run`.
ALTER TABLE messages ADD COLUMN thinking_ms INT;
