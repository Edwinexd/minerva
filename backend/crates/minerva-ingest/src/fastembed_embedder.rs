use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// Thread-safe wrapper around FastEmbed models.
/// Models are lazily initialized on first use and cached for reuse.
#[derive(Default)]
pub struct FastEmbedder {
    models: tokio::sync::Mutex<HashMap<String, Arc<Mutex<TextEmbedding>>>>,
}

impl FastEmbedder {
    pub fn new() -> Self {
        Self {
            models: tokio::sync::Mutex::new(HashMap::new()),
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
