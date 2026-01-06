use std::collections::HashMap;

use axum::{
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{header::CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use http_body::Body as HttpBody;

use crate::{git, helpers, AppState, GitResolveResult, HEADER_BRANCH, HEADER_REPO};

use super::file::try_static;

fn body_len<B: HttpBody>(body: &B) -> Option<u64> {
    let hint = body.size_hint();
    if let Some(exact) = hint.exact() {
        Some(exact)
    } else {
        hint.upper()
    }
}

fn insert_header_if_missing(headers: &mut HeaderMap, name: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    let lower = name.to_ascii_lowercase();
    if let Ok(header_name) = HeaderName::from_bytes(lower.as_bytes()) {
        if !headers.contains_key(&header_name) {
            if let Ok(header_value) = HeaderValue::from_str(value) {
                headers.insert(header_name, header_value);
            }
        }
    }
}

fn headify_response(resp: Response, branch: &str, repo: &str) -> Response {
    let (mut parts, body) = resp.into_parts();
    if parts.headers.get(CONTENT_LENGTH).is_none() {
        if let Some(len) = body_len(&body) {
            if let Ok(val) = HeaderValue::from_str(&len.to_string()) {
                parts.headers.insert(CONTENT_LENGTH, val);
            }
        }
    }

    insert_header_if_missing(&mut parts.headers, HEADER_BRANCH, branch);
    insert_header_if_missing(&mut parts.headers, HEADER_REPO, repo);

    Response::from_parts(parts, Body::empty())
}

/// HEAD / - returns same headers as GET but no body. Returns 204 No Content.
pub async fn head_root(
    State(state): State<AppState>,
    _headers: HeaderMap,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    // If index exists, signal 200 without body
    if let Some(resp) = try_static(&state, "index.html").await {
        let head_resp = headify_response(resp, "", "");
        if head_resp.status() == StatusCode::OK {
            return head_resp;
        }
    }
    StatusCode::NO_CONTENT.into_response()
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
            let head_resp = headify_response(resp, &branch, "");
            if head_resp.status() == StatusCode::OK {
                return head_resp;
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
            let head_resp = headify_response(resp, &branch, &repo_name);
            if head_resp.status() == StatusCode::OK {
                head_resp
            } else {
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
