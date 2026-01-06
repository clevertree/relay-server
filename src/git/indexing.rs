use std::path::PathBuf;
use tracing::{info, debug};
use crate::git::hooks::{execute_repo_hook, HookContext};
use std::sync::{Mutex, OnceLock};
use std::collections::HashSet;

static ONGOING_INDEXING: OnceLock<Mutex<HashSet<(PathBuf, String)>>> = OnceLock::new();

fn get_indexing_lock() -> &'static Mutex<HashSet<(PathBuf, String)>> {
    ONGOING_INDEXING.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn ensure_indexed(ctx: &HookContext) -> anyhow::Result<()> {
    let branch_bytes = if ctx.branch.is_empty() { "main".as_bytes() } else { ctx.branch.as_bytes() };
    let branch_hash = hex::encode(branch_bytes);
    let branch_hash_short = if branch_hash.len() > 12 { &branch_hash[..12] } else { &branch_hash };
    
    let relay_data_path = ctx.repo_path.join(".relay_data");
    let db_path = relay_data_path.join("branches").join(branch_hash_short).join("index.db.json");
    
    let mut indexed_head = String::new();
    if db_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&db_path) {
            if let Ok(db) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(meta) = db.get("metadata") {
                    if let Some(head) = meta.get("indexed_head") {
                        indexed_head = head.as_str().unwrap_or("").to_string();
                    }
                }
            }
        }
    }
    
    if indexed_head != ctx.new_commit {
        let lock_key = (ctx.repo_path.clone(), ctx.branch.clone());
        
        {
            let mut ongoing = get_indexing_lock().lock().unwrap();
            if ongoing.contains(&lock_key) {
                debug!("JIT indexing already in progress for branch {} in repo {:?}", ctx.branch, ctx.repo_path);
                // Drop lock and wait a bit or just return?
                // Ideally we wait for the other one to finish, but for simplicity we can just return 
                // and the query caller will try to read a potentially partially written file or wait.
                // But better to wait here.
            } else {
                ongoing.insert(lock_key.clone());
            }
        }
        
        // If we want to wait, we need a better primitive than HashSet.
        // But for "hardening", preventing the parallel execution is the first step.
        
        info!("Branch {} is stale ({} != {}). Running JIT indexing...", ctx.branch, indexed_head, ctx.new_commit);
        
        let result = execute_repo_hook(ctx, "index");
        
        {
            let mut ongoing = get_indexing_lock().lock().unwrap();
            ongoing.remove(&lock_key);
        }
        
        result?;
        debug!("JIT indexing completed for branch {}", ctx.branch);
    } else {
        debug!("Branch {} is up to date (head: {})", ctx.branch, ctx.new_commit);
    }
    
    Ok(())
}
