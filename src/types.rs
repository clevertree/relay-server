use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

/// Legacy response header (informational only; **not** used for repo selection).
pub const HEADER_REPO: &str = "X-Relay-Repo";
pub const HEADER_BRANCH: &str = "X-Relay-Branch";
pub const DEFAULT_BRANCH: &str = "main";
pub const DEFAULT_IPFS_CACHE_ROOT: &str = "/tmp/ipfs-cache";

#[derive(Clone)]
pub struct AppState {
    /// Directory containing bare repos (`name.git`). May also be a single `.git` parent.
    pub repo_path: PathBuf,
    pub static_paths: Vec<PathBuf>,
    /// Node FQDN for HTTP routing: `{repo}.{node_fqdn}` (e.g. `atlanta1.relaygateway.net`).
    /// Set via **`RELAY_PUBLIC_HOSTNAME`** or **`RELAY_NODE_FQDN`**.
    pub node_fqdn: Option<String>,
    /// Unique server name; required when authorized pull list is configured.
    pub relay_server_id: Option<String>,
    /// When set, git-pull only for listed repos and anchor validation after fetch.
    pub authorized_repos: Option<Arc<crate::authorized_repos::AuthorizedReposFile>>,
    /// Written by relay-install.sh (`state/features.json`); exposed in /api/config.
    pub features_manifest: Option<Arc<serde_json::Value>>,
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
    pub server: Option<ServerConfig>,
    #[serde(default)]
    pub git: Option<GitConfig>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct ServerConfig {
    pub hooks: Option<std::collections::HashMap<String, HookPath>>,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct GitConfig {
    #[serde(rename = "autoPush")]
    pub auto_push: Option<AutoPushConfig>,
    #[serde(rename = "branchRules")]
    pub branch_rules: Option<BranchRulesConfig>,
    pub github: Option<GithubHooksConfig>,
    /// Declared allowlist of Relay server IDs and key fingerprints (see docs/RELAY_TRUST_AND_BOOTSTRAP.md).
    #[serde(rename = "relayTrust")]
    pub relay_trust: Option<RelayTrustConfig>,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct BranchRulesConfig {
    pub default: Option<BranchRule>,
    pub branches: Option<Vec<BranchRuleNamed>>,
}

#[derive(Deserialize, Debug, Default, Serialize, Clone)]
pub struct BranchRuleNamed {
    pub name: String,
    pub rule: BranchRule,
}

#[derive(Deserialize, Debug, Default, Serialize, Clone)]
pub struct BranchRule {
    #[serde(rename = "requireSigned")]
    pub require_signed: Option<bool>,
    #[serde(rename = "allowedKeys")]
    pub allowed_keys: Option<Vec<String>>,
    #[serde(rename = "allowedKeyFingerprints")]
    pub allowed_key_fingerprints: Option<Vec<String>>,
    #[serde(rename = "allowUnsigned")]
    pub allow_unsigned: Option<bool>,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct RelayTrustConfig {
    #[serde(rename = "authorizedServerIds")]
    pub authorized_server_ids: Option<Vec<String>>,
    #[serde(rename = "authorizedServerKeyFingerprints")]
    pub authorized_server_key_fingerprints: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct GithubHooksConfig {
    pub enabled: bool,
    pub path: String,
    pub events: Vec<String>,
}

#[derive(Deserialize, Debug, Default, Serialize)]
pub struct AutoPushConfig {
    pub branches: Vec<String>,
    #[serde(rename = "originList")]
    pub origin_list: Vec<String>,
    #[serde(rename = "debounceSeconds")]
    pub debounce_seconds: Option<u64>,
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
    Respond(Response),
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

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Not Found: {0}")]
    NotFound(String),
    #[error("Internal Server Error: {0}")]
    Internal(String),
    #[error("Bad Request: {0}")]
    BadRequest(String),
    #[error("Git Error: {0}")]
    Git(#[from] git2::Error),
    #[error("Transpile Error: {0}")]
    Transpile(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Git(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            ApiError::Transpile(msg) => (StatusCode::BAD_REQUEST, msg),
        };

        let body = Json(serde_json::json!({
            "error": message,
            "ok": false,
        }));

        (status, body).into_response()
    }
}
