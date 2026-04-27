# Minerva

RAG platform for educational use at DSV, Stockholm University. Teachers upload course materials; students get an AI assistant grounded in those documents, with safeguards designed to support learning.

![Course list](docs/screenshots/01-home-courses.png)

## Features

- **Three RAG strategies**: `simple`, `parallel` (stream while retrieving), `FLARE` (logprob-triggered mid-stream retrieval).
- **Course knowledge graph**: documents are auto-classified (lecture, transcript, exercise, solution, ...) and cross-linked with `part_of_unit` / `solution_of` / `prerequisite_of` / `applied_in` edges. Retrieval expands top-k along the graph.
- **Extraction guard ("Aegis")**: per-turn intent + per-chunk output classifiers (`llama3.1-8b`), Socratic rewriter (`gpt-oss-120b`), teacher-facing review queue.
- **Pluggable embeddings**: admin-managed catalog (Snowflake arctic-embed, BGE, BAAI, GTE, mxbai, EmbeddingGemma, multilingual-e5, Qwen3-Embedding, OpenAI). Per-course rotation via lazy re-embed against versioned Qdrant collections.
- **Daily AI spending caps**: per-student-per-course and per-owner aggregate, both daily.
- **LMS integration**: Moodle local plugin (iframe + enrolment sync + MBZ import), site-level Moodle/Canvas LTI 1.3 with first-launch course binding, Canvas REST sync.
- **DSV Play transcript pipeline**: hourly VTT fetch + index for play.dsv.su.se URLs.
- **Auth**: Shibboleth (SAML) primary; HMAC-signed external-auth invites validated entirely inside Apache via `mod_lua`; attribute-based role auto-promotion rules.
- **Privacy & i18n**: pseudonymisation for `ext:` users, in-app data-handling ack, English + Swedish, WCAG 2.1 AA fixes.

## Architecture

![System overview](docs/diagrams/system-overview.png)

Apache unsets identity headers `early` outside of `mod_shib` / Lua paths. LMS, iframe, and service-account routes carry their own bearer-token or HMAC-signed-token middleware (see [Auth surfaces](#auth-surfaces)).

### Document ingest

![Ingest pipeline](docs/diagrams/ingest-pipeline.png)

The kind classifier runs *before* chunking, so assignments and solutions can be excluded from prompt context. Embeddings are written to a per-course Qdrant collection versioned by `(course_id, embedding_model)`; re-embedding under a new model is lazy and the old version stays live until rotation finishes. The KG linker reads excerpts and embeddings from Qdrant and caches per-pair decisions.

### Chat / RAG

![Chat pipeline](docs/diagrams/chat-pipeline.png)

Classifiers run on `llama3.1-8b` for latency; the Socratic rewriter on `gpt-oss-120b` for prose quality. Every classifier decision is appended to `conversation_flags` so teachers can audit activations from the "Needs Review" tab.

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
| Frontend | React 19, TypeScript, TanStack Router/Query, Tailwind, react-force-graph-2d, i18next |
| Database | PostgreSQL 16 |
| Vector DB | Qdrant (per-course versioned collections) |
| LLM | Cerebras (default) or any OpenAI-compatible endpoint |
| Embeddings | OpenAI or local fastembed |
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

CLA in [CLA.md](CLA.md). CI runs `cargo fmt`, `clippy`, `nextest`, frontend `tsc`, `eslint`. After editing any `sqlx::query!` macro:

```bash
docker compose up -d postgres
cd backend && DATABASE_URL=postgres://minerva:minerva@localhost:5432/minerva \
    cargo sqlx prepare --workspace
```

## License

[AGPL-3.0](LICENSE). Logo by Tilly Makrof-Johansson.
