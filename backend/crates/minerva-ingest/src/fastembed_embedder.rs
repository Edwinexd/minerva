use std::sync::{Arc, Mutex};
use std::time::Instant;

use candle_core::{DType, Device};
use fastembed::{
    EmbeddingModel, InitOptions, InitOptionsUserDefined, Pooling, QuantizationMode,
    Qwen3TextEmbedding, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use hf_hub::api::sync::ApiBuilder;
use serde::Serialize;

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
/// state, baseline app heap, transient ONNX inference buffers) needs the
/// remainder. 40% is conservative; the worker doesn't only allocate models.
const DEFAULT_CACHE_BUDGET_FRACTION: f64 = 0.4;

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
/// Wrapped in `Arc<Mutex<...>>` so the cache, in-flight embed callers,
/// and benchmark callers can all share a single inner handle while the
/// outer `Mutex` serializes inference on that one model.
#[derive(Clone)]
enum LoadedModel {
    Fast(Arc<Mutex<TextEmbedding>>),
    Qwen3(Arc<Mutex<Qwen3TextEmbedding>>),
}

struct CacheEntry {
    name: String,
    model: LoadedModel,
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
#[derive(Default)]
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

/// Per-model query-side prefix for asymmetric retrieval models.
///
/// Some embedding models are trained with distinct prompt templates for
/// queries vs documents (`query: ...` for the search side, `passage: ...`
/// for the indexed side). Calling them without those prefixes works but
/// gives up some recall.
///
/// We only apply the *query-side* prefix here, and only for models that
/// have always been query-prefixed in production. The document side is
/// never touched because:
/// 1. Existing Qdrant collections were built without prefixes. Switching
///    document prefixing on at ingest would silently mismatch every
///    chunk currently in storage; a rebuild is the only correct fix and
///    that's an explicit migration, not an automatic one.
/// 2. For arctic-m-v2.0 specifically, the model card *only* prescribes a
///    query prefix; documents stay bare by design.
///
/// Multilingual-e5-* is intentionally not in this list: its training
/// regime expects both `query:` and `passage:` prefixes, so prefixing
/// only the query side against bare-embedded documents would be an
/// asymmetric mismatch that's likely to *hurt* retrieval more than the
/// missing prefix already does. Fixing E5 properly requires a per-course
/// re-embed and is out of scope here.
pub fn query_prefix_for_model(model: &str) -> Option<&'static str> {
    match model {
        "Snowflake/snowflake-arctic-embed-m-v2.0" => Some("query: "),
        _ => None,
    }
}

/// Apply the query-side prefix for `model` (if any) to `query` and
/// return an owned `String`. Cheap when no prefix is registered (one
/// move, no allocation). Used by the retrieval call sites in
/// `strategy::common`; the embed pipeline doesn't go through this.
pub fn format_query_for_model(model: &str, query: &str) -> String {
    match query_prefix_for_model(model) {
        Some(prefix) => format!("{prefix}{query}"),
        None => query.to_string(),
    }
}

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

    /// Embed texts using the given model name. The model is loaded on first
    /// use (may download weights). Subsequent calls for the same model hit
    /// the cache. Calls for different models may evict LRU entries to stay
    /// under the configured RSS budget.
    pub async fn embed(
        &self,
        model_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let model = self.acquire(model_name).await?;

        let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for batch in texts.chunks(EMBED_BATCH_SIZE) {
            let batch = batch.to_vec();
            let model = model.clone();
            let batch_embeddings = tokio::task::spawn_blocking(move || run_embed(&model, batch))
                .await
                .map_err(|e| format!("spawn_blocking failed: {}", e))??;
            all_embeddings.extend(batch_embeddings);
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
    async fn benchmark_inner(
        &self,
        model_name: &str,
        dimensions: u64,
    ) -> Result<BenchmarkResult, String> {
        let corpus: Vec<String> = BENCHMARK_CORPUS.iter().map(|s| s.to_string()).collect();
        let corpus_size = corpus.len();

        let model = self.acquire(model_name).await?;
        let texts = corpus.clone();
        let model_for_blocking = model.clone();

        let secs = tokio::task::spawn_blocking(move || {
            // Warmup run; first inference can be much slower than steady
            // state (ONNX session init, candle kernel JIT, allocator
            // touching pages for the first time, ...).
            let _ = run_embed(&model_for_blocking, vec!["warmup".to_string()]);
            let start = std::time::Instant::now();
            run_embed(&model_for_blocking, texts)?;
            Ok::<f64, String>(start.elapsed().as_secs_f64())
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {}", e))??;

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
    /// `last_used` and return a clone of the handle. Otherwise evict LRU
    /// entries until the new model's estimated cost fits under the budget,
    /// then load the model through the appropriate backend, measure the
    /// RSS delta, and insert it.
    async fn acquire(&self, model_name: &str) -> Result<LoadedModel, String> {
        let mut cache = self.cache.lock().await;

        if let Some(idx) = cache.iter().position(|e| e.name == model_name) {
            cache[idx].last_used = Instant::now();
            return Ok(cache[idx].model.clone());
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
            let evicted = cache.remove(lru_idx);
            let footprint: u64 = cache.iter().map(|e| e.rss_cost_bytes).sum();
            tracing::info!(
                "fastembed cache: evicting {} ({} MiB) to make room for {} (footprint {} MiB / budget {} MiB)",
                evicted.name,
                evicted.rss_cost_bytes / (1024 * 1024),
                model_name,
                footprint / (1024 * 1024),
                self.cache_budget_bytes / (1024 * 1024),
            );
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
        let rss_after = read_rss_bytes();

        let cost = match (rss_before, rss_after) {
            (Some(b), Some(a)) => a.saturating_sub(b),
            _ => ESTIMATED_MODEL_COST_BYTES,
        };

        cache.push(CacheEntry {
            name: model_name.to_string(),
            model: loaded.clone(),
            rss_cost_bytes: cost,
            last_used: Instant::now(),
        });

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

        Ok(loaded)
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

fn compute_budget_bytes() -> u64 {
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

fn budget_source() -> &'static str {
    if std::env::var("MINERVA_FASTEMBED_CACHE_BUDGET_BYTES").is_ok() {
        "env override"
    } else if read_cgroup_memory_limit().is_some() {
        "40% of cgroup memory.max"
    } else {
        "static default (no cgroup, no env override)"
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

    #[test]
    fn arctic_m_v2_is_in_valid_local_models() {
        // `pipeline::VALID_LOCAL_MODELS` is the catalog the rest of the
        // app keys off (qdrant collection dim, admin policy, teacher
        // dropdown). If the custom-backend dispatch exists but the
        // catalog entry is missing, course owners can't actually pick
        // the model.
        use crate::pipeline::VALID_LOCAL_MODELS;
        let entry = VALID_LOCAL_MODELS
            .iter()
            .find(|(name, _)| *name == "Snowflake/snowflake-arctic-embed-m-v2.0")
            .expect("arctic-m-v2.0 missing from VALID_LOCAL_MODELS");
        assert_eq!(entry.1, 768, "arctic-m-v2.0 is 768-dim, not {}", entry.1);
    }
}
