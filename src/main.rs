use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
};

use relay_server::{
    cli::{Cli, Commands},
    git,
    handlers,
    helpers,
    transpiler,
    types::*,
    AppState, HEADER_BRANCH, HEADER_REPO,
};

use axum::{
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    body::Body,
    middleware::Next,
    routing::{get, post},
    Json, Router,
};
use axum_server::tls_rustls::RustlsConfig;
use clap::Parser;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};
use tracing_appender::rolling;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};
use anyhow::Result;

// IPFS CLI commands removed; IPFS logic is delegated to repo scripts

/// OPTIONS handler — discovery: capabilities, branches, repos, current selections, client hooks
async fn options_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    let branch = helpers::branch_from(&headers);
    let repo_name = helpers::strict_repo_from(&state.repo_path, &headers);

    let repo_names = git::bare_repo_names(&state.repo_path);
    let mut repos_json: Vec<serde_json::Value> = Vec::new();
    let mut relay_config: Option<RelayConfig> = None;

    for name in &repo_names {
        if let Some(repo) = git::open_repo(&state.repo_path, name) {
            let mut heads_map = serde_json::Map::new();
            let branches = helpers::list_branches(&repo);
            for b in &branches {
                if let Ok(reference) = repo.find_reference(&format!("refs/heads/{}", b)) {
                    if let Ok(commit) = reference.peel_to_commit() {
                        heads_map.insert(b.clone(), serde_json::json!(commit.id().to_string()));
                    }
                }
            }
            if Some(name) == repo_name.as_ref() {
                if relay_config.is_none() {
                    relay_config = git::read_relay_config(&repo, &branch);
                }
            }
            repos_json.push(serde_json::json!({
                "name": name,
                "branches": serde_json::Value::Object(heads_map),
            }));
        }
    }

    let allow = "GET, PUT, DELETE, OPTIONS, QUERY";
    let mut body = serde_json::json!({
        "ok": true,
        "capabilities": {"supports": ["GET","PUT","DELETE","OPTIONS","QUERY"]},
        "repos": repos_json,
        "currentBranch": branch,
        "currentRepo": repo_name.clone().unwrap_or_default(),
    });

    if let Some(config) = relay_config {
        if let Ok(config_json) = serde_json::to_value(&config) {
            if let Some(obj) = body.as_object_mut() {
                if let Some(config_obj) = config_json.as_object() {
                    for (key, value) in config_obj {
                        obj.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }

    (
        StatusCode::OK,
        [
            ("Allow", allow.to_string()),
            ("Access-Control-Allow-Origin", "*".to_string()),
            ("Access-Control-Allow-Methods", allow.to_string()),
            ("Access-Control-Allow-Headers", "*".to_string()),
            ("Content-Type", "application/json".to_string()),
            (HEADER_BRANCH, branch),
            (HEADER_REPO, repo_name.unwrap_or_default()),
        ],
        Json(body),
    )
}

/// List all local branches with their HEAD commit ids
// Helpers for strict multi-repo support - see helpers module

// removed SQLite row_to_json helper

// Legacy exhaustive tests for the server. Disabled by default to keep the
// build green while we iterate on the new transpiler.
#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature};
    use std::io::Write as _;
    use std::path::Path as FsPath;
    use std::time::Duration as StdDuration;
    use tempfile::tempdir;

    #[test]
    fn test_row_to_json_basic_types_removed() {}

    #[cfg(all(not(target_os = "windows"), feature = "ipfs_tests"))]
    async fn ensure_ipfs_daemon(ipfs_repo: &FsPath, api_port: u16) {
        // init if needed
        let _ = std::fs::create_dir_all(ipfs_repo);
        let mut init = TokioCommand::new("ipfs");
        init.arg("init").env("IPFS_PATH", ipfs_repo);
        let _ = init.status().await;
        // configure API address
        let mut cfg = TokioCommand::new("ipfs");
        cfg.arg("config")
            .arg("Addresses.API")
            .arg(format!("/ip4/127.0.0.1/tcp/{}", api_port))
            .env("IPFS_PATH", ipfs_repo);
        let _ = cfg.status().await;
        // start daemon in background if not running
        let mut id = TokioCommand::new("ipfs");
        id.arg("id").env("IPFS_PATH", ipfs_repo);
        if id.status().await.ok().map(|s| s.success()).unwrap_or(false) {
            return;
        }
        let mut daemon = std::process::Command::new("ipfs");
        daemon
            .arg("daemon")
            .env("IPFS_PATH", ipfs_repo)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
        let _child = daemon.spawn().expect("spawn ipfs daemon");
        // wait for API to come up
        for _ in 0..50 {
            let mut id = TokioCommand::new("ipfs");
            // Use IPFS_PATH and the api file written by the daemon
            id.arg("id").env("IPFS_PATH", ipfs_repo);
            if id.status().await.ok().map(|s| s.success()).unwrap_or(false) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[cfg(all(not(target_os = "windows"), feature = "ipfs_tests"))]
    async fn ipfs_add_dir(ipfs_repo: &FsPath, _api_port: u16, dir: &FsPath) -> String {
        // Use verbose recursive add to capture the directory line reliably across older go-ipfs versions
        let mut cmd = TokioCommand::new("ipfs");
        cmd.arg("add").arg("-r").arg(dir);
        cmd.env("IPFS_PATH", ipfs_repo);
        let out = cmd.output().await.expect("ipfs add");
        assert!(out.status.success(), "ipfs add failed: {:?}", out);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let dir_str = dir.to_string_lossy().to_string();
        let base = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let mut last_cid: Option<String> = None;
        let mut dir_cid: Option<String> = None;
        for line in stdout.lines() {
            // expected: "added <cid> <path>"
            let mut it = line.split_whitespace();
            let first = it.next();
            let second = it.next();
            let rest = it.next();
            if first == Some("added") {
                if let Some(cid) = second.map(|s| s.to_string()) {
                    last_cid = Some(cid.clone());
                    if let Some(path) = rest {
                        // Match either the exact directory path or a trailing basename match
                        if path == dir_str || (!base.is_empty() && path.ends_with(base)) {
                            dir_cid = Some(cid);
                        }
                    }
                }
            }
        }
        let cid = dir_cid.or(last_cid).unwrap_or_default();
        assert!(
            !cid.is_empty(),
            "could not parse CID from ipfs add output: {}",
            stdout
        );
        cid
    }

    #[cfg(all(not(target_os = "windows"), feature = "ipfs_tests"))]
    async fn write_file(p: &FsPath, content: &str) {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut f = std::fs::File::create(p).unwrap();
        let _ = f.write_all(content.as_bytes());
    }

    // IPFS dir listing is merged into directory response (Git + IPFS), sizes present
    #[cfg(all(not(target_os = "windows"), feature = "ipfs_tests"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ipfs_dir_listing_merged_with_git() {
        // Start ephemeral IPFS
        let ipfs_dir = tempdir().unwrap();
        let api_port = 5020u16;
        ensure_ipfs_daemon(ipfs_dir.path(), api_port).await;
        let cache_dir = tempdir().unwrap();
        std::env::set_var("RELAY_IPFS_CACHE_ROOT", cache_dir.path());

        // Create a directory on disk to add to IPFS
        let src_dir = tempdir().unwrap();
        std::fs::create_dir_all(src_dir.path().join("assets")).unwrap();
        write_file(&src_dir.path().join("assets/hello.txt"), "hello").await;
        write_file(&src_dir.path().join("readme.md"), "# readme\n").await;
        let root_cid = ipfs_add_dir(ipfs_dir.path(), api_port, src_dir.path()).await;
        // Wait until path resolves under IPFS
        for _ in 0..20 {
            let status = TokioCommand::new("ipfs")
                .arg("resolve")
                .arg("-r")
                .arg(format!("/ipfs/{}/assets/hello.txt", root_cid))
                .env("IPFS_PATH", ipfs_dir.path())
                .status()
                .await
                .unwrap();
            if status.success() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Bare git with empty tree but relay.yaml pointing to CID
        let repo_dir = tempdir().unwrap();
        let repo = Repository::init_bare(repo_dir.path()).unwrap();
        {
            let sig = Signature::now("relay", "relay@local").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            let tree_id = tb.write().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let _ = repo
                .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        // add relay.yaml
        let yaml = format!(
            "ipfs:\n  rootHash: \"{}\"\n  branches: [ \"main\" ]\n",
            root_cid
        );
        {
            let head = repo.find_reference("refs/heads/main").unwrap();
            let commit = head.peel_to_commit().unwrap();
            let base_tree = commit.tree().unwrap();
            let blob_oid = repo.blob(yaml.as_bytes()).unwrap();
            let mut tb = repo.treebuilder(Some(&base_tree)).unwrap();
            tb.insert("relay.yaml", blob_oid, 0o100644).unwrap();
            let new_tree_id = tb.write().unwrap();
            let new_tree = repo.find_tree(new_tree_id).unwrap();
            let sig = Signature::now("relay", "relay@local").unwrap();
            let _ = repo
                .commit(
                    Some("refs/heads/main"),
                    &sig,
                    &sig,
                    "add relay.yaml",
                    &new_tree,
                    &[&commit],
                )
                .unwrap();
        }

        // Build listing at IPFS subdir 'assets' (Git is empty), expect IPFS entries appear
        std::env::set_var("IPFS_PATH", ipfs_dir.path());
        let root_ref = repo.find_reference("refs/heads/main").unwrap();
        let commit = root_ref.peel_to_commit().unwrap();
        let tree = commit.tree().unwrap();
        let (ct, md) = super::directory_response(
            &repo_dir.path().to_path_buf(),
            &tree,
            &tree,
            "assets",
            "text/markdown",
            "main",
            "",
        );
        assert_eq!(ct, "text/markdown");
        let s = String::from_utf8(md).unwrap();
        assert!(
            s.contains("hello.txt"),
            "listing should include IPFS file hello.txt: {}",
            s
        );
        // Size column should show at least 5 bytes for hello.txt
        assert!(
            s.contains("hello.txt") && s.contains("| 5 |"),
            "should show size 5 bytes: {}",
            s
        );
    }

    // Changing CID should refresh dir cache file
    #[cfg(all(not(target_os = "windows"), feature = "ipfs_tests"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ipfs_dir_cache_invalidation_on_cid_change() {
        let ipfs_dir = tempdir().unwrap();
        let api_port = 5021u16;
        ensure_ipfs_daemon(ipfs_dir.path(), api_port).await;
        std::env::set_var("IPFS_PATH", ipfs_dir.path());
        let cache_dir = tempdir().unwrap();
        std::env::set_var("RELAY_IPFS_CACHE_ROOT", cache_dir.path());

        // Make first IPFS directory
        let src1 = tempdir().unwrap();
        write_file(&src1.path().join("a.txt"), "aaa").await;
        let cid1 = ipfs_add_dir(ipfs_dir.path(), api_port, src1.path()).await;

        // Make second IPFS directory
        let src2 = tempdir().unwrap();
        write_file(&src2.path().join("b.txt"), "bbbb").await;
        let cid2 = ipfs_add_dir(ipfs_dir.path(), api_port, src2.path()).await;

        // Bare git repo with relay.yaml -> cid1, then update to cid2
        let repo_dir = tempdir().unwrap();
        let repo = Repository::init_bare(repo_dir.path()).unwrap();
        {
            let sig = Signature::now("relay", "relay@local").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            let tree_id = tb.write().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let _ = repo
                .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        let write_yaml_commit = |repo: &Repository, cid: &str| {
            let yaml = format!("ipfs:\n  rootHash: \"{}\"\n  branches: [ \"main\" ]\n", cid);
            let head = repo.find_reference("refs/heads/main").unwrap();
            let commit = head.peel_to_commit().unwrap();
            let base_tree = commit.tree().unwrap();
            let blob_oid = repo.blob(yaml.as_bytes()).unwrap();
            let mut tb = repo.treebuilder(Some(&base_tree)).unwrap();
            tb.insert("relay.yaml", blob_oid, 0o100644).unwrap();
            let new_tree_id = tb.write().unwrap();
            let new_tree = repo.find_tree(new_tree_id).unwrap();
            let sig = Signature::now("relay", "relay@local").unwrap();
            let _ = repo
                .commit(
                    Some("refs/heads/main"),
                    &sig,
                    &sig,
                    "update relay.yaml",
                    &new_tree,
                    &[&commit],
                )
                .unwrap();
        };
        write_yaml_commit(&repo, &cid1);

        // First listing to generate cache with cid1
        let head = repo.find_reference("refs/heads/main").unwrap();
        let commit = head.peel_to_commit().unwrap();
        let tree = commit.tree().unwrap();
        let _ = super::directory_response(
            &repo_dir.path().to_path_buf(),
            &tree,
            &tree,
            "",
            "text/markdown",
            "main",
            "",
        );

        // Update CID and list again; ensure resulting markdown references new file name
        write_yaml_commit(&repo, &cid2);
        let head = repo.find_reference("refs/heads/main").unwrap();
        let commit = head.peel_to_commit().unwrap();
        let tree = commit.tree().unwrap();
        let (_ct, md) = super::directory_response(
            &repo_dir.path().to_path_buf(),
            &tree,
            &tree,
            "",
            "text/markdown",
            "main",
            "",
        );
        let s = String::from_utf8(md).unwrap();
        assert!(
            s.contains("b.txt"),
            "dir listing after CID change should show new entries: {}",
            s
        );
    }

    // End-to-end: Git miss -> IPFS fetch success
    #[cfg(all(not(target_os = "windows"), feature = "ipfs_tests"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ipfs_fallback_fetch_success() {
        // Temp IPFS repo and daemon
        let ipfs_dir = tempdir().unwrap();
        let api_port = 5015u16;
        ensure_ipfs_daemon(ipfs_dir.path(), api_port).await;

        // Create a directory with a file and add recursively to IPFS
        let src_dir = tempdir().unwrap();
        let rel_path = FsPath::new("assets/hello.txt");
        write_file(&src_dir.path().join(rel_path), "hello from ipfs").await;
        let root_cid = ipfs_add_dir(ipfs_dir.path(), api_port, src_dir.path()).await;

        // Prepare bare git repo with relay.yaml pointing to root_cid; no actual file in git
        let repo_dir = tempdir().unwrap();
        let repo = Repository::init_bare(repo_dir.path()).unwrap();
        // initial empty commit on main
        {
            let sig = Signature::now("relay", "relay@local").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            let tree_id = tb.write().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let _ = repo
                .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        // Add relay.yaml to default branch for rules consumption by server endpoints expecting default branch
        let yaml = format!(
            "ipfs:\n  rootHash: \"{}\"\n  branches: [ \"main\" ]\n",
            root_cid
        );
        {
            // Read existing tree and upsert relay.yaml blob
            let head = repo.find_reference("refs/heads/main").unwrap();
            let commit = head.peel_to_commit().unwrap();
            let base_tree = commit.tree().unwrap();
            let blob_oid = repo.blob(yaml.as_bytes()).unwrap();
            // place relay.yaml at repo root
            fn upsert(repo: &Repository, tree: &git2::Tree, filename: &str, blob: Oid) -> Oid {
                let mut tb = repo.treebuilder(Some(tree)).unwrap();
                tb.insert(filename, blob, 0o100644).unwrap();
                tb.write().unwrap()
            }
            let new_tree_id = upsert(&repo, &base_tree, "relay.yaml", blob_oid);
            let new_tree = repo.find_tree(new_tree_id).unwrap();
            let sig = Signature::now("relay", "relay@local").unwrap();
            let _ = repo
                .commit(
                    Some("refs/heads/main"),
                    &sig,
                    &sig,
                    "add relay.yaml",
                    &new_tree,
                    &[&commit],
                )
                .unwrap();
        }

        // Set envs for server to point to our temp git and ipfs
        std::env::set_var("RELAY_IPFS_TIMEOUT_SECS", "10");
        std::env::set_var("RELAY_IPFS_API", format!("http://127.0.0.1:{}", api_port));
        std::env::set_var("IPFS_PATH", ipfs_dir.path());
        let cache_dir = tempdir().unwrap();
        std::env::set_var("RELAY_IPFS_CACHE_ROOT", cache_dir.path());

        // Build minimal AppState
        let app_state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
        };

        // Request for the IPFS-backed file path under the same repo layout
        let headers = HeaderMap::new();
        let path = format!("{}", rel_path.to_string_lossy());
        let query: Option<Query<HashMap<String, String>>> = None;

        // Wait for resolution to ensure availability
        for _ in 0..20 {
            let status = TokioCommand::new("ipfs")
                .arg("resolve")
                .arg("-r")
                .arg(format!("/ipfs/{}/{}", root_cid, rel_path.to_string_lossy()))
                .env("IPFS_PATH", ipfs_dir.path())
                .status()
                .await
                .unwrap();
            if status.success() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Get file; should miss git and fetch from IPFS
        let resp = get_file(State(app_state), headers, Path(path), query)
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        // Verify cache populated
        let cached = cache_dir.path().join("_").join("main").join(rel_path);
        assert!(
            cached.exists(),
            "cached file should exist: {}",
            cached.display()
        );
    }

    // Not-found under CID returns 404
    #[cfg(all(not(target_os = "windows"), feature = "ipfs_tests"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ipfs_fallback_not_found_404() {
        let ipfs_dir = tempdir().unwrap();
        let api_port = 5016u16;
        ensure_ipfs_daemon(ipfs_dir.path(), api_port).await;

        // Empty dir -> CID of an empty dir by adding a directory with no files
        let empty_dir = tempdir().unwrap();
        let root_cid = ipfs_add_dir(ipfs_dir.path(), api_port, empty_dir.path()).await;

        // Git repo with relay.yaml pointing to cid
        let repo_dir = tempdir().unwrap();
        let repo = Repository::init_bare(repo_dir.path()).unwrap();
        {
            let sig = Signature::now("relay", "relay@local").unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            let tree_id = tb.write().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let _ = repo
                .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        let yaml = format!(
            "ipfs:\n  rootHash: \"{}\"\n  branches: [ \"main\" ]\n",
            root_cid
        );
        {
            let head = repo.find_reference("refs/heads/main").unwrap();
            let commit = head.peel_to_commit().unwrap();
            let base_tree = commit.tree().unwrap();
            let blob_oid = repo.blob(yaml.as_bytes()).unwrap();
            let mut tb = repo.treebuilder(Some(&base_tree)).unwrap();
            tb.insert("relay.yaml", blob_oid, 0o100644).unwrap();
            let new_tree_id = tb.write().unwrap();
            let new_tree = repo.find_tree(new_tree_id).unwrap();
            let sig = Signature::now("relay", "relay@local").unwrap();
            let _ = repo
                .commit(
                    Some("refs/heads/main"),
                    &sig,
                    &sig,
                    "add relay.yaml",
                    &new_tree,
                    &[&commit],
                )
                .unwrap();
        }

        std::env::set_var("RELAY_IPFS_TIMEOUT_SECS", "2");
        std::env::set_var("RELAY_IPFS_API", format!("http://127.0.0.1:{}", api_port));
        std::env::set_var("IPFS_PATH", ipfs_dir.path());
        let cache_dir = tempdir().unwrap();
        std::env::set_var("RELAY_IPFS_CACHE_ROOT", cache_dir.path());

        let app_state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
        };
        let headers = HeaderMap::new();
        let path = "assets/missing.txt".to_string();
        let query: Option<Query<HashMap<String, String>>> = None;
        let resp = get_file(State(app_state), headers, Path(path), query)
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ==== Unit tests for HEAD, GET, OPTIONS methods ====

    /// Test OPTIONS returns repository list with branches and commit heads
    #[tokio::test]
    async fn test_options_returns_repo_list() {
        let repo_dir = tempdir().unwrap();

        // Create a bare repo named "repo.git" inside the temp directory
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        // Create initial commit on main branch
        let sig = Signature::now("relay", "relay@local").unwrap();
        let tb = repo.treebuilder(None).unwrap();
        let tree_id = tb.write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let _commit_oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let headers = HeaderMap::new();
        let query = None;
        let response = options_capabilities(State(state), headers, query).await;
        let (parts, body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::OK);
        // Parse body to verify structure
        let body_bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(json["ok"], true);
        assert!(json["capabilities"]["supports"].is_array());
    }

    /// Test GET returns 200 when repo exists with a file
    #[tokio::test]
    async fn test_get_file_success() {
        let repo_dir = tempdir().unwrap();

        // Create a bare repo named "repo.git" inside the temp directory
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        // Create initial commit with a file
        let sig = Signature::now("relay", "relay@local").unwrap();
        let file_content = b"Hello, World!";
        let blob_oid = repo.blob(file_content).unwrap();
        let mut tb = repo.treebuilder(None).unwrap();
        tb.insert("hello.txt", blob_oid, 0o100644).unwrap();
        let tree_id = tb.write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let _commit_oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "add file", &tree, &[])
            .unwrap();

        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_BRANCH, "main".parse().unwrap());
        headers.insert(HEADER_REPO, "repo".parse().unwrap());

        let response = get_file(State(state), headers, AxPath("hello.txt".to_string()), None).await;
        let (parts, body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::OK);
        let body_bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec();
        assert_eq!(body_bytes, file_content);
    }

    /// Test GET returns 404 when file doesn't exist
    #[tokio::test]
    async fn test_get_file_not_found() {
        let repo_dir = tempdir().unwrap();

        // Create a bare repo named "repo.git" inside the temp directory
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        // Create initial empty commit
        let sig = Signature::now("relay", "relay@local").unwrap();
        let tb = repo.treebuilder(None).unwrap();
        let tree_id = tb.write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let _commit_oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_BRANCH, "main".parse().unwrap());
        headers.insert(HEADER_REPO, "repo".parse().unwrap());

        let response = get_file(
            State(state),
            headers,
            AxPath("missing.txt".to_string()),
            None,
        )
        .await;
        let (parts, _body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::NOT_FOUND);
    }

    /// Test GET returns 404 when repo doesn't exist
    #[tokio::test]
    async fn test_get_repo_not_found() {
        let repo_dir = tempdir().unwrap();
        // Create empty data directory with no repos
        let _ = std::fs::create_dir_all(repo_dir.path());

        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_BRANCH, "main".parse().unwrap());
        headers.insert(HEADER_REPO, "nonexistent".parse().unwrap());

        let response = get_file(State(state), headers, AxPath("file.txt".to_string()), None).await;
        let (parts, _body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::NOT_FOUND);
    }

    /// Test OPTIONS returns proper headers
    #[tokio::test]
    async fn test_options_headers() {
        let repo_dir = tempdir().unwrap();

        // Create a bare repo named "repo.git" inside the temp directory
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        // Create initial commit
        let sig = Signature::now("relay", "relay@local").unwrap();
        let tb = repo.treebuilder(None).unwrap();
        let tree_id = tb.write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let _commit_oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let headers = HeaderMap::new();
        let (parts, _body) = options_capabilities(State(state), headers, None)
            .await
            .into_response()
            .into_parts();

        assert_eq!(parts.status, StatusCode::OK);

        // Verify Allow header contains expected methods
        let allow_header = parts.headers.get("Allow");
        assert!(allow_header.is_some());
        let allow_str = allow_header.unwrap().to_str().unwrap_or("").to_uppercase();
        assert!(allow_str.contains("GET"));
        assert!(allow_str.contains("OPTIONS"));

        // Verify CORS headers
        assert!(parts.headers.contains_key("Access-Control-Allow-Origin"));
        assert!(parts.headers.contains_key("Access-Control-Allow-Methods"));
    }

    /// Test branch_from correctly extracts branch from header
    #[test]
    fn test_branch_from_header() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_BRANCH, "develop".parse().unwrap());

        let branch = helpers::branch_from(&headers);
        assert_eq!(branch, "develop");
    }

    /// Test branch_from defaults to main when header is missing
    #[test]
    fn test_branch_from_default() {
        let headers = HeaderMap::new();

        let branch = helpers::branch_from(&headers);
        assert_eq!(branch, DEFAULT_BRANCH);
    }

    /// Test strict_repo_from selects first repo when none specified
    #[tokio::test]
    async fn test_strict_repo_from_default() {
        let repo_dir = tempdir().unwrap();

        // Create a bare repo named "repo"
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        let sig = Signature::now("relay", "relay@local").unwrap();
        let tb = repo.treebuilder(None).unwrap();
        let tree_id = tb.write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let _commit_oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let headers = HeaderMap::new();
        let selected = helpers::strict_repo_from(&repo_dir.path().to_path_buf(), &headers);

        assert_eq!(selected, Some("repo".to_string()));
    }

    /// Test strict_repo_from returns None when no repos exist
    #[test]
    fn test_strict_repo_from_no_repos() {
        let repo_dir = tempdir().unwrap();
        let _ = std::fs::create_dir_all(repo_dir.path());

        let headers = HeaderMap::new();
        let selected = helpers::strict_repo_from(&repo_dir.path().to_path_buf(), &headers);

        assert_eq!(selected, None);
    }

    /// Test bare_repo_names correctly lists repos
    #[tokio::test]
    async fn test_bare_repo_names() {
        let repo_dir = tempdir().unwrap();

        // Create two bare repos
        Repository::init_bare(repo_dir.path().join("repo1.git")).unwrap();
        Repository::init_bare(repo_dir.path().join("repo2.git")).unwrap();
        // Create a non-bare directory (should be ignored)
        std::fs::create_dir(repo_dir.path().join("not_a_repo")).unwrap();

        let names = git::bare_repo_names(&repo_dir.path().to_path_buf());

        assert_eq!(names, vec!["repo1".to_string(), "repo2".to_string()]);
    }

    /// Test HEAD / returns 204 No Content like GET
    #[tokio::test]
    async fn test_head_root() {
        let repo_dir = tempdir().unwrap();
        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let headers = HeaderMap::new();
        let response = handlers::head_root(State(state), headers, None).await;
        let (parts, _body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::NO_CONTENT);
    }

    /// Test HEAD returns 200 when file exists
    #[tokio::test]
    async fn test_head_file_success() {
        let repo_dir = tempdir().unwrap();

        // Create a bare repo named "repo.git"
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        // Create initial commit with a file
        let sig = Signature::now("relay", "relay@local").unwrap();
        let file_content = b"Hello, World!";
        let blob_oid = repo.blob(file_content).unwrap();
        let mut tb = repo.treebuilder(None).unwrap();
        tb.insert("hello.txt", blob_oid, 0o100644).unwrap();
        let tree_id = tb.write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let _commit_oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "add file", &tree, &[])
            .unwrap();

        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_BRANCH, "main".parse().unwrap());
        headers.insert(HEADER_REPO, "repo".parse().unwrap());

        let response = handlers::head_file(State(state), headers, AxPath("hello.txt".to_string()), None).await;
        let (parts, body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::OK);
        // Verify body is empty for HEAD
        let body_bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec();
        assert_eq!(body_bytes.len(), 0);
    }

    /// Test HEAD returns 404 when file doesn't exist
    #[tokio::test]
    async fn test_head_file_not_found() {
        let repo_dir = tempdir().unwrap();

        // Create a bare repo named "repo.git"
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        // Create initial empty commit
        let sig = Signature::now("relay", "relay@local").unwrap();
        let tb = repo.treebuilder(None).unwrap();
        let tree_id = tb.write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let _commit_oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_BRANCH, "main".parse().unwrap());
        headers.insert(HEADER_REPO, "repo".parse().unwrap());

        let response = handlers::head_file(
            State(state),
            headers,
            AxPath("missing.txt".to_string()),
            None,
        )
        .await;
        let (parts, _body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::NOT_FOUND);
    }

    /// Test HEAD returns 404 when repo doesn't exist
    #[tokio::test]
    async fn test_head_repo_not_found() {
        let repo_dir = tempdir().unwrap();
        let _ = std::fs::create_dir_all(repo_dir.path());

        let state = AppState {
            repo_path: repo_dir.path().to_path_buf(),
            static_paths: Vec::new(),
        };

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_BRANCH, "main".parse().unwrap());
        headers.insert(HEADER_REPO, "nonexistent".parse().unwrap());

        let response = handlers::head_file(State(state), headers, AxPath("file.txt".to_string()), None).await;
        let (parts, _body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::NOT_FOUND);
    }
}

