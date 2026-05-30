use std::sync::{Arc, Mutex};
use std::time::Instant;

use candle_core::{DType, Device};
use fastembed::{
    EmbeddingModel, InitOptions, InitOptionsUserDefined, Pooling, QuantizationMode,
    Qwen3TextEmbedding, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use hf_hub::api::sync::ApiBuilder;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

/// Result of benchmarking a single embedding model.
#[derive(Clone, Debug, Serialize)]
pub struct BenchmarkResult {
    pub model: String,
    pub dimensions: u64,
    pub embeddings_per_second: f64,
    pub total_ms: f64,
    pub corpus_size: usize,
}

/// Maximum number of chunks passed to a single backend call. Keeps peak
/// tensor memory predictable regardless of document size.
const EMBED_BATCH_SIZE: usize = 32;

/// Qwen3 takes a `max_length` at load time; we cap at 4096 tokens. The
/// model itself supports much more, but every extra token grows the
/// activation tensors quadratically (full self-attention) which is the
/// quickest way to OOM the production pod. Course chunks are well under
/// 1k tokens in practice, so this is way past anything a real query or
/// chunk will hit.
const QWEN3_MAX_LENGTH: usize = 4096;

/// Fallback budget when the cgroup limit can't be read and no env override is set.
const DEFAULT_CACHE_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Fraction of the cgroup memory limit we let the cache consume by default.
/// The rest of the pod (request handlers, qdrant client buffers, classifier
/// state, baseline app heap, transient ONNX inference buffers, the
/// cross-encoder reranker, glibc fragmentation overhead) needs the
/// remainder.
///
/// Was 0.4 until we discovered the per-model cost we were recording (raw
/// RSS delta after `TextEmbedding::try_new`) didn't include the ORT CPU-EP
/// arena, which only materializes on the first forward pass. Once we
/// warm each model on load (see `FastEmbedder::acquire`), the measured
/// cost is roughly 2-3x what it was, and 40% of cgroup left the budget
/// too small to hold the four `STARTUP_BENCHMARK_MODELS` coexisting --
/// arctic-m + nomic alone are ~2.5 GiB. 55% gives ~3.4 GiB of cache room
/// on the prod 6 GiB pod, enough for the startup set, while leaving
/// ~2.7 GiB for everything else (baseline ~500 MiB, reranker ~500 MiB,
/// worker transients ~500 MiB, fragmentation + slack ~1 GiB).
const DEFAULT_CACHE_BUDGET_FRACTION: f64 = 0.55;

/// Fallback cost when RSS measurement isn't available (non-Linux dev hosts)
/// or returns nonsense. Picked to be on the high side of the largest model
/// we currently load through the ONNX path, so the budget logic still
/// throttles correctly without a real measurement.
const ESTIMATED_MODEL_COST_BYTES: u64 = 800 * 1024 * 1024;

/// What's currently sitting in the cache. Two backends:
/// * **ONNX** (the default fastembed path); `TextEmbedding`.
/// * **Candle** for Qwen3-Embedding (separate `Qwen3TextEmbedding` API
///   enabled by the `qwen3` feature on fastembed).
///
/// Wrapped in `Arc<Mutex<...>>` purely so the value can be cloned into
/// the dispatcher task and into each per-job `spawn_blocking` closure.
/// The mutex is never contended at runtime: exactly one dispatcher task
/// per loaded model exists, and that task is the only thing that ever
/// locks it (one job at a time). Without the dispatcher this used to be
/// the actual serialization point for inference, which is what made
/// chat embeds queue behind multi-batch ingest runs.
#[derive(Clone)]
enum LoadedModel {
    Fast(Arc<Mutex<TextEmbedding>>),
    Qwen3(Arc<Mutex<Qwen3TextEmbedding>>),
}

/// Priority level for one embed job submitted to a model's dispatcher.
/// The dispatcher always drains `High` before `Low`, so interactive
/// (chat) embeds jump the queue ahead of any pending ingest batches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Priority {
    /// Interactive embed (chat-side query vectors). Pre-empts pending
    /// ingest batches at every job boundary.
    High,
    /// Background embed (document ingestion). Yields to any waiting
    /// `High` job between its 32-chunk batches.
    Low,
}

/// One unit of work for the dispatcher: a batch of texts plus a
/// oneshot reply channel. We send the result back over `reply` after
/// `run_embed` completes, regardless of whether the caller is still
/// waiting (a cancelled caller simply drops the receiver, and `send`
/// returns Err which we ignore).
struct EmbedJob {
    texts: Vec<String>,
    reply: oneshot::Sender<Result<Vec<Vec<f32>>, String>>,
}

/// Per-model job dispatcher. Owns the `LoadedModel` (via the cloned
/// `Arc` inside) for as long as it exists, and serializes inference
/// through a single tokio task that pulls from two priority channels.
///
/// **Why a per-model task instead of a shared mutex**: the previous
/// design had every embed caller `spawn_blocking` directly against an
/// `Arc<Mutex<TextEmbedding>>`. That mutex is `std::sync::Mutex`, which
/// doesn't fair-queue, so a chat embed that arrived after an ingest
/// `spawn_blocking` had just released the lock typically lost the next
/// acquisition race to the ingest task's next batch. With hundreds of
/// chunks per ingest run, a single chat query could wait many seconds.
///
/// The dispatcher fixes that by making priority explicit: the task
/// loop does `tokio::select! { biased; high_rx.recv(); low_rx.recv() }`
/// so any High job that's ready always beats any Low job. The worst
/// case for a chat embed is now one in-flight batch of `EMBED_BATCH_SIZE`
/// chunks, which for the recommended models is a few hundred ms.
///
/// **Lifetime**: the dispatcher task lives until both senders drop,
/// which happens when the cache entry is evicted AND every in-flight
/// caller's `Arc<ModelDispatcher>` clone has returned. The `else`
/// arm of the `select!` then fires and the task exits, dropping the
/// `LoadedModel` and releasing its RSS.
struct ModelDispatcher {
    high_tx: mpsc::UnboundedSender<EmbedJob>,
    low_tx: mpsc::UnboundedSender<EmbedJob>,
}

