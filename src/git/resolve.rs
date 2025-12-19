use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use git2::ObjectType;
use std::path::PathBuf;
use tracing::error;

use crate::git::open_repo;
use crate::types::{GitResolveResult, HEADER_BRANCH, HEADER_REPO};

pub fn git_resolve_and_respond(
    repo_root: &PathBuf,
    _headers: &HeaderMap,
    branch: &str,
    repo_name: &str,
    decoded: &str,
) -> GitResolveResult {
    let repo = match open_repo(repo_root, repo_name) {
        Some(r) => r,
        None => {
            error!("open repo error: repo not found");
            return GitResolveResult::Respond(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    let refname = format!("refs/heads/{}", branch);
    let reference = match repo.find_reference(&refname) {
        Ok(r) => r,
        Err(_) => {
            return GitResolveResult::NotFound(decoded.to_string());
        }
    };
    let commit = match reference.peel_to_commit() {
        Ok(c) => c,
        Err(e) => {
            error!(?e, "peel to commit error");
            return GitResolveResult::Respond(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(e) => {
            error!(?e, "tree error");
            return GitResolveResult::Respond(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };

    // Path is used directly inside the selected repository
    let rel = decoded.trim_matches('/');

    // Empty path -> delegate to repo script (hooks/get.mjs)
    if rel.is_empty() {
        return GitResolveResult::NotFound(rel.to_string());
    }

    // File/dir resolution
    let path_obj = std::path::Path::new(rel);
    let entry = match tree.get_path(path_obj) {
        Ok(e) => e,
        Err(_) => return GitResolveResult::NotFound(rel.to_string()),
    };

    match entry.kind() {
        Some(ObjectType::Blob) => match repo.find_blob(entry.id()) {
            Ok(blob) => {
                let ct = mime_guess::from_path(rel)
                    .first_or_octet_stream()
                    .essence_str()
                    .to_string();
                let resp = (
                    StatusCode::OK,
                    [
                        ("Content-Type", ct),
                        (HEADER_BRANCH, branch.to_string()),
                        (HEADER_REPO, repo_name.to_string()),
                    ],
                    blob.content().to_vec(),
                )
                    .into_response();
                GitResolveResult::Respond(resp)
            }
            Err(e) => {
                error!(?e, "blob read error");
                GitResolveResult::Respond(StatusCode::INTERNAL_SERVER_ERROR.into_response())
            }
        },
        Some(ObjectType::Tree) => {
            // List directory contents as JSON
            match repo.find_tree(entry.id()) {
                Ok(dir_tree) => {
                    let mut entries = serde_json::json!({});
                    for item in dir_tree.iter() {
                        if let Some(name) = item.name() {
                            let kind = match item.kind() {
                                Some(ObjectType::Blob) => "file",
                                Some(ObjectType::Tree) => "dir",
                                _ => "unknown",
                            };
                            entries[name] = serde_json::json!({
                                "type": kind,
                                "path": format!("{}/{}", rel, name)
                            });
                        }
                    }
                    let resp = (
                        StatusCode::OK,
                        [
                            ("Content-Type", "application/json".to_string()),
                            (HEADER_BRANCH, branch.to_string()),
                            (HEADER_REPO, repo_name.to_string()),
                        ],
                        serde_json::to_string(&entries).unwrap_or_else(|_| "{}".to_string()),
                    )
                        .into_response();
                    GitResolveResult::Respond(resp)
                }
                Err(e) => {
                    error!(?e, "tree read error");
                    GitResolveResult::Respond(StatusCode::INTERNAL_SERVER_ERROR.into_response())
                }
            }
        }
        _ => GitResolveResult::NotFound(rel.to_string()),
    }
}
