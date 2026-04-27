# Document ingest pipeline

Source: `backend/crates/minerva-ingest/`. Triggered when a teacher uploads a
file, the Moodle/Canvas sync registers a URL, or an MBZ import enqueues a
backup. The same state machine is used for all entry points.

```mermaid
flowchart TD
  A[Upload / Moodle / Canvas / MBZ / play.dsv URL] --> B[(documents table<br/>status = pending)]
  B --> W[ingest worker<br/>claims rows]
  W --> R{mime / source}
  R -->|application/pdf, etc.| X[poppler / extractor]
  R -->|text/x-url play.dsv.su.se| AT[awaiting_transcript]
  R -->|text/x-url other| US[unsupported]
  AT --> TP[transcripts.yml<br/>hourly cron]
  TP --> X
  X --> CL[adversarial / kind classifier<br/>llama3.1-8b]
  CL --> CH[chunker]
  CH --> EM[embedder<br/>OpenAI or fastembed]
  EM --> Q[(Qdrant<br/>per-course versioned collection)]
  EM --> KG[KG linker<br/>cross-doc edges<br/>part_of_unit / solution_of /<br/>prerequisite_of / applied_in]
  KG --> PG[(role_rules / kg_state / linker_decisions)]
  Q --> READY[status = ready]
```

Notes:

- The classifier runs *before* chunking so assignments and solutions can be
  tagged and excluded from prompt context for student-facing chats.
- Embeddings are written to a per-course Qdrant collection that is versioned
  by `(course_id, embedding_model)`. Re-embedding under a new model creates
  a new collection version; the old one stays live until the rotation
  finishes (lazy re-embed).
- The KG linker reads excerpts and embeddings *from Qdrant*; it does not
  re-parse the original PDFs. Decisions are cached per pair so untouched
  pairs are not re-evaluated.