impl ModelDispatcher {
    /// Spawn the per-model dispatcher task and return a handle plus
    /// the task's `JoinHandle`. The caller stores both in the cache
    /// entry; the dispatcher Arc gets handed to embed callers, the
    /// JoinHandle is awaited on eviction so we can prove the
    /// `LoadedModel` (and its ORT arena) has fully dropped before the
    /// next model load begins. Without that await we'd see the cache's
    /// Arc<ModelDispatcher> drop synchronously while the dispatcher
    /// task is still partway through cleanup, then the next load's
    /// allocation lands on top of the unreleased arena and OOMs the
    /// pod.
    fn spawn(model: LoadedModel) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let (high_tx, mut high_rx) = mpsc::unbounded_channel::<EmbedJob>();
        let (low_tx, mut low_rx) = mpsc::unbounded_channel::<EmbedJob>();
        let task = tokio::spawn(async move {
            while let Some(job) = next_job(&mut high_rx, &mut low_rx).await {
                let model_for_blocking = model.clone();
                let texts = job.texts;
                let result =
                    tokio::task::spawn_blocking(move || run_embed(&model_for_blocking, texts))
                        .await
                        .map_err(|e| format!("spawn_blocking failed: {}", e))
                        .and_then(|r| r);
                // If the caller's future was cancelled the receiver is
                // gone; drop the result on the floor. The dispatcher
                // itself stays healthy.
                let _ = job.reply.send(result);
            }
            // Falling out of the loop here drops the captured `model`
            // (the outer LoadedModel) which releases the ORT runtime's
            // hold on the weights. The JoinHandle resolves after this
            // point; eviction can then call malloc_trim and observe a
            // real RSS drop.
        });
        (Arc::new(Self { high_tx, low_tx }), task)
    }

    /// Submit one batch at the given priority and await the embedding
    /// result. Failure modes: the dispatcher's senders are gone
    /// (shouldn't happen while the caller holds the `Arc`), or the
    /// reply channel was dropped (the worker task panicked).
    async fn embed_batch(
        &self,
        texts: Vec<String>,
        priority: Priority,
    ) -> Result<Vec<Vec<f32>>, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let job = EmbedJob {
            texts,
            reply: reply_tx,
        };
        let sender = match priority {
            Priority::High => &self.high_tx,
            Priority::Low => &self.low_tx,
        };
        sender
            .send(job)
            .map_err(|_| "embed dispatcher has shut down".to_string())?;
        reply_rx
            .await
            .map_err(|_| "embed dispatcher dropped the reply channel".to_string())?
    }
}

/// Pull the next job from the two priority channels, preferring `high`
/// strictly over `low`. Returns `None` only when both senders have been
/// dropped AND both channels are drained, which is the dispatcher's
/// shutdown signal.
///
/// Factored out of the dispatcher loop so the priority semantics can
/// be unit-tested without a real loaded model. `tokio::select!`'s
/// `biased` keyword polls arms top-to-bottom: if a `High` job is
/// ready it always wins; if only `Low` has something we take it; if
/// both are empty but at least one sender is alive we park until one
/// of them produces work.
async fn next_job(
    high_rx: &mut mpsc::UnboundedReceiver<EmbedJob>,
    low_rx: &mut mpsc::UnboundedReceiver<EmbedJob>,
) -> Option<EmbedJob> {
    tokio::select! {
        biased;
        Some(j) = high_rx.recv() => Some(j),
        Some(j) = low_rx.recv() => Some(j),
        else => None,
    }
}

struct CacheEntry {
    name: String,
    dispatcher: Arc<ModelDispatcher>,
    /// `JoinHandle` for the per-model dispatcher task. Awaited on
    /// eviction so we can be certain the task's loop has exited and
    /// the captured `LoadedModel` has dropped before the next model
    /// load starts allocating. `Option` so we can `take()` it out of
    /// the evicted entry without leaving an unusable placeholder
    /// behind; `None` once we've taken it. The cache itself never
    /// holds `None` entries.
    task: Option<tokio::task::JoinHandle<()>>,
    /// Bytes added to process RSS when this model was loaded; measured by
    /// diffing `/proc/self/status:VmRSS` before and after init. Drives both
    /// eviction decisions and per-load logging. On hosts without a readable
    /// VmRSS (e.g. macOS dev) this falls back to `ESTIMATED_MODEL_COST_BYTES`
    /// so the budget still throttles.
    rss_cost_bytes: u64,
    last_used: Instant,
}

/// Memory-budgeted LRU cache over fastembed / Qwen3 models.
///
/// Why a budget instead of a fixed slot count: model footprints differ by
/// 30x+ (MiniLM ~90 MB resident, Qwen3-0.6B ~2.4 GB resident on first load),
/// so "keep N models" either wastes memory or thrashes depending on which N
/// we pick. Instead we measure each model's actual RSS cost at load time
/// and evict LRU until the new model fits under the budget.
///
/// The budget caps the sum of measured per-model costs; it does NOT cap
/// total process RSS (other parts of the app can grow independently).
/// Default: 40% of the cgroup memory limit, env-overridable via
/// `MINERVA_FASTEMBED_CACHE_BUDGET_BYTES`.
///
/// Concurrency: the cache mutex is held only across admission (lookup +
/// possible eviction + load + insert). Embeds run with the lock released,
/// so multiple cached models can serve requests in parallel. Eviction is
/// best-effort; if an in-flight embed holds an `Arc` to an evicted
/// model, the model's memory lives until that embed finishes.
///
/// `benchmark_lock` is a separate `try_lock`-style mutex used only by the
/// admin "Run benchmark" path. It serves two purposes:
/// 1. Gives the admin UI a clean "Busy" affordance instead of silently
///    queueing multiple heavy model loads behind each other.
/// 2. Prevents an admin who fat-fingers the button N times from blocking
///    the worker for N x (load + benchmark) minutes; only one
///    admin-triggered benchmark can be queued at a time, the rest are
///    rejected up front.
pub struct FastEmbedder {
    cache: tokio::sync::Mutex<Vec<CacheEntry>>,
    benchmarks: tokio::sync::Mutex<Vec<BenchmarkResult>>,
    benchmark_lock: tokio::sync::Mutex<()>,
    cache_budget_bytes: u64,
}

/// Backend dispatch for a model id.
///
/// Three paths, all producing handles managed by the same LRU cache:
/// * `Fast`; model is one of fastembed-rs's built-in `EmbeddingModel`
///   variants. Loaded by name, weights downloaded by fastembed via hf-hub.
/// * `Qwen3`; candle-backed Qwen3-Embedding family (separate fastembed
///   API gated behind the `qwen3` feature).
/// * `Custom`; "bring your own ONNX": we download the model files
///   ourselves and feed them to fastembed's `UserDefinedEmbeddingModel`
///   API. Used for HF repos whose ONNX exports work but aren't part of
///   `EmbeddingModel` (e.g. snowflake-arctic-embed-m-v2.0, multilingual,
///   custom GteModel architecture). Output handle is still a
///   `TextEmbedding`, so `run_embed` doesn't need to know the difference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Fast,
    Qwen3,
    Custom,
}

fn backend_for(model_name: &str) -> Backend {
    if model_name.starts_with("Qwen/") {
        Backend::Qwen3
    } else if custom_model_spec(model_name).is_some() {
        Backend::Custom
    } else {
        Backend::Fast
    }
}

