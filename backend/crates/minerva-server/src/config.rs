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
    /// Default per-student-per-day token cap applied to newly created courses
    /// when the request omits `daily_token_limit` or sends 0. Existing courses
    /// are unaffected. 0 disables the default (= unlimited new courses).
    pub default_course_daily_token_limit: i64,
    /// Default per-owner aggregate daily token cap applied to *new* users on
    /// first login. Sums all tokens across courses they own. 0 = unlimited.
    /// Admin overrides per-user via /admin/users.
    pub default_owner_daily_token_limit: i64,
    /// How often a Canvas connection with `auto_sync = true` re-syncs.
    /// Measured in hours; 0 disables the background loop entirely.
    pub canvas_auto_sync_interval_hours: i32,
    /// When true, the worker routes PDFs/images to `awaiting_ocr` and
    /// play.dsv URLs to `awaiting_video_index` instead of the legacy
    /// extract-text / awaiting_transcript paths. Default false: the new
    /// states stay valid in the schema but the worker keeps the old
    /// behavior so the GPU pipeline can ship table-by-table behind a
    /// flag without disrupting production ingestion.
    pub ocr_pipeline_enabled: bool,
    /// RunPod API key for the backend's submitter. Used as
    /// `Authorization: Bearer <key>` against `runpod_api_base`. Optional
    /// because the OCR pipeline is gated; without it, submissions error
    /// at runtime but the rest of the app starts fine.
    pub runpod_api_key: Option<String>,
    /// RunPod serverless endpoint id (the ghcr.io/.../minerva-runpod-worker
    /// image runs behind it). Submitter targets POST {api_base}/v2/{id}/run.
    pub runpod_endpoint_id: Option<String>,
    /// Base URL for the RunPod API. Default is the public hosted API; lets
    /// us point at a staging or mock RunPod in tests.
    pub runpod_api_base: String,
    /// Per-second GPU rate used to compute estimated_cost_usd at job
    /// completion. RunPod has no native daily cap; we stamp this on each
    /// job at completion time so the daily-budget circuit breaker doesn't
    /// have to re-multiply on every poll.
    pub runpod_per_second_usd: f64,
    /// Soft daily cap on RunPod spend, in USD. When exceeded, the
    /// submitter pauses (docs stay in awaiting_*; cron picks them up
    /// next day). 0 disables the cap.
    pub runpod_daily_budget_usd: f64,
    /// Public base URL of this Minerva instance; embedded in service URLs
    /// handed to RunPod (RunPod fetches PDFs / bundles back from here).
    /// Falls back to `base_url` when unset; lets staging environments
    /// point RunPod at an internal hostname different from the user-facing
    /// `base_url`.
    pub runpod_callback_base: String,
    /// Sample-rate fraction for video frame extraction (e.g. "1/5" = one
    /// frame every 5 seconds). Hardcoded global default; per-course
    /// override is deferred until a course actually needs it.
    pub video_sample_fps: String,
}

impl Config {
    pub fn from_env() -> Result<Self, env::VarError> {
        let admin_usernames = env::var("MINERVA_ADMINS")
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
            default_course_daily_token_limit: env::var("MINERVA_DEFAULT_COURSE_DAILY_TOKEN_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100_000),
            default_owner_daily_token_limit: env::var("MINERVA_DEFAULT_OWNER_DAILY_TOKEN_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(500_000),
            canvas_auto_sync_interval_hours: env::var("MINERVA_CANVAS_AUTO_SYNC_INTERVAL_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(24),
            ocr_pipeline_enabled: env::var("MINERVA_OCR_PIPELINE_ENABLED").unwrap_or_default()
                == "true",
            runpod_api_key: env::var("RUNPOD_API_KEY").ok().filter(|s| !s.is_empty()),
            runpod_endpoint_id: env::var("RUNPOD_ENDPOINT_ID")
                .ok()
                .filter(|s| !s.is_empty()),
            runpod_api_base: env::var("RUNPOD_API_BASE")
                .unwrap_or_else(|_| "https://api.runpod.ai".to_string())
                .trim_end_matches('/')
                .to_string(),
            runpod_per_second_usd: env::var("MINERVA_RUNPOD_PER_SECOND_USD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.000_2),
            runpod_daily_budget_usd: env::var("MINERVA_RUNPOD_DAILY_BUDGET_USD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0),
            runpod_callback_base: env::var("MINERVA_RUNPOD_CALLBACK_BASE")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    env::var("MINERVA_BASE_URL")
                        .unwrap_or_else(|_| "https://minerva.dsv.su.se".to_string())
                        .trim_end_matches('/')
                        .to_string()
                }),
            video_sample_fps: env::var("MINERVA_VIDEO_SAMPLE_FPS")
                .unwrap_or_else(|_| "1/5".to_string()),
        })
    }

    pub fn is_admin(&self, eppn: &str) -> bool {
        let username = eppn.split('@').next().unwrap_or(eppn);
        self.admin_usernames.iter().any(|a| a == username)
    }
}
