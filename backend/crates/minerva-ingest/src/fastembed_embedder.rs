use std::sync::{Arc, Mutex};

use candle_core::{DType, Device};
use fastembed::{EmbeddingModel, InitOptions, Qwen3TextEmbedding, TextEmbedding};
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

/// What's currently sitting in the single-slot cache. Two backends:
/// * **ONNX** (the default fastembed path) -- `TextEmbedding`.
/// * **Candle** for Qwen3-Embedding (separate `Qwen3TextEmbedding` API
///   enabled by the `qwen3` feature on fastembed).
///
/// Wrapped in `Arc<Mutex<...>>` so a single embed call can release the
/// outer slot lock if needed while still holding an exclusive handle.
/// In practice we hold the slot lock across the entire embed call (see
/// `embed`) to make cross-model loads queue instead of stacking.
#[derive(Clone)]
enum LoadedModel {
    Fast(Arc<Mutex<TextEmbedding>>),
    Qwen3(Arc<Mutex<Qwen3TextEmbedding>>),
}

/// Single-slot model cache around fastembed.
///
/// Only one model is resident at a time. The `current` lock is held across
/// the entire embed call so that concurrent requests for *different* models
/// queue behind each other instead of both loading and OOMing the pod.
/// Requests for the *same* model share the cached handle (the inner blocking
/// mutex inside `TextEmbedding` / `Qwen3TextEmbedding` already serializes
/// inference, so this matches prior throughput for the same-model case).
///
/// `benchmark_lock` is a separate `try_lock`-style mutex used only by the
/// admin "Run benchmark" path. It serves two purposes:
/// 1. Gives the admin UI a clean "Busy" affordance instead of silently
///    queueing multiple heavy model loads behind each other.
/// 2. Prevents an admin who fat-fingers the button N times from blocking
///    the worker for N × (load + benchmark) minutes -- only one
///    admin-triggered benchmark can be queued at a time, the rest are
///    rejected up front.
#[derive(Default)]
pub struct FastEmbedder {
    current: tokio::sync::Mutex<Option<(String, LoadedModel)>>,
    benchmarks: tokio::sync::Mutex<Vec<BenchmarkResult>>,
    benchmark_lock: tokio::sync::Mutex<()>,
}

/// Backend dispatch for a model id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Fast,
    Qwen3,
}

fn backend_for(model_name: &str) -> Backend {
    if model_name.starts_with("Qwen/") {
        Backend::Qwen3
    } else {
        Backend::Fast
    }
}

impl FastEmbedder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Embed texts using the given model name. The model is loaded on
    /// first use (may download weights). If a different model is
    /// currently cached it is dropped before the new one is loaded so
    /// peak memory stays at one model.
    pub async fn embed(
        &self,
        model_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut slot = self.current.lock().await;
        let model = Self::acquire(&mut slot, model_name).await?;

        let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for batch in texts.chunks(EMBED_BATCH_SIZE) {
            let batch = batch.to_vec();
            let model = model.clone();
            let batch_embeddings = tokio::task::spawn_blocking(move || run_embed(&model, batch))
                .await
                .map_err(|e| format!("spawn_blocking failed: {}", e))??;
            all_embeddings.extend(batch_embeddings);
        }

        drop(slot);
        Ok(all_embeddings)
    }

    /// Run the benchmark corpus against every model in `models`,
    /// sequentially. Each iteration goes through the same single-slot
    /// admission path as `embed`, so the boot benchmark loop is correctly
    /// interleaved with worker embeds rather than stacking models on top
    /// of them.
    ///
    /// Heavy models (Qwen3 0.6B, multilingual-e5-large, bge-m3, …) are
    /// NOT in the boot list -- they're loaded on first real embed call or
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

        // Replace the cached results wholesale -- matches the prior
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
    /// (which the single-slot cache would otherwise queue, blocking the
    /// worker for the duration).
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
    /// benchmark. Goes through the slot's `acquire` admission, so a
    /// benchmark on a different model evicts whatever was loaded -- same
    /// as a real embed -- and the slot lock keeps it from racing with
    /// worker embed calls.
    async fn benchmark_inner(
        &self,
        model_name: &str,
        dimensions: u64,
    ) -> Result<BenchmarkResult, String> {
        let corpus: Vec<String> = BENCHMARK_CORPUS.iter().map(|s| s.to_string()).collect();
        let corpus_size = corpus.len();

        let mut slot = self.current.lock().await;
        let model = Self::acquire(&mut slot, model_name).await?;
        let texts = corpus.clone();
        let model_for_blocking = model.clone();

        let secs = tokio::task::spawn_blocking(move || {
            // Warmup run -- first inference can be much slower than steady
            // state (ONNX session init, candle kernel JIT, allocator
            // touching pages for the first time, …).
            let _ = run_embed(&model_for_blocking, vec!["warmup".to_string()]);
            let start = std::time::Instant::now();
            run_embed(&model_for_blocking, texts)?;
            Ok::<f64, String>(start.elapsed().as_secs_f64())
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {}", e))??;

        drop(slot);

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
    /// `run_benchmarks` and worker embeds also serialize on the
    /// single-slot mutex but don't show up here, on purpose -- admins
    /// don't need to know about them.
    pub async fn is_benchmark_running(&self) -> bool {
        self.benchmark_lock.try_lock().is_err()
    }

    /// Single-slot admission. If the requested model matches what's
    /// already loaded, hand back the cached handle. Otherwise evict
    /// (drops the old `Arc`, freeing the model once the inner mutex
    /// has no other holders) and load the new one through the right
    /// backend.
    async fn acquire(
        slot: &mut Option<(String, LoadedModel)>,
        model_name: &str,
    ) -> Result<LoadedModel, String> {
        if let Some((name, model)) = slot.as_ref() {
            if name == model_name {
                return Ok(model.clone());
            }
            tracing::info!(
                "fastembed cache: evicting {} to make room for {}",
                name,
                model_name
            );
            *slot = None;
        }

        let backend = backend_for(model_name);
        let name = model_name.to_string();
        let model = tokio::task::spawn_blocking(move || -> Result<LoadedModel, String> {
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
            }
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {}", e))??;

        *slot = Some((model_name.to_string(), model.clone()));
        tracing::info!("fastembed: loaded model {}", model_name);
        Ok(model)
    }
}

/// Synchronous embed dispatch -- runs inside the spawn_blocking task so
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