/// Loading recipe for a "bring your own ONNX" model.
///
/// We carry only the bits fastembed's `UserDefinedEmbeddingModel` needs --
/// the ONNX file, the four tokenizer JSONs, the pooling strategy, and a
/// max-length cap. Everything else (output normalization, `[CLS]` token
/// detection, etc.) is handled inside fastembed.
struct CustomModelSpec {
    /// HF repo id, e.g. `Snowflake/snowflake-arctic-embed-m-v2.0`.
    repo_id: &'static str,
    /// Path inside the repo to the ONNX graph to load. We ship int8
    /// quantized graphs by default to keep RSS predictable; the fp32
    /// variants of these models are typically >1 GB and would also need
    /// `with_external_initializer` plumbing to load (model.onnx +
    /// model.onnx.data split).
    onnx_path: &'static str,
    /// CLS or Mean. Sourced from each repo's `1_Pooling/config.json`.
    pooling: Pooling,
    /// Static = quantization is baked into the ONNX graph (int8 ops).
    /// `None` = the graph is fp32. We never use `Dynamic` here because
    /// dynamic-quant fastembed entries go through the `Fast` backend.
    quantization: QuantizationMode,
    /// Tokenizer max-length cap. Models with very long context windows
    /// (arctic-m-v2.0 trains at 8192) still get clamped here so a single
    /// pathological input can't blow the activation budget.
    max_length: usize,
}

// The per-model query-side prefix helpers are pure model-name logic
// with no engine dependency, so they live in `minerva-catalog` and the
// api can format a query before calling the remote embedder without
// linking this crate. Re-exported here for the engine's own tests and
// any in-process caller.
pub use minerva_catalog::{format_query_for_model, query_prefix_for_model};

fn custom_model_spec(model_name: &str) -> Option<CustomModelSpec> {
    match model_name {
        // Snowflake Arctic Embed M v2.0: multilingual (Swedish + English
        // matter for SU/DSV), 768-dim, ~311 MB int8 ONNX, CLS pooling per
        // the model's `1_Pooling/config.json`. Not in fastembed-rs's
        // `EmbeddingModel` enum yet (PR #239 still open upstream), so we
        // load the ONNX through `UserDefinedEmbeddingModel`.
        //
        // The model card prescribes asymmetric prompts: prefix queries
        // with `query: ` at retrieval time, leave document chunks bare
        // at ingestion. Plumbed through `query_prefix_for_model` below
        // and applied at the two query call sites in
        // `strategy::common`; the document-side `embed` in `pipeline`
        // is left untouched.
        "Snowflake/snowflake-arctic-embed-m-v2.0" => Some(CustomModelSpec {
            repo_id: "Snowflake/snowflake-arctic-embed-m-v2.0",
            onnx_path: "onnx/model_int8.onnx",
            pooling: Pooling::Cls,
            quantization: QuantizationMode::None,
            max_length: 512,
        }),
        _ => None,
    }
}

/// Download a custom model's files from the Hub and assemble it into a
/// loaded `TextEmbedding`. Runs inside `spawn_blocking`; the hf-hub
/// sync API blocks, and the ONNX session build is CPU-heavy.
fn load_custom_model(spec: &CustomModelSpec) -> Result<TextEmbedding, String> {
    // Reuse fastembed's hf-hub cache layout when possible: the env var
    // `HF_HOME` (and the per-app `HF_CACHE_DIR`) point at the shared
    // cache, so a model downloaded here can be reused by anything else
    // that consults the Hub. `ApiBuilder::default()` already honors
    // those envs.
    let api = ApiBuilder::new()
        .with_progress(true)
        .build()
        .map_err(|e| format!("hf-hub init failed: {}", e))?;
    let repo = api.model(spec.repo_id.to_string());

    let fetch = |relative: &str| -> Result<std::path::PathBuf, String> {
        repo.get(relative)
            .map_err(|e| format!("hf-hub fetch {}/{} failed: {}", spec.repo_id, relative, e))
    };

    let onnx_path = fetch(spec.onnx_path)?;
    let tokenizer_path = fetch("tokenizer.json")?;
    let tokenizer_config_path = fetch("tokenizer_config.json")?;
    let special_tokens_path = fetch("special_tokens_map.json")?;
    let config_path = fetch("config.json")?;

    let read = |p: std::path::PathBuf| -> Result<Vec<u8>, String> {
        std::fs::read(&p).map_err(|e| format!("read {}: {}", p.display(), e))
    };

    let tokenizer_files = TokenizerFiles {
        tokenizer_file: read(tokenizer_path)?,
        config_file: read(config_path)?,
        special_tokens_map_file: read(special_tokens_path)?,
        tokenizer_config_file: read(tokenizer_config_path)?,
    };

    let model = UserDefinedEmbeddingModel::new(read(onnx_path)?, tokenizer_files)
        .with_pooling(spec.pooling.clone())
        .with_quantization(spec.quantization);

    TextEmbedding::try_new_from_user_defined(
        model,
        InitOptionsUserDefined::new().with_max_length(spec.max_length),
    )
    .map_err(|e| format!("user-defined init failed for {}: {}", spec.repo_id, e))
}

impl Default for FastEmbedder {
    /// Same as `FastEmbedder::new()`; provided because clippy's
    /// `new_without_default` lint flags any `new() -> Self` without a
    /// matching `Default`. We can't `#[derive(Default)]` directly
    /// because `cache_budget_bytes` needs `compute_budget_bytes()`
    /// (cgroup-aware), not a zero default.
    fn default() -> Self {
        Self::new()
    }
}

impl FastEmbedder {
    pub fn new() -> Self {
        let cache_budget_bytes = compute_budget_bytes();
        tracing::info!(
            "fastembed cache: budget {} MiB ({})",
            cache_budget_bytes / (1024 * 1024),
            budget_source(),
        );
        Self {
            cache: tokio::sync::Mutex::new(Vec::new()),
            benchmarks: tokio::sync::Mutex::new(Vec::new()),
            benchmark_lock: tokio::sync::Mutex::new(()),
            cache_budget_bytes,
        }
    }

    /// Embed texts using the given model name at **background** priority.
    /// Used by the ingestion pipeline. The model is loaded on first use
    /// (may download weights); subsequent calls hit the cache. Calls for
    /// different models may evict LRU entries to stay under the
    /// configured RSS budget.
    ///
    /// Concurrent `embed_query` callers always run their jobs first,
    /// because the per-model dispatcher drains its high-priority channel
    /// before its low-priority one. The worst-case delay this imposes
    /// on a chat embed is one in-flight batch of `EMBED_BATCH_SIZE`
    /// chunks (a few hundred ms for the recommended models), not the
    /// full ingest run.
    pub async fn embed(
        &self,
        model_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        self.embed_with_priority(model_name, texts, Priority::Low)
            .await
    }

    /// Embed texts at **interactive** priority. Used for chat-side query
    /// vectors (RAG retrieval, graph-expansion partner search, FLARE
    /// re-retrieval). Each batch jumps ahead of any pending ingest
    /// batches at the dispatcher.
    ///
    /// Functionally identical to `embed` from the caller's point of
    /// view; same return shape, same cache admission, same model
    /// dispatch.
    pub async fn embed_query(
        &self,
        model_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        self.embed_with_priority(model_name, texts, Priority::High)
            .await
    }

