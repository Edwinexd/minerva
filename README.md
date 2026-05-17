# Minerva

RAG platform for educational use at DSV, Stockholm University. Teachers upload course materials; students get an AI assistant grounded in those documents, with safeguards designed to support learning.

![Course list](docs/screenshots/01-home-courses.png)

## Features

- **Two RAG strategies**: `simple` (one-shot retrieve-then-answer) and `FLARE` (multi-turn, logprob-triggered mid-stream retrieval). The legacy `parallel` strategy was retired in favour of the tool-use axis below.
- **Optional agentic phase**: orthogonal `tool_use_enabled` toggle on each course. When on, generation splits into a hidden-thinking research phase (model calls `keyword_search` / RAG seed / KG-expansion tools, with FLARE's logprob signal injected as a tool event) followed by a clean writeup phase. Per-tool expandable results, research thinking, and a research/writeup token-split are persisted alongside the message and rendered above the assistant bubble.
- **Inline citations + sources panel**: assistant replies get inline `[n]` badges (accepts naked-digit and filename-form variants); a right-rail sources panel shows what was actually cited, with a count button that resets the uncited-override on fresh-open.
- **Course knowledge graph**: documents are auto-classified (lecture, transcript, exercise, solution, ...) on `llama3.1-8b` and cross-linked with `part_of_unit` / `solution_of` / `prerequisite_of` / `applied_in` edges. Retrieval expands top-k along the graph. Gated by the `course_kg` feature flag.
- **Aegis prompt coaching**: per-keystroke live analyzer (`llama3.1-8b`, JSON-schema-strict) that returns 0..=2 tagged suggestions for the draft the student is typing, severity-coloured against an 8-kind CLEAR-grounded rubric, with a Beginner/Expert calibration toggle. Soft-blocks Send when suggestions are present; `Use ideas` rewrites the draft via `gpt-oss-120b`. Mobile drawer + tablet support; per-iteration history persisted for export. Gated by the `aegis` feature flag.
- **Extraction guard**: separate from Aegis. Per-turn intent classifier + per-chunk output check (`llama3.1-8b`) + Socratic rewriter (`gpt-oss-120b`), with KG-driven multi-turn proximity tracking. Gated by the `extraction_guard` feature flag.
- **Pluggable embeddings**: admin-managed catalog (Snowflake arctic-embed-m-v2.0 default, BGE, BAAI, GTE, mxbai, EmbeddingGemma, multilingual-e5, Qwen3-Embedding, OpenAI) with memory-budgeted LRU cache and on-demand benchmarks. Per-course rotation via lazy re-embed against versioned Qdrant collections.
- **Daily AI spending caps**: per-student-per-course and per-owner aggregate, both daily. Chat returns 429 with the optimistic bubble preserved and the real error surfaced.
- **Conversations UX**: theme toggle (light/dark/system) in the header, fresh new-chat as default landing, LLM-grounded suggested questions on the empty state (drawn from the three latest sources), bidirectional unread + explicit acknowledgements, frozen pins for owners, sticky teacher unreviewed-tab list.
- **LMS integration**: Moodle local plugin (iframe + enrolment sync + MBZ import), site-level Moodle/Canvas LTI 1.3 with first-launch course binding, Canvas REST sync.
- **DSV Play transcript pipeline**: hourly VTT fetch + index for play.dsv.su.se URLs; teacher-configurable Play designation codes drive automatic discovery of new lecture recordings.
- **Auth**: Shibboleth (SAML) primary; HMAC-signed external-auth invites validated entirely inside Apache via `mod_lua`; attribute-based role auto-promotion rules.
- **Study mode**: per-course pipeline (consent screen, pre-/post-surveys, hardcoded tasks with per-round Aegis on/off, lockout + JSONL export). Anonymous participant view + detail drill-in + GDPR delete. Gated by the `study_mode` flag; forces Aegis on for the duration.
- **Privacy & i18n**: pseudonymisation for `ext:` users, in-app data-handling ack, English + Swedish, WCAG 2.1 AA fixes.

## Architecture

![System overview](docs/diagrams/system-overview.svg)

Detail figures for the document-ingest and chat/RAG pipelines (including the FLARE multi-turn loop): [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Screenshots

| | |
|---|---|
| ![Course list](docs/screenshots/01-home-courses.png) | ![Chat](docs/screenshots/02-chat-new.png) |
| ![Teacher config](docs/screenshots/03-teacher-course-config.png) | ![Embedding catalog](docs/screenshots/04-admin-system-embedding.png) |
| ![Admin courses](docs/screenshots/05-admin-courses.png) | ![Admin users](docs/screenshots/06-admin-users.png) |
| ![Role rules](docs/screenshots/07-admin-rules.png) | ![Acknowledgements](docs/screenshots/08-acknowledgements.png) |

Regenerate with `docs/screenshots/regenerate.mjs` (see [docs/screenshots/README.md](docs/screenshots/README.md)).

## Tech stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust (Axum, SQLx, Tokio) |
| Frontend | React 19, TypeScript 6, Vite, TanStack Router/Query, Tailwind 4, react-force-graph-2d, i18next 26 |
| Frontend runtime | Node 26 (Alpine) in Docker |
| Database | PostgreSQL 16 |
| Vector DB | Qdrant (per-course versioned collections) |
| LLM | Cerebras (default; `llama3.1-8b` for classifiers + Aegis, `gpt-oss-120b` for rewrites + writeup) or any OpenAI-compatible endpoint |
| Embeddings | OpenAI or local fastembed (memory-budgeted LRU cache, HuggingFace cache persisted on `/data0` in prod) |
| Edge | Apache 2 with `mod_shib` + `mod_lua` |

## Getting started

```bash
cp .env.example .env  # add CEREBRAS_API_KEY, OPENAI_API_KEY
docker compose up
```

Backend on `:3000`, frontend dev on `:5173`. With `MINERVA_DEV_MODE=true` (compose default) Shibboleth is bypassed; the backend reads `X-Dev-User` and falls back to the first admin in `MINERVA_ADMINS`.

Production:

```bash
docker compose -f docker-compose.prod.yml up -d
# or
docker pull ghcr.io/edwinexd/minerva:master
```

For the k3s production layout used at DSV, see `k8s/`.

## Environment variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `QDRANT_URL` | Qdrant gRPC endpoint |
| `MINERVA_HMAC_SECRET` | Signs embed/invite/LTI tokens; mirrored to Apache for `mod_lua` |
| `MINERVA_ADMINS` | Comma-separated admin eppn prefixes |
| `MINERVA_DOCS_PATH` | Document storage path |
| `CEREBRAS_API_KEY` | Inference key |
| `OPENAI_API_KEY` | Embedding key (optional with fastembed) |
| `MINERVA_BASE_URL` | Public base URL for LTI tool URLs |
| `MINERVA_LTI_KEY_SEED` | RSA seed for LTI 1.3 (falls back to HMAC secret) |
| `MINERVA_SERVICE_API_KEY` | Bearer for `/api/service/*` pipelines |
| `MINERVA_DEV_MODE` | `true` bypasses Shibboleth |
| `MINERVA_DEFAULT_COURSE_DAILY_TOKEN_LIMIT` | Per-student-per-course default (`0` = unlimited) |
| `MINERVA_DEFAULT_OWNER_DAILY_TOKEN_LIMIT` | Per-owner aggregate default (`0` = unlimited) |
| `MINERVA_CANVAS_AUTO_SYNC_INTERVAL_HOURS` | Canvas re-sync interval |

See [.env.example](.env.example) for the rest.

## Auth surfaces

| Path prefix | Auth | Why |
|-------------|------|-----|
| `/api/integration/*` | Per-course API key | Moodle server-to-server |
| `/api/service/*` | Global service API key | Automated pipelines |
| `/api/embed/*`, `/embed/*` | HMAC-signed embed token | Iframe chat |
| `/lti/*` | LTI 1.3 (OIDC + JWT) | LMS-driven login |
| `/api/external-auth/*` | HMAC-signed invite token | External-auth callback |
| `/embedding-catalog` | Public read-only | Teacher feed of enabled models |
| everything else | Shibboleth | Default |

See [apache/README.md](apache/README.md) for the vhost.

## Contributing

CLA in [CLA.md](CLA.md). CI runs:

- **Backend**: `cargo fmt`, `cargo clippy --all-targets` (warnings treated as errors), `cargo build` (all with `SQLX_OFFLINE=true`).
- **Frontend**: `eslint --max-warnings 0`, `tsc -b`, `vite build`.
- **Moodle plugin**: `php -l` + `phpcs` against `moodlehq/moodle-cs`.
- **Apache**: lua syntax + unit tests + `apache2ctl configtest` for `apache/minerva-app.conf`.
- **Style gates**: ban emdashes + ban space-dash-dash-space anywhere a non-whitespace char precedes them on the line.
- **Migrations**: `migrations-immutable` blocks edits to already-committed `backend/migrations/*.sql` files (sqlx content-hashes them at startup).

Pre-commit mirrors the same set; install with `pre-commit install` (the hook is wired via `pipx install pre-commit`).

After editing any `sqlx::query!` / `query_as!` macro:

```bash
docker compose up -d postgres
cd backend && DATABASE_URL=postgres://minerva:minerva@localhost:5432/minerva \
    cargo sqlx prepare --workspace
git add .sqlx/
```

The committed `backend/.sqlx/` cache is what CI and the prod Dockerfile build against; forgetting this step fails locally in the pre-commit `cargo check`/`clippy` gate.

## License

[AGPL-3.0](LICENSE). Logo by Tilly Makrof-Johansson.
