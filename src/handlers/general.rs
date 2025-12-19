use std::path::PathBuf;

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use git2::Repository;
use serde::Serialize;

use crate::types::AppState;

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
