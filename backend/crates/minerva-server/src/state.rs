use crate::config::Config;
use crate::lti::LtiKeyPair;
use minerva_ingest::fastembed_embedder::FastEmbedder;
use qdrant_client::Qdrant;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    pub db: PgPool,
    pub qdrant: Arc<Qdrant>,
    pub config: Arc<Config>,
    pub lti: Arc<LtiKeyPair>,
    pub http_client: reqwest::Client,
    pub fastembed: Arc<FastEmbedder>,
}

impl AppState {
    pub async fn new(config: &Config) -> anyhow::Result<Self> {
        let db = minerva_db::postgres::create_pool(&config.database_url).await?;
        let qdrant = minerva_db::qdrant::create_client(&config.qdrant_url)
            .await
            .map_err(|e| anyhow::anyhow!("qdrant connection failed: {}", e))?;

        let lti = LtiKeyPair::from_seed(&config.lti_key_seed)?;
        tracing::info!("LTI 1.3 provider ready (kid={})", lti.kid);

        let fastembed = Arc::new(FastEmbedder::new());

        // Benchmark all supported FastEmbed models on startup.
        tracing::info!("running fastembed model benchmarks...");
        match fastembed
            .run_benchmarks(minerva_ingest::pipeline::VALID_QDRANT_MODELS)
            .await
        {
            Ok(results) => {
                tracing::info!("fastembed benchmarks complete ({} models)", results.len());
            }
            Err(e) => {
                tracing::warn!("fastembed benchmarks failed: {}", e);
            }
        }

        Ok(Self {
            db,
            qdrant: Arc::new(qdrant),
            config: Arc::new(config.clone()),
            lti: Arc::new(lti),
            http_client: reqwest::Client::new(),
            fastembed,
        })
    }
}
