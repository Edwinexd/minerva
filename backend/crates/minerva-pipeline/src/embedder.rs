use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
    usage: EmbeddingUsage,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
pub struct EmbeddingUsage {
    pub total_tokens: i64,
}

pub struct EmbeddingResult {
    pub embeddings: Vec<Vec<f32>>,
    pub total_tokens: i64,
}

/// Embed texts using OpenAI's text-embedding-3-small model.
/// Batches automatically (OpenAI supports up to 2048 inputs per request).
pub async fn embed_texts(
    client: &reqwest::Client,
    api_key: &str,
    texts: &[String],
) -> Result<EmbeddingResult, String> {
    if texts.is_empty() {
        return Ok(EmbeddingResult {
            embeddings: Vec::new(),
            total_tokens: 0,
        });
    }

    let batch_size = 512;
    let mut all_embeddings = Vec::with_capacity(texts.len());
    let mut total_tokens = 0i64;

    for batch in texts.chunks(batch_size) {
        let request = EmbeddingRequest {
            model: "text-embedding-3-small".to_string(),
            input: batch.to_vec(),
        };

        let response = client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("embedding request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("embedding API error {}: {}", status, body));
        }

        let result: EmbeddingResponse = response
            .json()
            .await
            .map_err(|e| format!("failed to parse embedding response: {}", e))?;

        total_tokens += result.usage.total_tokens;
        for item in result.data {
            all_embeddings.push(item.embedding);
        }
    }

    Ok(EmbeddingResult {
        embeddings: all_embeddings,
        total_tokens,
    })
}
