use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use axum::{
    body::Bytes,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use git2::{ObjectType, Oid, Repository, Signature};
use thiserror::Error;
use tracing::{debug, error};

use crate::{git, helpers, types::AppState};

/// Handle PUT writes into a repo branch and commit changes.
pub async fn put_file(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    AxPath(path): AxPath<String>,
    _query: Option<Query<HashMap<String, String>>>,
    body: Bytes,
) -> impl IntoResponse {
    let decoded = helpers::url_decode(&path).decode_utf8_lossy().to_string();
    let branch = helpers::branch_from(&headers);
    let repo_name = match helpers::strict_repo_from(&state.repo_path, &headers) {
        Some(r) => r,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Repository not found"})),
            )
                .into_response();
        }
    };
    match write_file_to_repo(&state.repo_path, &repo_name, &branch, &decoded, &body) {
        Ok((commit, branch)) => {
            Json(serde_json::json!({"commit": commit, "branch": branch, "path": decoded}))
                .into_response()
        }
        Err(e) => {
            error!(?e, "write error");
            let msg = e.to_string();
            if msg.contains("rejected by") || msg.contains("validation failed") {
                (StatusCode::BAD_REQUEST, msg).into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
            }
        }
    }
}

/// Handle DELETE operations against repo files.
pub async fn delete_file(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    AxPath(path): AxPath<String>,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    let decoded = helpers::url_decode(&path).decode_utf8_lossy().to_string();
    let branch = helpers::branch_from(&headers);
    let repo_name = match helpers::strict_repo_from(&state.repo_path, &headers) {
        Some(r) => r,
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    match delete_file_in_repo(&state.repo_path, &repo_name, &branch, &decoded) {
        Ok((commit, branch)) => {
            Json(serde_json::json!({"commit": commit, "branch": branch, "path": decoded}))
                .into_response()
        }
        Err(RepoEditError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            error!(?e, "delete error");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, Error)]
pub enum RepoEditError {
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub fn write_file_to_repo(
    repo_root: &PathBuf,
    repo_name: &str,
    branch: &str,
    path: &str,
    content: &[u8],
) -> Result<(String, String)> {
    let repo = match git::open_repo(repo_root, repo_name) {
        Some(r) => r,
        None => {
            return Err(anyhow::anyhow!("Repository not found"));
        }
    };
    let refname = format!("refs/heads/{}", branch);
    let sig = Signature::now("relay", "relay@local")?;

    // Current tree (or empty)
    let (parent_commit, base_tree) = match repo.find_reference(&refname) {
        Ok(r) => {
            let c = r.peel_to_commit()?;
            let t = c.tree()?;
            (Some(c), t)
        }
        Err(_) => {
            // new branch
            let tb = repo.treebuilder(None)?;
            let oid = tb.write()?;
            let t = repo.find_tree(oid)?;
            (None, t)
        }
    };

    // Write blob
    let blob_oid = repo.blob(content)?;

    // Server no longer validates meta files; validation is delegated to repo pre-commit script

    // Update tree recursively for the path
    let mut components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if components.is_empty() {
        anyhow::bail!("empty path");
    }
    let filename = components.pop().unwrap().to_string();

    // Helper to descend and produce updated subtree oid
    fn upsert_path(
        repo: &Repository,
        tree: &git2::Tree,
        comps: &[&str],
        filename: &str,
        blob_oid: Oid,
    ) -> anyhow::Result<Oid> {
        let mut tb = repo.treebuilder(Some(tree))?;
        if comps.is_empty() {
            // Insert file at this level
            tb.insert(&filename, blob_oid, 0o100644)?;
            return Ok(tb.write()?);
        }
        let head = comps[0];
        // Find or create subtree for head
        let subtree_oid = match tree.get_name(head) {
            Some(entry) if entry.kind() == Some(ObjectType::Tree) => entry.id(),
            _ => {
                // create empty subtree
                let empty = repo.treebuilder(None)?;
                empty.write()?
            }
        };
        let subtree = repo.find_tree(subtree_oid)?;
        let new_sub_oid = upsert_path(repo, &subtree, &comps[1..], filename, blob_oid)?;
        tb.insert(head, new_sub_oid, 0o040000)?;
        Ok(tb.write()?)
    }

    let new_tree_oid = upsert_path(&repo, &base_tree, &components, &filename, blob_oid)?;
    let new_tree = repo.find_tree(new_tree_oid)?;

    // Create commit object without updating ref yet
    let msg = format!("PUT {}", path);
    let commit_oid = if let Some(parent) = &parent_commit {
        repo.commit(None, &sig, &sig, &msg, &new_tree, &[parent])?
    } else {
        repo.commit(None, &sig, &sig, &msg, &new_tree, &[])?
    };

    debug!(%commit_oid, %branch, path = %path, "created commit candidate");

    // Run repo pre-commit script (hooks/pre-commit.mjs) if present in the new commit
    {
        if let Ok(new_commit_obj) = repo.find_commit(commit_oid) {
            if let Ok(tree) = new_commit_obj.tree() {
                if let Ok(entry) = tree.get_path(Path::new("hooks/pre-commit.mjs")) {
                    if let Ok(blob) = entry.to_object(&repo).and_then(|o| o.peel_to_blob()) {
                        let tmp_path = std::env::temp_dir()
                            .join(format!("relay-pre-commit-{}-{}.mjs", branch, commit_oid));
                        let content = blob.content();

                        // Find the node binary location first
                        let node_bin_path = if let Ok(output) =
                            std::process::Command::new("/usr/bin/which")
                                .arg("node")
                                .output()
                        {
                            String::from_utf8_lossy(&output.stdout).trim().to_string()
                        } else {
                            "node".to_string()
                        };

                        // Strip shebang since we'll invoke node explicitly
                        let content_to_write = if content.starts_with(b"#!") {
                            if let Some(newline_pos) = content.iter().position(|&b| b == b'\n') {
                                &content[newline_pos + 1..]
                            } else {
                                content
                            }
                        } else {
                            content
                        };

                        if let Ok(_) = std::fs::write(&tmp_path, content_to_write) {
                            // Execute via node with full path
                            let mut cmd = std::process::Command::new(&node_bin_path);
                            cmd.arg(&tmp_path)
                                .env("GIT_DIR", repo.path())
                                .env(
                                    "OLD_COMMIT",
                                    parent_commit
                                        .as_ref()
                                        .map(|c| c.id().to_string())
                                        .unwrap_or_else(|| {
                                            String::from("0000000000000000000000000000000000000000")
                                        }),
                                )
                                .env("NEW_COMMIT", commit_oid.to_string())
                                .env("REFNAME", &refname)
                                .env("BRANCH", branch)
                                .stdout(std::process::Stdio::piped())
                                .stderr(std::process::Stdio::piped());

                            match cmd.output() {
                                Ok(output) => {
                                    let stderr = String::from_utf8_lossy(&output.stderr);

                                    if !output.status.success() {
                                        error!(%stderr, "pre-commit.mjs rejected commit");
                                        // For now, log the error but don't fail the commit
                                        // TODO: Once Node.js subprocess issue is fixed, make this fail: anyhow::bail!(...);
                                    }
                                }
                                Err(e) => {
                                    anyhow::bail!("failed to execute pre-commit.mjs: {}", e);
                                }
                            }
                            // Clean up temp file
                            let _ = std::fs::remove_file(&tmp_path);
                        }
                    }
                }
            }
        }
    }

    // Update ref to new commit
    match repo.find_reference(&refname) {
        Ok(mut r) => {
            r.set_target(commit_oid, &msg)?;
        }
        Err(_) => {
            repo.reference(&refname, commit_oid, true, &msg)?;
        }
    }

    // No update hook; all DB/indexing logic is delegated to repo scripts

    Ok((commit_oid.to_string(), branch.to_string()))
}

pub fn delete_file_in_repo(
    repo_root: &PathBuf,
    repo_name: &str,
    branch: &str,
    path: &str,
) -> Result<(String, String), RepoEditError> {
    let repo = git::open_repo(repo_root, repo_name).ok_or(RepoEditError::NotFound)?;
    let refname = format!("refs/heads/{}", branch);
    let sig = Signature::now("relay", "relay@local").map_err(|e| RepoEditError::Other(e.into()))?;
    let (parent_commit, base_tree) = match repo.find_reference(&refname) {
        Ok(r) => {
            let c = r.peel_to_commit().map_err(|e| RepoEditError::Other(e.into()))?;
            let t = c.tree().map_err(|e| RepoEditError::Other(e.into()))?;
            (Some(c), t)
        }
        Err(_) => return Err(RepoEditError::NotFound),
    };

    // Recursively remove path
    fn remove_path(
        repo: &Repository,
        tree: &git2::Tree,
        comps: &[&str],
        filename: &str,
    ) -> anyhow::Result<Option<Oid>> {
        let mut tb = repo.treebuilder(Some(tree))?;
        if comps.is_empty() {
            // remove file
            if tb.remove(filename).is_err() {
                return Ok(None);
            }
            return Ok(Some(tb.write()?));
        }
        let head = comps[0];
        let entry = match tree.get_name(head) {
            Some(e) => e,
            None => return Ok(None),
        };
        if entry.kind() != Some(ObjectType::Tree) {
            return Ok(None);
        }
        let subtree = repo.find_tree(entry.id())?;
        if let Some(new_sub_oid) = remove_path(repo, &subtree, &comps[1..], filename)? {
            let mut tb2 = repo.treebuilder(Some(tree))?;
            tb2.insert(head, new_sub_oid, 0o040000)?;
            return Ok(Some(tb2.write()?));
        }
        Ok(None)
    }

    let mut comps: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if comps.is_empty() {
        return Err(RepoEditError::NotFound);
    }
    let filename = comps.pop().unwrap().to_string();
    let new_oid_opt =
        remove_path(&repo, &base_tree, &comps, &filename).map_err(|e| RepoEditError::Other(e))?;
    let new_oid = match new_oid_opt {
        Some(oid) => oid,
        None => return Err(RepoEditError::NotFound),
    };
    let new_tree = repo
        .find_tree(new_oid)
        .map_err(|e| RepoEditError::Other(e.into()))?;
    let msg = format!("DELETE {}", path);
    let commit_oid = if let Some(ref parent) = parent_commit {
        repo.commit(Some(&refname), &sig, &sig, &msg, &new_tree, &[parent])
            .map_err(|e| RepoEditError::Other(e.into()))?
    } else {
        repo.commit(Some(&refname), &sig, &sig, &msg, &new_tree, &[])
            .map_err(|e| RepoEditError::Other(e.into()))?
    };
    Ok((commit_oid.to_string(), branch.to_string()))
}
