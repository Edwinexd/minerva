use crate::config::Config;
use crate::llm::LlmRegistry;
use crate::lti::LtiKeyPair;
use crate::model_capabilities::CapabilityCache;
use crate::relink_scheduler::RelinkScheduler;
use crate::rules::RuleCache;
use minerva_core::rpc::{EmbedderClient, RerankerClient};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use std::sync::{Arc, Mutex};

/// Snapshot of the current admin classification backfill, returned by
/// `GET /admin/classification-stats` so the UI can show progress.
///
/// `ok + errors + skipped == total - remaining` (within race tolerance).
/// `started_at` is set when a backfill kicks off and cleared when it
/// finishes; the UI uses presence of this struct to decide whether to
/// poll.
#[derive(Debug, Clone)]
pub struct BackfillProgress {
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub total: usize,
    pub ok: usize,
    pub errors: usize,
    pub skipped: usize,
    /// True once the spawned task drains its queue. The struct stays
    /// in state for one more poll cycle so the UI can show a final
    /// "done" state, then gets cleared by the next backfill kick-off
    /// or a manual reset.
    pub finished: bool,
}

/// Shared progress tracker for the admin classification backfill.
/// Exposed via a Mutex for cheap atomic updates; there's only ever
/// one backfill task running at a time and the contention is the
/// admin page's polling refetch.
#[derive(Default)]
pub struct BackfillTracker {
    inner: Mutex<Option<BackfillProgress>>,
}

impl BackfillTracker {
    pub fn snapshot(&self) -> Option<BackfillProgress> {
        self.inner
            .lock()
            .expect("backfill tracker mutex poisoned")
            .clone()
    }

    pub fn start(&self, total: usize) {
        let mut g = self.inner.lock().expect("backfill tracker mutex poisoned");
        *g = Some(BackfillProgress {
            started_at: chrono::Utc::now(),
            total,
            ok: 0,
            errors: 0,
            skipped: 0,
            finished: false,
        });
    }

    pub fn record_ok(&self) {
        let mut g = self.inner.lock().expect("backfill tracker mutex poisoned");
        if let Some(p) = g.as_mut() {
            p.ok += 1;
        }
    }

    pub fn record_error(&self) {
        let mut g = self.inner.lock().expect("backfill tracker mutex poisoned");
        if let Some(p) = g.as_mut() {
            p.errors += 1;
        }
    }

    pub fn record_skipped(&self) {
        let mut g = self.inner.lock().expect("backfill tracker mutex poisoned");
        if let Some(p) = g.as_mut() {
            p.skipped += 1;
        }
    }

