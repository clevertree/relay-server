use std::collections::HashMap;
use std::path::PathBuf;

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use git2::Repository;
use serde::Serialize;

use crate::{authorized_repos, git, helpers, types::*};

/// Serve a minimal OpenAPI YAML specification (placeholder)
pub async fn get_openapi_yaml() -> impl IntoResponse {
    let yaml = r#"openapi: 3.0.0
info:
  title: Relay API
  version: 0.0.0
paths: {}
"#;
    (StatusCode::OK, [("Content-Type", "application/yaml")], yaml)
}

/// Serve Swagger UI HTML page
pub async fn get_swagger_ui() -> impl IntoResponse {
    let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="description" content="SwaggerUI" />
    <title>SwaggerUI</title>
    <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5.11.0/swagger-ui.css" />
</head>
<body>
<div id="swagger-ui"></div>
<script src="https://unpkg.com/swagger-ui-dist@5.11.0/swagger-ui-bundle.js" crossorigin></script>
<script>
    window.onload = () => {
        window.ui = SwaggerUIBundle({
            url: '/openapi.yaml',
            dom_id: '#swagger-ui',
            deepLinking: true,
            presets: [
                SwaggerUIBundle.presets.apis,
                SwaggerUIBundle.presets.standalone
            ],
            plugins: [
                SwaggerUIBundle.plugins.DownloadUrl
            ],
            layout: "BaseLayout"
        });
    };
    </script>
</body>
</html>"#;
    (StatusCode::OK, [("Content-Type", "text/html")], html)
}

/// GET /api/config — peers, repo list, optional `node_fqdn`, server id, authorized repo names
pub async fn get_api_config(State(state): State<AppState>) -> impl IntoResponse {
    #[derive(Serialize)]
    struct Config {
        peers: Vec<String>,
        repos: Vec<String>,
        /// Node hostname for HTTP: use **`{repo}.{node_fqdn}`** as `Host` (no `X-Relay-Repo`, no `?repo=`).
        #[serde(skip_serializing_if = "Option::is_none")]
        node_fqdn: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        relay_server_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        authorized_repos: Option<Vec<String>>,
        /// Install-time feature manifest (Piper, npm extensions, ports). See relay-install.sh.
        #[serde(skip_serializing_if = "Option::is_none")]
        installed_features: Option<serde_json::Value>,
    }

    let peer_list = std::env::var("RELAY_MASTER_PEER_LIST")
        .unwrap_or_default()
        .split(';')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>();

    let repos = git::bare_repo_names(&state.repo_path);

    let authorized_repos = state.authorized_repos.as_ref().map(|a| {
        a.repos
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    });

    let installed_features = state.features_manifest.as_ref().map(|m| {
        serde_json::json!({
            "manifest": m.as_ref(),
            "summary": summarize_features(m),
        })
    });

    let config = Config {
        peers: peer_list,
        repos,
        node_fqdn: state.node_fqdn.clone(),
        relay_server_id: state.relay_server_id.clone(),
        authorized_repos,
        installed_features,
    };
    (StatusCode::OK, Json(config))
}

fn summarize_features(m: &std::sync::Arc<serde_json::Value>) -> serde_json::Value {
    let f = m.get("features").cloned().unwrap_or(serde_json::json!({}));
    let piper = f.get("piper_tts");
    let npm = f.get("npm_extensions");
    serde_json::json!({
        "http_port": m.get("http_port"),
        "git_port": m.get("git_port"),
        "piper_tts": {
            "enabled": piper.and_then(|p| p.get("enabled")),
            "http_port": piper.and_then(|p| p.get("http_port")),
        },
        "npm_extensions": {
            "enabled": npm.and_then(|p| p.get("enabled")),
            "packages": npm.and_then(|p| p.get("packages")),
        },
    })
}

