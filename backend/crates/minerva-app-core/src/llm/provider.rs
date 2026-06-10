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

/// One model id (+ optional friendly name) as reported by a provider's
/// `/models` listing endpoint. The catalog is populated by fetching
/// these, never by a hardcoded list.
#[derive(Debug, Clone)]
pub struct ProviderModel {
    pub id: String,
    pub display_name: Option<String>,
}

/// Timeout for the provider `/models` fetch so a slow/unreachable
/// provider can't stall startup.
const MODELS_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

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

    /// List the models this provider currently offers (its `/models`
    /// endpoint). Used to populate the `chat_models` catalog at startup
    /// instead of a hardcoded list, so the catalog tracks what the
    /// provider actually serves.
    async fn list_models(&self) -> Result<Vec<ProviderModel>, String>;
}

/// Parse the `data: [{id, display_name?}, ...]` envelope shared by the
/// OpenAI `/v1/models` and Anthropic `/v1/models` listings.
fn parse_models_envelope(payload: &Value) -> Vec<ProviderModel> {
    payload
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|i| i.as_str())?;
                    let display_name = m
                        .get("display_name")
                        .and_then(|n| n.as_str())
                        .map(String::from);
                    Some(ProviderModel {
                        id: id.to_string(),
                        display_name,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Filter out obvious non-chat models (embeddings, audio, image,
/// moderation) from a provider listing so the chat catalog stays sane.
/// Conservative substring match; the admin still curates by enabling.
fn is_probably_chat_model(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    const EXCLUDE: &[&str] = &[
        "embed",
        "whisper",
        "tts",
        "dall-e",
        "dalle",
        "moderation",
        "rerank",
        "transcribe",
        "audio",
        "image",
        "guard",
    ];
    !EXCLUDE.iter().any(|frag| lower.contains(frag))
}

/// Fetch each configured provider's model list and `seed_if_missing`
/// every chat model into the catalog (disabled + unpriced until an admin
/// enables and prices it). Best-effort: a provider that can't be reached
/// is logged and skipped. Returns the number of newly-seeded rows.
/// Capability flags default by provider kind (logprobs from the provider,
/// tool-use assumed available for chat models).
pub async fn sync_chat_models(registry: &LlmRegistry, db: &sqlx::PgPool) -> usize {
    let mut seeded = 0usize;
    for id in registry.configured_ids() {
        let Some(provider) = registry.get(&id) else {
            continue;
        };
        match provider.list_models().await {
            Ok(models) => {
                let logprobs = provider.supports_logprobs();
                for m in models {
                    if !is_probably_chat_model(&m.id) {
                        continue;
                    }
                    let display = m.display_name.unwrap_or_else(|| m.id.clone());
                    match minerva_db::queries::chat_models::seed_if_missing(
                        db, &m.id, &id, &display, logprobs, true,
                    )
                    .await
                    {
                        Ok(true) => {
                            seeded += 1;
                            tracing::info!(
                                "chat_models: seeded {} from provider {} (enabled=false)",
                                m.id,
                                id
                            );
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!("chat_models: seed failed for {}: {}", m.id, e)
                        }
                    }
                }
            }
            Err(e) => tracing::warn!(
                "chat_models: could not fetch models from provider {}: {}",
                id,
                e
            ),
        }
    }
    seeded
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

    async fn list_models(&self) -> Result<Vec<ProviderModel>, String> {
        let models_url = self
            .chat_url
            .strip_suffix("/chat/completions")
            .map(|base| format!("{base}/models"))
            .unwrap_or_else(|| self.chat_url.clone());
        let resp = self
            .client
            .get(&models_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .timeout(MODELS_FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("status {}", resp.status()));
        }
        let payload: Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_models_envelope(&payload))
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

/// Anthropic Messages API (`/v1/messages`). Different from the OpenAI
/// shape: `x-api-key` + `anthropic-version` headers, the system prompt
/// is a top-level `system` field (not a message), and the SSE event
/// stream is `message_start` / `content_block_delta` / `message_delta` /
/// `message_stop` rather than `choices[].delta`. Normalizes both to
/// `ChatDelta` / `ChatUsage`. No per-token logprobs, so
/// `supports_logprobs() == false` (the capability gate keeps FLARE off
/// Anthropic models).
pub struct AnthropicProvider {
    id: String,
    messages_url: String,
    api_key: String,
    version: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    /// `base_url` is the API root (e.g. `https://api.anthropic.com`); the
    /// `/v1/messages` suffix is appended.
    pub fn new(
        id: impl Into<String>,
        base_url: &str,
        api_key: impl Into<String>,
        client: reqwest::Client,
    ) -> Self {
        let messages_url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
        Self {
            id: id.into(),
            messages_url,
            api_key: api_key.into(),
            version: "2023-06-01".to_string(),
            client,
        }
    }

    /// Translate Minerva's canonical OpenAI-shaped messages into the
    /// Anthropic request body: leading `system` messages are hoisted into
    /// the top-level `system` field; the rest stay as user/assistant
    /// messages. `max_tokens` is mandatory for Anthropic (default 4096).
    fn build_body(&self, req: &ChatRequest<'_>) -> Value {
        let mut system_parts: Vec<String> = Vec::new();
        let mut messages: Vec<Value> = Vec::new();
        for m in req.messages {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            if role == "system" {
                if let Some(s) = m.get("content").and_then(|c| c.as_str()) {
                    system_parts.push(s.to_string());
                }
            } else {
                messages.push(serde_json::json!({
                    "role": role,
                    "content": m.get("content").cloned().unwrap_or_else(|| Value::String(String::new())),
                }));
            }
        }
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": messages,
            "max_tokens": req.max_tokens.unwrap_or(4096),
            "stream": req.stream,
            // Anthropic temperature range is [0, 1]; clamp the canonical
            // value so a higher OpenAI-style temperature doesn't 400.
            "temperature": req.temperature.clamp(0.0, 1.0),
        });
        if !system_parts.is_empty() {
            body["system"] = serde_json::json!(system_parts.join("\n\n"));
        }
        body
    }

    /// POST `/v1/messages` with the Anthropic auth headers and the same
    /// retry/backoff shape as the OpenAI-compatible transport (retry on
    /// 5xx / connect / timeout, fail fast on 4xx).
    async fn post_with_retry(&self, body: &Value) -> Result<reqwest::Response, String> {
        const MAX_RETRIES: u32 = 3;
        const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);
        let mut last_err = String::new();
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(INITIAL_BACKOFF * 2u32.pow(attempt - 1)).await;
            }
            let result = self
                .client
                .post(&self.messages_url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", &self.version)
                .header("content-type", "application/json")
                .json(body)
                .send()
                .await;
            match result {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    }
                    let body_text = response.text().await.unwrap_or_default();
                    if status.is_server_error() {
                        last_err = format!("anthropic {status}: {body_text}");
                        tracing::warn!("{}", last_err);
                        continue;
                    }
                    return Err(format!("anthropic {status}: {body_text}"));
                }
                Err(e) if e.is_timeout() || e.is_connect() => {
                    last_err = format!("Request failed: {e}");
                    continue;
                }
                Err(e) => return Err(format!("Request failed: {e}")),
            }
        }
        Err(last_err)
    }
}

