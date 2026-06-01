//! Cross-encoder re-ranking for RAG retrieval.
//!
//! The embedding search (`strategy::common::embedding_search`) is a
//! bi-encoder: query and chunk are embedded *independently* and compared
//! by cosine. That is fast (one ANN lookup) but blind to token-level
//! interaction between the query and each candidate. A cross-encoder
//! re-ranker reads the `(query, chunk)` pair *together* through a
//! transformer and emits a single relevance logit, a much stronger
//! relevance signal at the cost of one forward pass per candidate. The
//! standard recipe (and the one we use) is:
//!
//!   1. over-fetch a candidate pool from the vector store (top-N, N >> k),
//!   2. score every candidate with the cross-encoder,
//!   3. keep the top-k by re-rank score.
//!
//! Re-ranking is independent of the embedding model: it reads chunk
//! *text*, not vectors, so switching a course's re-ranker never requires
//! a Qdrant collection rebuild (unlike rotating the embedding model), and
//! it works the same whether the course embeds locally or via OpenAI.
//!
//! ## Model selection
//!
//! The active re-ranker is chosen *per course* (`courses.reranker_model`),
//! mirroring how the embedding model is chosen. The admin-managed
//! `reranker_models` catalog gates which models a teacher may pick and
//! which is the default for new courses; see
//! `minerva_db::queries::reranker_models`. [`VALID_RERANKER_MODELS`] is
//! the compile-time catalog of model ids the runtime knows how to load,
//! the same role `pipeline::VALID_LOCAL_MODELS` plays for embeddings.
//!
//! The default ([`DEFAULT_RERANK_MODEL`]) is
//! `jinaai/jina-reranker-v2-base-multilingual` (~278M params). SU/DSV
//! course content is mixed Swedish + English, so a multilingual
//! cross-encoder matters; this one is the lightest multilingual model
//! fastembed ships, which keeps the resident RSS cost modest relative to
//! the heavier `rozgo/bge-reranker-v2-m3`.
//!
//! ## Concurrency / lifetime
//!
//! Models load lazily on first use (download + ONNX session build, both
//! slow) and then stay resident for the process lifetime in a per-model
//! cache, keyed by model id. The resident set is bounded by the
//! admin-enabled catalog (at most [`VALID_RERANKER_MODELS`] entries, and
//! in practice the one or two models courses actually select). Inference
//! (`TextRerank::rerank`) takes `&mut self` and is CPU-bound, so it runs
//! inside `spawn_blocking` behind an `Arc<Mutex<_>>`, exactly like the
//! embedder's per-model handle. Re-ranking only ever happens on the
//! interactive chat path (never bulk ingest), so there is no
//! ingest-vs-chat priority contention to manage here; a plain mutex per
//! model is sufficient.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fastembed::{RerankInitOptions, RerankerModel, TextRerank};
use serde::Serialize;

/// Default cross-encoder. Multilingual (Swedish + English), the lightest
/// multilingual model in fastembed's reranker catalog. Mirrored by the
/// `courses.reranker_model` column DEFAULT and the `reranker_models`
/// seed; kept here so validation / fallbacks have a single source.
pub use minerva_catalog::DEFAULT_RERANK_MODEL;

/// Compile-time catalog of re-ranker model ids the runtime can load.
///
/// Policy (which of these a teacher may actually pick, and which is the
/// default for new courses) lives in the `reranker_models` DB table;
/// this slice is just "code exists for these". Mirrors
/// `pipeline::VALID_LOCAL_MODELS` for embeddings. Each id must be a
/// `model_code` fastembed's [`RerankerModel`] understands (asserted in
/// tests).
pub use minerva_catalog::VALID_RERANKER_MODELS;

/// Token cap per `(query, chunk)` pair. Course chunks are ~500 tokens
/// (2000 chars) and queries are short, so 512 comfortably covers a pair
/// while bounding the per-candidate compute (attention cost grows with
/// sequence length).
const RERANK_MAX_LENGTH: usize = 512;

/// Candidates scored per inference batch. Keeps peak tensor memory
/// predictable when the candidate pool is large.
const RERANK_BATCH_SIZE: usize = 16;

