use std::collections::HashMap;
use std::path::PathBuf;

use axum::{
    extract::Query,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use hook_transpiler::{transpile, version as transpiler_version, TranspileError, TranspileOptions};

use crate::git;
use crate::types::*;

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

/// Transpile a hook file from git and return a response
pub fn transpile_hook_file(
    repo_path: &PathBuf,
    branch: &str,
    repo_name: &str,
    normalized_path: &str,
) -> Option<Response> {
    let source_bytes = git::read_file_from_repo(repo_path, branch, normalized_path).ok()?;
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
