# Minerva

Minerva is a retrieval-augmented generation (RAG) platform built for educational use at DSV, Stockholm University. Teachers upload course materials; students get an AI assistant that answers grounded in those documents, with safeguards designed to support learning instead of replacing it.

![Course list](docs/screenshots/01-home-courses.png)

## Highlights

- **Three RAG strategies**: `simple` (retrieve then generate), `parallel` (start streaming and retrieve concurrently), and `FLARE` (logprob-triggered mid-stream retrieval, with capped iterations, idle timeouts, and chunk-dedup guardrails).
- **Course-scoped knowledge graph**: every uploaded document is classified (lecture, lecture transcript, tutorial exercise, solution, assignment, etc.) and cross-linked with `part_of_unit`, `solution_of`, `prerequisite_of`, and `applied_in` edges. Retrieval expands the top-k along the graph; teachers can view and prune the graph from the course config page.
- **Extraction guard ("Aegis")**: per-turn intent classifier + per-chunk output classifier on `llama3.1-8b`, plus a Socratic rewriter on `gpt-oss-120b`. Activations land in a teacher-facing "Needs Review" queue with full per-turn traces.
- **Pluggable embeddings**: admin-managed catalog of fastembed and OpenAI models, including Snowflake `arctic-embed`, BGE, BAAI, GTE, mxbai, EmbeddingGemma, multilingual-e5, and Qwen3-Embedding. Per-course rotation runs as a lazy re-embed against versioned Qdrant collections, so live courses don't go offline during a model swap. A memory-budgeted LRU cache keeps the resident model set inside the pod's RSS budget.
- **Daily AI spending caps** at two layers: per-student-per-course and per-owner aggregate. Breaches return `429` with a friendly pointer to `lambda@dsv.su.se`.
- **LMS integration**: Moodle local plugin (iframe embed + enrolment sync + MBZ backup import), site-level Moodle/Canvas LTI 1.3 registration with first-launch course binding, Canvas REST sync (files / pages / external URLs).
- **DSV Play transcript pipeline**: hourly job pulls VTTs for play.dsv.su.se URLs and indexes them as searchable documents. Teachers can register course designations (`PROG1`, etc.) for automatic discovery.
- **Identity & access**: Shibboleth (SAML) primary; HMAC-signed external-auth invites for non-Shibboleth users (validated entirely inside Apache via `mod_lua`); attribute-based role auto-promotion rules; admin manual-override "lock" semantics.
- **Privacy**: identity pseudonymisation for `ext:` users (deterministic two-word EFF-wordlist names), in-app data-handling disclosure with per-user ack gate, end-to-end i18n (English + Swedish) with persistent override.
- **Teacher tooling**: feedback / correction-note flow, per-message stats (tokens, time, sources, retrieval count), per-course token-usage rollups, RAG debug tab, full document classification + bulk reclassify.
- **Accessibility**: WCAG 2.1 AA fixes across forms, modals, chat streams, and page titles; Swedish + English UI; mobile hamburger sidebar in the embed.

## Screenshots

| | |
|---|---|
| ![Course list](docs/screenshots/01-home-courses.png) | ![Chat](docs/screenshots/02-chat-new.png) |
| Course list with strategy / model / context-ratio at a glance | Student chat with the data-handling ack gate |
| ![Teacher course config](docs/screenshots/03-teacher-course-config.png) | ![Embedding catalog](docs/screenshots/04-admin-system-embedding.png) |
| Per-course config: strategy, embedding model, RAG context ratio, token cap | Admin embedding catalog with on-demand benchmarks |
| ![Admin courses](docs/screenshots/05-admin-courses.png) | ![Admin users](docs/screenshots/06-admin-users.png) |
| Platform admin: courses, owners, embedding model, daily caps | User management with role lock + reset-daily-usage |
| ![Role rules](docs/screenshots/07-admin-rules.png) | ![Acknowledgements](docs/screenshots/08-acknowledgements.png) |
| Attribute-based role auto-promotion rules | Acknowledgements page (logo by Tilly Makrof-Johansson) |

Screenshots are produced from a fresh `docker compose up` plus a Playwright script (`docs/screenshots/regenerate.mjs`); see [docs/screenshots/README.md](docs/screenshots/README.md) to regenerate them.

## How it fits together

### System overview

