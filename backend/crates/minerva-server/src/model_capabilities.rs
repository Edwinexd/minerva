//! Per-model capability cache, populated by probing Cerebras on
//! demand.
//!
//! Two capabilities matter for the strategy layer:
//!
//! * `supports_tools` ; whether the model accepts `tools[]` +
//!   `tool_choice` in chat-completions. Required for the
//!   `tool_use_enabled` checkbox (both `simple+tools` and
//!   `flare+tools`).
//! * `supports_logprobs` ; whether the model returns per-token
//!   logprobs when `logprobs: true` is set. Required for the
//!   `flare` strategy (with or without tools) since the FLARE
//!   low-confidence detector reads `logprob` off each content
//!   delta. Note that Cerebras may *accept* `logprobs: true` and
//!   then return `null`; we treat that as "not supported".
//!
//! Why a runtime probe instead of a static table:
//!
//!   * Cerebras adds and retires models on its own schedule.
//!     A hardcoded match arm gets stale fast and silently
//!     downgrades unknown models even when they're perfectly
//!     capable.
//!   * Cerebras's `/v1/models` endpoint returns model ids only,
//!     no capability metadata, so we have to actually call
//!     `/v1/chat/completions` to find out.
//!
//! Probe cost: one extra Cerebras request the first time any
//! given model name is observed (config save or chat request).
//! Results are cached for the lifetime of the process; model
//! behaviour doesn't change without a name change, so a process
//! restart on rollout is sufficient invalidation.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub supports_tools: bool,
    pub supports_logprobs: bool,
}

impl Capabilities {
    /// Conservative default for a probe that errored. Treating an
    /// unprobeable model as "supports nothing" forces a teacher to
    /// pick a different model rather than letting them enable a
    /// feature that will silently 5xx at chat time.
    pub const fn none() -> Self {
        Self {
            supports_tools: false,
            supports_logprobs: false,
        }
    }
}

/// Live cache of capability probes. Owned by `AppState`; shared
/// across all request handlers via `Arc` cloning.
#[derive(Clone)]
pub struct CapabilityCache {
    inner: Arc<RwLock<HashMap<String, Capabilities>>>,
    base_url: Arc<str>,
    api_key: Arc<str>,
    http: reqwest::Client,
}

impl CapabilityCache {
    pub fn new(base_url: String, api_key: String, http: reqwest::Client) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            base_url: base_url.into(),
            api_key: api_key.into(),
            http,
        }
    }

    /// Test-only constructor: pre-populates the cache with hand-
    /// supplied entries so unit tests can exercise `validate_config`
    /// without doing any network I/O. The base_url is unused along
    /// the cache-hit path; we still require non-empty strings so a
    /// careless test that fell through to a probe would 4xx loudly
    /// instead of silently hanging.
    #[cfg(test)]
    pub fn for_tests(seed: &[(&str, Capabilities)]) -> Self {
        let mut map = HashMap::new();
        for (k, v) in seed {
            map.insert((*k).to_string(), *v);
        }
        Self {
            inner: Arc::new(RwLock::new(map)),
            base_url: "http://127.0.0.1:65535/unused".into(),
            api_key: "test".into(),
            http: reqwest::Client::new(),
        }
    }

    /// Return cached capabilities, probing once if cold.
    pub async fn lookup(&self, model: &str) -> Capabilities {
        if let Some(c) = self.inner.read().await.get(model).copied() {
            return c;
        }
        let probed = self.probe(model).await;
        // `insert` rather than `entry().or_insert` so a concurrent
        // probe for the same model just overwrites with the same
        // value; cheap.
        self.inner.write().await.insert(model.to_string(), probed);
        tracing::info!(
            "model_capabilities: probed {} -> tools={}, logprobs={}",
            model,
            probed.supports_tools,
            probed.supports_logprobs,
        );
        probed
    }

    /// Issue a minimal Cerebras chat-completions request that
    /// exercises both `tools` and `logprobs`. `tool_choice: "none"`
    /// asks the model NOT to actually emit a tool call ; we only
    /// want to know whether the API accepts the parameter shape.
    ///
    /// If the combined request fails, we fall back to a request
    /// without `tools` to disambiguate "tools rejected" from
    /// "logprobs rejected" from "transient error".
    async fn probe(&self, model: &str) -> Capabilities {
        let probe_tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "_capability_probe",
                "description": "Capability probe; not actually called.",
                "parameters": {"type": "object", "properties": {}}
            }
        });
        let with_both = serde_json::json!({
            "model": model,
            "stream": false,
            "logprobs": true,
            "top_logprobs": 1,
            "max_tokens": 4,
            "tools": [probe_tool],
            "tool_choice": "none",
            "messages": [{"role": "user", "content": "."}]
        });

        match self.send(&with_both).await {
            Ok(resp) => {
                // 200 with both fields present -> the API accepted
                // tools. Logprobs support hinges on whether the
                // response actually carries per-token data;
                // Cerebras may accept the flag and return null.
                let logprobs_present = resp
                    .pointer("/choices/0/logprobs/content/0/logprob")
                    .and_then(|v| v.as_f64())
                    .is_some();
                Capabilities {
                    supports_tools: true,
                    supports_logprobs: logprobs_present,
                }
            }
            Err(err_with) => {
                // Probe failed. Re-run without `tools` to isolate.
                let without_tools = serde_json::json!({
                    "model": model,
                    "stream": false,
                    "logprobs": true,
                    "top_logprobs": 1,
                    "max_tokens": 4,
                    "messages": [{"role": "user", "content": "."}]
                });
                match self.send(&without_tools).await {
                    Ok(resp) => {
                        // 200 without tools = tools is what the API
                        // rejected. Logprobs may still be advertised
                        // but null; check the payload.
                        let logprobs_present = resp
                            .pointer("/choices/0/logprobs/content/0/logprob")
                            .and_then(|v| v.as_f64())
                            .is_some();
                        Capabilities {
                            supports_tools: false,
                            supports_logprobs: logprobs_present,
                        }
                    }
                    Err(err_without) => {
                        // Both failed; could be model missing,
                        // network error, auth issue. Be loud and
                        // default-deny so a teacher can't enable a
                        // feature that won't work.
                        tracing::warn!(
                            "model_capabilities: probes of {} failed (with tools: {}, without: {}); defaulting to none()",
                            model,
                            err_with,
                            err_without,
                        );
                        Capabilities::none()
                    }
                }
            }
        }
    }

    async fn send(&self, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        let resp = self
            .http
            .post(self.base_url.as_ref())
            .bearer_auth(self.api_key.as_ref())
            .json(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            // Truncate body for the log; Cerebras can return
            // multi-kb HTML on 5xx.
            let snippet: String = text.chars().take(240).collect();
            return Err(format!("{}: {}", status, snippet));
        }
        resp.json().await.map_err(|e| e.to_string())
    }
}

