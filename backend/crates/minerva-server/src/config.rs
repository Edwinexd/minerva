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
}

impl Config {
    pub fn from_env() -> Result<Self, env::VarError> {
        Ok(Self {
            host: env::var("MINERVA_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("MINERVA_PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .unwrap_or(3000),
            database_url: env::var("DATABASE_URL")?,
            qdrant_url: env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string()),
            cerebras_api_key: env::var("CEREBRAS_API_KEY")?,
            openai_api_key: env::var("OPENAI_API_KEY")?,
            hmac_secret: env::var("MINERVA_HMAC_SECRET")?,
            docs_path: env::var("MINERVA_DOCS_PATH").unwrap_or_else(|_| "./data/documents".to_string()),
        })
    }
}
