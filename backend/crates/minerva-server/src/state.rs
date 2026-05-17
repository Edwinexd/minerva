use crate::config::Config;
use crate::lti::LtiKeyPair;
use crate::model_capabilities::CapabilityCache;
use crate::relink_scheduler::RelinkScheduler;
use crate::rules::RuleCache;
use minerva_ingest::fastembed_embedder::FastEmbedder;
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
    pub fastembed: Arc<FastEmbedder>,
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

        let fastembed = Arc::new(FastEmbedder::new());

        // Sync `VALID_LOCAL_MODELS` into the admin-managed
        // `embedding_models` table. Existing rows are left alone (so an
        // admin's runtime toggle survives restarts); newly-added catalog
        // entries land disabled so they never auto-appear in the
        // teacher picker; the admin opts in deliberately. See
        // migration `20260427000001_embedding_models.sql` for the
        // initial-policy reasoning.
        for (model, _dims) in minerva_ingest::pipeline::VALID_LOCAL_MODELS {
            let inserted =
                minerva_db::queries::embedding_models::seed_if_missing(&db, model, false).await?;
            if inserted {
                tracing::info!(
                    "embedding_models: seeded new catalog entry {} (enabled=false)",
                    model
                );
            }
        }

        let rules = Arc::new(RuleCache::load(&db).await?);

        let http_client = reqwest::Client::new();
        // Capability cache shares the HTTP client so probes
        // benefit from connection pooling against Cerebras the
        // same way live chat traffic does.
        let model_capabilities = CapabilityCache::new(
            crate::strategy::common::CEREBRAS_CHAT_COMPLETIONS_URL.to_string(),
            config.cerebras_api_key.clone(),
            http_client.clone(),
        );
        Ok(Self {
            db: db.clone(),
            qdrant: Arc::new(qdrant),
            config: Arc::new(config.clone()),
            lti: Arc::new(lti),
            http_client,
            fastembed,
            rules,
            model_capabilities,
            relink_scheduler: Arc::new(RelinkScheduler::new(db)),
            backfill_tracker: Arc::new(BackfillTracker::default()),
        })
    }
}