/// Result of benchmarking one re-ranker model.
///
/// The cross-encoder's meaningful throughput metric is `(query, passage)`
/// pairs scored per second (one forward pass each), which is what a chat
/// turn pays when re-ranking its candidate pool. This is distinct from
/// the embedder's `embeddings_per_second` (bi-encoder, one pass per text).
#[derive(Clone, Debug, Serialize)]
pub struct RerankBenchmarkResult {
    pub model: String,
    pub pairs_per_second: f64,
    pub total_ms: f64,
    pub pairs: usize,
}

/// Errors from `benchmark_one`. `Busy` is the soft case (another
/// benchmark is already in flight); the route layer maps it to the same
/// `admin.benchmark_busy` code the embedding benchmark uses.
#[derive(Debug)]
pub enum BenchmarkError {
    Busy,
    Failed(String),
}

impl std::fmt::Display for BenchmarkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BenchmarkError::Busy => write!(f, "another reranker benchmark is already running"),
            BenchmarkError::Failed(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for BenchmarkError {}

/// Query used by the benchmark. Phrased like a typical course question so
/// the tokenised pair length is representative.
const RERANK_BENCHMARK_QUERY: &str =
    "Which techniques help a neural network generalize and avoid overfitting?";

/// Passage set scored against the benchmark query. A mix of on-topic and
/// off-topic academic sentences so the model does real work (it can't
/// short-circuit on trivially-irrelevant input). Length (~24 pairs) keeps
/// a benchmark run to a few seconds on the heavier models while still
/// being a stable throughput sample.
const RERANK_BENCHMARK_DOCS: &[&str] = &[
    "Dropout randomly zeroes activations during training to reduce overfitting.",
    "L2 weight decay penalizes large parameters and improves generalization.",
    "Early stopping halts training when validation loss stops improving.",
    "Data augmentation expands the training set with label-preserving transforms.",
    "Batch normalization stabilizes training and has a mild regularizing effect.",
    "Cross-validation estimates how well a model generalizes to unseen data.",
    "A larger training set usually reduces variance and overfitting.",
    "Label smoothing softens targets and discourages overconfident predictions.",
    "Transfer learning reuses features learned on a large pretraining corpus.",
    "The bias-variance tradeoff frames model complexity against fitting capacity.",
    "Ensembles average several models to lower variance.",
    "Weight initialization affects how quickly and stably a network trains.",
    "The French Revolution began in 1789 and reshaped European politics.",
    "TCP guarantees in-order delivery of bytes over an unreliable network.",
    "Photosynthesis converts light energy into chemical energy in plants.",
    "A binary search tree keeps keys ordered for logarithmic lookups.",
    "The Krebs cycle releases energy through the oxidation of acetyl-CoA.",
    "Supply and demand curves intersect at the market equilibrium price.",
    "Plate tectonics explains the slow drift of continents over time.",
    "A hash table offers average constant-time insertion and lookup.",
    "The speed of light in a vacuum is roughly 299,792 kilometers per second.",
    "Mitochondria are the primary site of ATP production in eukaryotic cells.",
    "Gradient descent iteratively steps parameters down the loss surface.",
    "Regularization trades a little training accuracy for better test accuracy.",
];

/// Lazily-loaded, per-model cross-encoder re-ranker cache.
///
/// Cheap to construct (empty cache); each model is loaded on first use
/// and kept resident afterwards. Keyed by the catalog model id.
pub struct FastReranker {
    /// `model_id -> loaded handle`. The outer `tokio::sync::Mutex`
    /// guards admission (lookup + load + insert); held across the load
    /// so two concurrent first-uses of the same model don't both
    /// download it. The inner `std::sync::Mutex<TextRerank>` serialises
    /// the `&mut self` inference call inside `spawn_blocking`.
    cache: tokio::sync::Mutex<HashMap<String, Arc<Mutex<TextRerank>>>>,
    /// Latest benchmark result per model, populated on demand by the
    /// admin "Run benchmark" button. In-memory only (lost on restart),
    /// same as the embedder's benchmark store.
    benchmarks: tokio::sync::Mutex<Vec<RerankBenchmarkResult>>,
    /// Serializes admin-triggered benchmarks (one model loaded + scored
    /// at a time) so a fat-fingered double-click can't stack two heavy
    /// model loads. `try_lock` gives the UI a clean "busy" affordance.
    benchmark_lock: tokio::sync::Mutex<()>,
}

impl Default for FastReranker {
    fn default() -> Self {
        Self::new()
    }
}

impl FastReranker {
    pub fn new() -> Self {
        Self {
            cache: tokio::sync::Mutex::new(HashMap::new()),
            benchmarks: tokio::sync::Mutex::new(Vec::new()),
            benchmark_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Benchmark one model: score the fixed `(query, passage)` set and
    /// report pairs/sec. A warmup pass (which also triggers the lazy
    /// load + download on first use) precedes the timed pass so the
    /// measurement reflects steady-state inference, not ONNX session
    /// init. Acquires `benchmark_lock` non-blockingly so a concurrent
    /// click bounces with `Busy` rather than stacking model loads.
    pub async fn benchmark_one(
        &self,
        model_code: &str,
    ) -> Result<RerankBenchmarkResult, BenchmarkError> {
        let _guard = self
            .benchmark_lock
            .try_lock()
            .map_err(|_| BenchmarkError::Busy)?;

        let query = RERANK_BENCHMARK_QUERY.to_string();
        let docs: Vec<String> = RERANK_BENCHMARK_DOCS
            .iter()
            .map(|s| s.to_string())
            .collect();
        let pairs = docs.len();

        // Warmup (also loads/downloads the model on first use). Errors
        // here are surfaced by the timed run below, so ignore them.
        let _ = self.rerank(model_code, query.clone(), docs.clone()).await;

        let start = Instant::now();
        self.rerank(model_code, query, docs)
            .await
            .map_err(BenchmarkError::Failed)?;
        let secs = start.elapsed().as_secs_f64();

        let result = RerankBenchmarkResult {
            model: model_code.to_string(),
            pairs_per_second: if secs > 0.0 { pairs as f64 / secs } else { 0.0 },
            total_ms: secs * 1000.0,
            pairs,
        };
        tracing::info!(
            "reranker benchmark {}: {:.1} pairs/sec ({:.0}ms for {} pairs)",
            result.model,
            result.pairs_per_second,
            result.total_ms,
            result.pairs,
        );

        let mut benchmarks = self.benchmarks.lock().await;
        if let Some(existing) = benchmarks.iter_mut().find(|b| b.model == result.model) {
            *existing = result.clone();
        } else {
            benchmarks.push(result.clone());
        }
        Ok(result)
    }

    /// Snapshot of stored benchmark results.
    pub async fn get_benchmarks(&self) -> Vec<RerankBenchmarkResult> {
        self.benchmarks.lock().await.clone()
    }

    /// True if an admin-triggered benchmark is currently running. The
    /// admin UI greys out every "Run benchmark" button while one is in
    /// flight.
    pub async fn is_benchmark_running(&self) -> bool {
        self.benchmark_lock.try_lock().is_err()
    }

    /// Get (loading on first use) the handle for `model_code`. Holds the
    /// admission lock across the load so a concurrent first-use of the
    /// same model waits rather than racing a second download.
    async fn handle(&self, model_code: &str) -> Result<Arc<Mutex<TextRerank>>, String> {
        let mut cache = self.cache.lock().await;
        if let Some(handle) = cache.get(model_code) {
            metrics::counter!("reranker_cache_hits_total", "model" => model_code.to_string())
                .increment(1);
            return Ok(Arc::clone(handle));
        }
        metrics::counter!("reranker_cache_misses_total", "model" => model_code.to_string())
            .increment(1);
        let code = model_code.to_string();
        let load_start = std::time::Instant::now();
        let loaded =
            tokio::task::spawn_blocking(move || -> Result<Arc<Mutex<TextRerank>>, String> {
                let model: RerankerModel = code
                    .parse()
                    .map_err(|e| format!("unknown reranker model {code}: {e}"))?;
                let rerank = TextRerank::try_new(
                    RerankInitOptions::new(model)
                        .with_max_length(RERANK_MAX_LENGTH)
                        .with_show_download_progress(true),
                )
                .map_err(|e| format!("reranker init failed for {code}: {e}"))?;
                tracing::info!("reranker: loaded model {code}");
                Ok(Arc::new(Mutex::new(rerank)))
            })
            .await
            .map_err(|e| format!("reranker load spawn_blocking failed: {e}"))??;
        metrics::histogram!("reranker_model_load_seconds", "model" => model_code.to_string())
            .record(load_start.elapsed().as_secs_f64());
        cache.insert(model_code.to_string(), Arc::clone(&loaded));
        // Resident-model gauges for the Models dashboard. The reranker
        // cache never evicts (bounded by the small admin catalog), so
        // these only climb from 0 to 1 per model; the timeline still
        // shows when each model first loaded.
        metrics::gauge!("reranker_models_loaded", "model" => model_code.to_string()).set(1.0);
        metrics::gauge!("reranker_cache_models_count").set(cache.len() as f64);
        Ok(loaded)
    }

    /// Score every `(query, document)` pair with the `model_code`
    /// cross-encoder and return `(original_index, score)` pairs sorted by
    /// score descending.
    ///
    /// `original_index` indexes into `documents` as passed, so the caller
    /// can reorder its own parallel metadata. Returns an empty vec for an
    /// empty input. Errors (model load failure, inference failure) are
    /// surfaced to the caller, which is expected to fail open (keep the
    /// embedding-order results) rather than break the chat turn.
    pub async fn rerank(
        &self,
        model_code: &str,
        query: String,
        documents: Vec<String>,
    ) -> Result<Vec<(usize, f32)>, String> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let handle = self.handle(model_code).await?;
        tokio::task::spawn_blocking(move || -> Result<Vec<(usize, f32)>, String> {
            let mut model = handle
                .lock()
                .map_err(|e| format!("reranker lock poisoned: {e}"))?;
            // `return_documents = false`: we only need indices + scores;
            // the caller still owns the original chunk objects.
            let results = model
                .rerank(query, &documents, false, Some(RERANK_BATCH_SIZE))
                .map_err(|e| format!("rerank inference failed: {e}"))?;
            Ok(results.into_iter().map(|r| (r.index, r.score)).collect())
        })
        .await
        .map_err(|e| format!("rerank spawn_blocking failed: {e}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_in_catalog() {
        assert!(
            VALID_RERANKER_MODELS.contains(&DEFAULT_RERANK_MODEL),
            "default reranker must be a catalog member",
        );
    }

    #[test]
    fn every_catalog_id_is_a_known_reranker() {
        // Guard against a typo'd catalog id that would only blow up on the
        // first live rerank. `RerankerModel: FromStr` matches on
        // fastembed's model_code list, so a parse failure here means the
        // catalog drifted from what fastembed can actually load.
        for id in VALID_RERANKER_MODELS {
            let parsed: Result<RerankerModel, _> = id.parse();
            assert!(parsed.is_ok(), "catalog reranker id not recognized: {id}");
        }
    }

    #[tokio::test]
    async fn empty_documents_short_circuit() {
        let r = FastReranker::new();
        // No model load happens for an empty candidate set, so this runs
        // without any weights on disk.
        let out = r
            .rerank(DEFAULT_RERANK_MODEL, "anything".to_string(), Vec::new())
            .await
            .unwrap();
        assert!(out.is_empty());
    }

    /// Live smoke test: loads the real default model (downloads weights
    /// on first run) and checks the cross-encoder ranks a topically
    /// relevant passage above an irrelevant one. Ignored by default
    /// because it needs network + ~280MB of weights; run with
    /// `cargo nextest run -p minerva-embed-engine --run-ignored all -E 'test(live_rerank)'`.
    #[tokio::test]
    #[ignore = "downloads model weights; run manually"]
    async fn live_rerank_orders_relevant_first() {
        let reranker = FastReranker::new();
        let query = "How does photosynthesis convert sunlight into energy?".to_string();
        let docs = vec![
            // 0: irrelevant
            "The French Revolution began in 1789 and reshaped European politics.".to_string(),
            // 1: relevant
            "Photosynthesis lets plants turn light energy into chemical energy stored as glucose."
                .to_string(),
            // 2: irrelevant
            "TCP guarantees in-order delivery of bytes over an unreliable network.".to_string(),
        ];
        let order = reranker
            .rerank(DEFAULT_RERANK_MODEL, query, docs)
            .await
            .expect("rerank");
        assert_eq!(order.len(), 3);
        // The relevant passage (original index 1) must rank first.
        assert_eq!(
            order[0].0, 1,
            "expected relevant doc ranked first: {order:?}"
        );
        // Scores must be sorted descending.
        assert!(order[0].1 >= order[1].1 && order[1].1 >= order[2].1);
    }
}