    pub fn finish(&self) {
        let mut g = self.inner.lock().expect("backfill tracker mutex poisoned");
        if let Some(p) = g.as_mut() {
            p.finished = true;
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    pub db: PgPool,
    pub qdrant: Arc<Qdrant>,
    pub config: Arc<Config>,
    pub lti: Arc<LtiKeyPair>,
    pub http_client: reqwest::Client,
    /// Embedder client. Phase 0: in-process [`LocalEmbedderClient`]
    /// wrapping a `FastEmbedder` (zero behaviour change). Phase 1:
    /// either in-process or a remote gRPC client based on
    /// `MINERVA_EMBEDDER_URL`. Phase 4: gRPC-only.
    pub fastembed: Arc<dyn EmbedderClient>,
    /// Cross-encoder re-ranker client. Phase 0: in-process
    /// [`LocalRerankerClient`] wrapping a `FastReranker`. Phase 2: gated
    /// by `MINERVA_RERANKER_URL`. Phase 4: gRPC-only.
    pub reranker: Arc<dyn RerankerClient>,
    /// In-memory cache of compiled role rules. Reads (every authenticated
    /// request) take an Arc snapshot; writes (admin CRUD on rules) call
    /// `reload`. See `crate::rules`.
    pub rules: Arc<RuleCache>,
    /// Debounced per-course relink queue. Every classification change
    /// (worker auto, single-doc reclassify, teacher override) marks the
    /// course dirty here; a sweep loop drains and relinks. See
    /// `crate::relink_scheduler`.
    pub relink_scheduler: Arc<RelinkScheduler>,
    /// Per-model capability cache populated by probing Cerebras
    /// on first observation. Read on every `update_course` call
    /// to validate the (model, strategy, tool_use) triple before
    /// persisting; also available to runtime paths that want to
    /// short-circuit before issuing requests the model can't
    /// satisfy. See `crate::model_capabilities`.
    pub model_capabilities: CapabilityCache,
    /// Provider-agnostic LLM registry, built once from env/secret. Holds
    /// an `Arc<dyn ChatProvider>` per configured provider id
    /// (`cerebras`, `openai`, ...); a chat model resolves its provider
    /// via `chat_models.provider` and this registry. See `crate::llm`.
    pub llm: Arc<LlmRegistry>,
    /// Live progress of the admin classification backfill task.
    /// `None` when no backfill has run since the last server restart;
    /// `Some(_)` while one is running and for the cycle after it
    /// finishes. See `crate::routes::admin::backfill_classifications`.
    pub backfill_tracker: Arc<BackfillTracker>,
}

impl AppState {
    pub async fn new(config: &Config) -> anyhow::Result<Self> {
        let db = minerva_db::postgres::create_pool(&config.database_url).await?;

        let qdrant = minerva_db::qdrant::create_client(&config.qdrant_url)
            .await
            .map_err(|e| anyhow::anyhow!("qdrant connection failed: {}", e))?;

        let lti = LtiKeyPair::from_seed(&config.lti_key_seed)?;
        tracing::info!("LTI 1.3 provider ready (kid={})", lti.kid);

        // Embedder / reranker clients. Remote (gRPC) whenever the
        // service URL is set, which is the production topology
        // (api / worker / scheduler reach the model-server pods over
        // gRPC). The in-process engine fallback is only compiled into
        // `local-engine` builds for single-process local dev; see
        // `build_embedder` / `build_reranker` below.
        let fastembed = build_embedder(config).await?;
        let reranker = build_reranker(config).await?;

        // Sync `VALID_LOCAL_MODELS` into the admin-managed
        // `embedding_models` table. Existing rows are left alone (so an
        // admin's runtime toggle survives restarts); newly-added catalog
        // entries land disabled so they never auto-appear in the
        // teacher picker; the admin opts in deliberately. See
        // migration `20260427000001_embedding_models.sql` for the
        // initial-policy reasoning.
        for (model, _dims) in minerva_catalog::VALID_LOCAL_MODELS {
            let inserted =
                minerva_db::queries::embedding_models::seed_if_missing(&db, model, false).await?;
            if inserted {
                tracing::info!(
                    "embedding_models: seeded new catalog entry {} (enabled=false)",
                    model
                );
            }
        }

        // Same sync for the re-ranker catalog. The migration seeds the
        // default multilingual model enabled+default; everything else in
        // `VALID_RERANKER_MODELS` lands here disabled on first sight, so
        // enabling a heavier reranker is a deliberate admin opt-in.
        for model in minerva_catalog::VALID_RERANKER_MODELS {
            let inserted =
                minerva_db::queries::reranker_models::seed_if_missing(&db, model, false).await?;
            if inserted {
                tracing::info!(
                    "reranker_models: seeded new catalog entry {} (enabled=false)",
                    model
                );
            }
        }

        // Seed `system_defaults` for every knob in the registry that
        // isn't already in the DB. Existing rows are left alone (so
        // an admin's edit in the UI persists across restarts);
        // newly-added registry entries land at their env-var value
        // if set, else the hard-coded fallback. See
        // `crate::system_defaults` for the registry and policy.
        crate::system_defaults::seed_all(&db).await?;

        let rules = Arc::new(RuleCache::load(&db).await?);

        let http_client = reqwest::Client::new();
        // Capability cache shares the HTTP client so probes
        // benefit from connection pooling against Cerebras the
        // same way live chat traffic does.
        let model_capabilities = CapabilityCache::new(
            crate::llm::CEREBRAS_CHAT_COMPLETIONS_URL.to_string(),
            config.cerebras_api_key.clone(),
            http_client.clone(),
        );

        // Provider-agnostic LLM registry. Built from env/secret keys;
        // only providers with a present key are registered.
        let llm = Arc::new(LlmRegistry::from_config(http_client.clone(), config));

        Ok(Self {
            db: db.clone(),
            qdrant: Arc::new(qdrant),
            config: Arc::new(config.clone()),
            lti: Arc::new(lti),
            http_client,
            fastembed,
            reranker,
            rules,
            model_capabilities,
            llm,
            relink_scheduler: Arc::new(RelinkScheduler::new(db)),
            backfill_tracker: Arc::new(BackfillTracker::default()),
        })
    }
}

/// Build the embedder client.
///
/// Remote gRPC when `MINERVA_EMBEDDER_URL` is set: the production
/// topology, where the api / worker / scheduler reach the
/// `minerva-embedder` pod over gRPC and never link the model engine.
/// The in-process `FastEmbedder` fallback is only available in
/// `local-engine` builds (single-process local dev); a non-local-engine
/// build with no URL is a misconfiguration and errors out rather than
/// silently running without an embedder.
async fn build_embedder(config: &Config) -> anyhow::Result<Arc<dyn EmbedderClient>> {
    if let Some(url) = &config.embedder_url {
        tracing::info!("embedder: remote ({url})");
        return Ok(Arc::new(
            minerva_rpc::RemoteEmbedderClient::connect(url.clone())
                .await
                .map_err(|e| anyhow::anyhow!("remote embedder connect: {e}"))?,
        ));
    }
    build_local_embedder()
}

/// Build the reranker client. Same remote-vs-local policy as
/// [`build_embedder`].
async fn build_reranker(config: &Config) -> anyhow::Result<Arc<dyn RerankerClient>> {
    if let Some(url) = &config.reranker_url {
        tracing::info!("reranker: remote ({url})");
        return Ok(Arc::new(
            minerva_rpc::RemoteRerankerClient::connect(url.clone())
                .await
                .map_err(|e| anyhow::anyhow!("remote reranker connect: {e}"))?,
        ));
    }
    build_local_reranker()
}

#[cfg(feature = "local-engine")]
fn build_local_embedder() -> anyhow::Result<Arc<dyn EmbedderClient>> {
    tracing::info!("embedder: in-process FastEmbedder (local-engine build)");
    Ok(Arc::new(minerva_rpc_local::LocalEmbedderClient::new(
        Arc::new(minerva_embed_engine::fastembed_embedder::FastEmbedder::new()),
    )))
}

#[cfg(not(feature = "local-engine"))]
fn build_local_embedder() -> anyhow::Result<Arc<dyn EmbedderClient>> {
    anyhow::bail!(
        "MINERVA_EMBEDDER_URL is unset and this build has no in-process embedder (the `local-engine` feature is off). Set MINERVA_EMBEDDER_URL to the minerva-embedder service, or build with --features local-engine for single-process local dev."
    )
}

#[cfg(feature = "local-engine")]
fn build_local_reranker() -> anyhow::Result<Arc<dyn RerankerClient>> {
    tracing::info!("reranker: in-process FastReranker (local-engine build)");
    Ok(Arc::new(minerva_rpc_local::LocalRerankerClient::new(
        Arc::new(minerva_embed_engine::reranker::FastReranker::new()),
    )))
}

#[cfg(not(feature = "local-engine"))]
fn build_local_reranker() -> anyhow::Result<Arc<dyn RerankerClient>> {
    anyhow::bail!(
        "MINERVA_RERANKER_URL is unset and this build has no in-process reranker (the `local-engine` feature is off). Set MINERVA_RERANKER_URL to the minerva-reranker service, or build with --features local-engine for single-process local dev."
    )
}
