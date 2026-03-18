//! Authorized repo list + anchor commit validation for pull operations.
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct AuthorizedReposFile {
    /// Must match RELAY_SERVER_ID when both are set.
    #[serde(default)]
    pub relay_server_id: Option<String>,
    #[serde(default)]
    pub repos: HashMap<String, RepoAnchor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoAnchor {
    /// Git object id; must be an ancestor of `branch` after every successful pull.
    pub anchor_commit: String,
    #[serde(default = "default_branch")]
    pub branch: String,
}

fn default_branch() -> String {
    "main".to_string()
}

pub fn load_from_path(path: &Path) -> anyhow::Result<AuthorizedReposFile> {
    let s = std::fs::read_to_string(path)?;
    let cfg: AuthorizedReposFile = serde_yaml::from_str(&s)?;
    Ok(cfg)
}

/// When allowlist is active: repo must be listed (server id checked at startup).
pub fn ensure_pull_allowed(cfg: &AuthorizedReposFile, _relay_server_id: Option<&str>, repo_name: &str) -> Result<(), String> {
    if cfg.repos.is_empty() {
        return Err(
            "authorized-repos file has no repos — configure at least one repo entry".into(),
        );
    }
    cfg.repos
        .get(repo_name)
        .ok_or_else(|| format!("repo '{}' is not in authorized-repos list", repo_name))?;
    Ok(())
}

/// After fetch: branch tip must contain anchor_commit in its history.
pub fn validate_anchor(repo: &git2::Repository, repo_name: &str, cfg: &AuthorizedReposFile) -> Result<(), String> {
    let anchor = cfg
        .repos
        .get(repo_name)
        .ok_or_else(|| format!("repo '{}' missing from allowlist", repo_name))?;
    let anchor_oid =
        git2::Oid::from_str(anchor.anchor_commit.trim()).map_err(|e| format!("bad anchor_commit: {}", e))?;
    let refname = format!("refs/heads/{}", anchor.branch);
    let head_oid = repo
        .refname_to_id(&refname)
        .map_err(|_| format!("{}:{} missing after pull", repo_name, anchor.branch))?;
    let base = repo
        .merge_base(anchor_oid, head_oid)
        .map_err(|e| format!("merge-base: {}", e))?;
    if base != anchor_oid {
        return Err(format!(
            "trust anchor {} not ancestor of {}:{} — pull rejected",
            anchor.anchor_commit, repo_name, anchor.branch
        ));
    }
    Ok(())
}

/// Roll back refs/heads/main to previous tip (best effort).
pub fn rollback_main(repo: &git2::Repository, prev: Option<&str>) {
    match prev {
        Some(s) => {
            if let Ok(oid) = git2::Oid::from_str(s.trim()) {
                let _ = repo.reference("refs/heads/main", oid, true, "relay trust rollback");
            }
        }
        None => {
            if let Ok(mut r) = repo.find_reference("refs/heads/main") {
                let _ = r.delete();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature};
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[test]
    fn anchor_must_be_ancestor_of_head() {
        let td = tempdir().unwrap();
        let repo = Repository::init(td.path()).unwrap();
        let sig = Signature::now("t", "t@t").unwrap();
        let tb = repo.treebuilder(None).unwrap();
        let t1 = tb.write().unwrap();
        let tree1 = repo.find_tree(t1).unwrap();
        let c1 = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "1", &tree1, &[])
            .unwrap();
        let p1 = repo.find_commit(c1).unwrap();
        let mut tb2 = repo.treebuilder(None).unwrap();
        let f = repo.blob(b"x").unwrap();
        tb2.insert("f", f, 0o100644).unwrap();
        let t2 = tb2.write().unwrap();
        let tree2 = repo.find_tree(t2).unwrap();
        let c2 = repo
            .commit(
                Some("refs/heads/main"),
                &sig,
                &sig,
                "2",
                &tree2,
                &[&p1],
            )
            .unwrap();

        let mut cfg = AuthorizedReposFile {
            relay_server_id: None,
            repos: HashMap::new(),
        };
        cfg.repos.insert(
            "r".into(),
            RepoAnchor {
                anchor_commit: c1.to_string(),
                branch: "main".into(),
            },
        );
        assert!(validate_anchor(&repo, "r", &cfg).is_ok());

        cfg.repos.get_mut("r").unwrap().anchor_commit = c2.to_string();
        assert!(validate_anchor(&repo, "r", &cfg).is_ok());

        let orphan = repo.blob(b"y").unwrap();
        let mut tb3 = repo.treebuilder(None).unwrap();
        tb3.insert("y", orphan, 0o100644).unwrap();
        let t3 = tb3.write().unwrap();
        let tree3 = repo.find_tree(t3).unwrap();
        let evil = repo
            .commit(None, &sig, &sig, "evil", &tree3, &[])
            .unwrap();
        repo.reference("refs/heads/main", evil, true, "m").unwrap();
        cfg.repos.get_mut("r").unwrap().anchor_commit = c1.to_string();
        assert!(validate_anchor(&repo, "r", &cfg).is_err());
    }
}
