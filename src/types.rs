use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const HEADER_REPO: &str = "X-Relay-Repo";
pub const HEADER_BRANCH: &str = "X-Relay-Branch";
pub const DEFAULT_BRANCH: &str = "main";
pub const DEFAULT_IPFS_CACHE_ROOT: &str = "/tmp/ipfs-cache";

#[derive(Clone)]
pub struct AppState {
    // Repository ROOT directory containing bare repos (name.git)
    pub repo_path: PathBuf,
    // Additional static directories to serve from root before Git
    pub static_paths: Vec<PathBuf>,
}

#[derive(Deserialize, Debug)]
pub struct RulesDoc {
    pub rules: Vec<String>,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct RelayConfig {
    #[serde(default)]
    pub client: Option<ClientConfig>,
    #[serde(default)]
    pub server: Option<serde_json::Value>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct ClientConfig {
    #[serde(default)]
    pub hooks: HooksConfig,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub get: Option<HookPath>,
    #[serde(default)]
    pub query: Option<HookPath>,
}

#[derive(Deserialize, Debug, Serialize)]
pub struct HookPath {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct TranspileRequest {
    pub code: String,
    pub filename: Option<String>,
    #[serde(default)]
    pub to_common_js: bool,
}

#[derive(Debug, Serialize)]
pub struct TranspileResponse {
    pub code: Option<String>,
    pub map: Option<String>,
    pub diagnostics: Option<String>,
    pub ok: bool,
}

pub enum GitResolveResult {
    Respond(axum::response::Response),
    NotFound(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("not found")]
    NotFound,
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}
