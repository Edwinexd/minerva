//! Synthetic live memory-regression probe. Run through an operator SSH
//! tunnel after deploying a budget change; no course content is submitted.
//!
//! cargo build -p minerva-rpc --example embedding_memory_probe
//! target/debug/examples/embedding_memory_probe http://127.0.0.1:56051

use minerva_core::rpc::EmbedderClient;
use minerva_rpc::embedder::RemoteEmbedderClient;

#[tokio::main]
async fn main() -> Result<(), String> {
    let endpoint = std::env::args().nth(1).ok_or("expected an embedder URL")?;
    let client = RemoteEmbedderClient::connect(endpoint).await?;
    let models = [
        ("Snowflake/snowflake-arctic-embed-m-v2.0", 768),
        ("nomic-ai/nomic-embed-text-v1.5", 768),
        ("Qdrant/clip-ViT-B-32-text", 512),
    ];
    // Uneven multi-batch inputs, long enough to exercise tokenizer caps.
    // Mix code, Swedish and mathematical notation instead of only English
    // sentences: the previous character-sized warmup missed dense inputs.
    let texts: Vec<String> = (0..65)
        .map(|n| {
            format!(
                "sample {n}: {}",
                "räkna x_i=17; SELECT A[i] FROM T; sum(a*b) ".repeat(100)
            )
        })
        .collect();
    for round in 1..=3 {
        for (model, dimensions) in models {
            let started = std::time::Instant::now();
            let vectors = client.embed(model, texts.clone()).await?;
            if vectors.len() != texts.len()
                || vectors.iter().any(|v| {
                    v.len() != dimensions
                        || v.iter().any(|n| !n.is_finite())
                        || !v.iter().any(|n| *n != 0.0)
                })
            {
                return Err(format!("invalid vectors from {model}"));
            }
            println!(
                "round {round}: {model}: {} vectors, {dimensions} dimensions, {:.1}s",
                vectors.len(),
                started.elapsed().as_secs_f64()
            );
        }
    }
    Ok(())
}