```mermaid
flowchart LR
  subgraph clients[Clients]
    BR[Web SPA - React + TanStack]
    EMB[Iframe embed]
    MOO[Moodle plugin<br/>local_minerva]
    CV[Canvas LMS]
    LTI[LTI 1.3 platform]
  end

  subgraph edge[Apache edge]
    SH[mod_shib<br/>Shibboleth SSO]
    LU[mod_lua<br/>external-auth invites]
  end

  subgraph app[minerva-app pod]
    API[axum HTTP API]
    WORK[ingest worker]
    KGW[KG linker / sweeper]
    CRON[Canvas + transcript schedulers]
  end

  subgraph data[Stateful]
    PG[(PostgreSQL 16)]
    QD[(Qdrant)]
    DOCS[/data0/minerva/data]
    HF[/HuggingFace<br/>fastembed cache/]
  end

  subgraph ai[External AI]
    CB[Cerebras /<br/>OpenAI-compatible LLM]
    OAI[OpenAI<br/>embeddings]
    PLAY[play.dsv.su.se<br/>VTT transcripts]
  end

  BR --> SH
  EMB --> API
  MOO --> API
  CV --> API
  LTI --> API
  SH --> API
  LU --> API

  API --> PG
  API --> QD
  API --> DOCS
  API --> CB
  WORK --> OAI
  WORK --> HF
  WORK --> QD
  KGW --> CB
  CRON --> CV
  CRON --> PLAY
```

