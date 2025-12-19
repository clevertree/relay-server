use std::collections::HashMap;
use std::path::PathBuf;

use axum::{
    extract::Query,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use git2::Repository;
use percent_encoding::percent_decode_str;

use crate::types::*;
use hook_transpiler::{transpile, version as transpiler_version, TranspileError, TranspileOptions};

/// Extract repo name from X-Relay-Repo header or repo.{hostname} subdomain
pub fn strict_repo_from(root: &PathBuf, headers: &HeaderMap) -> Option<String> {
    // Header first
    if let Some(h) = headers.get(crate::types::HEADER_REPO).and_then(|v| v.to_str().ok()) {
        let name = h.trim().trim_matches('/');
        if !name.is_empty() {
            let name = name.to_string();
            if crate::git::bare_repo_names(root).iter().any(|n| n == &name) {
                return Some(name);
            }
        }
    }
    // Sub-subdomain: first label in host if there are 3+ labels
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        let host = host.split(':').next().unwrap_or(host); // strip port
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() >= 3 {
            let candidate = parts[0].to_string();
            if crate::git::bare_repo_names(root).iter().any(|n| n == &candidate) {
                return Some(candidate);
            }
        }
    }
    // Default: first available
    crate::git::bare_repo_names(root).into_iter().next()
}

/// Resolve the branch name from X-Relay-Branch header, defaults to main
pub fn branch_from(headers: &HeaderMap) -> String {
    if let Some(h) = headers.get(crate::types::HEADER_BRANCH).and_then(|v| v.to_str().ok()) {
        if !h.is_empty() {
            return h.to_string();
        }
    }
    crate::types::DEFAULT_BRANCH.to_string()
}

/// Minimal URL percent-decoder wrapper used by handlers.
/// Returns a percent-decoder so callers can choose utf8 lossless decoding.
pub fn url_decode(input: &str) -> percent_encoding::PercentDecode<'_> {
    percent_decode_str(input)
}

/// Parse boolean-like strings for transpile query parameters
fn parse_bool_like(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Check if request includes transpile flag in query or headers
pub fn should_transpile_request(
    headers: &HeaderMap,
    query: &Option<Query<HashMap<String, String>>>,
) -> bool {
    if let Some(q) = query {
        if let Some(val) = q.get("transpile") {
            if parse_bool_like(val) {
                return true;
            }
        }
    }
    if let Some(header) = headers
        .get("x-relay-transpile")
        .and_then(|v| v.to_str().ok())
    {
        if parse_bool_like(header) {
            return true;
        }
    }
    false
}

/// Check if a file path is a transpilable hook file (.jsx, .tsx, .ts, .mts, .mjs under hooks/)
pub fn is_transpilable_hook_path(path: &str) -> bool {
    let normalized = path.trim_start_matches('/').to_ascii_lowercase();
    if !normalized.starts_with("hooks/") {
        return false;
    }
    normalized.ends_with(".jsx")
        || normalized.ends_with(".tsx")
        || normalized.ends_with(".ts")
        || normalized.ends_with(".mts")
        || normalized.ends_with(".mjs")
}

/// Add transpiler version header to response
pub fn add_transpiler_version_header(resp: &mut Response) {
    if let Ok(val) = axum::http::HeaderValue::from_str(transpiler_version()) {
        resp.headers_mut().insert(
            axum::http::header::HeaderName::from_static("x-relay-transpiler-version"),
            val,
        );
    }
}

/// Map transpile errors to HTTP status and message
fn map_transpile_error(err: TranspileError) -> (StatusCode, String) {
    match err {
        TranspileError::ParseError {
            filename,
            line,
            col,
            message,
        } => (
            StatusCode::BAD_REQUEST,
            format!(
                "Parse error in {} at {}:{} — {}",
                filename, line, col, message
            ),
        ),
        TranspileError::TransformError { filename, source } => (
            StatusCode::BAD_REQUEST,
            format!("Transform error in {} — {}", filename, source),
        ),
        TranspileError::CodegenError { filename, source } => (
            StatusCode::BAD_REQUEST,
            format!("Code generation error in {} — {}", filename, source),
        ),
    }
}

/// Build a transpile error response with proper headers
pub fn build_transpile_error_response(
    err: TranspileError,
    branch: Option<&str>,
    repo: Option<&str>,
) -> Response {
    let (status, diag) = map_transpile_error(err);
    let mut resp = (
        status,
        axum::Json(TranspileResponse {
            code: None,
            map: None,
            diagnostics: Some(diag),
            ok: false,
        }),
    )
        .into_response();
    let headers = resp.headers_mut();
    if let Some(branch_value) = branch {
        if let Ok(val) = axum::http::HeaderValue::from_str(branch_value) {
            headers.insert(crate::types::HEADER_BRANCH, val);
        }
    }
    if let Some(repo_value) = repo {
        if let Ok(val) = axum::http::HeaderValue::from_str(repo_value) {
            headers.insert(crate::types::HEADER_REPO, val);
        }
    }
    add_transpiler_version_header(&mut resp);
    resp
}

/// Read a file from a git repository
pub fn read_file_from_repo(
    repo_path: &PathBuf,
    branch: &str,
    path: &str,
) -> Result<Vec<u8>, ReadError> {
    let repo = Repository::open_bare(repo_path).map_err(|e| ReadError::Other(e.into()))?;
    let refname = format!("refs/heads/{}", branch);
    let reference = repo
        .find_reference(&refname)
        .map_err(|_| ReadError::NotFound)?;
    let commit = reference
        .peel_to_commit()
        .map_err(|_| ReadError::NotFound)?;
    let tree = commit.tree().map_err(|e| ReadError::Other(e.into()))?;
    let entry = tree
        .get_path(std::path::Path::new(path))
        .map_err(|_| ReadError::NotFound)?;
    let blob = repo
        .find_blob(entry.id())
        .map_err(|e| ReadError::Other(e.into()))?;
    Ok(blob.content().to_vec())
}

/// Transpile a hook file from git and return a response
pub fn transpile_hook_file(
    repo_path: &PathBuf,
    branch: &str,
    repo_name: &str,
    normalized_path: &str,
) -> Option<Response> {
    let source_bytes = read_file_from_repo(repo_path, branch, normalized_path).ok()?;
    let source = String::from_utf8(source_bytes).ok()?;
    let filename = std::path::Path::new(normalized_path)
        .file_name()
        .and_then(|f| f.to_str())
        .map(|s| s.to_string());
    let opts = TranspileOptions {
        filename,
        react_dev: cfg!(debug_assertions),
        to_commonjs: false,
        pragma: Some("h".to_string()),
        pragma_frag: None,
    };
    match transpile(&source, opts) {
        Ok(out) => {
            let mut resp = (
                StatusCode::OK,
                [
                    ("Content-Type", "text/javascript".to_string()),
                    (crate::types::HEADER_BRANCH, branch.to_string()),
                    (crate::types::HEADER_REPO, repo_name.to_string()),
                ],
                out.code,
            )
                .into_response();
            add_transpiler_version_header(&mut resp);
            Some(resp)
        }
        Err(err) => Some(build_transpile_error_response(
            err,
            Some(branch),
            Some(repo_name),
        )),
    }
}

/// List branch names from a repository
pub fn list_branches(repo: &Repository) -> Vec<String> {
    let mut out = vec![];
    if let Ok(mut iter) = repo.branches(None) {
        while let Some(Ok((b, _))) = iter.next() {
            if let Ok(name) = b.name() {
                if let Some(s) = name {
                    out.push(s.to_string());
                }
            }
        }
    }
    out
}
