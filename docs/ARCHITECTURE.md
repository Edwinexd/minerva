# Minerva architecture

The figures here are rendered from [`docs/diagrams/build.py`](diagrams/build.py)
(graphviz under the hood). To regenerate after editing:

```bash
sudo apt-get install graphviz
python3 -m venv /tmp/diags
/tmp/diags/bin/pip install graphviz diagrams
/tmp/diags/bin/python docs/diagrams/build.py
```

## System overview

![System overview](diagrams/system-overview.svg)

Apache unsets identity headers `early` outside of `mod_shib` / Lua paths;
LMS, iframe, and service-account routes carry their own bearer-token or
HMAC-signed-token middleware. Double-headed arrows indicate read/write
relationships; single-headed arrows are push-only.

## Service topology

The backend is five Rust binaries built from one Cargo workspace, split by
what each process must link so a fat ingest OOM can't take down the API and
the API can scale without carrying model weights:

- **minerva-app** (`minerva-server`) is the only binary that links the axum
  route tree. It owns HTTP / auth / Shibboleth / LTI / external-invites, all
  `/api/*` and `/api/service/*` routes, and chat-strategy execution. It holds
  no model weights and reaches the embedder / reranker over gRPC, so it scales
  horizontally.
- **minerva-worker** runs the document-claim loop, the ingest pipeline
  (`process_document`), the classifier, the MBZ parser, and the stale-doc +
  relink sweepers. Axum-free.
- **minerva-scheduler** runs only the periodic pollers: Canvas auto-sync, LTI
  NRPS reconcile, LTI platform-health probe, and pending-platform cleanup. One
  replica (so it needs no advisory locks), axum-free, tiny. Kept off the
  worker's restart lifecycle so an ingest OOM or a worker roll never pauses
  NRPS or resets the scheduler clocks. It is the sole owner of these loops (the
  worker no longer runs them as a fallback). If the single pod is down the loops
  pause and resume on recovery, which is acceptable because every loop is a
  DB-driven "find what's due" query and the 30-day platform-orphan grace dwarfs
  any normal outage window.
- **minerva-embedder** and **minerva-reranker** are stateless gRPC (tonic)
  model servers, each owning its own FastEmbed / cross-encoder LRU and
  HuggingFace cache. They are internal-only (ClusterIP, no ingress). Caller
  pods carry the `minerva-internal-client` label as the intended selector for a
  NetworkPolicy, and a shared `MINERVA_INTERNAL_RPC_TOKEN` is the planned
  RPC-auth control. Neither the NetworkPolicy object nor the token check is
  applied yet, so today any in-cluster pod can reach them unauthenticated.
  Tracked as the next hardening step.

Shared, axum-free code (AppState, config, the Canvas sync engine, the LTI NRPS
client, classification, the scheduler loops, `AppError`) lives in
`minerva-app-core`. The api crate layers axum on top and enables app-core's
`axum` feature, which switches on `AppError`'s `IntoResponse` impl; the worker
and scheduler depend on app-core without that feature, so their images link no
axum 0.8 and no model engine (verified with `cargo tree`: neither pulls
axum 0.8, fastembed, candle, or ort).

Key invariants:

- **Postgres is the work queue** (`SELECT ... FOR UPDATE SKIP LOCKED` on
  `documents.status`); there is no message broker, and the claim query is
  replica-safe so workers can scale out.
- **The shared filesystem is content-addressed.** api / worker / scheduler
  share one hostPath at `/data/documents` (every file is written once under a
  UUID path and the DB row is the synchronization point, so there is never a
  two-writer race). The embedder and reranker each get a separate hf-cache
  hostPath. Single-node k3s makes hostPath behave as ReadWriteMany; going
  multi-node would swap `/data/documents` for an RWX PVC with no code change.
- **Memory is budgeted per pod.** `MemBudget` (a MiB-permit pool) is sized per
  service: the worker (pipeline + classifier + MBZ), the embedder (ORT scratch
  outside its LRU), and the reranker (model loads) each carry one; the api and
  scheduler hold no fat allocators and run it as a no-op.
- **One embedder replica by default.** The FastEmbed LRU is per-pod, so a
  second replica doubles resident model memory; scale out only if the
  embedder's Status RPC reports queue contention.

All five images come from one `docker/Dockerfile.prod`; a `TARGET_BIN`
build-arg picks the binary and only the api image bundles the frontend.

## Document ingest pipeline

![Ingest pipeline](diagrams/ingest-pipeline.svg)

The kind classifier runs *before* chunking, so assignments and solutions can
be excluded from prompt context for student-facing chats. Embeddings are
written to a per-course Qdrant collection versioned by
`(course_id, embedding_model)`; re-embedding under a new model is lazy and
the old version stays live until rotation finishes. The KG linker reads
excerpts and embeddings from Qdrant (no PDF re-parsing) and caches per-pair
decisions.

## Chat / RAG pipeline

![Chat pipeline](diagrams/chat-pipeline.svg)

Both strategies share the same retrieval ; KG expansion ; prompt assembly
; LLM core. They differ only in *when* retrieval happens relative to
generation:

- **simple**: retrieve once, then generate. The default.
- **FLARE**: per-sentence feedback loop *during* generation. The LLM streams
  a sentence; if any token is low-confidence, the partial sentence is fed
  back as the next retrieval query and generation resumes against the
  augmented context. Iteration is capped per response (the dashed blue
  arrow in the diagram). FLARE doesn't precede the regular pipeline; it
  loops inside it.

The legacy `parallel` strategy (stream + retrieve concurrently) was retired
in migration `20260519000001_tool_use_and_drop_parallel.sql`; existing rows
were remapped to `simple`. Its replacement is the orthogonal **tool-use**
axis: each course has a `tool_use_enabled` toggle that, when on, splits
generation into a hidden-thinking research phase (the model calls
`keyword_search`, RAG-seed and KG-expansion tools, and for `flare` the
logprob signal is injected as a tool event) followed by a clean writeup
phase. Research thinking, per-tool expandable results, wall-clock duration,
and a research/writeup token-split (prompt + completion subsets) are
persisted on the message and rendered above the assistant bubble.

Two independent classifier paths sit around the chat hot path:

- **Extraction guard** (gated by `extraction_guard`): per-turn intent
  classifier before generation, per-chunk output classifier after, KG-driven
  multi-turn proximity tracking, and a Socratic rewriter on `gpt-oss-120b`
  when the output check trips. Every decision is appended to
  `conversation_flags` so teachers can audit activations from the
  "Needs Review" tab.
- **Aegis** (gated by `aegis`): pre-send prompt-coaching analyzer that
  runs off the hot path. Every debounced keystroke POSTs to
  `/aegis/analyze`; the verdict the student had on screen at submit is
  persisted alongside the message. Severity-tagged suggestions (CLEAR-grounded
  rubric, 8 kinds) soft-block Send; `Use ideas` POSTs to `/aegis/rewrite`
  for a `gpt-oss-120b` revision of the draft.

Inline citations: replies carry `[n]` badges (naked-digit + filename-form
variants both accepted); the right-rail sources panel reports actually-cited
sources first with a toggle for the uncited remainder.

Classifiers run on `gpt-oss-120b` at `reasoning_effort: low` (the previous
cheaper `llama3.1-8b` path was deprecated by Cerebras; everything in the
classifier stack collapsed onto gpt-oss as a result, and low effort keeps
the latency profile roughly where it was). Token spend lands in
`course_token_usage` under per-feature categories so daily caps
(per-student-per-course + per-owner aggregate) cover Aegis, the
extraction guard, and the writeup phase as cleanly as they cover the main
chat reply.