/// POST /git-pull — fetch from `origin` into a **bare** repo.
/// Target resolution: **`Host: {repo}.{node_fqdn}`** (same as HTTP file serving). No `?repo=` or `X-Relay-Repo`.
/// Legacy: if `RELAY_REPO_PATH` is itself a bare `*.git` directory and no per-repo dirs exist, that repo is used.
pub async fn post_git_pull(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    #[derive(Serialize)]
    struct GitPullResponse {
        success: bool,
        message: String,
        updated: bool,
        repo: Option<String>,
        before_commit: Option<String>,
        after_commit: Option<String>,
        error: Option<String>,
    }

    let names = git::bare_repo_names(&state.repo_path);

    let (bare_path, repo_label) = if let Some(n) =
        helpers::repo_from_host(&state.repo_path, state.node_fqdn.as_deref(), &headers)
    {
        (state.repo_path.join(format!("{}.git", n)), n)
    } else if names.is_empty() && Repository::open_bare(&state.repo_path).is_ok() {
        let stem = state
            .repo_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("repo")
            .to_string();
        (state.repo_path.clone(), stem)
    } else {
        let msg = if state.node_fqdn.is_none() {
            "No bare repository resolved. Set RELAY_PUBLIC_HOSTNAME to this node's FQDN and call with Host: {repo}.{that-fqdn}; repos live under RELAY_REPO_PATH as <repo>.git".to_string()
        } else {
            format!(
                "No bare repository resolved. Use Host header: {{repo}}.{}",
                state.node_fqdn.as_deref().unwrap_or("")
            )
        };
        return (
            StatusCode::BAD_REQUEST,
            Json(GitPullResponse {
                success: false,
                message: msg.clone(),
                updated: false,
                repo: None,
                before_commit: None,
                after_commit: None,
                error: Some(msg),
            }),
        );
    };

    if let Some(ref cfg) = state.authorized_repos {
        if let Err(msg) =
            authorized_repos::ensure_pull_allowed(cfg, state.relay_server_id.as_deref(), &repo_label)
        {
            tracing::warn!("git-pull denied: {}", msg);
            return (
                StatusCode::FORBIDDEN,
                Json(GitPullResponse {
                    success: false,
                    message: msg.clone(),
                    updated: false,
                    repo: Some(repo_label),
                    before_commit: None,
                    after_commit: None,
                    error: Some(msg),
                }),
            );
        }
    }

    let repo = match Repository::open_bare(&bare_path) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Failed to open bare repo at {:?}: {}", bare_path, e);
            tracing::error!("git-pull: {}", msg);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GitPullResponse {
                    success: false,
                    message: msg.clone(),
                    updated: false,
                    repo: Some(repo_label),
                    before_commit: None,
                    after_commit: None,
                    error: Some(msg),
                }),
            );
        }
    };

    let before_commit = repo
        .find_reference("refs/heads/main")
        .ok()
        .and_then(|r| r.target())
        .map(|o| o.to_string());

    let mut remote = match repo.find_remote("origin") {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Remote 'origin' not found: {}", e);
            return (
                StatusCode::OK,
                Json(GitPullResponse {
                    success: false,
                    message: msg.clone(),
                    updated: false,
                    repo: Some(repo_label),
                    before_commit,
                    after_commit: None,
                    error: Some(msg),
                }),
            );
        }
    };

    let fetch_specs = ["+refs/heads/*:refs/heads/*"];
    let fetch_res = remote.fetch(&fetch_specs, None, None);
    if let Err(e) = fetch_res {
        let fallback = remote.fetch(&["+refs/heads/main:refs/heads/main"], None, None);
        if let Err(e2) = fallback {
            let msg = format!("Fetch failed: {} (fallback main: {})", e, e2);
            tracing::warn!("git-pull: {}", msg);
            return (
                StatusCode::OK,
                Json(GitPullResponse {
                    success: false,
                    message: msg.clone(),
                    updated: false,
                    repo: Some(repo_label),
                    before_commit: before_commit.clone(),
                    after_commit: None,
                    error: Some(msg),
                }),
            );
        }
    }

    let after_commit = repo
        .find_reference("refs/heads/main")
        .ok()
        .and_then(|r| r.target())
        .map(|o| o.to_string());

    if let Some(ref cfg) = state.authorized_repos {
        if let Err(msg) = authorized_repos::validate_anchor(&repo, &repo_label, cfg) {
            tracing::warn!("git-pull trust validation failed: {}", msg);
            authorized_repos::rollback_main(&repo, before_commit.as_deref());
            return (
                StatusCode::FORBIDDEN,
                Json(GitPullResponse {
                    success: false,
                    message: msg.clone(),
                    updated: false,
                    repo: Some(repo_label),
                    before_commit: before_commit.clone(),
                    after_commit: after_commit.clone(),
                    error: Some(msg),
                }),
            );
        }
    }

    let updated = before_commit != after_commit;
    let message = if updated {
        format!(
            "Fetched {}. main: {:?} -> {:?}",
            repo_label,
            before_commit,
            after_commit
        )
    } else {
        format!("{} already up to date with origin (main)", repo_label)
    };
    tracing::info!("git-pull: {}", message);

    (
        StatusCode::OK,
        Json(GitPullResponse {
            success: true,
            message,
            updated,
            repo: Some(repo_label),
            before_commit,
            after_commit,
            error: None,
        }),
    )
}

