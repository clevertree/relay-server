use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::engine::general_purpose;
use base64::Engine;
use tracing::{error, info, warn};

use crate::{
    git, helpers, transpiler, AppState, GitResolveResult, HEADER_BRANCH, HEADER_REPO,
    DEFAULT_IPFS_CACHE_ROOT,
};

/// GET file handler — resolves from Git first, then hooks/get.mjs, then static
pub async fn handle_get_file(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(path): axum::extract::Path<String>,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    info!(%path, "get_file called");
    let decoded = helpers::url_decode(&path).decode_utf8_lossy().to_string();
    info!(decoded = %decoded, "decoded path");

    let branch = helpers::branch_from(&headers);
    let repo_name_opt = helpers::strict_repo_from(&state.repo_path, &headers);
    let repo_name: String;
    if repo_name_opt.is_none() {
        if let Some(resp) = try_static(&state, &decoded).await {
            return resp;
        }
        let error_msg = format!(
            "Not Found\n\nPath: {}\nBranch: {}\nRepo: (none - no X-Relay-Repo header)\nStatic dirs searched: {:?}\n\nNo file found in static directories.",
            decoded, branch, state.static_paths
        );
        return (
            StatusCode::NOT_FOUND,
            [
                ("Content-Type", "text/plain".to_string()),
                (HEADER_BRANCH, branch.clone()),
                (HEADER_REPO, "".to_string()),
            ],
            error_msg,
        )
            .into_response();
    } else {
        repo_name = repo_name_opt.unwrap();
    }
    let normalized_path = decoded.trim_start_matches('/').to_string();

    if transpiler::helpers::should_transpile_request(&headers, &_query)
        && transpiler::helpers::is_transpilable_hook_path(&normalized_path)
    {
        if let Some(transpiled) = transpiler::helpers::transpile_hook_file(
            &state.repo_path,
            &branch,
            &repo_name,
            &normalized_path,
        ) {
            return transpiled;
        }
    }

    info!(%branch, "resolved branch");

    let git_result =
        git::git_resolve_and_respond(&state.repo_path, &headers, &branch, &repo_name, &decoded);
    match git_result {
        GitResolveResult::Respond(resp) => return resp,
        GitResolveResult::NotFound(rel_missing) => {
            let hook_resp = run_get_script_or_404(&state, &branch, &repo_name, &rel_missing).await;
            if hook_resp.status() != StatusCode::NOT_FOUND {
                return hook_resp;
            }
            if let Some(resp) = try_static(&state, &decoded).await {
                return resp;
            }
            return hook_resp;
        }
    }
}

