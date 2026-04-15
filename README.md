# Minerva

Minerva is a retrieval-augmented generation (RAG) platform built for educational use at DSV, Stockholm University. It lets teachers upload course materials and gives students an AI assistant that answers questions grounded in those documents.

## Features

- **Multiple RAG strategies:** simple, parallel (stream while retrieving), and FLARE (logprobs-guided retrieval)
- **Course management:** teachers create courses, upload PDFs/documents, and invite students via links
- **Role-based access:** students, teachers, and admins with Shibboleth (SAML) authentication
- **External-auth invites:** time-limited access for non-Shibboleth users (external collaborators)
- **Role auto-promotion rules:** attribute-based rules that promote users to teacher at login
- **LMS integration:** Moodle plugin (iframe embed + enrolment sync) and LTI 1.3 support
- **Canvas sync:** pull files, pages, and external URLs from a Canvas LMS course automatically
- **Transcript pipeline:** indexes DSV Play video transcripts as searchable documents
- **Per-course and per-owner AI spending caps:** daily token limits with configurable defaults
- **Usage tracking:** per-student token usage, daily breakdowns, admin dashboard
- **Pluggable embedding:** OpenAI or local fastembed models, configurable per course
- **Pluggable inference:** Cerebras (default) or any OpenAI-compatible model, configurable per course

## Tech stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust (Axum, SQLx, Tokio) |
| Frontend | React 19, TypeScript, TanStack Router/Query, Tailwind CSS |
| Database | PostgreSQL 16 |
| Vector DB | Qdrant |
| LLM | Cerebras (default), any OpenAI-compatible endpoint |
| Embeddings | OpenAI (default), or local fastembed |
| Container | Docker, multi-stage production build |

## Project structure

```
backend/
  crates/
    minerva-server/    # HTTP API, routes, RAG strategies
    minerva-core/      # Shared models and types
    minerva-db/        # PostgreSQL + Qdrant data layer
    minerva-ingest/    # Document extraction, chunking, embedding
  migrations/          # SQL migrations
frontend/              # React SPA
docker/                # Dockerfiles (dev + prod)
apache/                # Apache vhost config + mod_lua external-auth hook
moodle-plugin/         # Moodle local_minerva plugin
scripts/               # Transcript pipeline and other automation scripts
k8s/                   # Kubernetes (Kustomize) manifests
terraform/             # GitHub secrets management
```

## Getting started

### Prerequisites

- Docker and Docker Compose
- Cerebras API key (for inference)
- OpenAI API key (for embeddings, unless using a local fastembed model)

### Development

```bash
cp .env.example .env
# Edit .env with your API keys

docker compose up
```

This starts the backend (port 3000), frontend dev server (port 5173), PostgreSQL, and Qdrant.

In dev mode (`MINERVA_DEV_MODE=true`, set by default in `docker-compose.yml`) Shibboleth is not required -- the backend reads the `X-Dev-User` header (or falls back to the first admin in `MINERVA_ADMINS`).

### Production

```bash
cp .env.example .env
# Edit .env with production values

docker compose -f docker-compose.prod.yml up -d
```

The production build bundles the frontend into a single container with the backend, served on port 3000.

A pre-built image is available from GHCR:

```bash
docker pull ghcr.io/edwinexd/minerva:master
```

## Environment variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `QDRANT_URL` | Qdrant gRPC endpoint |
| `MINERVA_HMAC_SECRET` | Secret for signing embed/invite/LTI tokens |
| `MINERVA_ADMINS` | Comma-separated admin usernames (eppn prefix before `@`) |
| `MINERVA_DOCS_PATH` | Document storage path |
| `CEREBRAS_API_KEY` | Cerebras API key for inference |
| `OPENAI_API_KEY` | OpenAI API key for embeddings (optional if using local fastembed) |
| `MINERVA_BASE_URL` | Public base URL used for LTI tool URLs (default: `https://minerva.dsv.su.se`) |
| `MINERVA_LTI_KEY_SEED` | RSA key seed for LTI 1.3 (falls back to `MINERVA_HMAC_SECRET`) |
| `MINERVA_SERVICE_API_KEY` | Global service API key for `/api/service/` automated pipelines |
| `MINERVA_DEV_MODE` | Set `true` to bypass Shibboleth in development |
| `MINERVA_DEFAULT_COURSE_DAILY_TOKEN_LIMIT` | Per-student-per-course daily token cap for new courses (default `100000`, `0` = unlimited) |
| `MINERVA_DEFAULT_OWNER_DAILY_TOKEN_LIMIT` | Per-owner aggregate daily token cap for new users (default `500000`, `0` = unlimited) |

See [.env.example](.env.example) for defaults and additional tunables.

## Moodle integration

A Moodle local plugin (`local_minerva`) is included in `moodle-plugin/`. It embeds the AI chat inside Moodle courses via iframe, syncs enrolments, and uploads course materials. See [moodle-plugin/local/minerva/](moodle-plugin/local/minerva/) for setup.

## LTI 1.3

Minerva can act as an LTI 1.3 Tool Provider. Teachers register their LMS as a platform per course (`/courses/{id}/lti`). On launch the LMS signs an OIDC id_token; Minerva validates it and issues an embed token so the student lands in the correct course chat.

## Canvas sync

Teachers can connect a Canvas LMS course to Minerva (`/courses/{id}/canvas`). Minerva pulls files, module pages, and external URLs from Canvas and registers them as documents. Re-syncs run automatically on a configurable interval (`MINERVA_CANVAS_AUTO_SYNC_INTERVAL_HOURS`) or can be triggered manually.

## Apache setup

The main application runs behind Apache `mod_shib`. See [apache/README.md](apache/README.md) for the full vhost configuration, including:

- External-auth (`mod_lua`) for non-Shibboleth invite links
- Identity headers trusted from Shibboleth and the Lua hook

### Paths excluded from Shibboleth

| Path prefix | Auth method | Why |
|-------------|-------------|-----|
| `/api/integration/*` | Per-course API key (Bearer token) | Moodle server-to-server calls |
| `/api/service/*` | Global service API key (Bearer token) | Automated pipelines (transcript fetcher, etc.) |
| `/api/embed/*` | HMAC-signed embed token | Iframe chat API |
| `/embed/*` | Embed token (query param) | Iframe frontend route |
| `/lti/*` | LTI 1.3 (OIDC + signed JWT) | LTI login, launch, JWKS -- called by the LMS |
| `/api/external-auth/*` | HMAC-signed invite token | External-auth invite callback |

Everything else requires a valid Shibboleth session.

## License

[AGPL-3.0](LICENSE)