/// Validate a `(model, strategy, tool_use_enabled)` triple before
/// persisting a course config. Refuses combinations the model
/// can't satisfy at runtime, surfacing the mismatch loudly to the
/// teacher rather than silently downgrading on every chat request.
///
/// Async because a cache miss triggers a probe.
pub async fn validate_config(
    cache: &CapabilityCache,
    model: &str,
    strategy: &str,
    tool_use_enabled: bool,
) -> Result<(), CapabilityMismatch> {
    let caps = cache.lookup(model).await;
    if tool_use_enabled && !caps.supports_tools {
        return Err(CapabilityMismatch::ToolsUnsupported);
    }
    if strategy == "flare" && !caps.supports_logprobs {
        return Err(CapabilityMismatch::LogprobsUnsupported);
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum CapabilityMismatch {
    /// `tool_use_enabled = true` but the chosen model doesn't
    /// accept `tools[]` / never returns `tool_calls`. Translation
    /// key: `course.model_does_not_support_tools`.
    ToolsUnsupported,
    /// `strategy = "flare"` but the chosen model doesn't return
    /// per-token logprobs (FLARE's low-confidence trigger reads
    /// them). Translation key:
    /// `course.model_does_not_support_logprobs`.
    LogprobsUnsupported,
}

impl CapabilityMismatch {
    pub fn translation_key(&self) -> &'static str {
        match self {
            CapabilityMismatch::ToolsUnsupported => "course.model_does_not_support_tools",
            CapabilityMismatch::LogprobsUnsupported => "course.model_does_not_support_logprobs",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: Capabilities = Capabilities {
        supports_tools: true,
        supports_logprobs: true,
    };
    const TOOLS_ONLY: Capabilities = Capabilities {
        supports_tools: true,
        supports_logprobs: false,
    };

    #[tokio::test]
    async fn validate_rejects_tools_on_capable_strategy_with_incapable_model() {
        let cache = CapabilityCache::for_tests(&[("toolless-model", Capabilities::none())]);
        let result = validate_config(&cache, "toolless-model", "simple", true).await;
        assert_eq!(result, Err(CapabilityMismatch::ToolsUnsupported));
    }

    #[tokio::test]
    async fn validate_rejects_flare_on_logprob_less_model() {
        let cache = CapabilityCache::for_tests(&[("llama3.1-8b", TOOLS_ONLY)]);
        let result = validate_config(&cache, "llama3.1-8b", "flare", false).await;
        assert_eq!(result, Err(CapabilityMismatch::LogprobsUnsupported));
    }

    #[tokio::test]
    async fn validate_accepts_simple_without_tools_on_any_capability_set() {
        // The cheapest combination has no capability requirement,
        // so even a probe that returned `none()` passes.
        let cache = CapabilityCache::for_tests(&[("unknown", Capabilities::none())]);
        assert!(validate_config(&cache, "unknown", "simple", false)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn validate_accepts_flare_plus_tools_on_full_model() {
        let cache = CapabilityCache::for_tests(&[("qwen-3-235b-a22b-instruct-2507", FULL)]);
        assert!(
            validate_config(&cache, "qwen-3-235b-a22b-instruct-2507", "flare", true)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn cache_hit_skips_probe() {
        // Cache pre-populated; lookup() should return the seeded
        // value without ever calling `probe` (which would 4xx
        // against the bogus base_url the test constructor uses).
        let cache = CapabilityCache::for_tests(&[("cached", FULL)]);
        assert_eq!(cache.lookup("cached").await, FULL);
    }

    #[tokio::test]
    async fn cache_miss_falls_back_to_none_when_probe_unreachable() {
        // No pre-seeded entry. The constructor's bogus base_url
        // means both probe paths error out, and `probe` falls
        // through to `Capabilities::none()`. This is the
        // default-deny path that protects misconfiguration.
        let cache = CapabilityCache::for_tests(&[]);
        let caps = cache.lookup("never-heard-of-this-one").await;
        assert_eq!(caps, Capabilities::none());
        // Second lookup should hit the cache (no second probe).
        assert_eq!(
            cache.lookup("never-heard-of-this-one").await,
            Capabilities::none()
        );
    }
}
