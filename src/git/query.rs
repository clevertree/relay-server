use std::path::Path;
use serde_json::Value;
use tracing::debug;
use crate::git::indexing::ensure_indexed;
use crate::git::hooks::HookContext;
use crate::git;

pub fn execute_query(
    repo_root: &Path,
    repo_name: &str,
    branch: &str,
    query: Option<Value>,
    collection: &str,
) -> anyhow::Result<Value> {
    let repo_full_path = repo_root.join(format!("{}.git", repo_name));
    
    // Get current HEAD for JIT indexing
    let repo = git2::Repository::open_bare(&repo_full_path)?;
    let head = git::get_branch_commit_info(&repo, branch)
        .ok_or_else(|| anyhow::anyhow!("Branch {} not found", branch))?.0;

    // Prepare context for indexing
    let ctx = HookContext {
        repo_path: repo_full_path.clone(),
        old_commit: String::new(),
        new_commit: head,
        refname: format!("refs/heads/{}", branch),
        branch: branch.to_string(),
        is_verified: true,
        files: std::collections::HashMap::new(),
    };

    // Run JIT indexing if stale
    ensure_indexed(&ctx)?;

    // Now perform the query against the database
    let branch_bytes = if branch.is_empty() { "main".as_bytes() } else { branch.as_bytes() };
    let branch_hash = hex::encode(branch_bytes);
    let branch_hash_short = if branch_hash.len() > 12 { &branch_hash[..12] } else { &branch_hash };
    let db_path = repo_full_path.join(".relay_data").join("branches").join(branch_hash_short).join("index.db.json");

    if !db_path.exists() {
        return Ok(serde_json::json!([]));
    }

    let db_content = std::fs::read_to_string(&db_path)?;
    let db: Value = serde_json::from_str(&db_content)?;

    let mut results = db.get("collections")
        .and_then(|c| c.get(collection))
        .cloned()
        .unwrap_or(serde_json::json!([]));

    // Filtering logic
    if let Some(query_val) = query {
        if let Some(results_arr) = results.as_array_mut() {
            if let Some(q_str) = query_val.as_str() {
                if !q_str.is_empty() {
                    let q_lower = q_str.to_lowercase();
                    results_arr.retain(|item| {
                        if let Some(obj) = item.as_object() {
                            for value in obj.values() {
                                if let Some(s) = value.as_str() {
                                    if s.to_lowercase().contains(&q_lower) {
                                        return true;
                                    }
                                }
                            }
                        }
                        false
                    });
                }
            } else if let Some(q_obj) = query_val.as_object() {
                results_arr.retain(|item| {
                    if let Some(item_obj) = item.as_object() {
                        for (k, v) in q_obj {
                            if item_obj.get(k) != Some(v) {
                                return false;
                            }
                        }
                        true
                    } else {
                        false
                    }
                });
            }
        }
    }

    Ok(results)
}