    /// Shared body for `embed` and `embed_query`. Splits the input into
    /// `EMBED_BATCH_SIZE` jobs and submits each to the model's
    /// dispatcher at the chosen priority. The dispatcher serializes
    /// jobs across all callers (every cached model has exactly one
    /// dispatcher task), and the per-batch granularity is what gives
    /// chat embeds their preemption point: an ingest call with N
    /// chunks submits ceil(N / 32) low jobs, and a high job that
    /// arrives mid-ingest only has to wait for the currently-running
    /// batch to finish before its turn.
    async fn embed_with_priority(
        &self,
        model_name: &str,
        texts: Vec<String>,
        priority: Priority,
    ) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let dispatcher = self.acquire(model_name).await?;

        let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for batch in texts.chunks(EMBED_BATCH_SIZE) {
            let part = dispatcher.embed_batch(batch.to_vec(), priority).await?;
            all_embeddings.extend(part);
        }
        Ok(all_embeddings)
    }

    /// Run the benchmark corpus against every model in `models`,
    /// sequentially. Each iteration goes through the same budgeted cache as
    /// `embed`, so models the budget can hold stay warm after the benchmark
    /// completes (the worker's first real embed against them avoids a
    /// cold load).
    ///
    /// Heavy models (Qwen3 0.6B, multilingual-e5-large, bge-m3, ...) are
    /// NOT in the boot list; they're loaded on first real embed call or
    /// when an admin clicks "Run benchmark" on the admin system page.
    pub async fn run_benchmarks(
        &self,
        models: &[(&str, u64)],
    ) -> Result<Vec<BenchmarkResult>, String> {
        let mut results = Vec::new();

        for &(model_name, dimensions) in models {
            tracing::info!("benchmarking fastembed model: {}", model_name);
            let result = self.benchmark_inner(model_name, dimensions).await?;
            results.push(result);
        }

        // Replace the cached results wholesale; matches the prior
        // single-shot boot benchmark semantics. Per-model updates from
        // admin clicks go through `benchmark_one`, which upserts a
        // single row.
        *self.benchmarks.lock().await = results.clone();
        Ok(results)
    }

    /// Run a benchmark for a single model on demand. Used by the admin
    /// "Run benchmark" UI. Acquires `benchmark_lock` non-blockingly so a
    /// second concurrent click bounces with `BenchmarkError::Busy` rather
    /// than stacking another heavy model load on top of the running one
    /// (which would consume budget and potentially evict a model the
    /// worker is actively using).
    pub async fn benchmark_one(
        &self,
        model_name: &str,
        dimensions: u64,
    ) -> Result<BenchmarkResult, BenchmarkError> {
        let _guard = self
            .benchmark_lock
            .try_lock()
            .map_err(|_| BenchmarkError::Busy)?;

        let result = self
            .benchmark_inner(model_name, dimensions)
            .await
            .map_err(BenchmarkError::Failed)?;

        // Upsert in the cached results so the teacher dropdown picks
        // up the new speed number on its next refetch.
        let mut benchmarks = self.benchmarks.lock().await;
        if let Some(existing) = benchmarks.iter_mut().find(|b| b.model == model_name) {
            *existing = result.clone();
        } else {
            benchmarks.push(result.clone());
        }

        Ok(result)
    }

    /// Shared body for both the boot-time loop and the on-demand admin
    /// benchmark. Goes through the same budgeted cache admission as a
    /// real embed; so the benchmark may evict an LRU entry, but the
    /// freshly-loaded benchmark target stays in cache (last_used is now)
    /// and is available to the worker if they share a model.
    ///
    /// Submits via `Priority::High` so the benchmark doesn't interleave
    /// with concurrent ingest batches and report misleading numbers.
    /// The two submissions (warmup + measurement) go through the same
    /// dispatcher task sequentially, so the measurement is bounded by
    /// real inference time on the same `LoadedModel` the warmup just
    /// touched.
    async fn benchmark_inner(
        &self,
        model_name: &str,
        dimensions: u64,
    ) -> Result<BenchmarkResult, String> {
        let corpus: Vec<String> = BENCHMARK_CORPUS.iter().map(|s| s.to_string()).collect();
        let corpus_size = corpus.len();

        let dispatcher = self.acquire(model_name).await?;

        // Warmup run; first inference can be much slower than steady
        // state (ONNX session init, candle kernel JIT, allocator
        // touching pages for the first time, ...).
        let _ = dispatcher
            .embed_batch(vec!["warmup".to_string()], Priority::High)
            .await;
        let start = std::time::Instant::now();
        dispatcher.embed_batch(corpus, Priority::High).await?;
        let secs = start.elapsed().as_secs_f64();

        let embeddings_per_second = corpus_size as f64 / secs;
        let total_ms = secs * 1000.0;

        tracing::info!(
            "  {}: {:.1} embeddings/sec ({:.0}ms for {} texts)",
            model_name,
            embeddings_per_second,
            total_ms,
            corpus_size,
        );

        Ok(BenchmarkResult {
            model: model_name.to_string(),
            dimensions,
            embeddings_per_second,
            total_ms,
            corpus_size,
        })
    }

    /// Get stored benchmark results.
    pub async fn get_benchmarks(&self) -> Vec<BenchmarkResult> {
        self.benchmarks.lock().await.clone()
    }

    /// True if an admin-triggered benchmark is currently running.
    /// Cheap (just `try_lock`). The admin UI uses this to grey out
    /// every "Run benchmark" button while one is in flight, without
    /// paying for a full fetch of the running task's state.
    ///
    /// Note: this only reflects the `benchmark_one` path. Boot-time
    /// `run_benchmarks` and worker embeds also serialize on the cache
    /// admission lock but don't show up here, on purpose; admins
    /// don't need to know about them.
    pub async fn is_benchmark_running(&self) -> bool {
        self.benchmark_lock.try_lock().is_err()
    }

    /// Cache admission. If the requested model is in the cache, bump its
    /// `last_used` and return a clone of its dispatcher handle.
    /// Otherwise evict LRU entries until the new model's estimated cost
    /// fits under the budget, then load the model through the
    /// appropriate backend, spawn its dispatcher task, measure the RSS
    /// delta, and insert it.
    ///
    /// Returns an `Arc<ModelDispatcher>` rather than a raw model handle
    /// so the caller submits jobs through the per-model priority queue
    /// instead of running inference inline. See `ModelDispatcher` for
    /// the priority semantics.
    async fn acquire(&self, model_name: &str) -> Result<Arc<ModelDispatcher>, String> {
        let mut cache = self.cache.lock().await;

        if let Some(idx) = cache.iter().position(|e| e.name == model_name) {
            cache[idx].last_used = Instant::now();
            return Ok(Arc::clone(&cache[idx].dispatcher));
        }

        // Pick an estimate for the new model's cost. If we've measured
        // anything, the largest measured value is a reasonable upper bound;
        // otherwise fall back to a generous default so the budget still
        // throttles on hosts without RSS introspection.
        let estimate = cache
            .iter()
            .map(|e| e.rss_cost_bytes)
            .max()
            .unwrap_or(ESTIMATED_MODEL_COST_BYTES);

        // Evict LRU until adding `estimate` would fit under the budget.
        // If the cache is empty we just load whatever's asked for; the
        // pod's memory limit is the backstop.
        while !cache.is_empty() {
            let used: u64 = cache.iter().map(|e| e.rss_cost_bytes).sum();
            if used + estimate <= self.cache_budget_bytes {
                break;
            }
            let lru_idx = cache
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
                .expect("cache non-empty");
            let mut evicted = cache.remove(lru_idx);
            let evicted_task = evicted.task.take();
            let evicted_cost = evicted.rss_cost_bytes;
            let footprint: u64 = cache.iter().map(|e| e.rss_cost_bytes).sum();
            tracing::info!(
                "fastembed cache: evicting {} ({} MiB) to make room for {} (footprint {} MiB / budget {} MiB)",
                evicted.name,
                evicted_cost / (1024 * 1024),
                model_name,
                footprint / (1024 * 1024),
                self.cache_budget_bytes / (1024 * 1024),
            );
            // Drop the evicted CacheEntry (which drops the cache's
            // last Arc<ModelDispatcher>, closes both senders) THEN
            // await the dispatcher task's JoinHandle. The task
            // resolves only after the captured `LoadedModel` has
            // dropped and ORT has actually released the weights. This
            // is the part malloc_trim alone couldn't fix: without the
            // await, the next allocation landed on top of the
            // unreleased arena and OOM-killed the pod.
            //
            // In-flight `embed()` callers' Arc clones keep the
            // dispatcher senders alive past the cache drop; in that
            // case the await blocks until those calls complete.
            // That's the intended behaviour - we'd rather wait than
            // load on top.
            //
            // Mutex is released before the await so other acquire
            // callers can progress.
            drop(cache);
            drop(evicted);
            if let Some(handle) = evicted_task {
                let _ = handle.await;
            }
            wait_for_rss_drop(evicted_cost).await;
            cache = self.cache.lock().await;
        }

        let rss_before = read_rss_bytes();
        let backend = backend_for(model_name);
        let name = model_name.to_string();
        let loaded = tokio::task::spawn_blocking(move || -> Result<LoadedModel, String> {
            match backend {
                Backend::Fast => {
                    let model_enum = parse_fast_model_name(&name)?;
                    let m = TextEmbedding::try_new(
                        InitOptions::new(model_enum).with_show_download_progress(true),
                    )
                    .map_err(|e| format!("fastembed init failed for {}: {}", name, e))?;
                    Ok(LoadedModel::Fast(Arc::new(Mutex::new(m))))
                }
                Backend::Qwen3 => {
                    // CPU + F32 is the safe default. F16 would halve memory
                    // (~1.2 GB vs 2.4 GB for 0.6B) but candle's CPU F16
                    // path is slower than F32 in the ggml/candle
                    // benchmarks I've seen, so we keep F32 unless prod
                    // RAM becomes the bottleneck.
                    let m = Qwen3TextEmbedding::from_hf(
                        &name,
                        &Device::Cpu,
                        DType::F32,
                        QWEN3_MAX_LENGTH,
                    )
                    .map_err(|e| format!("qwen3 init failed for {}: {}", name, e))?;
                    Ok(LoadedModel::Qwen3(Arc::new(Mutex::new(m))))
                }
                Backend::Custom => {
                    let spec = custom_model_spec(&name)
                        .ok_or_else(|| format!("no custom-model spec for {}", name))?;
                    let m = load_custom_model(&spec)?;
                    Ok(LoadedModel::Fast(Arc::new(Mutex::new(m))))
                }
            }
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {}", e))??;

        // Warm up the model with a batch matching real operational
        // shape before taking the RSS measurement. ORT's CPU EP (and
        // to a lesser extent candle) lazily allocate an "arena" /
        // scratch pool on the first forward pass that's substantially
        // larger than the bare model weights and persists for the
        // lifetime of the session.
        //
        // The arena sizes itself to the *largest tensor shape* ORT
        // has seen. Activation memory scales linearly with batch and
        // sequence length, and attention scratch is O(seq^2), so the
        // dominant axis is the chunk length. The two earlier
        // iterations both undersized this:
        //
        //   1. `vec!["warmup"]` (batch-1, ~5 tokens): arena ~5% of
        //      operational. First real benchmark grew it; OOM.
        //   2. `BENCHMARK_CORPUS` as-is (batch-32, ~25 tokens/text):
        //      arena ~10% of operational. The corpus is tuned for
        //      benchmarking throughput, not arena sizing. First real
        //      worker batch with 500-token chunks grew it; OOM again.
        //
        // So we pad each corpus sentence to `WARMUP_CHARS` (=
        // chunker's default `chunk_size`, the largest chunk a real
        // ingest job ever submits), which sizes the arena to
        // operational chunks at operational batch size. Subsequent
        // worker / chat inferences with same-or-smaller shapes won't
        // grow it further.
        //
        // Cost: a few hundred ms per model on startup. Trivial
        // compared to model load itself.
        //
        // We deliberately ignore warmup errors: a broken model
        // shouldn't be reusable, but we still want the load to
        // succeed and the caller to see the inference error on their
        // actual request. The next inference will surface the same
        // error.
        const WARMUP_CHARS: usize = 2000;
        let loaded_for_warmup = loaded.clone();
        let warmup_corpus: Vec<String> = BENCHMARK_CORPUS
            .iter()
            .map(|s| {
                let mut t = String::with_capacity(WARMUP_CHARS + s.len());
                while t.len() < WARMUP_CHARS {
                    t.push_str(s);
                    t.push(' ');
                }
                t.truncate(WARMUP_CHARS);
                t
            })
            .collect();
        let _ = tokio::task::spawn_blocking(move || {
            let _ = run_embed(&loaded_for_warmup, warmup_corpus);
        })
        .await;
        let rss_after = read_rss_bytes();

        let cost = match (rss_before, rss_after) {
            (Some(b), Some(a)) => a.saturating_sub(b),
            _ => ESTIMATED_MODEL_COST_BYTES,
        };

        // Spawn the per-model dispatcher and hand both the cache and
        // the caller their own `Arc` clone. The task keeps running as
        // long as either Arc lives; when the entry is evicted AND every
        // in-flight caller has returned, both senders drop, the
        // dispatcher's `select!` falls through to its `else` arm, the
        // task exits, and `loaded` is finally dropped, freeing RSS.
        let (dispatcher, task) = ModelDispatcher::spawn(loaded);

        cache.push(CacheEntry {
            name: model_name.to_string(),
            dispatcher: Arc::clone(&dispatcher),
            task: Some(task),
            rss_cost_bytes: cost,
            last_used: Instant::now(),
        });

        // Defensive post-load eviction. The pre-load eviction loop above
        // uses the `max(rss_cost_bytes)` across previously-cached entries
        // as the cost estimate for the new model. With honest
        // post-warmup measurement that estimate is reasonable, but a
        // first-of-its-kind larger model (e.g. arctic-m loading after a
        // run of small English models) still costs more than anything
        // previously seen, so the pre-load eviction can leave us over
        // budget. Sweep the LRU again now that we know the real cost.
        // We never evict the entry we just inserted: `last_used =
        // Instant::now()` makes it the newest, so `min_by_key` picks an
        // older one.
        while cache.len() > 1 {
            let used: u64 = cache.iter().map(|e| e.rss_cost_bytes).sum();
            if used <= self.cache_budget_bytes {
                break;
            }
            let lru_idx = cache
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
                .expect("cache len > 1");
            let mut evicted = cache.remove(lru_idx);
            let evicted_task = evicted.task.take();
            let evicted_cost = evicted.rss_cost_bytes;
            let footprint: u64 = cache.iter().map(|e| e.rss_cost_bytes).sum();
            tracing::info!(
                "fastembed cache: post-load eviction of {} ({} MiB), pre-load estimate was too low (footprint {} MiB / budget {} MiB)",
                evicted.name,
                evicted_cost / (1024 * 1024),
                footprint / (1024 * 1024),
                self.cache_budget_bytes / (1024 * 1024),
            );
            // Same await-the-dispatcher-task discipline as the
            // pre-load sweep above. See the long comment there for
            // why this is required to keep the cgroup safe.
            drop(cache);
            drop(evicted);
            if let Some(handle) = evicted_task {
                let _ = handle.await;
            }
            wait_for_rss_drop(evicted_cost).await;
            cache = self.cache.lock().await;
        }

        let footprint: u64 = cache.iter().map(|e| e.rss_cost_bytes).sum();
        let cached_count = cache.len();
        match (rss_before, rss_after) {
            (Some(_), Some(after)) => tracing::info!(
                "fastembed: loaded model {} (+{} MiB, RSS now {} MiB, cache footprint {} MiB / budget {} MiB, {} cached)",
                model_name,
                cost / (1024 * 1024),
                after / (1024 * 1024),
                footprint / (1024 * 1024),
                self.cache_budget_bytes / (1024 * 1024),
                cached_count,
            ),
            _ => tracing::info!(
                "fastembed: loaded model {} (RSS measurement unavailable, assumed +{} MiB, {} cached)",
                model_name,
                cost / (1024 * 1024),
                cached_count,
            ),
        };

        Ok(dispatcher)
    }
}

/// Synchronous embed dispatch; runs inside the spawn_blocking task so
/// the (blocking) inner mutex on the model handle and the (blocking)
/// inference call don't tie up the tokio runtime.
fn run_embed(model: &LoadedModel, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    match model {
        LoadedModel::Fast(m) => {
            let mut m = m
                .lock()
                .map_err(|e| format!("fastembed lock poisoned: {}", e))?;
            m.embed(texts, None)
                .map_err(|e| format!("fastembed embed failed: {}", e))
        }
        LoadedModel::Qwen3(m) => {
            // `Qwen3TextEmbedding::embed` takes `&self`, so a Mutex
            // isn't strictly needed for soundness. We keep one for
            // symmetry with the fastembed path and so future internal
            // state (KV cache, scratch buffers) doesn't bite us.
            let m = m
                .lock()
                .map_err(|e| format!("qwen3 lock poisoned: {}", e))?;
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            m.embed(&refs)
                .map_err(|e| format!("qwen3 embed failed: {}", e))
        }
    }
}

/// Errors from `benchmark_one`. `Busy` is the soft case (another admin
/// benchmark is already in flight) and the route layer maps it to a
/// `BadRequest` with code `admin.benchmark_busy` so the frontend can
/// show a friendly toast.
#[derive(Debug)]
pub enum BenchmarkError {
    Busy,
    Failed(String),
}

impl std::fmt::Display for BenchmarkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BenchmarkError::Busy => write!(f, "another benchmark is already running"),
            BenchmarkError::Failed(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for BenchmarkError {}

fn parse_fast_model_name(name: &str) -> Result<EmbeddingModel, String> {
    match name {
        // Original 4 (English).
        "sentence-transformers/all-MiniLM-L6-v2" => Ok(EmbeddingModel::AllMiniLML6V2),
        "BAAI/bge-small-en-v1.5" => Ok(EmbeddingModel::BGESmallENV15),
        "BAAI/bge-base-en-v1.5" => Ok(EmbeddingModel::BGEBaseENV15),
        "nomic-ai/nomic-embed-text-v1.5" => Ok(EmbeddingModel::NomicEmbedTextV15),

        // Multilingual.
        "intfloat/multilingual-e5-small" => Ok(EmbeddingModel::MultilingualE5Small),
        "intfloat/multilingual-e5-base" => Ok(EmbeddingModel::MultilingualE5Base),
        "intfloat/multilingual-e5-large" => Ok(EmbeddingModel::MultilingualE5Large),
        "BAAI/bge-m3" => Ok(EmbeddingModel::BGEM3),
        "google/embeddinggemma-300m" => Ok(EmbeddingModel::EmbeddingGemma300M),

        // English, top-of-MTEB-class upgrades.
        "mixedbread-ai/mxbai-embed-large-v1" => Ok(EmbeddingModel::MxbaiEmbedLargeV1),
        "Alibaba-NLP/gte-large-en-v1.5" => Ok(EmbeddingModel::GTELargeENV15),
        "snowflake/snowflake-arctic-embed-l" => Ok(EmbeddingModel::SnowflakeArcticEmbedL),

        _ => Err(format!("unsupported fastembed model: {}", name)),
    }
}

/// Poll `/proc/self/status` until VmRSS drops by at least `evicted_cost`
/// (measured against the value at entry), or `EVICTION_WAIT_BUDGET`
/// elapses. Used after dropping a cache entry's `Arc<ModelDispatcher>`
/// to make sure the model is actually freed from RSS before the next
/// acquire loads a replacement on top of it.
///
/// "At least the evicted cost" is generous; we accept any drop of
/// `>= evicted_cost / 2` because the recorded cost includes the warmup
/// arena, and tokio's blocking pool can hold scratch buffers across
/// task boundaries that take a bit longer to release.
async fn wait_for_rss_drop(evicted_cost: u64) {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
    const EVICTION_WAIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
    let Some(before) = read_rss_bytes() else {
        return; // RSS introspection unavailable, can't tell either way
    };

    // Force glibc to return freed pages to the kernel. Without this the
    // dispatcher Arc has dropped but ORT's arena pool sits in glibc's
    // freelist as RSS for an indeterminate time, so the next model load
    // allocates on top of the lingering arena and OOM-kills the pod.
    // `malloc_trim(0)` is a no-op on musl + non-Linux; gated by
    // `cfg(target_env = "gnu")` plus a tokio::spawn_blocking because
    // the call can take low-tens-of-ms on a fragmented heap and we
    // don't want to stall the dispatcher loop.
    trim_glibc_heap().await;

    let target_drop = evicted_cost / 2;
    let deadline = std::time::Instant::now() + EVICTION_WAIT_BUDGET;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        if let Some(now) = read_rss_bytes() {
            if before.saturating_sub(now) >= target_drop {
                tracing::debug!(
                    "fastembed cache: eviction freed {} MiB (target {} MiB)",
                    (before - now) / (1024 * 1024),
                    target_drop / (1024 * 1024),
                );
                return;
            }
        }
    }
    let now = read_rss_bytes().unwrap_or(before);
    tracing::warn!(
        "fastembed cache: eviction did not free expected memory within {}s (released {} of {} MiB); next load may be tight",
        EVICTION_WAIT_BUDGET.as_secs(),
        before.saturating_sub(now) / (1024 * 1024),
        evicted_cost / (1024 * 1024),
    );
}

/// glibc-specific: ask malloc to return freed pages from its arena
/// freelist back to the kernel via sbrk + munmap. Without this the
/// post-eviction RSS stays artificially high because freed ORT arenas
/// sit in glibc's freelist, even after the Arc<LoadedModel> has
/// dropped. The next model load then allocates ON TOP of the lingering
/// arena and trips the cgroup limit.
///
/// no-op on musl / non-Linux: nothing to call. The
/// `cfg(target_env = "gnu")` gate on the libc dep handles the
/// availability; the wrapping fn always exists so the call site is
/// portable.
#[cfg(target_env = "gnu")]
async fn trim_glibc_heap() {
    // malloc_trim can take low-tens-of-ms on a fragmented heap; hand
    // off to the blocking pool so the dispatcher's await point doesn't
    // block on it.
    let _ = tokio::task::spawn_blocking(|| {
        // SAFETY: `malloc_trim` is a glibc extension with a stable
        // signature `int malloc_trim(size_t pad)`. Returns 1 if memory
        // was released, 0 otherwise. We don't care about the return
        // value; we want the side effect.
        unsafe {
            libc::malloc_trim(0);
        }
    })
    .await;
}

#[cfg(not(target_env = "gnu"))]
async fn trim_glibc_heap() {
    // musl / macOS: nothing equivalent. Allocator returns pages
    // promptly on free anyway.
}

fn read_rss_bytes() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

/// The byte budget the fastembed model cache is sized at on this pod.
/// Exposed so the shared `MemBudget` can compute its own total as
/// `cgroup_limit - this - baseline_reserve` without duplicating the
/// env-override + cgroup-fraction logic.
pub fn compute_budget_bytes() -> u64 {
    if let Ok(v) = std::env::var("MINERVA_FASTEMBED_CACHE_BUDGET_BYTES") {
        if let Ok(n) = v.parse::<u64>() {
            return n;
        }
    }
    if let Some(limit) = read_cgroup_memory_limit() {
        return ((limit as f64) * DEFAULT_CACHE_BUDGET_FRACTION) as u64;
    }
    DEFAULT_CACHE_BUDGET_BYTES
}

fn budget_source() -> String {
    if std::env::var("MINERVA_FASTEMBED_CACHE_BUDGET_BYTES").is_ok() {
        "env override".to_string()
    } else if read_cgroup_memory_limit().is_some() {
        format!(
            "{}% of cgroup memory.max",
            (DEFAULT_CACHE_BUDGET_FRACTION * 100.0).round() as u32
        )
    } else {
        "static default (no cgroup, no env override)".to_string()
    }
}

fn read_cgroup_memory_limit() -> Option<u64> {
    // Try cgroup v2 first; fall back to v1. Both expose a single number in
    // bytes. v2 uses "max" (literal) for unlimited; v1 uses a sentinel that
    // exceeds physical memory. In either unlimited case we return None so
    // the caller falls back to the static default.
    if let Ok(s) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
        let trimmed = s.trim();
        if trimmed == "max" {
            return None;
        }
        if let Ok(n) = trimmed.parse::<u64>() {
            return Some(n);
        }
    }
    if let Ok(s) = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
        if let Ok(n) = s.trim().parse::<u64>() {
            // v1 sentinel "unlimited" tends to be a value larger than any
            // realistic RAM. Treat anything north of 1 PiB as unlimited.
            if n >= 1u64 << 50 {
                return None;
            }
            return Some(n);
        }
    }
    None
}

