# Microservices split

Status: **partially implemented (proper-ms).** The runtime topology (5
binaries: api / embedder / reranker / worker / scheduler, gRPC between
them) shipped earlier. The crate-level dependency split is now done for
the expensive part: see "Implemented so far" below. The original design
text is kept verbatim afterwards as the reference. See AGENTS.md
"Architecture debt" for the one-paragraph version.

## Implemented so far (proper-ms)

The actual crate layout diverges from the original sketch below (which
folded the engine back into `minerva-ingest`). What shipped:

- `minerva-embed-engine` (NEW): the ONLY crate compiling fastembed /
  candle / hf-hub. Holds `FastEmbedder`, `FastReranker`, `MemBudget`,
  `compute_budget_bytes`. Linked only by `minerva-embedder` /
  `minerva-reranker`.
- `minerva-catalog` (NEW, no-deps): model-id tables
  (`VALID_LOCAL_MODELS`, `STARTUP_BENCHMARK_MODELS`,
  `VALID_RERANKER_MODELS`, ...) + the query-prefix helpers, so every
  tier shares them without pulling the engine.
- `minerva-mbz` (NEW): the Moodle `.mbz` parser (tar / gzip / xml).
- `minerva-pipeline`: the old `minerva-ingest`, slimmed and engine-free
  (pipeline / chunker / pdf / OpenAI-HTTP embed / Classifier trait).
- `minerva-rpc` (remote gRPC clients, engine-free) split from
  `minerva-rpc-local` (in-process `Local*` wrappers over the engine).
- `AppState` no longer compiles the engine ("Leak A"): the in-process
  embedder/reranker is behind a default-off `local-engine` feature on
  `minerva-server` (the dev Dockerfile builds with it; prod doesn't).
  Verified via `cargo tree`: api / worker / scheduler are engine-free.
- `classification` decoupled from `strategy::common` (shared Cerebras /
  payload / `RagChunk` helpers moved to `minerva-server::llm`); the
  worker's doc-claim path decoupled from `routes::*`
  (`relink_course` -> `relink_scheduler`, `extension_from_filename` ->
  `minerva-pipeline`).

Deferred (lower value now the engine is isolated; axum/mbz compile in
seconds): carving `minerva` / `minerva-worker` / `minerva-scheduler`
into separate crates over a shared axum-free `minerva-app-core`, and the
`minerva-integrations` crate. That needs splitting `lti.rs` /
`system_defaults` across the axum boundary and converting `canvas.rs`
(63 `AppError` sites) for a fully axum-free scheduler.

---

## Why

The backend today is a single Rust process that owns:

- HTTP / auth / routing (axum)
- The per-doc ingest worker loop (`worker.rs`)
- The FastEmbed model LRU (`fastembed_embedder.rs`, ~3.4 GiB budget)
- The cross-encoder reranker LRU (`reranker.rs`)
- Chat strategy execution (synchronous embed + rerank per request)
- A handful of periodic schedulers (Canvas auto-sync, LTI NRPS, LTI
  health, stale-doc sweep, relink sweeper)

One process, one cgroup, one allocator, one tokio runtime. Concretely:

1. An OOM in the ingest worker (e.g. a Qwen3 load colliding with an
   in-flight MBZ parse) kills the API and the UI goes down. Mid-typing
   chat sessions get a connection drop. We've had this happen.
2. The reranker and the embedder fight for the same heap. A first-time
   reranker load can push the embedder LRU into eviction churn even
   though they're independent caches.
3. We can't scale the API horizontally without paying ~3.4 GiB per
   replica for embedder weights that only the worker and the chat
   query lane actually need.
4. Defensive plumbing (`MemBudget`, the warmup-before-measure dance,
   `wait_for_rss_drop`) gets simpler after the split: each piece
   protects one pod's cgroup against its own fat allocators, instead
   of every concern fighting in one shared budget. MemBudget itself
   stays; the embedder pod and the worker pod each still have more
   than one fat allocator and need it. See the "What changes shape
   but survives" section for the per-service shape.

The clean split is `minerva-api`, `minerva-embedder`,
`minerva-reranker`, `minerva-worker`, and `minerva-scheduler`. The
first four come from the AGENTS.md sketch; `minerva-scheduler` is
added here so the periodic background pollers (Canvas auto-sync, LTI
NRPS reconcile, LTI platform-health probe, pending-platform cleanup)
don't share a restart lifecycle with the fat ingest worker. This doc
fills in the contracts, phasing, and rollback story.

## Target topology

```
                       ┌──────────────────────┐
                       │ Apache (Shib + Lua)  │
                       └─────────┬────────────┘
                                 │  HTTPS (user traffic)
                                 ▼
┌────────────────────────────────────────────────────────────────────┐
│ minerva-api  (Deployment, replicas: 2-N)                           │
│   axum HTTP, auth middleware, all /api/* and /api/service/* routes │
│   chat strategy execution                                          │
│   AppState: db, qdrant, http, rules, capabilities, ext invites     │
│   clients: EmbedderClient, RerankerClient                          │
│   resources: req 256Mi / lim 1Gi   (no model weights resident)     │
└─────────┬──────────────────┬──────────────────┬───────────────────-┘
          │ pg               │ qdrant           │ gRPC (internal ClusterIP)
          ▼                  ▼                  ▼
┌──────────────┐     ┌──────────────┐     ┌────────────────────────────┐
│ postgres     │     │ qdrant       │     │ minerva-embedder           │
│ (existing)   │     │ (existing)   │     │ Deployment, replicas: 1    │
└──────────────┘     └──────────────┘     │ owns FastEmbedder LRU      │
       ▲     ▲                            │ resources: req 3Gi/lim 5Gi │
       │     │                            └──────────────┬─────────────┘
       │     │                                           │ gRPC
       │     │                                           │
┌──────────────────────────────────────┐                 │
│ minerva-worker (Deployment, repl: 1) │─────────────────┤
│   document claim loop                │                 │ ┌────────────────┐
│   pipeline::process_document         │────gRPC────────▶│ │ minerva-       │
│   stale-doc + relink sweepers        │                 │ │ reranker       │
│   clients: EmbedderClient,           │                 │ │ Deployment     │
│            RerankerClient (optional) │                 │ │ replicas: 1    │
│   resources: req 512Mi/lim 1.5Gi     │                 │ │ owns FastRer.. │
└──────────┬──────────┬────────────────┘                 │ │ req 1Gi/lim2Gi │
           │ pg       │ qdrant                           │ └────────────────┘
           ▼          ▼                                  │
                                                         │
┌──────────────────────────────────────┐                 │
│ minerva-scheduler (Deployment, r: 1) │                 │
│   Canvas auto-sync                   │                 │
│   LTI NRPS reconcile                 │                 │
│   LTI platform-health probe          │                 │
│   pending-platform cleanup           │                 │
│   resources: req 64Mi / lim 256Mi    │                 │
└──────────┬───────────────────────────┘                 │
           │ pg                                          │
           ▼                                             │
       (postgres, above)                                 │
```

Key invariants:

- Postgres is still the work queue (`SELECT ... FOR UPDATE SKIP LOCKED`
  on `documents.status = 'pending'`). No new message broker.
- Qdrant still has exactly two writers (worker upserts, eventual
  housekeeping); no change in collection naming.
- minerva-embedder and minerva-reranker are internal-only (ClusterIP,
  no Ingress, no Shib).
- Three pods (api, worker, scheduler) share a hostPath mount at
  `/data0/minerva/data` for documents. Embedder and reranker each
  get their own separate hostPath for HuggingFace model caches.
  Details under "Persistent volumes and shared filesystem" below.

## Service contracts

### Protocol choice: gRPC (tonic)

HTTP/JSON works fine for the embedder's request shape (string in,
float array out), but:

