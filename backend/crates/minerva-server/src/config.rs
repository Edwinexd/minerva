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
        })
    }

    pub fn is_admin(&self, eppn: &str) -> bool {
        let username = eppn.split('@').next().unwrap_or(eppn);
        self.admin_usernames.iter().any(|a| a == username)
    }
}
