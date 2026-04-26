use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "minerva" }))
}

#[derive(Deserialize)]
struct CerebrasModelsResponse {
    data: Vec<CerebrasModel>,
}

#[derive(Deserialize)]
struct CerebrasModel {
    id: String,
}

pub async fn models(State(state): State<AppState>) -> Json<Value> {
    let client = reqwest::Client::new();
    let result = client
        .get("https://api.cerebras.ai/v1/models")
        .header(
            "Authorization",
            format!("Bearer {}", state.config.cerebras_api_key),
        )
        .send()
        .await;

    let models = match result {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<CerebrasModelsResponse>().await {
                Ok(data) => data
                    .data
                    .into_iter()
                    .map(|m| {
                        let name = m.id.replace('-', " ");
                        // Capitalize first letter of each word
                        let name = name
                            .split_whitespace()
                            .map(|w| {
                                let mut c = w.chars();
                                match c.next() {
                                    None => String::new(),
                                    Some(f) => f.to_uppercase().to_string() + c.as_str(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        json!({ "id": m.id, "name": name })
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
                    tracing::warn!("failed to parse cerebras models: {}", e);
                    fallback_models()
                }
            }
        }
        Ok(resp) => {
            tracing::warn!("cerebras models API returned {}", resp.status());
            fallback_models()
        }
        Err(e) => {
            tracing::warn!("failed to fetch cerebras models: {}", e);
            fallback_models()
        }
    };

    Json(json!({ "models": models }))
}

pub async fn embedding_benchmarks(State(state): State<AppState>) -> Json<Value> {
    let results = state.fastembed.get_benchmarks().await;
    Json(json!({ "benchmarks": results }))
}

/// Auth-gated catalog feed for the teacher dropdown. Returns
/// `{ models: [{model, dimensions, benchmark | null}, …] }` filtered to
/// `enabled = true` rows. Anything an admin has disabled in
/// `/admin/system` disappears from the picker on next refetch.
///
/// Unknown / unseeded catalog entries are skipped silently rather than
/// shown disabled; the admin endpoint is the right place to surface
/// "this model exists in code but isn't in the policy table."
pub async fn embedding_models(
    State(state): State<AppState>,
) -> Result<Json<Value>, crate::error::AppError> {
    let benchmarks = state.fastembed.get_benchmarks().await;
    let bench_lookup: std::collections::HashMap<
        &str,
        &minerva_ingest::fastembed_embedder::BenchmarkResult,
    > = benchmarks.iter().map(|b| (b.model.as_str(), b)).collect();

    let policy: std::collections::HashMap<String, bool> =
        minerva_db::queries::embedding_models::list_all(&state.db)
            .await?
            .into_iter()
            .map(|r| (r.model, r.enabled))
            .collect();

    let models: Vec<Value> = minerva_ingest::pipeline::VALID_LOCAL_MODELS
        .iter()
        .filter(|(name, _)| policy.get(*name).copied().unwrap_or(false))
        .map(|(name, dims)| {
            let benchmark = bench_lookup.get(name).map(|b| {
                json!({
                    "model": b.model,
                    "dimensions": b.dimensions,
                    "embeddings_per_second": b.embeddings_per_second,
                    "total_ms": b.total_ms,
                    "corpus_size": b.corpus_size,
                })
            });
            json!({
                "model": name,
                "dimensions": dims,
                "benchmark": benchmark,
            })
        })
        .collect();

    Ok(Json(json!({ "models": models })))
}

fn fallback_models() -> Vec<Value> {
    vec![
        json!({ "id": "qwen-3-235b-a22b-instruct-2507", "name": "Qwen 3 235B A22B Instruct" }),
        json!({ "id": "llama3.1-8b", "name": "Llama 3.1 8B" }),
        json!({ "id": "gpt-oss-120b", "name": "GPT OSS 120B" }),
    ]
}