/// Serve ACME HTTP-01 challenge files from a configured directory
pub async fn serve_acme_challenge(base_dir: &str, subpath: &str) -> impl IntoResponse {
    let rel = subpath
        .split('/')
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .collect::<Vec<_>>()
        .join("/");
    if rel.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = PathBuf::from(base_dir).join(rel);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (StatusCode::OK, [("Content-Type", "text/plain")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// OPTIONS handler — discovery: capabilities, branches, repos, current selections, client hooks
pub async fn options_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    let branch = helpers::branch_from(&headers);
    let repo_name =
        helpers::repo_from_host(&state.repo_path, state.node_fqdn.as_deref(), &headers);

    let repo_names = git::bare_repo_names(&state.repo_path);
    let mut repos_json: Vec<serde_json::Value> = Vec::new();
    let mut relay_config: Option<RelayConfig> = None;

    for name in &repo_names {
        if let Some(repo) = git::open_repo(&state.repo_path, name) {
            let mut heads_map = serde_json::Map::new();
            let branches = helpers::list_branches(&repo);
            for b in &branches {
                if let Ok(reference) = repo.find_reference(&format!("refs/heads/{}", b)) {
                    if let Ok(commit) = reference.peel_to_commit() {
                        heads_map.insert(b.clone(), serde_json::json!(commit.id().to_string()));
                    }
                }
            }
            if Some(name) == repo_name.as_ref() {
                if relay_config.is_none() {
                    relay_config = git::read_relay_config(&repo, &branch);
                }
            }
            repos_json.push(serde_json::json!({
                "name": name,
                "branches": serde_json::Value::Object(heads_map),
            }));
        }
    }

    let allow = "GET, PUT, DELETE, OPTIONS, QUERY";
    let mut body = serde_json::json!({
        "ok": true,
        "capabilities": {"supports": ["GET","PUT","DELETE","OPTIONS","QUERY"]},
        "repos": repos_json,
        "currentBranch": branch,
        "currentRepo": repo_name.clone().unwrap_or_default(),
    });

    if let Some(config) = relay_config {
        if let Ok(config_json) = serde_json::to_value(&config) {
            if let Some(obj) = body.as_object_mut() {
                if let Some(config_obj) = config_json.as_object() {
                    for (key, value) in config_obj {
                        obj.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }

    (
        StatusCode::OK,
        [
            ("Allow", allow.to_string()),
            ("Content-Type", "application/json".to_string()),
            (HEADER_BRANCH, branch),
            (HEADER_REPO, repo_name.unwrap_or_default()),
        ],
        Json(body),
    )
}

pub async fn get_root(
    State(state): State<AppState>,
    _headers: HeaderMap,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    // Try serving SPA index.html from configured static paths
    if let Some(resp) = crate::handlers::try_static(&state, "index.html").await {
        return resp;
    }
    StatusCode::NO_CONTENT.into_response()
}

/// POST /hooks/github/{repo} — Triggered by GitHub webhooks to pull updates
pub async fn post_github_hook(
    State(state): State<AppState>,
    axum::extract::Path(repo_name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let repo_name = repo_name.trim().trim_end_matches(".git");
    if repo_name.is_empty() {
        return (StatusCode::BAD_REQUEST, "Missing repo name").into_response();
    }

    let full_repo_path = state.repo_path.join(format!("{}.git", repo_name));
    if !full_repo_path.exists() {
        return (StatusCode::NOT_FOUND, format!("Repository {}.git not found", repo_name)).into_response();
    }

    // Identify if GitHub hooks are enabled in .relay.yaml
    let repo = match git2::Repository::open_bare(&full_repo_path) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to open repo: {}", e)).into_response(),
    };

    let relay_config = match git::read_relay_config(&repo, "main") {
        Some(c) => c,
        None => return (StatusCode::FORBIDDEN, "No .relay.yaml found").into_response(),
    };

    let gh_config = match relay_config.git.and_then(|g| g.github) {
        Some(gh) if gh.enabled => gh,
        _ => return (StatusCode::FORBIDDEN, "GitHub hooks not enabled in .relay.yaml").into_response(),
    };

    tracing::info!("GitHub hook received for repo: {}, path: {}", repo_name, gh_config.path);

    // Trigger git pull (fetch + merge/reset)
    // For bare repos, we just fetch
    let res = match repo.find_remote("origin") {
        Ok(mut remote) => {
            if let Err(e) = remote.fetch(&["main"], None, None) {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Fetch failed: {}", e)).into_response()
            } else {
                (StatusCode::OK, "GitHub hook processed, repository fetched").into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Remote 'origin' not found: {}", e)).into_response(),
    };
    res
}
