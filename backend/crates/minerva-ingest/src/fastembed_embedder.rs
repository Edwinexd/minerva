use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
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

/// Thread-safe wrapper around FastEmbed models.
/// Models are lazily initialized on first use and cached for reuse.
#[derive(Default)]
pub struct FastEmbedder {
    models: tokio::sync::Mutex<HashMap<String, Arc<Mutex<TextEmbedding>>>>,
    benchmarks: tokio::sync::Mutex<Vec<BenchmarkResult>>,
}

impl FastEmbedder {
    pub fn new() -> Self {
        Self {
            models: tokio::sync::Mutex::new(HashMap::new()),
            benchmarks: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Embed texts using the given model name.
    /// The model is loaded on first use (may download weights).
    pub async fn embed(
        &self,
        model_name: &str,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let model = self.get_or_init(model_name).await?;

        tokio::task::spawn_blocking(move || {
            let mut model = model
                .lock()
                .map_err(|e| format!("fastembed lock poisoned: {}", e))?;
            model
                .embed(texts, None)
                .map_err(|e| format!("fastembed embed failed: {}", e))
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {}", e))?
    }

    /// Benchmark all supported models and store results.
    /// Each model is loaded (warming the cache) and timed embedding a small corpus.
    pub async fn run_benchmarks(
        &self,
        models: &[(&str, u64)],
    ) -> Result<Vec<BenchmarkResult>, String> {
        let corpus: Vec<String> = BENCHMARK_CORPUS.iter().map(|s| s.to_string()).collect();
        let corpus_size = corpus.len();
        let mut results = Vec::new();

        for &(model_name, dimensions) in models {
            tracing::info!("benchmarking fastembed model: {}", model_name);

            let model = self.get_or_init(model_name).await?;
            let texts = corpus.clone();

            let result = tokio::task::spawn_blocking(move || {
                let mut model = model
                    .lock()
                    .map_err(|e| format!("fastembed lock poisoned: {}", e))?;

                // Warmup run (first inference can be slower due to ONNX session init)
                let _ = model.embed(vec!["warmup".to_string()], None);

                let start = std::time::Instant::now();
                let _ = model
                    .embed(texts, None)
                    .map_err(|e| format!("benchmark embed failed: {}", e))?;
                Ok::<f64, String>(start.elapsed().as_secs_f64())
            })
            .await
            .map_err(|e| format!("spawn_blocking failed: {}", e))??;

            let embeddings_per_second = corpus_size as f64 / result;
            let total_ms = result * 1000.0;

            tracing::info!(
                "  {}: {:.1} embeddings/sec ({:.0}ms for {} texts)",
                model_name,
                embeddings_per_second,
                total_ms,
                corpus_size,
            );

            results.push(BenchmarkResult {
                model: model_name.to_string(),
                dimensions,
                embeddings_per_second,
                total_ms,
                corpus_size,
            });
        }

        *self.benchmarks.lock().await = results.clone();
        Ok(results)
    }

    /// Get stored benchmark results.
    pub async fn get_benchmarks(&self) -> Vec<BenchmarkResult> {
        self.benchmarks.lock().await.clone()
    }

    async fn get_or_init(&self, model_name: &str) -> Result<Arc<Mutex<TextEmbedding>>, String> {
        let mut models = self.models.lock().await;

        if let Some(model) = models.get(model_name) {
            return Ok(Arc::clone(model));
        }

        let name = model_name.to_string();
        let model = tokio::task::spawn_blocking(move || {
            let model_enum = parse_model_name(&name)?;
            TextEmbedding::try_new(InitOptions::new(model_enum).with_show_download_progress(true))
                .map_err(|e| format!("fastembed init failed for {}: {}", name, e))
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {}", e))??;

        let model = Arc::new(Mutex::new(model));
        models.insert(model_name.to_string(), Arc::clone(&model));
        tracing::info!("fastembed: loaded model {}", model_name);
        Ok(model)
    }
}

fn parse_model_name(name: &str) -> Result<EmbeddingModel, String> {
    match name {
        "sentence-transformers/all-MiniLM-L6-v2" => Ok(EmbeddingModel::AllMiniLML6V2),
        "BAAI/bge-small-en-v1.5" => Ok(EmbeddingModel::BGESmallENV15),
        "BAAI/bge-base-en-v1.5" => Ok(EmbeddingModel::BGEBaseENV15),
        "nomic-ai/nomic-embed-text-v1.5" => Ok(EmbeddingModel::NomicEmbedTextV15),
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
