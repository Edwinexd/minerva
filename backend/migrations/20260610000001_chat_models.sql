-- Admin-managed catalog of chat / utility LLM models, mirroring the
-- embedding_models / reranker_models pattern but adding the provider
-- reference and per-model pricing.
--
-- A model's *provider* (cerebras | openai | anthropic | groq | ...) is
-- referenced here, but provider credentials + base URLs stay in
-- env/secret (CEREBRAS_API_KEY, OPENAI_API_KEY, ...); no secrets in
-- Postgres. The provider id maps to an entry in the runtime LlmRegistry.
--
-- Pricing is admin-entered USD per million tokens. NULL vs 0 are
-- load-bearing and different:
--   * NULL  = "we don't know the cost" -> the model is unusable (cannot
--             be enabled / default / utility-default / course-selected;
--             billing against it is a hard error, never a silent $0).
--   * 0     = "genuinely free" (on-prem / self-hosted) -> a valid,
--             usable, billable-at-$0 state.
-- The chat_models_enabled_requires_price CHECK refuses enabled = TRUE
-- while either rate is NULL; the admin must enter a real number first
-- (0 is allowed, NULL is not).

CREATE TABLE chat_models (
    model                TEXT PRIMARY KEY,        -- provider's model id, e.g. "gpt-4o-mini"
    provider             TEXT NOT NULL,           -- registry id: cerebras|openai|anthropic|groq|...
    display_name         TEXT NOT NULL,
    enabled              BOOLEAN NOT NULL DEFAULT FALSE,
    is_default           BOOLEAN NOT NULL DEFAULT FALSE,  -- the course-chat default
    is_utility_default   BOOLEAN NOT NULL DEFAULT FALSE,  -- classification / KG / aegis default
    input_usd_per_mtok   NUMERIC(12,6),           -- NULL = unknown (unusable); 0 = genuinely free
    output_usd_per_mtok  NUMERIC(12,6),           -- NULL = unknown (unusable); 0 = genuinely free
    supports_logprobs    BOOLEAN NOT NULL DEFAULT FALSE,
    supports_tool_use    BOOLEAN NOT NULL DEFAULT FALSE,
    price_source_url     TEXT,                     -- last scrape source, audit
    price_updated_at     TIMESTAMPTZ,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- a model may only be enabled once both rates are known (0 is fine, NULL is not)
    CONSTRAINT chat_models_enabled_requires_price
        CHECK (NOT enabled OR (input_usd_per_mtok IS NOT NULL AND output_usd_per_mtok IS NOT NULL))
);

-- At most one course-chat default and at most one utility default.
CREATE UNIQUE INDEX chat_models_single_default
    ON chat_models ((is_default)) WHERE is_default = TRUE;
CREATE UNIQUE INDEX chat_models_single_utility_default
    ON chat_models ((is_utility_default)) WHERE is_utility_default = TRUE;

-- Seed the existing Cerebras model enabled + default + utility-default,
-- with its documented Cerebras pay-per-token price pinned at
-- migration-write time ($0.35 / $0.75 per 1M input / output tokens,
-- verified June 2026). Concrete (non-NULL) rates are required so the
-- row satisfies the enabled-requires-price CHECK and the first deploy
-- has a working, priced default before the catalog seed runs. These are
-- also the rates the token-to-USD limit backfill reads (see the
-- cost-limits migration), so there is no separate reference constant.
INSERT INTO chat_models (
    model, provider, display_name, enabled, is_default, is_utility_default,
    input_usd_per_mtok, output_usd_per_mtok, supports_logprobs, supports_tool_use
) VALUES (
    'gpt-oss-120b', 'cerebras', 'GPT-OSS 120B (Cerebras)', TRUE, TRUE, TRUE,
    0.35, 0.75, TRUE, TRUE
) ON CONFLICT (model) DO NOTHING;