- Embedding payloads at ingest time are large (a 32-chunk batch of
  2000-char passages is ~64 KB request, ~96 KB response for a 768-dim
  model). gRPC over HTTP/2 reuses one connection, avoids JSON encoding
  of every f32, and lets the worker stream batches without
  request/response coupling.
- We get a generated client / server for free; the Rust ecosystem
  story (`tonic`) is mature.
- Internal traffic only, so we don't pay the gRPC-from-the-browser
  tax.

Both new services speak gRPC. The proto files live in a new
`minerva-rpc` workspace crate so both client and server consume the
same generated types.

Auth on internal RPC: shared bearer token `MINERVA_INTERNAL_RPC_TOKEN`
(env var, k8s Secret). NetworkPolicy restricts the embedder/reranker
Services to pods labelled `minerva-internal-client: "true"`. Defence
in depth; either alone is enough.

### minerva-embedder

`proto/embedder.proto`:

```protobuf
service Embedder {
    // High-priority lane. Used by api on the chat query path.
    rpc EmbedQuery (EmbedRequest) returns (EmbedResponse);
    // Low-priority lane. Used by worker for ingest batches.
    rpc Embed (EmbedRequest) returns (EmbedResponse);
    // Server-streaming for very large ingest jobs so the worker can
    // pipeline. Optional in phase 1; ship in phase 2 if needed.
    rpc EmbedStream (stream EmbedRequest) returns (stream EmbedResponse);

    // Admin: run the boot benchmark set or one specific model.
    rpc RunBenchmarks (BenchmarkRequest) returns (BenchmarkResponse);
    rpc GetBenchmarks (BenchmarksQuery) returns (BenchmarkResponse);

    // Liveness / model-list. Returns the loaded model id set and
    // RSS estimates so the api's admin "embedder status" page can
    // render without scraping logs.
    rpc Status (StatusRequest) returns (StatusResponse);
}

message EmbedRequest {
    string model_name = 1;       // matches existing local model ids
    repeated string texts = 2;
    string request_id = 3;       // for tracing; opaque
}
message EmbedResponse {
    repeated FloatVec vectors = 1;
}
message FloatVec { repeated float values = 1; }
```

Priority is encoded by RPC method, not a request field, so the
existing dispatcher's `tokio::select! { biased }` shape carries
over verbatim. The server side just calls
`FastEmbedder::embed_query` for `EmbedQuery` and `FastEmbedder::embed`
for `Embed`.

Batching: callers pass full lists. The server still chops them into
`EMBED_BATCH_SIZE` jobs internally so chat preemption points are
preserved.

Errors: existing string errors are wrapped in `tonic::Status` with
`code = Internal` for everything except "unknown model name" which
becomes `InvalidArgument`. The client maps these back to the existing
`Result<_, String>` shape on the calling side.

### minerva-reranker

`proto/reranker.proto`:

