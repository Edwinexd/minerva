# Minerva

High-performance RAG system for DSV Stockholm University.

## Stack

- **Backend**: Rust (Axum), PostgreSQL (sqlx), Qdrant vectors, OpenAI embeddings, Cerebras LLM
- **Frontend**: React + TypeScript, Vite, TanStack Router (file-based routing), TanStack Query, shadcn/ui + Tailwind CSS v4
- **Auth**: Apache2 + mod_shib (Shibboleth) handles SAML. App reads `REMOTE_USER` header.
- **Admin**: Hardcoded admin user `edsu8469`

## Project Structure

```
backend/           Cargo workspace
  crates/
    minerva-server/  Binary: Axum routes, auth middleware
    minerva-core/    Lib: domain models, services
    minerva-ingest/  Lib: PDF parse, chunk, embed, store
    minerva-db/      Lib: Postgres + Qdrant wrappers
  migrations/        sqlx postgres migrations
frontend/          React SPA (Vite)
  src/routes/        TanStack Router file-based routes
  src/components/ui/ shadcn components
docker/            Dockerfile
```

## Development

```bash
# Backend
cd backend && cargo run

# Frontend
cd frontend && npm run dev

# Full stack
docker-compose up
```

## Environment

Copy `.env.example` to `.env` and fill in:
- `DATABASE_URL` - Postgres connection string
- `QDRANT_URL` - Qdrant gRPC endpoint
- `CEREBRAS_API_KEY` - Cerebras inference API key
- `OPENAI_API_KEY` - OpenAI embeddings API key
- `MINERVA_HMAC_SECRET` - Secret for signed URL HMAC

## Conventions

- No emdashes (enforced by pre-commit and CI)
- No Claude/Anthropic branding colors in UI
- Rust: use `match` freely (idiomatic), no switch equivalent
- Frontend: TanStack Router for routing, TanStack Query for data fetching
- Pre-commit hooks: trailing whitespace, emdash ban, cargo check, cargo clippy
