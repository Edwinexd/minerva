//! Provider-agnostic LLM abstraction.
//!
//! Every chat / classification call in Minerva used to hit Cerebras
//! directly through a hardcoded URL. This module introduces a thin
//! `ChatProvider` trait so OpenAI, Groq, Together, Anthropic, or any
//! self-hosted OpenAI-compatible endpoint are first-class, selected
//! per model. The concrete `OpenAiCompatibleProvider` is a verbatim
//! generalization of the old `cerebras_request_with_retry` +
//! `stream_cerebras_to_client` logic to a configurable base URL / key;
//! the SSE parsing, UTF-8 frame-carry, idle timeout, and
//! `stream_options.include_usage` behaviour are unchanged.
//!
//! Provider *credentials* stay in env/secret (`CEREBRAS_API_KEY`,
//! `OPENAI_API_KEY`, ...); the DB only ever references *which* provider
//! a model belongs to. The `LlmRegistry` is built once at startup and
//! holds an `Arc<dyn ChatProvider>` per provider whose key is present.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::config::Config;

/// Idle timeout between consecutive SSE frames. Protects every streaming
/// call against a silently-stalled TCP connection that never delivers
/// `[DONE]`. Applied per `stream.next().await`, not as a total deadline.
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Stable provider ids (the `chat_models.provider` column references these).
pub const PROVIDER_CEREBRAS: &str = "cerebras";
pub const PROVIDER_OPENAI: &str = "openai";
pub const PROVIDER_ANTHROPIC: &str = "anthropic";
pub const PROVIDER_GROQ: &str = "groq";

/// One normalized streaming delta from any provider.
#[derive(Debug, Clone, Default)]
pub struct ChatDelta {
    pub text: Option<String>,
    pub logprob: Option<f32>,
}

/// Final token usage, normalized across providers.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ChatUsage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

/// Wire shape a provider speaks. Drives request/response normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// `/v1/chat/completions`; OpenAI, Cerebras, Groq, Together, ...
    OpenAiCompatible,
    /// `/v1/messages`; Anthropic.
    Anthropic,
}

/// A single chat request, in Minerva's canonical (OpenAI message) shape.
/// Providers translate this to their own wire format.
pub struct ChatRequest<'a> {
    pub model: &'a str,
    /// OpenAI message array (`[{"role":..,"content":..}, ...]`).
    pub messages: &'a [Value],
    pub temperature: f64,
    pub max_tokens: Option<u32>,
    pub stream: bool,
    pub logprobs: bool,
}

/// A chat-completion backend. One instance per configured provider id.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Registry id (`"cerebras"`, `"openai"`, ...).
    fn id(&self) -> &str;
    /// Wire shape (drives capability-aware request building).
    fn kind(&self) -> ProviderKind;
    /// Whether the provider can return per-token logprobs (FLARE needs it).
    fn supports_logprobs(&self) -> bool;

    /// For OpenAI-compatible providers, the `(chat_url, api_key)` pair so
    /// the bespoke FLARE / research-phase streaming loops (which parse
    /// tool-calls + logprobs inline) can reuse the shared transport.
    /// `None` for non-OpenAI-compatible providers (Anthropic), which
    /// those loops never run against (capability-gated upstream).
    fn openai_endpoint(&self) -> Option<(&str, &str)> {
        None
    }

    /// Streaming chat: pushes normalized deltas onto `delta_tx` as they
    /// arrive, returns the final usage. The channel closes when the
    /// provider drops `delta_tx` (on completion / error).
    async fn stream(
        &self,
        req: ChatRequest<'_>,
        delta_tx: mpsc::Sender<ChatDelta>,
    ) -> Result<ChatUsage, String>;

    /// Non-streaming chat (classification / KG / aegis): full text + usage.
    async fn complete(&self, req: ChatRequest<'_>) -> Result<(String, ChatUsage), String>;
}

/// Covers Cerebras, OpenAI, Groq, Together, and any other endpoint that
/// speaks the OpenAI `/v1/chat/completions` protocol (request body, SSE
/// delta shape, `usage` block). Only the base URL + key differ.
pub struct OpenAiCompatibleProvider {
    id: String,
    /// Full chat-completions URL (base + `/chat/completions`).
    chat_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    /// `base_url` is the API base (e.g. `https://api.openai.com/v1`); the
    /// `/chat/completions` suffix is appended.
    pub fn new(
        id: impl Into<String>,
        base_url: &str,
        api_key: impl Into<String>,
        client: reqwest::Client,
    ) -> Self {
        let chat_url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        Self {
            id: id.into(),
            chat_url,
            api_key: api_key.into(),
            client,
        }
    }