Apache unsets identity headers `early` for any path that does not pass through `mod_shib` or the Lua external-auth hook. Per-route exemptions for LMS / iframe / service-account traffic carry their own bearer-token or HMAC-signed-token middleware in the backend (see [Auth surfaces](#auth-surfaces)).

Detail diagrams: [docs/diagrams/system-overview.md](docs/diagrams/system-overview.md).

### Document ingest pipeline

Triggered by direct upload, Moodle/Canvas sync, MBZ import, or a play.dsv URL drop. The same state machine handles all sources.

```mermaid
flowchart TD
  A[Upload / Moodle / Canvas / MBZ / play.dsv URL] --> B[(documents table<br/>status = pending)]
  B --> W[ingest worker - claims rows]
  W --> R{mime / source}
  R -->|application/pdf, etc.| X[poppler / extractor]
  R -->|text/x-url play.dsv.su.se| AT[awaiting_transcript]
  R -->|text/x-url other| US[unsupported]
  AT --> TP[transcripts.yml hourly cron]
  TP --> X
  X --> CL[adversarial / kind classifier<br/>llama3.1-8b]
  CL --> CH[chunker]
  CH --> EM[embedder<br/>OpenAI or fastembed]
  EM --> Q[(Qdrant - per-course<br/>versioned collection)]
  EM --> KG[KG linker<br/>cross-doc edges]
  KG --> PG[(linker_decisions / kg_state)]
  Q --> READY[status = ready]
```

The classifier runs *before* chunking so assignments and solutions can be tagged and excluded from prompt context. Embeddings are written to a per-course Qdrant collection versioned by `(course_id, embedding_model)`; re-embedding under a new model creates a new collection version and the old one stays live until rotation finishes (lazy re-embed). The KG linker reads excerpts and embeddings *from Qdrant*, never re-parses PDFs, and caches per-pair decisions so untouched pairs aren't re-evaluated.

Detail: [docs/diagrams/ingest-pipeline.md](docs/diagrams/ingest-pipeline.md).

### Chat / RAG pipeline

The three strategies share retrieval, KG expansion, and the extraction-guard layers; they differ only in *when* retrieval happens relative to generation.

```mermaid
flowchart TD
  U[student message] --> EG1{intent classifier<br/>llama3.1-8b}
  EG1 -->|benign| S{strategy}
  EG1 -->|exfil intent| LIFT[kg_state - refusal lift]
  LIFT --> S

  S -->|simple| R1[embed query -> Qdrant top-k]
  S -->|parallel| P1[start LLM stream + retrieve concurrently]
  S -->|FLARE| F1[generate sentence -> low-logprob trigger -> retrieve]

  R1 --> KG[KG expansion<br/>part_of_unit / applied_in partners]
  P1 --> KG
  F1 --> KG

  KG --> CTX[assemble prompt<br/>system + KG-expanded chunks +<br/>conversation history]
  CTX --> LLM[Cerebras / OpenAI-compatible<br/>SSE stream]

  LLM --> EG2{output classifier<br/>per chunk}
  EG2 -->|clean| OUT[stream to student]
  EG2 -->|over-extraction| RW[Socratic rewrite<br/>gpt-oss-120b]
  RW --> OUT

  OUT --> LOG[(conversation_flags +<br/>course_token_usage)]
  LOG --> CAP{daily caps - student + owner}
  CAP -->|over| 429[HTTP 429 next turn]
```

Classifiers run on `llama3.1-8b` for latency; the Socratic rewriter runs on `gpt-oss-120b` because it needs coherent prose. Every classifier decision and rewrite is appended to `conversation_flags` so teachers can audit activations from the "Needs Review" tab.

Detail: [docs/diagrams/chat-pipeline.md](docs/diagrams/chat-pipeline.md).

## Tech stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust (Axum, SQLx, Tokio) |
| Frontend | React 19, TypeScript, TanStack Router/Query, Tailwind CSS, react-force-graph-2d, i18next |
| Database | PostgreSQL 16 |
| Vector DB | Qdrant (per-course versioned collections) |
| LLM | Cerebras (default), any OpenAI-compatible endpoint |
| Embeddings | OpenAI (default) or local fastembed (Snowflake arctic-embed, BGE, BAAI, GTE, mxbai, EmbeddingGemma, multilingual-e5, Qwen3-Embedding, ...) |
| Container | Docker, multi-stage production build |
| Edge | Apache 2 with `mod_shib` + `mod_lua` |

## Project structure

```
backend/
  crates/
    minerva-server/    # HTTP API, routes, RAG strategies, KG linker, extraction guard
    minerva-core/      # Shared models and types
    minerva-db/        # PostgreSQL + Qdrant data layer
    minerva-ingest/    # Document extraction, classification, chunking, embedding
  migrations/          # SQL migrations
  .sqlx/               # Offline query cache (CI + prod build with SQLX_OFFLINE=true)
frontend/              # React SPA (with iframe-embed, LTI, admin, teacher views)
docker/                # Dockerfiles (dev + prod)
apache/                # Apache vhost config + mod_lua external-auth hook
moodle-plugin/         # Moodle local_minerva plugin (auto-mirrored to a public repo + Gitea)
scripts/               # Transcript pipeline and other automation scripts
k8s/                   # Kubernetes (Kustomize) manifests
terraform/             # GitHub secrets management
docs/                  # Architecture diagrams + README screenshots
```

## Getting started

### Prerequisites

- Docker + Docker Compose
- Cerebras API key (for inference), or any OpenAI-compatible endpoint
- OpenAI API key (for embeddings); optional if you only enable local fastembed models

### Development

```bash
cp .env.example .env
# Edit .env with your API keys

docker compose up
```

This starts the backend (port 3000), frontend dev server (port 5173), PostgreSQL (5432), and Qdrant (6333/6334). With `MINERVA_DEV_MODE=true` (the compose default) Shibboleth is bypassed; the backend reads the `X-Dev-User` header and falls back to the first admin in `MINERVA_ADMINS`.

### Production

```bash
cp .env.example .env
# Edit .env with production values

docker compose -f docker-compose.prod.yml up -d
```

The production build bundles the frontend into a single container with the backend on port 3000.

A pre-built image is available from GHCR:

```bash
docker pull ghcr.io/edwinexd/minerva:master
```

For the Kubernetes (k3s) production layout used at DSV, see `k8s/` and the deployment notes in [AGENTS.md](AGENTS.md).

## Environment variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `QDRANT_URL` | Qdrant gRPC endpoint |
| `MINERVA_HMAC_SECRET` | Secret for signing embed/invite/LTI tokens (also keyed by Apache for `mod_lua`) |
| `MINERVA_ADMINS` | Comma-separated admin usernames (eppn prefix before `@`) |
| `MINERVA_DOCS_PATH` | Document storage path |
| `CEREBRAS_API_KEY` | Cerebras API key for inference |
| `OPENAI_API_KEY` | OpenAI API key for embeddings (optional if only using local fastembed) |
| `MINERVA_BASE_URL` | Public base URL used for LTI tool URLs (default: `https://minerva.dsv.su.se`) |
| `MINERVA_LTI_KEY_SEED` | RSA key seed for LTI 1.3 (falls back to `MINERVA_HMAC_SECRET`) |
| `MINERVA_SERVICE_API_KEY` | Global service API key for `/api/service/` automated pipelines |
| `MINERVA_DEV_MODE` | Set `true` to bypass Shibboleth in development |
| `MINERVA_DEFAULT_COURSE_DAILY_TOKEN_LIMIT` | Per-student-per-course daily token cap for new courses (default `100000`, `0` = unlimited) |
| `MINERVA_DEFAULT_OWNER_DAILY_TOKEN_LIMIT` | Per-owner aggregate daily token cap for new users (default `500000`, `0` = unlimited) |
| `MINERVA_CANVAS_AUTO_SYNC_INTERVAL_HOURS` | How often to re-pull Canvas-linked courses |

See [.env.example](.env.example) for defaults and additional tunables.

## Knowledge graph

Each document is classified (`lecture`, `lecture_transcript`, `tutorial_exercise`, `solution`, `assignment`, ...) by `gpt-oss-120b` from content alone (filename signals are stripped). Cross-document edges are inferred by an embedding-prefiltered, per-pair-parallel LLM linker that grounds decisions in *content excerpts* read directly from Qdrant. Edge kinds: `part_of_unit`, `solution_of`, `prerequisite_of`, `applied_in`. Decisions are cached per pair and re-evaluated by a debounced sweeper when classifications change. Teachers can view the graph (rendered with `react-force-graph-2d`), reject individual edges, and export the data; KG behaviour is gated per-course via feature flags.

## Extraction guard ("Aegis")

A two-stage safeguard for assessment integrity. Per-turn intent classifier + per-chunk output classifier on `llama3.1-8b`; over-extraction triggers a Socratic rewrite on `gpt-oss-120b`. Engagement detection lifts the constraint mid-conversation and is surfaced in the teacher dashboard as a per-turn trace. Classifications, activations, and rewrites are append-only in `conversation_flags`; the K5 owl-shield icon in the UI signals the affordance to students.

## Moodle integration

A Moodle local plugin (`local_minerva`, targeted at Moodle 4.5 LTS) is included in `moodle-plugin/`. It embeds the AI chat inside Moodle courses via iframe, syncs enrolments, uploads course materials (visibility-filtered), and supports importing `.mbz` Moodle course backups. The plugin source is auto-mirrored to a public `Edwinexd/moodle-local_minerva` repo and to `gitea.dsv.su.se/edsu8469/moodle-local_minerva` (subtree-split, preserves per-commit history).

A site-level integration key on the Minerva side enables zero-touch course linking from Moodle, with an optional eppn domain allowlist.

## LTI 1.3

Minerva can act as an LTI 1.3 Tool Provider, both per-course (`/courses/{id}/lti`) and at the site level for Moodle/Canvas with first-launch course binding. On launch the LMS signs an OIDC id_token; Minerva validates it and issues an embed token so the student lands in the correct course chat. Site-level platforms support an optional eppn domain allowlist. Students who launch into a course they aren't enrolled in get an "add as student" path that surfaces a teacher approval queue.

## Canvas sync

Teachers can connect a Canvas LMS course to Minerva (`/courses/{id}/canvas`). Minerva pulls files, module pages, and external URLs from Canvas and registers them as documents. Re-syncs run automatically on a configurable interval (`MINERVA_CANVAS_AUTO_SYNC_INTERVAL_HOURS`) or can be triggered manually.

## Auth surfaces

The main application runs behind Apache `mod_shib`. See [apache/README.md](apache/README.md) for the full vhost configuration, including the pure-Lua HMAC validation used for external-auth invites (no subrequests, no shell-out, secret kept in `/etc/apache2/secrets/minerva-hmac`).

| Path prefix | Auth method | Why |
|-------------|-------------|-----|
| `/api/integration/*` | Per-course API key (Bearer) | Moodle server-to-server calls |
| `/api/service/*` | Global service API key (Bearer) | Automated pipelines (transcript fetcher, etc.) |
| `/api/embed/*` | HMAC-signed embed token | Iframe chat API |
| `/embed/*` | Embed token (query param) | Iframe frontend route |
| `/lti/*` | LTI 1.3 (OIDC + signed JWT) | LTI login, launch, JWKS (called by the LMS) |
| `/api/external-auth/*` | HMAC-signed invite token | External-auth invite callback |
| `/embedding-catalog` | Public read-only | Teacher feed of enabled embedding models |

Everything else requires a valid Shibboleth session. Identity headers are unset `early` outside of those entry points so a client cannot spoof them.

## Contributing

A contributor license agreement is in place ([CLA.md](CLA.md)); first-time contributors will be asked to sign before a PR is merged.

CI runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest`, frontend `tsc`, and `eslint`. Pre-commit also enforces `cargo check` / `clippy` with `SQLX_OFFLINE=true`, so any change to a `sqlx::query!` / `query_as!` macro requires re-running:

```bash
docker compose -f docker-compose.yml up -d postgres
cd backend && DATABASE_URL=postgres://minerva:minerva@localhost:5432/minerva \
    cargo sqlx prepare --workspace
git add .sqlx/
```

## Acknowledgements

Logo design by Tilly Makrof-Johansson. See the in-app `/acknowledgements` page for full credits and the third-party libraries Minerva is built on.

## License

[AGPL-3.0](LICENSE)
