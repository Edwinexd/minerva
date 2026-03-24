use crate::config::Config;
use qdrant_client::Qdrant;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    pub db: PgPool,
    pub qdrant: Arc<Qdrant>,
    pub config: Arc<Config>,
}

impl AppState {
    pub async fn new(config: &Config) -> anyhow::Result<Self> {
        let db = minerva_db::postgres::create_pool(&config.database_url).await?;
        let qdrant = minerva_db::qdrant::create_client(&config.qdrant_url).await
            .map_err(|e| anyhow::anyhow!("qdrant connection failed: {}", e))?;

        Ok(Self {
            db,
            qdrant: Arc::new(qdrant),
            config: Arc::new(config.clone()),
        })
    }
}