    fn build_body(&self, req: &ChatRequest<'_>) -> Value {
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": req.messages,
            "temperature": req.temperature,
            "stream": req.stream,
        });
        if req.stream {
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        if let Some(max_tokens) = req.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }
        if req.logprobs {
            body["logprobs"] = Value::Bool(true);
            body["top_logprobs"] = serde_json::json!(1);
        }
        body
    }
}

#[async_trait]
impl ChatProvider for OpenAiCompatibleProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAiCompatible
    }

    fn supports_logprobs(&self) -> bool {
        true
    }

    fn openai_endpoint(&self) -> Option<(&str, &str)> {
        Some((&self.chat_url, &self.api_key))
    }

    async fn complete(&self, req: ChatRequest<'_>) -> Result<(String, ChatUsage), String> {
        let mut req = req;
        req.stream = false;
        let body = self.build_body(&req);
        let response = super::cerebras_request_with_retry_to(
            &self.client,
            &self.chat_url,
            &self.api_key,
            &body,
        )
        .await?;
        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("{} response decode: {e}", self.id))?;
        if let Some(err) = payload.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(msg.to_string());
        }
        let text = payload["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (prompt, completion) = super::extract_cerebras_usage(&payload).unwrap_or((0, 0));
        Ok((
            text,
            ChatUsage {
                prompt_tokens: prompt as i64,
                completion_tokens: completion as i64,
            },
        ))
    }

    async fn stream(
        &self,
        req: ChatRequest<'_>,
        delta_tx: mpsc::Sender<ChatDelta>,
    ) -> Result<ChatUsage, String> {
        let mut req = req;
        req.stream = true;
        let body = self.build_body(&req);
        let response = super::cerebras_request_with_retry_to(
            &self.client,
            &self.chat_url,
            &self.api_key,
            &body,
        )
        .await?;

        let mut stream = response.bytes_stream();
        // Raw TCP frames may split multi-byte UTF-8 codepoints across
        // chunks; accumulate bytes and promote only validated prefixes.
        let mut byte_carry: Vec<u8> = Vec::new();
        let mut buffer = String::new();
        let mut usage = ChatUsage::default();

        'outer: loop {
            let next = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
                Ok(n) => n,
                Err(_) => {
                    return Err(format!(
                        "{} stream idle timeout ({}s)",
                        self.id,
                        STREAM_IDLE_TIMEOUT.as_secs()
                    ));
                }
            };
            let chunk = match next {
                Some(Ok(c)) => c,
                Some(Err(e)) => {
                    tracing::error!("{} stream error: {}", self.id, e);
                    return Err(format!("Stream interrupted: {}", e));
                }
                None => break, // stream closed without [DONE]
            };
            byte_carry.extend_from_slice(&chunk);
            let valid_up_to = match std::str::from_utf8(&byte_carry) {
                Ok(_) => byte_carry.len(),
                Err(e) => e.valid_up_to(),
            };
            if valid_up_to > 0 {
                let valid_str = std::str::from_utf8(&byte_carry[..valid_up_to])
                    .expect("prefix was UTF-8 validated");
                buffer.push_str(valid_str);
                byte_carry.drain(..valid_up_to);
            }

            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim().to_string();
                buffer.drain(..=line_end);

                if line == "data: [DONE]" {
                    break 'outer;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                        if let Some(err) = parsed.get("error") {
                            let msg = err["message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string();
                            return Err(msg);
                        }

                        if let Some(text) = parsed["choices"][0]["delta"]["content"].as_str() {
                            let logprob = parsed["choices"][0]["logprobs"]["content"][0]["logprob"]
                                .as_f64()
                                .map(|v| v as f32);
                            if delta_tx
                                .send(ChatDelta {
                                    text: Some(text.to_string()),
                                    logprob,
                                })
                                .await
                                .is_err()
                            {
                                return Err("delta receiver dropped".to_string());
                            }
                        }

                        if let Some(u) = parsed.get("usage") {
                            if !u.is_null() {
                                usage.prompt_tokens = u["prompt_tokens"].as_i64().unwrap_or(0);
                                usage.completion_tokens =
                                    u["completion_tokens"].as_i64().unwrap_or(0);
                            }
                        }
                    }
                }
            }
        }

        Ok(usage)
    }
}

