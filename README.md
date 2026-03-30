# Minerva

Minerva is a retrieval-augmented generation (RAG) platform built for educational use at DSV, Stockholm University. It lets teachers upload course materials and gives students an AI assistant that answers questions grounded in those documents.

## Features

- **Multiple RAG strategies:** simple, parallel (stream while retrieving), and FLARE (logprobs-guided retrieval)
- **Course management:** teachers create courses, upload PDFs, and invite students via links
- **Role-based access:** students, teachers, and admins with Shibboleth (SAML) authentication
- **Usage tracking:** per-student token usage, daily breakdowns, configurable limits
- **Admin dashboard:** manage users, suspend accounts, view system-wide usage

## Tech stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust (Axum, SQLx, Tokio) |
| Frontend | React 19, TypeScript, TanStack Router/Query, Tailwind CSS |
| Database | PostgreSQL 16 |
| Vector DB | Qdrant |
| LLM | Cerebras (primary), OpenAI (embeddings) |
| Container | Docker, multi-stage production build |

## Project structure

```
backend/
  crates/
    minerva-server/    # HTTP API, routes, RAG strategies
    minerva-core/      # Shared models and types
    minerva-db/        # PostgreSQL + Qdrant data layer
    minerva-ingest/    # PDF extraction, chunking, embedding
  migrations/          # SQL migrations
frontend/              # React SPA
docker/                # Dockerfiles (dev + prod)
```

## Getting started

### Prerequisites

- Docker and Docker Compose
- Cerebras API key (for inference)
- OpenAI API key (for embeddings)

### Development

```bash
cp .env.example .env
# Edit .env with your API keys

docker compose up
```

This starts the backend (port 3000), frontend (port 5173), PostgreSQL, and Qdrant.

### Production

```bash
cp .env.example .env
# Edit .env with production values

docker compose -f docker-compose.prod.yml up -d
```

The production build bundles the frontend into a single container with the backend, served on port 3000.

A pre-built image is also available from GHCR:

```bash
docker pull ghcr.io/edwinexd/minerva:master
```

## Environment variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `QDRANT_URL` | Qdrant gRPC endpoint |
| `MINERVA_HMAC_SECRET` | Secret for signing tokens |
| `MINERVA_ADMINS` | Comma-separated admin usernames (eppn prefix) |
| `MINERVA_DOCS_PATH` | Document storage path |
| `CEREBRAS_API_KEY` | Cerebras API key for inference |
| `OPENAI_API_KEY` | OpenAI API key for embeddings |

See [.env.example](.env.example) for defaults.

## Moodle integration

A Moodle local plugin (`local_minerva`) is included in `moodle-plugin/`. It embeds the AI chat inside Moodle courses via iframe, syncs enrolments, and uploads course materials. See [moodle-plugin/local/minerva/](moodle-plugin/local/minerva/) for setup.

### Routes that must be excluded from SSO / Shibboleth

The main application sits behind Apache `mod_shib` which sets the `REMOTE_USER` header. The following API paths and frontend routes must **not** be behind Shibboleth (i.e. they need `ShibRequestSetting requireSession 0` or should be excluded from the `<Location>` block):

| Path prefix | Purpose |
|-------------|---------|
| `/api/health` | Health check endpoint |
| `/api/models` | Model list (public) |
| `/api/dev/config` | Dev mode config (no-ops in production) |
| `/api/integration/*` | Integration API for external services (Moodle plugin). Authenticated via per-course API keys (Bearer token), not SSO. |
| `/api/embed/*` | Embeddable chat API for iframe usage. Authenticated via HMAC-signed embed tokens, not SSO. |
| `/embed/*` | Frontend embed route (iframe chat UI). Uses embed tokens from query params, not SSO. |

**Example Apache config** (adjust to your setup):

```apache
# Protect the main application with Shibboleth.
<Location />
    AuthType shibboleth
    ShibRequestSetting requireSession 1
    Require valid-user
</Location>

# Exclude routes that use their own auth (integration API, embed).
<LocationMatch "^/api/(health|models|dev|integration|embed)">
    ShibRequestSetting requireSession 0
    Require all granted
</LocationMatch>

<LocationMatch "^/embed/">
    ShibRequestSetting requireSession 0
    Require all granted
</LocationMatch>
```

All other routes (`/api/auth/*`, `/api/courses/*`, `/api/admin/*`, the main frontend at `/`, `/course/*`, `/teacher/*`, `/admin/*`) require Shibboleth authentication which provides the `REMOTE_USER` header.

## License

[AGPL-3.0](LICENSE)
