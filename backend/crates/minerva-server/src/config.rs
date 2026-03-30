use std::env;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub qdrant_url: String,
    pub cerebras_api_key: String,
    pub openai_api_key: String,
    pub hmac_secret: String,
    pub docs_path: String,
    /// Comma-separated list of admin eppn usernames (prefix before @).
    /// e.g. "edsu8469,isak1234"
    pub admin_usernames: Vec<String>,
    /// When true, allows dev auth bypass (X-Dev-User header or default user)
    pub dev_mode: bool,
    /// Optional API key for service-to-service integration (e.g. Moodle plugin).
    /// Set via MINERVA_API_KEY. When set, the /api/integration/* routes are enabled.
    pub api_key: Option<String>,
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
            openai_api_key: env::var("OPENAI_API_KEY")?,
            hmac_secret: env::var("MINERVA_HMAC_SECRET")?,
            docs_path: env::var("MINERVA_DOCS_PATH")
                .unwrap_or_else(|_| "./data/documents".to_string()),
            admin_usernames,
            dev_mode: env::var("MINERVA_DEV_MODE").unwrap_or_default() == "true",
            api_key: env::var("MINERVA_API_KEY").ok().filter(|s| !s.is_empty()),
        })
    }

    pub fn is_admin(&self, eppn: &str) -> bool {
        let username = eppn.split('@').next().unwrap_or(eppn);
        self.admin_usernames.iter().any(|a| a == username)
    }
}
