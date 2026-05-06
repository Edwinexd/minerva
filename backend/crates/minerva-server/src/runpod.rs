//! Thin HTTP client for RunPod's serverless async API.
//!
//! Why a hand-rolled client: the official `runpod` SDK is Python-only, the
//! REST surface is tiny (submit, status, list), and we want to keep
//! reqwest's connection pool shared with the rest of the backend rather
//! than ship a parallel client. Errors are mapped onto a single enum
//! consumed by the worker so retry/dead-letter decisions live in one place.
//!
//! Endpoints (per RunPod docs at <https://docs.runpod.io/serverless/endpoints/operations>):
//!
//!   POST {api_base}/v2/{endpoint_id}/run        async submit
//!   GET  {api_base}/v2/{endpoint_id}/status/{job_id}
//!   GET  {api_base}/v2/{endpoint_id}/runs       list recent jobs (for reconciliation)
//!
//! Auth: `Authorization: Bearer <RUNPOD_API_KEY>` on every request.

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum RunpodError {
    #[error("RunPod is not configured (RUNPOD_API_KEY / RUNPOD_ENDPOINT_ID missing)")]
    NotConfigured,
    #[error("RunPod request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("RunPod responded {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("RunPod response was not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Configuration captured once per request to keep `submit` etc. callable
/// without threading the whole `Config` struct through.
#[derive(Debug, Clone)]
pub struct RunpodConfig {
    pub api_base: String,
    pub api_key: String,
    pub endpoint_id: String,
}

impl RunpodConfig {
    /// Pull the three required pieces out of the app config. Returns
    /// `NotConfigured` so callers can short-circuit cleanly when the OCR
    /// pipeline is enabled but RunPod creds aren't set yet.
    pub fn from_app(config: &crate::config::Config) -> Result<Self, RunpodError> {
        let api_key = config
            .runpod_api_key
            .as_ref()
            .ok_or(RunpodError::NotConfigured)?;
        let endpoint_id = config
            .runpod_endpoint_id
            .as_ref()
            .ok_or(RunpodError::NotConfigured)?;
        Ok(Self {
            api_base: config.runpod_api_base.clone(),
            api_key: api_key.clone(),
            endpoint_id: endpoint_id.clone(),
        })
    }
}

/// Body of `POST /v2/{endpoint_id}/run`. RunPod swallows whatever JSON
/// we put under `input` and hands it to our handler unchanged.
#[derive(Debug, Serialize)]
struct SubmitRequest<'a> {
    input: &'a serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // status is logged but not branched on; keep as wire-format documentation
pub struct SubmitResponse {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // delay_time_ms reported by RunPod but not currently used; preserved for billing audits
pub struct StatusResponse {
    pub id: String,
    /// One of: IN_QUEUE, IN_PROGRESS, COMPLETED, FAILED, CANCELLED, TIMED_OUT
    pub status: String,
    /// Present on COMPLETED.
    #[serde(default)]
    pub output: Option<serde_json::Value>,
    /// Present on FAILED.
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    /// Wallclock execution time in milliseconds, when reported.
    #[serde(default, rename = "executionTime")]
    pub execution_time_ms: Option<i64>,
    /// Worker delay before pickup in milliseconds, when reported.
    #[serde(default, rename = "delayTime")]
    pub delay_time_ms: Option<i64>,
}

impl StatusResponse {
    /// GPU billable time. Best signal we get without querying the billing
    /// API; matches the wallclock the handler was running. If RunPod
    /// stops emitting it (it's omitted while IN_QUEUE), returns None.
    pub fn gpu_seconds(&self) -> Option<f32> {
        self.execution_time_ms.map(|ms| ms as f32 / 1000.0)
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // status surfaced to logs but not used for branching; preserved for debugging
pub struct ListedJob {
    pub id: String,
    pub status: String,
    /// The input we submitted. Used to recover client_request_id during
    /// reconciliation.
    #[serde(default)]
    pub input: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    jobs: Vec<ListedJob>,
}

/// Submit an async job. The handler reads `input.task` to dispatch and
/// `input.client_request_id` so we can find the job again if the
/// PATCH-after-submit window crashes on our side.
pub async fn submit(
    http: &reqwest::Client,
    cfg: &RunpodConfig,
    input: &serde_json::Value,
) -> Result<SubmitResponse, RunpodError> {
    let url = format!("{}/v2/{}/run", cfg.api_base, cfg.endpoint_id);
    let resp = http
        .post(&url)
        .bearer_auth(&cfg.api_key)
        .json(&SubmitRequest { input })
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RunpodError::Status { status, body });
    }
    let parsed: SubmitResponse = resp.json().await?;
    Ok(parsed)
}

/// Poll a single job. Used by the worker's poll loop on every tick.
pub async fn get_status(
    http: &reqwest::Client,
    cfg: &RunpodConfig,
    job_id: &str,
) -> Result<StatusResponse, RunpodError> {
    let url = format!("{}/v2/{}/status/{}", cfg.api_base, cfg.endpoint_id, job_id);
    let resp = http.get(&url).bearer_auth(&cfg.api_key).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RunpodError::Status { status, body });
    }
    let parsed: StatusResponse = resp.json().await?;
    Ok(parsed)
}

/// List recent jobs at the endpoint. Reconciliation matches input
/// payload's `client_request_id` against orphaned 'submitting' rows.
pub async fn list_recent(
    http: &reqwest::Client,
    cfg: &RunpodConfig,
) -> Result<Vec<ListedJob>, RunpodError> {
    // RunPod's listing endpoint returns the most recent few hundred jobs
    // by default; no pagination needed for our reconciliation window
    // (orphans older than ~5min that we never got an id back for).
    let url = format!("{}/v2/{}/runs", cfg.api_base, cfg.endpoint_id);
    let resp = http.get(&url).bearer_auth(&cfg.api_key).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RunpodError::Status { status, body });
    }
    let parsed: ListResponse = resp.json().await?;
    Ok(parsed.jobs)
}

/// Find a recent job whose input payload carries the given
/// `client_request_id`. Returns None if RunPod never received the
/// submission (network blip between our INSERT and our network call) so
/// the caller can mark the orphan failed and reschedule.
pub async fn find_by_client_id(
    http: &reqwest::Client,
    cfg: &RunpodConfig,
    client_request_id: &str,
) -> Result<Option<ListedJob>, RunpodError> {
    for job in list_recent(http, cfg).await? {
        if let Some(input) = &job.input {
            let matches = input
                .get("client_request_id")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == client_request_id);
            if matches {
                return Ok(Some(job));
            }
        }
    }
    Ok(None)
}
