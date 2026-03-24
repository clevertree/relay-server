use std::path::PathBuf;

use axum::http::HeaderMap;
use git2::Repository;
use percent_encoding::percent_decode_str;

fn normalize_fqdn(s: &str) -> String {
    s.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Extract repo slug from `Host` when it matches `{repo}.{node_fqdn}`.
/// `repo` must be a single DNS label (no dots). Returns `None` if `host == node_fqdn` (no repo prefix).
pub fn repo_slug_from_host(host_header: &str, node_fqdn: &str, bare_names: &[String]) -> Option<String> {
    let host = normalize_fqdn(host_header.split(':').next().unwrap_or(host_header));
    let node = normalize_fqdn(node_fqdn);
    if node.is_empty() {
        return None;
    }
    let suffix = format!(".{}", node);
    if host == node {
        return None;
    }
    if !host.ends_with(&suffix) {
        return None;
    }
    let slug = &host[..host.len() - suffix.len()];
    if slug.is_empty() || slug.contains('.') {
        return None;
    }
    if bare_names.iter().any(|n| n == slug) {
        Some(slug.to_string())
    } else {
        None
    }
}

/// Resolve bare repo name from request `Host` and configured node FQDN.
pub fn repo_from_host(
    root: &PathBuf,
    node_fqdn: Option<&str>,
    headers: &HeaderMap,
) -> Option<String> {
    let names = crate::git::bare_repo_names(root);
    if names.is_empty() {
        return None;
    }
    let node = node_fqdn?.trim();
    if node.is_empty() {
        return None;
    }
    let h = headers.get("host")?.to_str().ok()?;
    repo_slug_from_host(h, node, &names)
}

/// Resolve the branch name from `X-Relay-Branch` header, defaults to **main**.
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