/// Representative text corpus for benchmarking embedding throughput.
/// Simulates typical document chunks (academic/educational content).
const BENCHMARK_CORPUS: &[&str] = &[
    "Machine learning is a subset of artificial intelligence that enables systems to learn from data.",
    "The gradient descent algorithm iteratively adjusts parameters to minimize a loss function.",
    "Neural networks consist of interconnected layers of nodes that process and transform input data.",
    "Overfitting occurs when a model learns noise in training data rather than the underlying pattern.",
    "Cross-validation helps estimate how well a model will generalize to unseen data.",
    "The transformer architecture relies on self-attention mechanisms to process sequential data.",
    "Regularization techniques like dropout and L2 penalty help prevent model overfitting.",
    "Transfer learning allows models pretrained on large datasets to be fine-tuned for specific tasks.",
    "Convolutional neural networks are particularly effective for image recognition tasks.",
    "Recurrent neural networks maintain hidden state to process sequences of variable length.",
    "The backpropagation algorithm computes gradients by applying the chain rule through network layers.",
    "Batch normalization stabilizes training by normalizing layer inputs across mini-batches.",
    "Attention mechanisms allow models to focus on relevant parts of the input when generating output.",
    "Embedding vectors represent discrete tokens as continuous vectors in a learned feature space.",
    "The softmax function converts raw logits into a probability distribution over classes.",
    "Data augmentation artificially increases training set size by applying transformations to existing data.",
    "Hyperparameter tuning involves searching for the optimal configuration of model training settings.",
    "The bias-variance tradeoff describes the tension between model simplicity and fitting capacity.",
    "Generative adversarial networks pit a generator against a discriminator in a minimax game.",
    "Reinforcement learning agents learn optimal policies through trial-and-error interaction with environments.",
    "The curse of dimensionality makes distance metrics less meaningful in high-dimensional spaces.",
    "Principal component analysis reduces dimensionality by projecting data onto orthogonal axes of maximum variance.",
    "Support vector machines find the hyperplane that maximizes the margin between classes.",
    "Random forests combine multiple decision trees to reduce variance and improve prediction accuracy.",
    "The ROC curve plots true positive rate against false positive rate at various classification thresholds.",
    "Feature engineering transforms raw data into representations that better capture underlying patterns.",
    "The learning rate controls the step size during gradient-based optimization of model parameters.",
    "Ensemble methods combine predictions from multiple models to achieve better generalization.",
    "Natural language processing enables computers to understand, interpret, and generate human language.",
    "Tokenization splits text into meaningful units that can be processed by language models.",
    "Word embeddings like Word2Vec capture semantic relationships between words in vector space.",
    "The BERT model uses bidirectional context to produce rich token representations for downstream tasks.",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_env_override_parses() {
        std::env::set_var("MINERVA_FASTEMBED_CACHE_BUDGET_BYTES", "12345");
        assert_eq!(compute_budget_bytes(), 12345);
        std::env::remove_var("MINERVA_FASTEMBED_CACHE_BUDGET_BYTES");
    }

    #[test]
    fn budget_falls_back_to_static_default_when_no_cgroup() {
        std::env::remove_var("MINERVA_FASTEMBED_CACHE_BUDGET_BYTES");
        // Either we're in a cgroup (CI or prod) and read_cgroup returns
        // a real number, or we're on macOS dev where it returns None and
        // we hit the static fallback. Both branches are valid; this
        // test just guards against a panic in compute_budget_bytes.
        let n = compute_budget_bytes();
        assert!(n > 0);
    }

    #[test]
    fn backend_dispatch_routes_arctic_m_v2_to_custom() {
        // The arctic-m-v2.0 ONNX is loaded through `UserDefinedEmbeddingModel`,
        // not the built-in `EmbeddingModel` enum. If somebody adds a new
        // model and accidentally drops the `custom_model_spec` arm, this
        // catches it before the route silently falls back to `Fast` and
        // panics deep inside `parse_fast_model_name` at boot.
        assert_eq!(
            backend_for("Snowflake/snowflake-arctic-embed-m-v2.0"),
            Backend::Custom,
        );
        assert_eq!(backend_for("Qwen/Qwen3-Embedding-0.6B"), Backend::Qwen3);
        assert_eq!(backend_for("BAAI/bge-m3"), Backend::Fast);
    }

    #[test]
    fn query_prefix_only_applied_to_arctic_m_v2() {
        // arctic-m-v2.0 is the one model we currently prefix at query time.
        assert_eq!(
            query_prefix_for_model("Snowflake/snowflake-arctic-embed-m-v2.0"),
            Some("query: "),
        );
        // Multilingual-e5 deliberately has no prefix wired up here even
        // though the model card recommends one; we'd need a per-course
        // rebuild with `passage:` on the doc side first. Guard the
        // omission so a well-meaning future change can't sneak it in.
        assert_eq!(
            query_prefix_for_model("intfloat/multilingual-e5-large"),
            None,
        );
        assert_eq!(query_prefix_for_model("BAAI/bge-m3"), None);
    }

    #[test]
    fn format_query_prepends_or_returns_owned_clone() {
        assert_eq!(
            format_query_for_model("Snowflake/snowflake-arctic-embed-m-v2.0", "hello world"),
            "query: hello world",
        );
        assert_eq!(
            format_query_for_model("BAAI/bge-m3", "hello world"),
            "hello world",
        );
    }

    /// Build a sentinel `EmbedJob` whose `texts` field is a single
    /// label, so tests can assert which job came out of `next_job`.
    /// The reply channel is created and immediately dropped; we never
    /// actually run inference in these tests.
    fn make_job(label: &str) -> EmbedJob {
        let (reply, _rx) = oneshot::channel();
        EmbedJob {
            texts: vec![label.to_string()],
            reply,
        }
    }

    fn job_label(job: &EmbedJob) -> &str {
        &job.texts[0]
    }

    #[tokio::test]
    async fn next_job_prefers_high_when_both_channels_ready() {
        // The whole point of the priority lane: if there's a queued
        // high job AND a queued low job at the moment the dispatcher
        // becomes ready, the high one runs first. `biased` in
        // `tokio::select!` is what guarantees this, so this test
        // would catch a regression that removes `biased` (the
        // select would then become arbitrary-order).
        let (high_tx, mut high_rx) = mpsc::unbounded_channel();
        let (low_tx, mut low_rx) = mpsc::unbounded_channel();

        // Submit low first to rule out "we just got the first one
        // sent." A wrong implementation that ignores priority and
        // returns whichever channel was filled first would surface
        // "low" here.
        low_tx.send(make_job("low")).unwrap();
        high_tx.send(make_job("high")).unwrap();

        let first = next_job(&mut high_rx, &mut low_rx).await.unwrap();
        assert_eq!(job_label(&first), "high");

        let second = next_job(&mut high_rx, &mut low_rx).await.unwrap();
        assert_eq!(job_label(&second), "low");
    }

    #[tokio::test]
    async fn next_job_takes_low_when_high_is_empty() {
        // Hot path for the steady-state ingest case: no chat traffic,
        // dispatcher still has work to do, must not block on the high
        // channel.
        let (_high_tx, mut high_rx) = mpsc::unbounded_channel();
        let (low_tx, mut low_rx) = mpsc::unbounded_channel();

        low_tx.send(make_job("low-only")).unwrap();

        let job = next_job(&mut high_rx, &mut low_rx).await.unwrap();
        assert_eq!(job_label(&job), "low-only");
    }

    #[tokio::test]
    async fn next_job_returns_none_when_both_senders_dropped() {
        // Dispatcher shutdown signal: cache evicts the entry, all
        // in-flight callers return, both `mpsc::UnboundedSender`s are
        // dropped, both `recv()` calls return None, `select!`'s `else`
        // arm fires, the dispatcher task exits. Without this the task
        // would leak.
        let (high_tx, mut high_rx) = mpsc::unbounded_channel::<EmbedJob>();
        let (low_tx, mut low_rx) = mpsc::unbounded_channel::<EmbedJob>();
        drop(high_tx);
        drop(low_tx);

        assert!(next_job(&mut high_rx, &mut low_rx).await.is_none());
    }

    #[tokio::test]
    async fn next_job_wakes_when_high_arrives_after_pending() {
        // Verifies the dispatcher actually parks (rather than spinning
        // or busy-erroring) when both channels are empty but senders
        // are still alive. We start the future, send a high job while
        // it's parked, and check it picks up.
        let (high_tx, mut high_rx) = mpsc::unbounded_channel();
        let (_low_tx, mut low_rx) = mpsc::unbounded_channel::<EmbedJob>();

        let waiter = tokio::spawn(async move {
            let job = next_job(&mut high_rx, &mut low_rx).await.unwrap();
            job_label(&job).to_string()
        });

        // Give the spawned task a chance to actually park on the
        // select. `yield_now` is enough here because both arms
        // immediately return Pending and re-register their wakers.
        tokio::task::yield_now().await;
        high_tx.send(make_job("late-high")).unwrap();

        assert_eq!(waiter.await.unwrap(), "late-high");
    }

    #[test]
    fn arctic_m_v2_is_in_valid_local_models() {
        // `pipeline::VALID_LOCAL_MODELS` is the catalog the rest of the
        // app keys off (qdrant collection dim, admin policy, teacher
        // dropdown). If the custom-backend dispatch exists but the
        // catalog entry is missing, course owners can't actually pick
        // the model.
        use minerva_catalog::VALID_LOCAL_MODELS;
        let entry = VALID_LOCAL_MODELS
            .iter()
            .find(|(name, _)| *name == "Snowflake/snowflake-arctic-embed-m-v2.0")
            .expect("arctic-m-v2.0 missing from VALID_LOCAL_MODELS");
        assert_eq!(entry.1, 768, "arctic-m-v2.0 is 768-dim, not {}", entry.1);
    }
}
