#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature};
    use std::collections::HashMap;
    use std::path::Path as FsPath;
    use tempfile::tempdir;
    use axum::{
        extract::{Path as AxPath, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        Json,
    };
    use crate::{handlers, git, helpers, types::*};

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
        let response = handlers::options_capabilities(State(state), headers, query).await;
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

        let response = handlers::handle_get_file(State(state), headers, AxPath("hello.txt".to_string()), None).await;
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

        let response = handlers::handle_get_file(
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

        let response = handlers::handle_get_file(State(state), headers, AxPath("file.txt".to_string()), None).await;
        let (parts, _body) = response.into_response().into_parts();

        assert_eq!(parts.status, StatusCode::NOT_FOUND);
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

    /// Test QUERY method returns results from the mock database
    #[tokio::test]
    async fn test_query_method_success() {
        let repo_dir = tempdir().unwrap();
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        // Create initial commit with .relay.yaml
        let sig = Signature::now("relay", "relay@local").unwrap();
        let config_content = r#"
server:
  hooks:
    index: 
      path: hooks/server/index.mjs
"#;
        let config_oid = repo.blob(config_content.as_bytes()).unwrap();
        
        // Also create a dummy index.mjs so it exists during extraction
        let index_js_content = "process.exit(0);";
        let index_js_oid = repo.blob(index_js_content.as_bytes()).unwrap();

        let mut tb_hooks = repo.treebuilder(None).unwrap();
        let mut tb_server = repo.treebuilder(None).unwrap();
        let mut tb_root = repo.treebuilder(None).unwrap();

        tb_server.insert("index.mjs", index_js_oid, 0o100644).unwrap();
        let server_tree_id = tb_server.write().unwrap();

        tb_hooks.insert("server", server_tree_id, 0o040000).unwrap();
        let hooks_tree_id = tb_hooks.insert("server", server_tree_id, 0o040000).unwrap(); // Wait, treebuilder is different?
        // Let's use a simpler way
        
        let mut tb = repo.treebuilder(None).unwrap();
        tb.insert(".relay.yaml", config_oid, 0o100644).unwrap();
        // tb.insert("hooks/server/index.mjs", index_js_oid, 0o100644).unwrap(); // This failed because of /
        
        // Proper way with git2 treebuilder
        let mut tb_server = repo.treebuilder(None).unwrap();
        tb_server.insert("index.mjs", index_js_oid, 0o100644).unwrap();
        let server_tid = tb_server.write().unwrap();
        
        let mut tb_hooks = repo.treebuilder(None).unwrap();
        tb_hooks.insert("server", server_tid, 0o040000).unwrap();
        let hooks_tid = tb_hooks.write().unwrap();
        
        tb.insert("hooks", hooks_tid, 0o040000).unwrap();
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

        let query_body = serde_json::json!({
            "collection": "index",
            "query": { "title": "Test Item" }
        });

        let response = handlers::handle_query(
            State(state),
            headers,
            AxPath("query".to_string()),
            None,
            Some(Json(query_body))
        ).await;

        let (parts, body) = response.into_response().into_parts();
        assert_eq!(parts.status, StatusCode::OK);

        let body_bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        
        let results = json["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Test Item");
    }

    #[tokio::test]
    async fn test_query_method_path_success() {
        let repo_dir = tempdir().unwrap();
        let repo_path = repo_dir.path().join("repo.git");
        let repo = Repository::init_bare(&repo_path).unwrap();

        // Create initial commit with some data in the db
        // For this test, index.mjs just returns search term as results for simplicity
        let sig = Signature::now("relay", "relay@local").unwrap();
        let config_content = r#"
server:
  hooks:
    index: 
      path: hooks/server/index.mjs
"#;
        let config_oid = repo.blob(config_content.as_bytes()).unwrap();
        
        let index_js_content = r#"
const fs = require('fs');
const ctx = JSON.parse(fs.readFileSync(0, 'utf8'));
// Just echo the search query back as a result
const query = ctx.query;
const results = [{ title: typeof query === 'string' ? query : query.title }];
console.log(JSON.stringify(results));
process.exit(0);
"#;
        let index_js_oid = repo.blob(index_js_content.as_bytes()).unwrap();

        let mut tb_server = repo.treebuilder(None).unwrap();
        tb_server.insert("index.mjs", index_js_oid, 0o100644).unwrap();
        let server_tid = tb_server.write().unwrap();
        
        let mut tb_hooks = repo.treebuilder(None).unwrap();
        tb_hooks.insert("server", server_tid, 0o040000).unwrap();
        let hooks_tid = tb_hooks.write().unwrap();
        
        let mut tb = repo.treebuilder(None).unwrap();
        tb.insert(".relay.yaml", config_oid, 0o100644).unwrap();
        tb.insert("hooks", hooks_tid, 0o040000).unwrap();
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

        // Call with path "Test" instead of "query"
        let response = handlers::handle_query(
            State(state),
            headers,
            AxPath("Test".to_string()),
            None,
            None
        ).await;

        let (parts, body) = response.into_response().into_parts();
        assert_eq!(parts.status, StatusCode::OK);

        let body_bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        
        let results = json["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Test Item");
    }
}
