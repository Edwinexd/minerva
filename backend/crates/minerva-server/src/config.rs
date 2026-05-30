use std::env;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub qdrant_url: String,
    pub cerebras_api_key: String,
    /// OpenAI API key for embeddings. Optional if all courses use local embedding provider.
    pub openai_api_key: String,
    pub hmac_secret: String,
    pub docs_path: String,
    /// Comma-separated list of admin eppn usernames (prefix before @).
    /// e.g. "edsu8469,isak1234"
    pub admin_usernames: Vec<String>,
    /// When true, allows dev auth bypass (X-Dev-User header or default user)
    pub dev_mode: bool,
    /// Maximum number of documents processed concurrently by the background worker.
    pub max_concurrent_ingests: usize,
    /// Directory containing the frontend static files (built SPA).
    pub static_dir: Option<String>,
    /// Seed for deterministic LTI RSA key generation. Falls back to HMAC secret.
    pub lti_key_seed: String,
    /// Public base URL for this Minerva instance (e.g. "https://minerva.dsv.su.se").
    /// Used to construct absolute LTI tool URLs.
    pub base_url: String,
    /// Dev-only: proxy unmatched requests to this URL (e.g. Vite dev server).
    pub dev_proxy: Option<String>,
    /// Global service API key for automated pipelines (e.g. transcript fetcher).
    /// Authenticated via `Authorization: Bearer <key>` on service endpoints.
    pub service_api_key: Option<String>,
    /// gRPC URL of the remote `minerva-embedder` service (e.g.
    /// `http://minerva-embedder.minerva.svc.cluster.local:50051`). When
    /// `Some`, the in-process `FastEmbedder` is replaced by a
    /// `RemoteEmbedderClient` that talks to that pod over HTTP/2.
    /// When `None`, the binary stays on the in-process variant
    /// (current monolith behaviour). Phase 4 deletes this Option and
    /// hard-requires the URL.
    pub embedder_url: Option<String>,
    /// gRPC URL of the remote `minerva-reranker` service. Same
    /// semantics as `embedder_url`.
    pub reranker_url: Option<String>,
    /// Whether this binary should run the document-processing worker
    /// loop (`worker::start`). Default true so the pre-Phase-3 image
    /// keeps working unchanged; flipped to `false` on the api during
    /// the Phase 3 cutover once the dedicated `minerva-worker` pod is
    /// claiming docs from the same queue. The worker binary itself
    /// ignores this flag (it always runs the worker).
    pub run_worker: bool,
    /// Whether this binary should run the periodic scheduler loops
    /// (Canvas auto-sync, LTI NRPS reconcile, platform-health probe,
    /// pending-platform cleanup). Default true. Used by Phase 3.5 to
    /// flip the worker pod off the scheduler loops once the
    /// dedicated `minerva-scheduler` pod is running them; the api
    /// honours it too if `run_worker` is also true (back-compat
    /// monolith path).
    pub run_scheduler: bool,
    //
    // Note: the four fields that used to live here ;
    //   default_course_daily_token_limit
    //   default_owner_daily_token_limit
    //   canvas_auto_sync_interval_hours
    //   lti_nrps_sync_interval_hours
    // ; moved into the admin-tunable `system_defaults` table. Their
    // env vars (`MINERVA_DEFAULT_COURSE_DAILY_TOKEN_LIMIT`,
    // `MINERVA_DEFAULT_OWNER_DAILY_TOKEN_LIMIT`,
    // `MINERVA_CANVAS_AUTO_SYNC_INTERVAL_HOURS`,
    // `MINERVA_LTI_NRPS_SYNC_INTERVAL_HOURS`) are still honoured as
    // *seeds* for fresh installs via `crate::system_defaults::seed_all`,
    // but the runtime reads come from the DB so an admin can edit them
    // live in /admin/defaults without a redeploy.
}

impl Config {
    pub fn from_env() -> Result<Self, env::VarError> {
        let admin_usernames: Vec<String> = env::var("MINERVA_ADMINS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(Self {
            host: env::var("MINERVA_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("MINERVA_PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .unwrap_or(3000),
            database_url: env::var("DATABASE_URL")?,
            qdrant_url: env::var("QDRANT_URL")
                .unwrap_or_else(|_| "http://localhost:6334".to_string()),
            cerebras_api_key: env::var("CEREBRAS_API_KEY")?,
            openai_api_key: env::var("OPENAI_API_KEY").unwrap_or_default(),
            hmac_secret: env::var("MINERVA_HMAC_SECRET")?,
            docs_path: env::var("MINERVA_DOCS_PATH")
                .unwrap_or_else(|_| "./data/documents".to_string()),
            admin_usernames,
            dev_mode: env::var("MINERVA_DEV_MODE").unwrap_or_default() == "true",
            max_concurrent_ingests: env::var("MINERVA_MAX_CONCURRENT_INGESTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4),
            static_dir: env::var("MINERVA_STATIC_DIR")
                .ok()
                .filter(|p| std::path::Path::new(p).is_dir()),
            lti_key_seed: env::var("MINERVA_LTI_KEY_SEED")
                .unwrap_or_else(|_| env::var("MINERVA_HMAC_SECRET").unwrap_or_default()),
            base_url: env::var("MINERVA_BASE_URL")
                .unwrap_or_else(|_| "https://minerva.dsv.su.se".to_string())
                .trim_end_matches('/')
                .to_string(),
            dev_proxy: env::var("MINERVA_DEV_PROXY").ok().filter(|s| !s.is_empty()),
            service_api_key: env::var("MINERVA_SERVICE_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            embedder_url: env::var("MINERVA_EMBEDDER_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            reranker_url: env::var("MINERVA_RERANKER_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            // Default `true` so an old image / unflipped env keeps the
            // pre-Phase-3 monolith behaviour. The Phase 3 cutover
            // flips it to `false` on the api once the worker pod is
            // confirmed claiming docs from the same Postgres queue.
            run_worker: parse_bool_env("MINERVA_RUN_WORKER", true),
            // Same defaulting story for the Phase 3.5 scheduler flag.
            run_scheduler: parse_bool_env("MINERVA_RUN_SCHEDULER", true),
        })
    }

    pub fn is_admin(&self, eppn: &str) -> bool {
        let username = eppn.split('@').next().unwrap_or(eppn);
        self.admin_usernames.iter().any(|a| a == username)
    }
}

/// Parse a `MINERVA_RUN_*` style boolean env var. Accepts the obvious
/// off-strings (`false`, `0`, `no`, `off`, case-insensitive) and
/// falls back to `default` for everything else (including unset).
fn parse_bool_env(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(v) => {
            let v = v.to_ascii_lowercase();
            !(v == "false" || v == "0" || v == "no" || v == "off")
        }
        Err(_) => default,
    }
}