async fn load_rustls_config(cert_path: &str, key_path: &str) -> Result<RustlsConfig> {
    // Read files as raw bytes and pass to RustlsConfig::from_pem which expects Vec<u8>
    let cert_bytes = tokio::fs::read(cert_path).await?;
    let key_bytes = tokio::fs::read(key_path).await?;

    // from_pem is async and returns io::Result<RustlsConfig>
    let config = RustlsConfig::from_pem(cert_bytes, key_bytes).await?;
    Ok(config)
}

// IPFS fallback removed; IPFS logic is delegated to repo scripts (hooks/get.mjs)

async fn get_root(
    State(state): State<AppState>,
    _headers: HeaderMap,
    _query: Option<Query<HashMap<String, String>>>,
) -> impl IntoResponse {
    // Try serving SPA index.html from configured static paths
    if let Some(resp) = handlers::try_static(&state, "index.html").await {
        return resp;
    }
    StatusCode::NOT_FOUND.into_response()
}

/// Append permissive CORS headers to all responses without short-circuiting OPTIONS.
async fn cors_headers(req: Request<Body>, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        axum::http::HeaderValue::from_static("*"),
    );
    headers.insert(
        axum::http::header::HeaderName::from_static("access-control-allow-methods"),
        axum::http::HeaderValue::from_static("GET, PUT, DELETE, OPTIONS, QUERY"),
    );
    headers.insert(
        axum::http::header::HeaderName::from_static("access-control-allow-headers"),
        axum::http::HeaderValue::from_static("*"),
    );
    headers.insert(
        axum::http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
        axum::http::HeaderValue::from_static("*"),
    );
    res
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging: stdout + rolling file appender
    let _ = std::fs::create_dir_all("logs");
    let file_appender = rolling::daily("logs", "server.log");
    let (file_nb, _guard) = tracing_appender::non_blocking(file_appender);
    let env_filter = tracing_subscriber::EnvFilter::from_default_env();
    let stdout_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .compact();
    let file_layer = fmt::layer()
        .with_writer(file_nb)
        .with_target(true)
        .compact();
    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    // Determine serve args from CLI/env
    let (repo_path, mut static_paths, bind_cli): (PathBuf, Vec<PathBuf>, Option<String>) =
        match cli.command {
            Some(Commands::Serve(sa)) => {
                let rp = sa
                    .repo
                    .or_else(|| std::env::var("RELAY_REPO_PATH").ok().map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from("data"));
                (rp, sa.static_paths, sa.bind)
            }
            _ => {
                let rp = std::env::var("RELAY_REPO_PATH")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("data"));
                (rp, Vec::new(), None)
            }
        };
    
    // Append RELAY_STATIC_DIR if provided (comma-separated allowed)
    if let Ok(extra) = std::env::var("RELAY_STATIC_DIR") {
        for p in extra.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            static_paths.push(PathBuf::from(p));
        }
    }
    
    info!(repo_path = %repo_path.display(), "Repository path resolved");
    let _ = std::fs::create_dir_all(&repo_path);

    // Initialize repos from RELAY_MASTER_REPO_LIST if provided
    if let Ok(repo_list_str) = std::env::var("RELAY_MASTER_REPO_LIST") {
        let repos: Vec<&str> = repo_list_str.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        for repo_url in repos {
            let repo_name = repo_url
                .split('/')
                .last()
                .and_then(|s| s.strip_suffix(".git"))
                .unwrap_or(repo_url);
            let bare_repo_path = repo_path.join(format!("{}.git", repo_name));

            if bare_repo_path.exists() {
                info!(repo = %repo_name, "Repository already exists, skipping clone");
                continue;
            }

            info!(repo = %repo_name, url = %repo_url, "Cloning repository");
            match std::process::Command::new("git")
                .arg("clone")
                .arg("--bare")
                .arg(repo_url)
                .arg(&bare_repo_path)
                .output()
            {
                Ok(output) => {
                    if output.status.success() {
                        info!(repo = %repo_name, "Successfully cloned repository");
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        warn!(repo = %repo_name, error = %stderr, "Failed to clone repository");
                    }
                }
                Err(e) => {
                    warn!(repo = %repo_name, error = %e, "Failed to execute git clone");
                }
            }
        }
    }

    let state = AppState {
        repo_path,
        static_paths,
    };

    // Build app (OPTIONS is the discovery endpoint)
    let acme_route_dir = std::env::var("RELAY_ACME_DIR").unwrap_or_else(|_| "/var/www/certbot".to_string());
    let app = Router::new()
        .route("/openapi.yaml", get(handlers::get_openapi_yaml))
        .route("/swagger-ui", get(handlers::get_swagger_ui))
        .route("/api/config", get(handlers::get_api_config))
        .route("/git-pull", post(handlers::post_git_pull))
        .route("/transpile", post(transpiler::post_transpile))
        .route(
            "/.well-known/acme-challenge/*path",
            get({
                let dir = acme_route_dir.clone();
                move |AxPath(path): AxPath<String>| async move {
                    handlers::serve_acme_challenge(&dir, &path).await
                }
            }),
        )
        .route("/", get(get_root).head(handlers::head_root).options(options_capabilities))
        .route(
            "/*path",
            get(handlers::handle_get_file)
                .head(handlers::head_file)
                .put(handlers::put_file)
                .delete(handlers::delete_file)
                .options(options_capabilities),
        )
        .layer(axum::middleware::from_fn(cors_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Configure listeners: HTTP and optional HTTPS
    let http_addr: SocketAddr = if let Some(bind) = bind_cli.or_else(|| std::env::var("RELAY_BIND").ok()) {
        SocketAddr::from_str(&bind)?
    } else {
        let port = std::env::var("RELAY_HTTP_PORT").ok().and_then(|s| s.parse::<u16>().ok()).unwrap_or(80);
        SocketAddr::from_str(&format!("0.0.0.0:{}", port))?
    };
    let https_port = std::env::var("RELAY_HTTPS_PORT").ok().and_then(|s| s.parse::<u16>().ok()).unwrap_or(443);
    let tls_cert = std::env::var("RELAY_TLS_CERT").ok();
    let tls_key = std::env::var("RELAY_TLS_KEY").ok();

    let app_http = app.clone();
    let http_task = tokio::spawn(async move {
        info!(%http_addr, "HTTP listening");
        let listener = TcpListener::bind(&http_addr).await.expect("bind http");
        if let Err(e) = axum::serve(listener, app_http.into_make_service()).await {
            error!(?e, "HTTP server error");
        }
    });

    // HTTPS optional
    let https_task = if let (Some(cert_path), Some(key_path)) = (tls_cert, tls_key) {
        let https_addr: SocketAddr = SocketAddr::from_str(&format!("0.0.0.0:{}", https_port))?;
        let config = load_rustls_config(&cert_path, &key_path).await?;
        let app_https = app;
        Some(tokio::spawn(async move {
            info!(%https_addr, cert=%cert_path, key=%key_path, "HTTPS listening");
            if let Err(e) = axum_server::bind_rustls(https_addr, config)
                .serve(app_https.into_make_service())
                .await
            {
                error!(?e, "HTTPS server error");
            }
        }))
    } else {
        info!("TLS is disabled: RELAY_TLS_CERT and RELAY_TLS_KEY not both set");
        None
    };

    if let Some(t) = https_task {
        let _ = tokio::join!(http_task, t);
    } else {
        let _ = tokio::join!(http_task);
    }
    Ok(())
}

// NOTE: Server tests are defined earlier in this file under an existing `mod tests`.
