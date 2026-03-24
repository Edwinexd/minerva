use qdrant_client::Qdrant;

pub async fn create_client(url: &str) -> Result<Qdrant, Box<dyn std::error::Error>> {
    let client = Qdrant::from_url(url).build()?;
    tracing::info!("qdrant client connected to {}", url);
    Ok(client)
}
