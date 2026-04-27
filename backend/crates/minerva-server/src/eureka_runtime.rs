//! Compile-time-gated eureka-2 runtime context.
//!
//! Holds the LLM client, embedder, and extractor wired from env. Only
//! compiled when the `eureka` cargo feature is on; the rest of the
//! server crate references the type via `#[cfg(feature = "eureka")]`
//! gates, so a non-eureka build never imports the integration crate.
//!
//! Configuration (env, defaults in parens):
//!   EUREKA_LLM_PROVIDER       (openai_compatible)
//!   EUREKA_LLM_BASE_URL       (https://api.cerebras.ai/v1)
//!   EUREKA_LLM_MODEL          (llama-3.3-70b)
//!   EUREKA_LLM_API_KEY        (falls back to CEREBRAS_API_KEY)
//!   EUREKA_EMBED_PROVIDER     (openai_compatible)
//!   EUREKA_EMBED_BASE_URL     (https://api.openai.com/v1)
//!   EUREKA_EMBED_MODEL        (text-embedding-3-large)
//!   EUREKA_EMBED_DIM          (1024)
//!   EUREKA_EMBED_API_KEY      (falls back to OPENAI_API_KEY)
//!
//! Construction is fail-soft: if env is missing the runtime is `None`
//! and the admin endpoints return 503 instead of panicking at startup.
//! That way a Minerva deployment can ship with the feature compiled in
//! but unconfigured for individual environments.

use std::sync::Arc;

use minerva_eureka::eureka_2::embed::OpenAiCompatibleEmbedder;
use minerva_eureka::eureka_2::extract::Extractor;
use minerva_eureka::eureka_2::llm::{Anthropic, Gemini, LlmKind, OpenAiCompatible};

use crate::config::Config;

pub struct EurekaContext {
    pub embedder: Arc<OpenAiCompatibleEmbedder>,
    pub extractor: Arc<Extractor<LlmKind>>,
    pub embedding_dim: usize,
    pub llm_model: String,
    pub embed_model: String,
}

impl std::fmt::Debug for EurekaContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EurekaContext")
            .field("llm_model", &self.llm_model)
            .field("embed_model", &self.embed_model)
            .field("embedding_dim", &self.embedding_dim)
            .finish()
    }
}

impl EurekaContext {
    /// Build the runtime from env. Returns `None` (with a warning)
    /// when required keys are missing so the server can still boot.
    pub fn from_env(config: &Config) -> Option<Self> {
        let provider =
            std::env::var("EUREKA_LLM_PROVIDER").unwrap_or_else(|_| "openai_compatible".into());
        let llm_model =
            std::env::var("EUREKA_LLM_MODEL").unwrap_or_else(|_| "llama-3.3-70b".into());
        let llm_base_url = std::env::var("EUREKA_LLM_BASE_URL")
            .unwrap_or_else(|_| "https://api.cerebras.ai/v1".into());
        let llm_api_key =
            std::env::var("EUREKA_LLM_API_KEY").unwrap_or_else(|_| config.cerebras_api_key.clone());

        if llm_api_key.is_empty() {
            tracing::warn!(
                "eureka: no EUREKA_LLM_API_KEY or CEREBRAS_API_KEY set; concept-graph endpoints will return 503"
            );
            return None;
        }

        let llm = match provider.as_str() {
            "openai_compatible" => LlmKind::OpenAiCompatible(OpenAiCompatible::new(
                llm_base_url,
                llm_api_key,
                &llm_model,
            )),
            "anthropic" => LlmKind::Anthropic(Anthropic::new(llm_api_key, &llm_model)),
            "gemini" => LlmKind::Gemini(Gemini::new(llm_api_key, &llm_model)),
            other => {
                tracing::error!(
                    "eureka: unknown EUREKA_LLM_PROVIDER {} (expected openai_compatible | anthropic | gemini)",
                    other
                );
                return None;
            }
        };

        let embed_provider =
            std::env::var("EUREKA_EMBED_PROVIDER").unwrap_or_else(|_| "openai_compatible".into());
        if embed_provider != "openai_compatible" {
            tracing::error!(
                "eureka: only openai_compatible embed provider is supported; got {}",
                embed_provider
            );
            return None;
        }
        let embed_model =
            std::env::var("EUREKA_EMBED_MODEL").unwrap_or_else(|_| "text-embedding-3-large".into());
        let embed_base_url = std::env::var("EUREKA_EMBED_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let embed_api_key =
            std::env::var("EUREKA_EMBED_API_KEY").unwrap_or_else(|_| config.openai_api_key.clone());
        if embed_api_key.is_empty() {
            tracing::warn!(
                "eureka: no EUREKA_EMBED_API_KEY or OPENAI_API_KEY set; concept-graph endpoints will return 503"
            );
            return None;
        }
        let embedding_dim: usize = std::env::var("EUREKA_EMBED_DIM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1024);

        let embedder = OpenAiCompatibleEmbedder::new(
            embed_base_url,
            embed_api_key,
            &embed_model,
            embedding_dim,
        );

        let extractor = Arc::new(Extractor::new(llm));

        tracing::info!(
            llm_model = %llm_model,
            embed_model = %embed_model,
            embedding_dim,
            "eureka: runtime ready"
        );

        Some(Self {
            embedder: Arc::new(embedder),
            extractor,
            embedding_dim,
            llm_model,
            embed_model,
        })
    }
}