```protobuf
service Reranker {
    rpc Rerank (RerankRequest) returns (RerankResponse);
    rpc BenchmarkOne (BenchmarkOneRequest) returns (RerankBenchmark);
    rpc GetBenchmarks (BenchmarksQuery) returns (BenchmarksResponse);
    rpc Status (StatusRequest) returns (StatusResponse);
}

message RerankRequest {
    string model_code = 1;
    string query = 2;
    repeated string documents = 3;
    string request_id = 4;
}
message RerankResponse {
    repeated ScoredIndex results = 1;  // sorted best-first
}
message ScoredIndex {
    uint32 index = 1;
    float score = 2;
}
```

The existing `FastReranker::rerank` returns `Vec<(usize, f32)>`
sorted best-first. Direct mapping.

Top-k cap: the current api over-fetches and reranks the full
candidate set, then trims. We keep that behaviour; the reranker
doesn't need a top-k parameter. Trimming stays in api.

### Client crate: minerva-rpc

New workspace crate. Owns:

- `proto/embedder.proto`, `proto/reranker.proto`
- `build.rs` running `tonic_build`
- Thin Rust wrappers (`EmbedderClient`, `RerankerClient`) that
  preserve the existing `Result<Vec<Vec<f32>>, String>` and
  `Result<Vec<(usize, f32)>, BenchmarkError>` signatures, so call
  sites barely change.
- Connection pooling via `tonic::transport::Channel::balance_list`
  pointed at the Service DNS. (Single replica today, but the
  channel-balance shape lets us bump replicas with no code change.)

This is what makes the migration cheap: existing call sites like
`fastembed.embed_query(model, texts)` become
`state.embedder.embed_query(model, texts)` with the same return
type, and the `Arc<FastEmbedder>` field in `AppState` becomes an
`EmbedderClient`.

## What moves and what stays

### What moves into minerva-embedder

- `minerva-ingest::fastembed_embedder` (the whole module): LRU,
  dispatcher, warmup, benchmark machinery, RSS measurement,
  `wait_for_rss_drop`.
