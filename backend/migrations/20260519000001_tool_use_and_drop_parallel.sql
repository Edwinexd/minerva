-- Adds the orthogonal `tool_use_enabled` toggle on courses, and retires
-- the legacy `parallel` generation strategy.
--
-- Tool use is a per-course flag, orthogonal to strategy. When TRUE,
-- the model gains access to a tool catalog during a "research" phase
-- (visible as thinking) that precedes a clean single-pass writeup.
-- Strategy choice (`simple` vs `flare`) only determines which retrieval
-- signals run during the research phase:
--   * simple + tools = model-initiated tool calls only
--   * flare  + tools = tool calls + FLARE's logprob-driven implicit retrieval
--
-- Defaults to FALSE so existing courses keep their current behaviour
-- (legacy `simple` or `flare` paths, no research/writeup split).
ALTER TABLE courses
    ADD COLUMN tool_use_enabled BOOLEAN NOT NULL DEFAULT FALSE;

-- `parallel` is retired: it streamed without RAG context for first-token
-- latency, then aborted and restarted with retrieved chunks. In practice
-- that restart is uglier than waiting for RAG, and any course that
-- benefited from it can get a better story from `simple` (clean) or
-- `flare` (forward-looking). Remap existing rows and shift the default.
UPDATE courses SET strategy = 'simple' WHERE strategy = 'parallel';
ALTER TABLE courses ALTER COLUMN strategy SET DEFAULT 'simple';