/// Registry of configured providers, keyed by provider id. Built once at
/// startup from env/secret; only providers whose key is present land in
/// the map, so `get` returns `None` for an unconfigured provider and the
/// admin layer can surface "provider key absent" before enabling a model.
pub struct LlmRegistry {
    providers: HashMap<String, Arc<dyn ChatProvider>>,
}

impl LlmRegistry {
    /// Build the registry from the process config + env. A provider is
    /// registered only when its API key is non-empty. Base URLs default
    /// to each provider's production endpoint and are overridable via
    /// `MINERVA_LLM_BASE_URL__<PROVIDER>` (uppercased id) for self-hosted
    /// or proxy deployments.
    pub fn from_config(client: reqwest::Client, config: &Config) -> Self {
        let mut providers: HashMap<String, Arc<dyn ChatProvider>> = HashMap::new();

        // OpenAI-compatible providers: (id, default base, key).
        let openai_compatible: [(&str, &str, String); 3] = [
            (
                PROVIDER_CEREBRAS,
                "https://api.cerebras.ai/v1",
                config.cerebras_api_key.clone(),
            ),
            (
                PROVIDER_OPENAI,
                "https://api.openai.com/v1",
                config.openai_api_key.clone(),
            ),
            (
                PROVIDER_GROQ,
                "https://api.groq.com/openai/v1",
                std::env::var("GROQ_API_KEY").unwrap_or_default(),
            ),
        ];

        for (id, default_base, key) in openai_compatible {
            if key.trim().is_empty() {
                continue;
            }
            let base = base_url_override(id).unwrap_or_else(|| default_base.to_string());
            providers.insert(
                id.to_string(),
                Arc::new(OpenAiCompatibleProvider::new(
                    id,
                    &base,
                    key,
                    client.clone(),
                )),
            );
            tracing::info!("llm registry: provider '{id}' configured");
        }

        Self { providers }
    }

    /// Resolve a provider by id. `None` when its key was absent at startup.
    pub fn get(&self, id: &str) -> Option<Arc<dyn ChatProvider>> {
        self.providers.get(id).cloned()
    }

    /// Whether a provider id is configured (its key was present).
    pub fn has(&self, id: &str) -> bool {
        self.providers.contains_key(id)
    }

    /// Sorted list of configured provider ids (for admin display).
    pub fn configured_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.providers.keys().cloned().collect();
        ids.sort();
        ids
    }
}

/// Read `MINERVA_LLM_BASE_URL__<PROVIDER>` (uppercased id), if set.
fn base_url_override(id: &str) -> Option<String> {
    let var = format!("MINERVA_LLM_BASE_URL__{}", id.to_uppercase());
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_provider_appends_chat_completions_path() {
        let p = OpenAiCompatibleProvider::new(
            "cerebras",
            "https://api.cerebras.ai/v1",
            "k",
            reqwest::Client::new(),
        );
        let (url, key) = p.openai_endpoint().unwrap();
        assert_eq!(url, "https://api.cerebras.ai/v1/chat/completions");
        assert_eq!(key, "k");
        assert_eq!(p.kind(), ProviderKind::OpenAiCompatible);
        assert!(p.supports_logprobs());
    }

    #[test]
    fn openai_provider_trims_trailing_slash() {
        let p = OpenAiCompatibleProvider::new(
            "openai",
            "https://api.openai.com/v1/",
            "k",
            reqwest::Client::new(),
        );
        assert_eq!(
            p.openai_endpoint().unwrap().0,
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn build_body_includes_logprobs_only_when_requested() {
        let p = OpenAiCompatibleProvider::new("x", "http://h/v1", "k", reqwest::Client::new());
        let messages = vec![serde_json::json!({"role": "user", "content": "hi"})];
        let req = ChatRequest {
            model: "m",
            messages: &messages,
            temperature: 0.0,
            max_tokens: None,
            stream: true,
            logprobs: false,
        };
        let body = p.build_body(&req);
        assert!(body.get("logprobs").is_none());
        assert_eq!(body["stream_options"]["include_usage"], Value::Bool(true));

        let req2 = ChatRequest {
            logprobs: true,
            ..req
        };
        let body2 = p.build_body(&req2);
        assert_eq!(body2["logprobs"], Value::Bool(true));
    }
}