async fn run_get_script_or_404(
    state: &AppState,
    branch: &str,
    repo_name: &str,
    rel_missing: &str,
) -> Response {
    let repo = match git::open_repo(&state.repo_path, repo_name) {
        Some(r) => r,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "Repository not found").into_response(),
    };
    let refname = format!("refs/heads/{}", branch);
    let commit = match repo
        .find_reference(&refname)
        .and_then(|r| r.peel_to_commit())
    {
        Ok(c) => c,
        Err(_) => {
            let error_msg = format!(
                "Not Found\n\nPath: {}\nBranch: {} (not found in repo)\nRepo: {}\n\nBranch does not exist in repository.",
                rel_missing, branch, repo_name
            );
            return (StatusCode::NOT_FOUND, error_msg).into_response();
        }
    };
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => {
            let error_msg = format!(
                "Not Found\n\nPath: {}\nBranch: {}\nRepo: {}\n\nFailed to read tree from branch.",
                rel_missing, branch, repo_name
            );
            return (StatusCode::NOT_FOUND, error_msg).into_response();
        }
    };
    let entry = match tree.get_path(std::path::Path::new("hooks/get.mjs")) {
        Ok(e) => e,
        Err(_) => {
            let error_msg = format!(
                "Not Found\n\nPath: {}\nBranch: {}\nRepo: {}\n\nFile not found in git repository. No hooks/get.mjs found to handle dynamic routing.",
                rel_missing, branch, repo_name
            );
            return (StatusCode::NOT_FOUND, error_msg).into_response();
        }
    };
    let blob = match entry.to_object(&repo).and_then(|o| o.peel_to_blob()) {
        Ok(b) => b,
        Err(_) => {
            let error_msg = format!(
                "Not Found\n\nPath: {}\nBranch: {}\nRepo: {}\n\nFile not found in git repository. hooks/get.mjs exists but cannot be read.",
                rel_missing, branch, repo_name
            );
            return (StatusCode::NOT_FOUND, error_msg).into_response();
        }
    };
    let tmp = std::env::temp_dir().join(format!("relay-get-{}-{}.mjs", branch, commit.id()));
    if let Err(e) = std::fs::write(&tmp, blob.content()) {
        error!(?e, "failed to write get.mjs temp file");
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    let node_bin = std::env::var("RELAY_NODE_BIN").unwrap_or_else(|_| "node".to_string());
    let mut cmd = std::process::Command::new(node_bin);
    cmd.arg(&tmp)
        .env("GIT_DIR", repo.path())
        .env("BRANCH", branch)
        .env("REL_PATH", rel_missing)
        .env(
            "CACHE_ROOT",
            std::env::var("RELAY_IPFS_CACHE_ROOT")
                .unwrap_or_else(|_| DEFAULT_IPFS_CACHE_ROOT.to_string()),
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            error!(?e, "failed to execute get.mjs");
            let _ = std::fs::remove_file(&tmp);
            return (StatusCode::NOT_FOUND, "Not Found").into_response();
        }
    };
    let _ = std::fs::remove_file(&tmp);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(%stderr, "get.mjs non-success status");
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    let val: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            warn!(?e, "get.mjs returned invalid JSON");
            return (StatusCode::NOT_FOUND, "Not Found").into_response();
        }
    };
    let kind = val.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    match kind {
        "file" => {
            let ct = val
                .get("contentType")
                .and_then(|v| v.as_str())
                .unwrap_or("application/octet-stream");
            let b64 = val.get("bodyBase64").and_then(|v| v.as_str()).unwrap_or("");
            match general_purpose::STANDARD.decode(b64.as_bytes()) {
                Ok(bytes) => (
                    StatusCode::OK,
                    [
                        ("Content-Type", ct.to_string()),
                        (HEADER_BRANCH, branch.to_string()),
                        (HEADER_REPO, repo_name.to_string()),
                    ],
                    bytes,
                )
                    .into_response(),
                Err(e) => {
                    warn!(?e, "failed to decode get.mjs bodyBase64");
                    (StatusCode::NOT_FOUND, "Not Found").into_response()
                }
            }
        }
        "dir" => (
            StatusCode::OK,
            [
                ("Content-Type", "application/json".to_string()),
                (HEADER_BRANCH, branch.to_string()),
                (HEADER_REPO, repo_name.to_string()),
            ],
            axum::Json(val),
        )
            .into_response(),
        _ => (
            StatusCode::NOT_FOUND,
            [
                ("Content-Type", "text/plain".to_string()),
                (HEADER_BRANCH, branch.to_string()),
                (HEADER_REPO, repo_name.to_string()),
            ],
            "Not Found",
        )
            .into_response(),
    }
}

/// Try to serve a file from static paths
pub async fn try_static(state: &AppState, rel: &str) -> Option<Response> {
    for base in &state.static_paths {
        let candidate = base.join(rel.trim_start_matches('/'));
        if candidate.is_file() {
            match tokio::fs::read(&candidate).await {
                Ok(bytes) => {
                    let ct = mime_guess::from_path(&candidate)
                        .first_or_octet_stream()
                        .essence_str()
                        .to_string();
                    let mut resp =
                        (StatusCode::OK, [("Content-Type", ct.clone())], bytes).into_response();
                    let headers = resp.headers_mut();
                    headers.insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("public, max-age=3600"),
                    );
                    headers.insert(
                        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                        axum::http::HeaderValue::from_static("*"),
                    );
                    return Some(resp);
                }
                Err(e) => {
                    warn!(?e, path=%candidate.to_string_lossy(), "Failed to read static file");
                }
            }
        }
    }
    None
}
