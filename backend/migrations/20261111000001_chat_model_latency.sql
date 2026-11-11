-- One row per streamed chat answer: how long the model took to show the
-- first token and how fast it wrote the rest. The teacher's model picker
-- shows medians over the last week, so a slow model is visible before a
-- course switches to it. Rows are keyed by the model and provider that
-- actually served the call, so a fallback route is measured as itself.
-- Nothing ties a row to a user, course or conversation.

CREATE TABLE chat_model_latency (
    id BIGSERIAL PRIMARY KEY,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Request sent to first visible token, reasoning included: the wait
    -- a student sees.
    ttft_ms INTEGER NOT NULL CHECK (ttft_ms >= 0),
    -- First generated token (reasoning or text) to end of stream.
    decode_ms INTEGER NOT NULL CHECK (decode_ms >= 0),
    -- Tokens generated in that window.
    output_tokens INTEGER NOT NULL CHECK (output_tokens >= 0),
    prompt_tokens INTEGER NOT NULL CHECK (prompt_tokens >= 0)
);

CREATE INDEX chat_model_latency_model_recorded_at
    ON chat_model_latency (model, recorded_at);
CREATE INDEX chat_model_latency_recorded_at
    ON chat_model_latency (recorded_at);
