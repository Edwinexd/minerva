# System overview

```mermaid
flowchart LR
  subgraph clients[Clients]
    BR[Web SPA<br/>React + TanStack]
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
    DOCS[/data0/minerva/data/]
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

Apache trust boundary: Shibboleth-issued identity headers and the Lua
external-auth headers are unset `early` for any request that does not come
through one of those two paths. Per-route exemptions for LMS / iframe /
service-account traffic are handled by their own bearer-token or
HMAC-signed-token middleware in the backend.
