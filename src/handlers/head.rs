use std::collections::HashMap;

use axum::{
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};

use crate::{git, helpers, AppState, GitResolveResult, HEADER_BRANCH, HEADER_REPO};

use super::file::try_static;

/// HEAD / - returns same headers as GET but no body. Returns 204 No Content.
pub async fn head_root(
    State(state): State<AppState>,
    _headers: HeaderMap,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    // If index exists, signal 200 without body
    if let Some(resp) = try_static(&state, "index.html").await {
        let (parts, _body) = resp.into_parts();
        if parts.status == StatusCode::OK {
            return StatusCode::OK.into_response();
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

/// HEAD handler for files. Returns same headers as GET but no body.
/// Returns 200 if file exists, 404 if not found or repo doesn't exist.
pub async fn head_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxPath(path): AxPath<String>,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    let decoded = helpers::url_decode(&path).decode_utf8_lossy().to_string();

    let branch = helpers::branch_from(&headers);
    let repo_name_opt = helpers::strict_repo_from(&state.repo_path, &headers);
    let repo_name: String;
    if repo_name_opt.is_none() {
        // No repo selected: treat as Git 404 and check static for existence
        if let Some(resp) = try_static(&state, &decoded).await {
            let (parts, _body) = resp.into_parts();
            if parts.status == StatusCode::OK {
                return (
                    StatusCode::OK,
                    [
                        (
                            "Content-Type",
                            parts
                                .headers
                                .get("Content-Type")
                                .and_then(|h| h.to_str().ok())
                                .unwrap_or("application/octet-stream")
                                .to_string(),
                        ),
                        (HEADER_BRANCH, branch.clone()),
                        (HEADER_REPO, "".to_string()),
                    ],
                )
                    .into_response();
            }
        }
        return (
            StatusCode::NOT_FOUND,
            [
                ("Content-Type", "text/plain".to_string()),
                (HEADER_BRANCH, branch.clone()),
                (HEADER_REPO, "".to_string()),
            ],
        )
            .into_response();
    } else {
        repo_name = repo_name_opt.unwrap();
    }

    // Resolve via Git - if found, return headers without body
    match git::git_resolve_and_respond(&state.repo_path, &headers, &branch, &repo_name, &decoded) {
        GitResolveResult::Respond(resp) => {
            // If GET would have succeeded, return 200 with same headers but no body
            let (parts, _body) = resp.into_parts();
            if parts.status == StatusCode::OK {
                (
                    StatusCode::OK,
                    [
                        (
                            "Content-Type",
                            parts
                                .headers
                                .get("Content-Type")
                                .and_then(|h| h.to_str().ok())
                                .unwrap_or("application/octet-stream")
                                .to_string(),
                        ),
                        (HEADER_BRANCH, branch),
                        (HEADER_REPO, repo_name),
                    ],
                )
                    .into_response()
            } else {
                // Return same status as GET would
                StatusCode::NOT_FOUND.into_response()
            }
        }
        GitResolveResult::NotFound(_) => {
            // File not found in Git - return 404 without checking hooks
            (
                StatusCode::NOT_FOUND,
                [
                    ("Content-Type", "text/plain".to_string()),
                    (HEADER_BRANCH, branch),
                    (HEADER_REPO, repo_name),
                ],
            )
                .into_response()
        }
    }
}
