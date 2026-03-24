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

fn fallback_models() -> Vec<Value> {
    vec![
        json!({ "id": "llama-3.3-70b", "name": "Llama 3.3 70B" }),
        json!({ "id": "llama-4-scout-17b-16e-instruct", "name": "Llama 4 Scout 17B" }),
        json!({ "id": "qwen-3-32b", "name": "Qwen 3 32B" }),
    ]
}