- `pipeline::VALID_LOCAL_MODELS`, `pipeline::STARTUP_BENCHMARK_MODELS`
  move with it (they're the model catalog and warmup set).
- ORT runtime initialisation (currently implicit via fastembed).

### What moves into minerva-reranker

- `minerva-ingest::reranker` (the whole module).
- `VALID_RERANKER_MODELS` constant.
- The reranker benchmark machinery.

### What moves into minerva-worker

- `minerva-server::worker` (just the doc claim loop, semaphore, and
  the stale-doc sweeper that resets `processing` > 600 s old). The
  periodic schedulers that today share this file move to
  minerva-scheduler instead, see below.
- `minerva-server::classification::CerebrasClassifier` (only the
  worker classifies).
- `minerva-server::relink_scheduler::spawn_sweep` (relink is a
  worker concern; the api just signals into the dirty queue). Stays
  with worker rather than scheduler because the actual sweep does
  fat work (reads classifications, builds the graph, writes back
  to qdrant) and it's tightly coupled to the ingest pipeline's
  output. The "tick every 60 s" shape it shares with the scheduler-
  bound tasks is superficial.
- The MBZ parser and bulk reclassify-all paths.
- `minerva-ingest::pipeline::process_document` becomes a worker-side
  call (worker imports `minerva-ingest`, which by then no longer
  owns the embedder).

### What moves into minerva-scheduler

A new tiny pod whose only job is the "wake every 60 s, scan DB, hit
external API" pollers. None of these allocate fat memory or hold
long-lived state; carving them out gets them off the worker's
restart lifecycle.

- Canvas auto-sync loop (currently `worker::start` spawn around
  line 152).
- LTI NRPS reconcile loop (currently around line 218).
- LTI platform-health probe loop (currently around line 352).
- Pending-platform cleanup (currently around line 314).
- The `SCHEDULE_TICK` constant and `schedule_ticker()` helper move
  with them.

The scheduler binary's `main` is a small `tokio::select!` of these
four loops plus a `/health` HTTP endpoint on a non-public port.
Single replica by design (so we don't need advisory locks); on
restart, every loop's next tick is "within 60 s", same as today.

The stale-doc sweeper does **not** move here. It belongs with the
worker because it's the recovery half of the worker's claim
loop; splitting them would mean a worker that restarts loses its
own crash-recovery sweep until the scheduler's next tick.

### What stays in minerva-api

- All HTTP routes (`routes/*`), auth middleware, LTI, Shib, ext
  invites, admin UI handlers.
- Chat strategy execution (`strategy/`). It calls EmbedderClient and
  RerankerClient instead of the local Arcs. The synchronous embed
  + search + rerank + LLM chain is unchanged from the user's POV.
- `RuleCache`, `CapabilityCache`, `BackfillTracker`,
  `relink_scheduler::mark_dirty` (just the signal side; the sweep
  task moves).
- The LTI / Canvas / NRPS / platform-health periodic tasks. Open
  question, see "Periodic tasks" below.
- The transcript-pipeline `/api/service/` routes; the GitHub-
  Service-API-key handling. These are HTTP I/O, no model work.

### What changes shape but survives

- **`MemBudget` stays, but becomes per-service.** The reason the
  primitive exists (give fat background ops a single MiB-permit
  pool so collective allocation can't overrun the cgroup) still
  applies inside any pod that has more than one fat allocator, or
  that has fat allocators outside its model LRU. The split doesn't
  remove that need; it just resizes the pool per pod:
  - **worker**: still has multiple fat allocators (`process_document`
    per-doc work, MBZ parse, bulk reclassify-all, KG linker). All
    of them share the worker pod's cgroup. MemBudget keeps doing
    its current job there, sized to
    `worker_cgroup - baseline_reserve` (no embedder cache to
    subtract anymore, since the embedder is in another pod).
  - **embedder**: the FastEmbedder LRU has its own measured-cost
    budget, but inflight inference still allocates ORT scratch /
    attention buffers outside the LRU's accounting. MemBudget
    covers those, sized as
    `embedder_cgroup - fastembed_cache_budget - baseline_reserve`,
    same shape as today just per-pod. This is also where any
    future "load a one-off custom model on admin request" lands.
  - **reranker**: the FastReranker LRU currently has no budget at
    all (it's "load once, never evict"). Two reasonable answers:
    give the reranker LRU its own measured-cost budget mirroring
    FastEmbedder (preferred long-term), or use MemBudget as the
    sole accounting until the LRU grows up. Phase 2 ships with
    MemBudget guarding reranker loads; phase 4 cleanup decides
    whether to add a proper LRU budget.
  - **api**: chat strategy doesn't allocate fat (token streaming
    is small, the embedder/reranker calls are gRPC). No MemBudget
    needed in the api pod. The struct field still exists in
    AppState for symmetry but is constructed as
    `MemBudget::new(0)` and never acquired against; cheaper than
    plumbing an `Option` through every call site that today takes
    `Arc<MemBudget>`.
  - **scheduler**: same shape as api. Periodic pollers do nothing
    but DB queries and outbound HTTP. `MemBudget::new(0)`
    placeholder, never acquired.
- `BASELINE_RESERVE_MIB` survives but moves into each service's
  startup, with potentially different values (the embedder pod has
  much higher irreducible floor than the api or scheduler pod). The
  cgroup-reading helper lives in `minerva-core` so all five
  services (and the embedder's own internal LRU sizing) consume one
  copy.
- `wait_for_rss_drop` stays inside the embedder for its LRU
  eviction. Not used elsewhere.
- The FastEmbedder LRU and its warmup-before-measure machinery
  stay; they're still the right design for the embedder service.
  The budget number is just the embedder pod's full cgroup minus
  its own MemBudget and reserve, no longer a fraction of a shared
  pod budget.
- The doc-claim queue (`documents.status` + `FOR UPDATE SKIP
  LOCKED`) is unchanged. Worker replicas can be scaled up; the
  query is already replica-safe.
- The stale-doc sweeper (resets `processing` > 600 s) moves with
  the worker. With multiple worker replicas this becomes a
  "whichever replica wakes first cleans up" pattern; the sweep
  query is idempotent so a race is fine.

## Periodic tasks: where do they live

The worker today also runs Canvas auto-sync, LTI NRPS reconcile,
LTI platform health, pending-platform cleanup. None of these are
memory-heavy; they're just "wake every 60 s, scan DB, do HTTP".

Rejected options:

- **Leave them with the worker.** Simple, but means an ingest OOM
  pauses NRPS reconciliation and every worker roll restarts the
  scheduler clocks for no good reason. Conflates "fat ingest
  engine" with "tiny periodic cron".
- **Move them into the api.** Cleaner separation by concern, but
  with api replicas > 1 every replica races on the same DB rows at
  every tick, which forces `pg_try_advisory_lock` plumbing or
  capping api at one replica (defeating the horizontal-scale
  motivation for the split).

Chosen: **`minerva-scheduler` as a dedicated single-replica pod.**
Tiny (~50 to 100 MiB), no model weights, no advisory locks needed (one
replica by design), and decoupled from both api and worker restart
lifecycles. Cost is one more Deployment + image-build target. Per
"What moves into minerva-scheduler" above for the exact contents.

The relink sweeper specifically does **not** move with the
scheduler-bound tasks; it stays with the worker because its sweep
does fat work tightly coupled to ingest classification output.

## Postgres + Qdrant clients

Each service gets its own client(s). Pool sizes per service:

| Service           | PgPool max | Qdrant | Notes                              |
|-------------------|------------|--------|------------------------------------|
| minerva-api       | 20         | yes    | unchanged from today               |
| minerva-worker    | 10         | yes    | reads claim queue, writes vectors  |
| minerva-scheduler | 4          | no     | low-traffic pollers, short queries |
| minerva-embedder  | 0          | no     | stateless model server             |
| minerva-reranker  | 0          | no     | stateless model server             |

Embedder / reranker do not touch postgres or qdrant. Model catalogs
(`embedding_models` / `reranker_models` tables) are managed from
the api side; the embedder takes whatever model name it's handed
and tries to load. Seeding (currently in `AppState::new`) stays in
api startup.

`SQLX_OFFLINE` + `.sqlx` cache: the embedder / reranker crates
don't pull sqlx, so they get out of the offline-cache requirement
entirely. The api and worker both need the cache; we keep the
single committed `backend/.sqlx/` and both binaries' builds use it.

## Persistent volumes and shared filesystem

Today everything runs in one pod with one hostPath mount
(`/data0/minerva/data` mapped to `/data`), under which sit:

- `/data/documents/{course_id}/{doc_id}.{ext}` - user uploads,
  ingested documents, transcripts, GitHub PDFs, Canvas-pulled
  files, `.url` stubs.
- `/data/hf-cache` - HuggingFace model weight cache (ONNX files
  for fastembed + reranker models + Qwen3 + custom Snowflake).

After the split, multiple pods need read/write access to subsets
of this. Grep across the codebase confirms the writers and
readers:

### `/data/documents` - shared between api, worker, scheduler

Writers:
- **api**: doc uploads (`routes/documents.rs`), Canvas pulls (admin
  "Sync now"), transcript-pipeline `POST` (`routes/service.rs`),
  course-merge file relocation (`routes/admin.rs`), `.url` stub
  creation, dev seed.
- **worker**: GitHub PDF download into a child doc
  (`worker.rs::download_github_pdf`), MBZ-extracted files. Worker
  reads every file it processes; some flows are write-then-read in
  the same pod, others are written by api/scheduler and read by
  worker.
- **scheduler**: Canvas auto-sync downloads files to disk via the
  same `routes::canvas::run_sync` codepath that api uses
  interactively. Scheduler is the *only* caller of that codepath
  after Phase 3.5 unless we change something (see below).

Readers:
- **api**: doc download (`routes/documents.rs`), system disk-usage
  report (`routes/system.rs`), course-merge source scan.
- **worker**: every `process_document` call reads the file off
  disk before chunking.

Why this is safe with naive shared-FS semantics: every file is
written once under a UUID-based filename. There is no two-writer
race because there is no two-writer file. The lifecycle is
"writer flips a DB row from `pending` to processed; readers gate
on the DB row". The DB is the synchronization point; the
filesystem is content-addressed storage.

### Scheduler: file mount or HTTP shim?

Two choices for how the scheduler handles its file writes:

**Option A (chosen): mount `/data/documents` on scheduler too.**
Three pods (api / worker / scheduler) get the same hostPath mount,
each writes under unique UUID paths. Cost: one extra mount on a
tiny pod. The scheduler binary statically links the Canvas /
NRPS / health handler code from the shared crate (per the
"Crates" section), so the same function runs whether triggered
interactively from api or periodically from scheduler.

**Option B (rejected): scheduler is FS-free; calls into api over
internal HTTP for the actual sync.** Cleaner separation of concerns
(scheduler is pure cron), but adds a new internal API surface
(`POST /api/internal/canvas/sync/{conn_id}`, similar for NRPS), and
turns every Canvas / LTI auto-sync into a network call across pods
with retry semantics to reason about. Not worth the new surface
area for the modest "scheduler doesn't touch the FS" benefit.

If we later want to revisit (e.g. running scheduler outside the
node that has hostPath), the migration is: add the internal HTTP
endpoints, flip scheduler to call them, remove the mount. Same
shape as the embedder/reranker dual-mode pattern.

### `/data/hf-cache` - split per model server, not shared

Today's single cache held both fastembed embedding models and the
cross-encoder reranker models. Post-split they go to disjoint
caches:

- `/data0/minerva/hf-cache-embed/` mounted only on minerva-embedder
- `/data0/minerva/hf-cache-rerank/` mounted only on minerva-reranker

The two model sets never overlap (embedders are encoder models,
rerankers are cross-encoders), so a shared cache would just be a
shared mount point with two disjoint subtrees. Splitting them
gives each service its own pod-local backup and restore story,
and avoids one service's pod-restart needing to remount a volume
in active use by the other.

Multiple replicas of the same service (e.g. embedder scaled to 2)
share their cache. `hf-hub` uses atomic-rename file installs, so
a concurrent first-download of the same model produces a
duplicate download in the worst case (each pod fetches it
independently), then both atomic-rename into place. No corruption.
This is unchanged from today's behaviour and is fine; if it ever
matters, an init container with a flock prevents the duplicate
fetch.

### What does not need shared storage

- **postgres / qdrant data dirs**: already isolated to their own
  pods, unchanged.
- **chat / LTI / Shib / ext-invite state**: all in postgres or
  HMAC-derived cookies. No filesystem.
- **temp files**: `tempfile` crate writes to pod-local `/tmp`.
  Used by the MBZ parser. Pod-local is correct (the file is
  consumed in the same task that creates it).

### k3s single-node vs future multi-node

The hostPath approach works because everything runs on a single
node (`minerva.dsv.su.se`). On a single node, hostPath is
effectively ReadWriteMany - every pod scheduled there sees the
same directory. We pin all five Deployments to the prod node by
default node-selector (already implicit since there's only one
node), so this stays correct after the split.

If we ever go multi-node (Canvas integration probably forces this
sooner than chat, given Canvas file pulls can be GiB-scale):

- `/data/documents` becomes a real PVC backed by NFS or CephFS
  with `accessModes: [ReadWriteMany]`. The application code
  doesn't change; the StorageClass does.
- `/data/hf-cache-*` can either follow (multi-node embedder
  replicas share one cache) or stay node-local per pod (each
  embedder replica fetches its own copy; minor RAM/disk waste).
  Pick when we get there.
- The scheduler's hostPath mount becomes a problem only if
  scheduler can land on a node that doesn't have the volume
  mounted. The fix is to upgrade `/data/documents` to RWX, same
  as above. Until then, node-pinning works.

The doc is single-node-first. The multi-node escape hatch is
explicit so we don't lock ourselves in.

## Image / Cargo layout

### Crates (post-split)

```
backend/
├── crates/
│   ├── minerva-core/        (shared types + `mem_budget` moves
│   │                         here so all five service binaries
│   │                         can depend on it without pulling
│   │                         minerva-ingest. Plus the cgroup-
│   │                         reading helper currently duplicated
│   │                         between mem_budget and
│   │                         fastembed_embedder.)
│   ├── minerva-db/          (unchanged: pg + qdrant + queries)
│   ├── minerva-rpc/         (NEW: proto + generated clients/servers)
│   ├── minerva-ingest/      (slimmed: pipeline + chunker + pdf +
│   │                         classifier trait. No longer owns
│   │                         FastEmbedder, FastReranker, or
│   │                         MemBudget.)
│   ├── minerva-embedder/    (NEW: owns FastEmbedder, tonic server,
│   │                         binary entrypoint `minerva-embedder`)
│   ├── minerva-reranker/    (NEW: owns FastReranker, tonic server,
│   │                         binary entrypoint `minerva-reranker`)
│   ├── minerva-worker/      (NEW: doc claim loop + stale-doc
│   │                         sweeper + relink sweeper + classifier,
│   │                         depends on minerva-ingest +
│   │                         minerva-rpc clients,
│   │                         binary entrypoint `minerva-worker`)
│   ├── minerva-scheduler/   (NEW: Canvas + LTI NRPS + platform-
│   │                         health + pending-platform-cleanup
│   │                         pollers. Depends on minerva-db and
│   │                         minerva-server::lti_nrps / canvas
│   │                         helpers (which probably need to slide
│   │                         into a shared crate as part of this
│   │                         phase; see Phase 3.5 notes).
│   │                         Binary entrypoint `minerva-scheduler`.)
│   └── minerva-server/      (slimmed: HTTP + chat strategy + relink
│                             signal side + transcript-pipeline
│                             service API. Binary entrypoint
│                             `minerva` as today.)
```

### Docker images

Five images, one binary each. Shared base layer for compilation.

`docker/Dockerfile.prod` becomes a multi-target build with a
`TARGET_BIN` build arg:

```dockerfile
ARG TARGET_BIN=minerva

FROM rust:latest AS backend-builder
... (existing dep cache logic, then:)
RUN cargo build --release -p ${TARGET_BIN}

FROM debian:trixie-slim AS runtime-base
... (existing apt-get layer; poppler-utils only needed by worker
     but cheap enough to keep in shared base)

FROM runtime-base AS runtime
ARG TARGET_BIN
COPY --from=backend-builder /app/backend/target/release/${TARGET_BIN} /usr/local/bin/app
... (per-target ENV in overlay)
CMD ["/usr/local/bin/app"]
```

Build pipeline (`docker.yml`) gains a 5-element matrix:
`[minerva, minerva-worker, minerva-embedder, minerva-reranker,
minerva-scheduler]`. Each pushes to
`ghcr.io/edwinexd/minerva-${target}:${tag}` (or keeps the single
repo with tag prefixes; either works). Frontend build only runs in
the `minerva` target.

The dep-cache layer is identical across all five images, so
building five images in parallel still only pays the cargo-fetch
cost once (BuildKit `--mount=type=cache`).

### Kustomize

`k8s/base/` gains:

- `embedder.yaml` (Deployment + ClusterIP Service). hostPath mount
  `/data0/minerva/hf-cache-embed` -> `/data/hf-cache`, `HF_HOME=
  /data/hf-cache`. No docs mount.
- `reranker.yaml` (Deployment + ClusterIP Service). Same shape as
  embedder, hostPath `/data0/minerva/hf-cache-rerank` instead. No
  docs mount.
- `worker.yaml` (Deployment, no Service - it's a client, not a
  server). hostPath `/data0/minerva/data` -> `/data`,
  `MINERVA_DOCS_PATH=/data/documents`. No hf-cache mount (worker
  talks to embedder over gRPC).
- `scheduler.yaml` (Deployment, no Service - all egress, no
  inbound traffic. Health probe on a local-only port.) Same docs
  mount as worker (`/data0/minerva/data` -> `/data`,
  `MINERVA_DOCS_PATH=/data/documents`) so Canvas auto-sync can
  write downloaded files. No hf-cache.
- `internal-rpc-netpol.yaml` (NetworkPolicy gating ingress to
  embedder/reranker on the client-label)

`app.yaml` (the renamed minerva-api Deployment) keeps the docs
mount (api both serves downloads and accepts uploads), drops the
hf-cache mount entirely (moves split to embedder + reranker),
drops `MINERVA_FASTEMBED_CACHE_BUDGET_BYTES` and
`MINERVA_MEM_BUDGET_MIB` env vars, drops `MALLOC_TRIM_THRESHOLD_`
/ `MALLOC_ARENA_MAX` (only helped because of embedder allocator
pressure that's now in another pod). Resource limits shrink to
~1 GiB.

Migration note for the hf-cache split: on first deploy, both new
hostPaths are empty. The embedder pod will re-download its model
set on first request. That's a one-time ~3-5 minute first-request
latency hit per model (matches a cold pod restart today). If we
want to avoid it, the deploy script can pre-populate
`/data0/minerva/hf-cache-embed/` and `/data0/minerva/hf-cache-
rerank/` by copying the relevant subtrees out of the existing
`/data0/minerva/hf-cache/` before the cutover. Cheap and worth
doing.

`k8s/overlays/prod/` patches: image tag injection per-target
(today the deploy workflow sets one tag; it now sets four).

### Resource sizing first pass

Sized for the 16 GiB prod node, with `request == limit` on every
pod so they all get k8s `Guaranteed` QoS class. We do our own
memory budgeting inside each process (FastEmbedder LRU at 55% of
cgroup, MemBudget for the rest), so the k8s burstable gap buys us
nothing useful and equal values keep the cgroup limit stable from
process startup; the scheduler can't over-promise these pods'
memory to a noisy neighbour, and they sit at the top of the
eviction order during real node pressure.

| Pod                | mem (req=lim) | cpu (req=lim) | Notes                              |
|--------------------|---------------|---------------|------------------------------------|
| minerva-api        | 1 Gi          | 1000 m        | no model weights resident          |
| minerva-worker     | 2 Gi          | 1500 m        | pipeline + classifier + MemBudget  |
| minerva-scheduler  | 256 Mi        | 200 m         | 4 pollers + sqlx + reqwest         |
| minerva-embedder   | 5 Gi          | 2000 m        | cache 3.8 GiB explicit + 1.2 GiB baseline |
| minerva-reranker   | 2 Gi          | 1500 m        | default + 1 secondary resident     |

Total: ~10.25 GiB minerva services. Plus postgres (~256 Mi - 1 Gi),
qdrant (~256 Mi - 2 Gi), and k3s + OS (~500 Mi - 1 Gi) on the same
node lands around ~11-13 GiB and leaves a comfortable slack on a
16 GiB box.

### Embedder sizing: explicit cache budget instead of the default fraction

The `FastEmbedder` LRU defaults to 55% of cgroup `memory.max` (per
`DEFAULT_CACHE_BUDGET_FRACTION`). That fraction was tuned for the
monolith pod, where the *other* 45% had to fit HTTP routes + auth
+ classifier + chat strategy + sqlx pool + qdrant client + ingest
worker.

In the embedder-alone pod none of that runs. Steady-state baseline
is just the gRPC server + tokio + ORT runtime + glibc + tonic
connections, roughly 500 MiB to 1 GiB. Letting 45% of the cgroup
sit idle as "non-cache room" wastes most of the pod.

So the manifest sets `MINERVA_FASTEMBED_CACHE_BUDGET_BYTES`
explicitly:

- 5 GiB cgroup
- 3.8 GiB cache budget
  (`MINERVA_FASTEMBED_CACHE_BUDGET_BYTES=4080218931`,
  i.e. 3.8 * 2^30)
- ~1.2 GiB baseline = ample for the embedder-alone baseline + ORT
  inference scratch + slack

3.8 GiB holds the two heavy models we care about resident together:
arctic-m (~1 GiB warmed) + nomic (~2.7 GiB warmed) = ~3.7 GiB
measured, with ~100 MiB slack so the smaller startup-set models
can swap in without evicting either heavy.

Two specific wins from the upgrade vs the 6 GiB monolith:

1. **Embedder cache stops thrashing.** At the monolith's
   55%-of-6-GiB (~3.3 GiB cache budget), the four-model startup
   set couldn't all fit; loading nomic (warmed ~2.7 GiB) evicted
   everything else. At 3.8 GiB explicit budget in the embedder-
   alone pod, nomic + arctic-m coexist with ~100 MiB slack so the
   smaller MiniLM-class models can swap in without evicting either
   heavy.
2. **Reranker keeps the default model warm across admin flips.**
   2 GiB holds jina v2 base (~600 MiB warmed) plus one
   secondary; admin benchmarks of the heavier bge-v2-m3
   evict the secondary slot but not the default, so user chat
   turns never pay a cold load during a benchmark click.

## Cutover plan (phased, each phase rollback-safe)

The repo lives on master and ships continuously; the split must
never wedge a deploy. Phases are designed so each is independently
shippable and revertable.

### Phase 0: minerva-rpc crate, no behaviour change

- Add the `minerva-rpc` crate with proto definitions and generated
  types.
- `EmbedderClient` and `RerankerClient` impls that wrap an
  `Arc<FastEmbedder>` / `Arc<FastReranker>` directly (no RPC yet).
  This lets us swap call sites to the client trait now, and only
  swap the impl later.
- All call sites (`strategy/common.rs`, `worker.rs`,
  `pipeline.rs`) start going through the client trait.

Risk: ~zero. Behaviour unchanged. Rollback: revert PR.

### Phase 1: minerva-embedder service, dual-mode

- Ship the `minerva-embedder` crate with the tonic server.
- New env var `MINERVA_EMBEDDER_URL`. If set, api and worker use
  the gRPC `EmbedderClient`; if unset, the in-process direct
  client from phase 0.
- Deploy the embedder Deployment + Service to prod without
  setting `MINERVA_EMBEDDER_URL` on api/worker. Verify embedder
  pod stays healthy under synthetic load (admin "Run benchmarks"
  endpoint routed through it).
- Flip `MINERVA_EMBEDDER_URL` on api in a separate deploy.
  Observe chat latency. If embedder p99 is comparable to the
  in-process baseline, leave it on. Else: unset, debug, re-flip.
- Flip on worker.

Risk: the flip is the only delicate step. Rollback: unset the env
var, pod rolls without the gRPC client, embedder pod becomes a
no-op.

### Phase 2: minerva-reranker service, same dual-mode

Same shape as Phase 1, `MINERVA_RERANKER_URL`.

### Phase 3: minerva-worker carve-out

- Extract `worker.rs` + `classification.rs` + `relink_scheduler.rs`
  into the `minerva-worker` crate. The periodic Canvas / LTI / NRPS
  / health / cleanup loops stay in the worker binary in this phase
  (Phase 3.5 moves them out); the worker binary boots all of them
  exactly as it boots them in the monolith today, so behaviour is
  unchanged.
- New env var `MINERVA_RUN_WORKER` on the api binary. Default
  `true` (so an old image keeps working). Worker binary always
  runs its loop.
- Deploy minerva-worker Deployment to prod. Verify it claims docs
  alongside the still-running in-api worker (they coexist via
  `SKIP LOCKED`; both racing on the queue is fine).
- Flip `MINERVA_RUN_WORKER=false` on api in a separate deploy.

Risk: small. The queue semantics already tolerate multiple
claimants. Rollback: set `MINERVA_RUN_WORKER=true` on api; worker
Deployment can keep running or be scaled to 0.

### Phase 3.5: minerva-scheduler carve-out

- Extract the four periodic loops (Canvas auto-sync, LTI NRPS
  reconcile, LTI platform-health probe, pending-platform cleanup)
  from `worker.rs` into the `minerva-scheduler` crate. The
  underlying handlers (`routes::canvas::run_sync`,
  `lti_nrps::reconcile_context`, etc.) likely need to slide out of
  `minerva-server` into a shared crate so both the scheduler binary
  and (for ad-hoc admin-triggered syncs) the api binary can call
  them. Candidates for that shared home: a new
  `minerva-integrations` crate, or fold them into `minerva-ingest`
  / a new `minerva-platforms` module. Decide during Phase 3.5.
- New env var `MINERVA_RUN_SCHEDULER` on the worker binary.
  Default `true` so an unflipped worker keeps doing what it does
  today. Scheduler binary always runs its loops.
- Deploy minerva-scheduler Deployment to prod. Verify each loop
  ticks on the new pod (log line per tick already exists). The
  worker is still running the same loops at this point; the dupe
  is safe because each loop's DB query is idempotent and the
  "find what's due" pattern naturally handles two callers.
- Flip `MINERVA_RUN_SCHEDULER=false` on worker in a separate
  deploy. Worker now only runs the doc claim loop + stale sweeper
  + relink sweeper.

Risk: small. Dual-running for one deploy cycle catches any "loop
silently doesn't fire on the new pod" regression before we turn
off the in-worker copy. Rollback: set `MINERVA_RUN_SCHEDULER=true`
on worker; scheduler Deployment can be scaled to 0 or left
running.

### Phase 4: cleanup

- Remove the dual-mode env-var fallbacks.
- Resize each surviving MemBudget instance to its new pod's
  cgroup. Worker drops the "subtract fastembed cache" term from
  the sizing formula (no cache in that pod anymore); embedder
  keeps it. Api wires `MemBudget::new(0)` as a no-op placeholder
  to keep AppState symmetric. Reranker decides between MemBudget
  and a measured-cost LRU budget (see "What changes shape but
  survives"). `BASELINE_RESERVE_MIB` becomes a per-service
  constant rather than a single shared one.
- Remove `MALLOC_TRIM_THRESHOLD_` / `MALLOC_ARENA_MAX` from the
  api pod spec; keep them on the embedder pod where they still
  help.
- Update memprobe to label its origin (api vs worker vs embedder
  vs reranker) so log greps land on the right pod.
- Drop `minerva-ingest`'s dependency on `fastembed` / `candle` /
  `hf-hub`. Worker and api builds get faster and slimmer.

Risk: zero (deletions of code already proven unused).

## Observability

- Each service emits `tracing` JSON with a `service` field
  (`api` / `worker` / `embedder` / `reranker`).
- `request_id` flows from the api edge through to the embedder /
  reranker via the gRPC field above; chat traces tie together
  across pods.
- memprobe stays in each service, labelled with the service name.
- New metric: `embedder_inference_duration_ms{model, priority}`
  emitted as `tracing::info` summary records every 60 s, parsed
  by the existing log-driven monitoring.
- Embedder cache state (loaded models, RSS, last_used) exposed via
  the `Status` RPC; api's admin "embedder status" page reads it
  for the system dashboard.

## Out of scope for this design

- Replacing Postgres-as-queue with NATS / Redis. Not warranted at
  current volume.
- Splitting api into "auth/CRUD" vs "chat strategy" (e.g. a
  dedicated chat-server). The chat path is HTTP-shaped, lives
  fine in api.
- Multi-region. Single-cluster only.
- Cross-replica sticky chat sessions. Conversation state is in
  Postgres; any api replica can serve any message.

## Open questions

1. Single embedder replica vs HPA? FastEmbedder LRU is per-pod, so
   two replicas double the memory cost but halve the worst-case
   queue depth. Start with 1, add HPA only if we see queue
   contention. Status RPC's `queue_depth` field will tell us.
2. Reranker priority lane: today there is no priority distinction
   because reranker is only called from chat. If we ever rerank
   from ingest too, we'll want the same High/Low split as the
   embedder. Easy retrofit; not needed now.
3. Should the api still seed `embedding_models` / `reranker_models`
   catalogs, or does that move to a one-shot Job? Current
   recommendation: keep in api startup; it's idempotent and runs
   in <1 s on a 20-conn pool.

## What this design does NOT do

- Does not change any external API surface. Browsers, Moodle, LTI
  consumers, the transcript pipeline, and the GitHub Service API
  see byte-identical traffic.
- Does not touch the database schema.
- Does not change Qdrant collection layout.

If any of those need to change later, they get their own design
docs.
