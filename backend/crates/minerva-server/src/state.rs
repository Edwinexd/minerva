use crate::config::Config;
use crate::lti::LtiKeyPair;
use crate::rules::RuleCache;
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
    /// In-memory cache of compiled role rules. Reads (every authenticated
    /// request) take an Arc snapshot; writes (admin CRUD on rules) call
    /// `reload`. See `crate::rules`.
    pub rules: Arc<RuleCache>,
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

        let rules = Arc::new(RuleCache::load(&db).await?);

        Ok(Self {
            db,
            qdrant: Arc::new(qdrant),
            config: Arc::new(config.clone()),
            lti: Arc::new(lti),
            http_client: reqwest::Client::new(),
            fastembed,
            rules,
        })
    }
}
