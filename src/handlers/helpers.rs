use std::path::PathBuf;

use axum::http::HeaderMap;
use git2::Repository;
use percent_encoding::percent_decode_str;

/// Repo resolution: valid `X-Relay-Repo` → subdomain label → `RELAY_DEFAULT_REPO` → first bare name (sorted).
pub fn strict_repo_from(
    root: &PathBuf,
    default_repo_name: Option<&str>,
    headers: &HeaderMap,
) -> Option<String> {
    let names = crate::git::bare_repo_names(root);
    if names.is_empty() {
        return None;
    }
    if let Some(h) = headers.get(crate::types::HEADER_REPO).and_then(|v| v.to_str().ok()) {
        let name = h.trim().trim_matches('/').trim_end_matches(".git");
        if !name.is_empty() && names.iter().any(|n| n == name) {
            return Some(name.to_string());
        }
    }
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        let host = host.split(':').next().unwrap_or(host);
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() >= 3 {
            let candidate = parts[0];
            if names.iter().any(|n| n == candidate) {
                return Some(candidate.to_string());
            }
        }
    }
    if let Some(d) = default_repo_name {
        let d = d.trim().trim_end_matches(".git");
        if !d.is_empty() && names.iter().any(|n| n == d) {
            return Some(d.to_string());
        }
    }
    names.into_iter().next()
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