#[async_trait]
impl ChatProvider for AnthropicProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn supports_logprobs(&self) -> bool {
        false
    }

    async fn complete(&self, req: ChatRequest<'_>) -> Result<(String, ChatUsage), String> {
        let mut req = req;
        req.stream = false;
        let body = self.build_body(&req);
        let response = self.post_with_retry(&body).await?;
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
        let text = payload["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        let usage = ChatUsage {
            prompt_tokens: payload["usage"]["input_tokens"].as_i64().unwrap_or(0),
            completion_tokens: payload["usage"]["output_tokens"].as_i64().unwrap_or(0),
        };
        Ok((text, usage))
    }

    async fn list_models(&self) -> Result<Vec<ProviderModel>, String> {
        let models_url = self
            .messages_url
            .strip_suffix("/v1/messages")
            .map(|base| format!("{base}/v1/models"))
            .unwrap_or_else(|| self.messages_url.clone());
        let resp = self
            .client
            .get(&models_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.version)
            .timeout(MODELS_FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("status {}", resp.status()));
        }
        let payload: Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_models_envelope(&payload))
    }

    async fn stream(
        &self,
        req: ChatRequest<'_>,
        delta_tx: mpsc::Sender<ChatDelta>,
    ) -> Result<ChatUsage, String> {
        let mut req = req;
        req.stream = true;
        let body = self.build_body(&req);
        let response = self.post_with_retry(&body).await?;

        let mut stream = response.bytes_stream();
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
                None => break,
            };
            byte_carry.extend_from_slice(&chunk);
            let valid_up_to = match std::str::from_utf8(&byte_carry) {
                Ok(_) => byte_carry.len(),
                Err(e) => e.valid_up_to(),
            };
            if valid_up_to > 0 {
                buffer.push_str(
                    std::str::from_utf8(&byte_carry[..valid_up_to])
                        .expect("prefix was UTF-8 validated"),
                );
                byte_carry.drain(..valid_up_to);
            }

            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim().to_string();
                buffer.drain(..=line_end);

                // Anthropic SSE interleaves `event: <type>` and
                // `data: <json>` lines. We dispatch off the JSON payload's
                // own `type` field and ignore the `event:` lines.
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                match parsed.get("type").and_then(|t| t.as_str()) {
                    Some("message_start") => {
                        if let Some(i) = parsed
                            .pointer("/message/usage/input_tokens")
                            .and_then(|v| v.as_i64())
                        {
                            usage.prompt_tokens = i;
                        }
                    }
                    Some("content_block_delta")
                        if parsed.pointer("/delta/type").and_then(|t| t.as_str())
                            == Some("text_delta") =>
                    {
                        if let Some(text) = parsed.pointer("/delta/text").and_then(|t| t.as_str()) {
                            if delta_tx
                                .send(ChatDelta {
                                    text: Some(text.to_string()),
                                    logprob: None,
                                })
                                .await
                                .is_err()
                            {
                                return Err("delta receiver dropped".to_string());
                            }
                        }
                    }
                    Some("message_delta") => {
                        if let Some(o) = parsed
                            .pointer("/usage/output_tokens")
                            .and_then(|v| v.as_i64())
                        {
                            usage.completion_tokens = o;
                        }
                    }
                    Some("message_stop") => break 'outer,
                    Some("error") => {
                        let msg = parsed
                            .pointer("/error/message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("anthropic stream error")
                            .to_string();
                        return Err(msg);
                    }
                    _ => {}
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

        // Anthropic (Messages API). Key from env, like the optional
        // OpenAI-compatible providers; base URL overridable for proxies.
        let anthropic_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        if !anthropic_key.trim().is_empty() {
            let base = base_url_override(PROVIDER_ANTHROPIC)
                .unwrap_or_else(|| "https://api.anthropic.com".to_string());
            providers.insert(
                PROVIDER_ANTHROPIC.to_string(),
                Arc::new(AnthropicProvider::new(
                    PROVIDER_ANTHROPIC,
                    &base,
                    anthropic_key,
                    client.clone(),
                )),
            );
            tracing::info!("llm registry: provider '{}' configured", PROVIDER_ANTHROPIC);
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
    fn anthropic_body_hoists_system_and_defaults_max_tokens() {
        let p = AnthropicProvider::new(
            "anthropic",
            "https://api.anthropic.com",
            "k",
            reqwest::Client::new(),
        );
        let messages = vec![
            serde_json::json!({"role": "system", "content": "sys A"}),
            serde_json::json!({"role": "user", "content": "hi"}),
        ];
        let req = ChatRequest {
            model: "m",
            messages: &messages,
            temperature: 0.9,
            max_tokens: None,
            stream: true,
            // logprobs is ignored by Anthropic (no per-token logprobs).
            logprobs: true,
        };
        let body = p.build_body(&req);
        assert_eq!(body["system"], "sys A");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(body["max_tokens"], 4096);
        assert!(body.get("logprobs").is_none());
        assert_eq!(p.kind(), ProviderKind::Anthropic);
        assert!(!p.supports_logprobs());
        assert!(p.openai_endpoint().is_none());
        assert_eq!(p.messages_url, "https://api.anthropic.com/v1/messages");
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
