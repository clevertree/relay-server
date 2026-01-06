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

use crate::{git, helpers, types::*};

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

/// GET /api/config — returns configuration including peer list from RELAY_MASTER_PEER_LIST
pub async fn get_api_config() -> impl IntoResponse {
    #[derive(Serialize)]
    struct Config {
        peers: Vec<String>,
    }

    let peer_list = std::env::var("RELAY_MASTER_PEER_LIST")
        .unwrap_or_default()
        .split(';')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>();

    let config = Config { peers: peer_list };
    (StatusCode::OK, Json(config))
}

/// POST /git-pull — performs git pull from origin on the bare repository
pub async fn post_git_pull(State(state): State<AppState>) -> impl IntoResponse {
    #[derive(Serialize)]
    struct GitPullResponse {
        success: bool,
        message: String,
        updated: bool,
        before_commit: Option<String>,
        after_commit: Option<String>,
        error: Option<String>,
    }

    let repo_path = &state.repo_path;

    match Repository::open(repo_path) {
        Ok(repo) => {
            let before_commit = repo
                .head()
                .ok()
                .and_then(|h| h.target())
                .map(|oid| oid.to_string());

            match repo.find_remote("origin") {
                Ok(mut remote) => match remote.fetch(&["main"], None, None) {
                    Ok(_) => {
                        let fetch_head = repo.find_reference("FETCH_HEAD");
                        let updated = if let Ok(fetch_ref) = fetch_head {
                            match fetch_ref.target() {
                                Some(fetch_oid) => match repo.head() {
                                    Ok(head_ref) => {
                                        if let Some(head_oid) = head_ref.target() {
                                            if fetch_oid != head_oid {
                                                match repo.set_head_detached(fetch_oid) {
                                                    Ok(_) => match repo.checkout_head(None) {
                                                        Ok(_) => true,
                                                        Err(_) => false,
                                                    },
                                                    Err(_) => false,
                                                }
                                            } else {
                                                false
                                            }
                                        } else {
                                            false
                                        }
                                    }
                                    Err(_) => false,
                                },
                                None => false,
                            }
                        } else {
                            false
                        };

                        let after_commit = repo
                            .head()
                            .ok()
                            .and_then(|h| h.target())
                            .map(|oid| oid.to_string());

                        let message = if updated {
                            format!(
                                "Repository updated from origin. Before: {}, After: {}",
                                before_commit.clone().unwrap_or_default(),
                                after_commit.clone().unwrap_or_default()
                            )
                        } else {
                            "Repository is already up to date with origin".to_string()
                        };

                        tracing::info!("git-pull: {}", message);
                        (
                            StatusCode::OK,
                            Json(GitPullResponse {
                                success: true,
                                message,
                                updated,
                                before_commit,
                                after_commit,
                                error: None,
                            }),
                        )
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to fetch from origin: {}", e);
                        tracing::warn!("git-pull error: {}", error_msg);
                        (
                            StatusCode::OK,
                            Json(GitPullResponse {
                                success: false,
                                message: error_msg.clone(),
                                updated: false,
                                before_commit,
                                after_commit: None,
                                error: Some(error_msg),
                            }),
                        )
                    }
                },
                Err(e) => {
                    let error_msg = format!("Failed to find remote 'origin': {}", e);
                    tracing::warn!("git-pull error: {}", error_msg);
                    (
                        StatusCode::OK,
                        Json(GitPullResponse {
                            success: false,
                            message: error_msg.clone(),
                            updated: false,
                            before_commit,
                            after_commit: None,
                            error: Some(error_msg),
                        }),
                    )
                }
            }
        }
        Err(e) => {
            let error_msg = format!("Failed to open repository at {:?}: {}", repo_path, e);
            tracing::error!("git-pull error: {}", error_msg);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GitPullResponse {
                    success: false,
                    message: error_msg.clone(),
                    updated: false,
                    before_commit: None,
                    after_commit: None,
                    error: Some(error_msg),
                }),
            )
        }
    }
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
    let repo_name = helpers::strict_repo_from(&state.repo_path, &headers);

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

/// POST /hooks/github — Triggered by GitHub webhooks to pull updates
pub async fn post_github_hook(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let repo_name = match params.get("repo") {
        Some(name) => name,
        None => return (StatusCode::BAD_REQUEST, "Missing repo query parameter").into_response(),
    };

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
