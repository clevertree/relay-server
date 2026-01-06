use std::path::PathBuf;
use std::process::Command;
use tracing::{error, debug};

pub struct HookContext {
    pub repo_path: PathBuf,
    pub old_commit: String,
    pub new_commit: String,
    pub refname: String,
    pub branch: String,
    pub is_verified: bool,
    /// Pre-extracted file contents from the repository (maps path to base64 content)
    pub files: std::collections::HashMap<String, String>,
}

pub fn execute_repo_hook(
    ctx: &HookContext,
    hook_name: &str,
) -> anyhow::Result<bool> {
    // Read .relay.yaml from the NEW_COMMIT using git CLI (quarantine-aware)
    let config_output = Command::new("git")
        .arg("-C").arg(&ctx.repo_path)
        .arg("show")
        .arg(format!("{}:.relay.yaml", ctx.new_commit))
        .output()?;

    let config: crate::types::RelayConfig = if config_output.status.success() {
        let content = String::from_utf8_lossy(&config_output.stdout);
        serde_yaml::from_str(&content)?
    } else {
        debug!("No .relay.yaml found in commit {}, skipping hooks", ctx.new_commit);
        return Ok(true);
    };

    // Find the hook path in config
    let hook_path = match hook_name {
        "pre-commit" | "pre-receive" | "post-receive" | "index" => {
            config.server.as_ref()
                .and_then(|s| s.hooks.as_ref())
                .and_then(|h| h.get(hook_name))
                .map(|p| p.path.as_str())
        },
        _ => None,
    };

    let hook_path = match hook_path {
        Some(p) => p,
        None => {
            debug!("Hook '{}' not configured in .relay.yaml", hook_name);
            return Ok(true); 
        }
    };

    // Create a temporary directory for the whole hook environment
    let tmp_dir = tempfile::Builder::new()
        .prefix(&format!("relay-hook-env-{}-{}-", hook_name, &ctx.new_commit[..8]))
        .tempdir()?;
    let tmp_dir_path = tmp_dir.path().to_path_buf();

    // Extract the hook's directory from the tree using git archive (quarantine-aware)
    let hook_dir_path = std::path::Path::new(hook_path).parent().unwrap_or(std::path::Path::new(""));
    let archive_output = Command::new("git")
        .arg("-C").arg(&ctx.repo_path)
        .arg("archive")
        .arg("--format=tar")
        .arg(&ctx.new_commit)
        .arg(hook_dir_path)
        .output()?;

    if archive_output.status.success() {
        let mut child = Command::new("tar")
            .arg("-x")
            .arg("-C").arg(&tmp_dir_path)
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(&archive_output.stdout)?;
        }
        child.wait()?;
    }

    // The script might be nested in tmp_dir because git archive preserves paths
    // but it preserves them relative to the archive root (which is the repo root)
    // So the script will be at tmp_dir/hook_path
    let tmp_script_path = tmp_dir_path.join(hook_path);
    if !tmp_script_path.exists() {
        error!("Hook script '{}' not found in commit after extraction", hook_path);
        return Ok(true);
    }

    // Read script content to strip shebang if present
    let content = std::fs::read(&tmp_script_path)?;
    let content_to_write = if content.starts_with(b"#!") {
        if let Some(newline_pos) = content.iter().position(|&b| b == b'\n') {
            &content[newline_pos + 1..]
        } else {
            &content[..]
        }
    } else {
        &content[..]
    };
    std::fs::write(&tmp_script_path, content_to_write)?;

    // Write RelayHost.mjs to the tmp_dir
    let relay_host_source = if cfg!(test) {
        include_str!("RelayHostMod.mjs")
    } else {
        include_str!("RelayHost.mjs")
    };
    let relay_host_path = tmp_dir_path.join("RelayHost.mjs");
    std::fs::write(&relay_host_path, relay_host_source)?;

    let node_bin = match Command::new("which").arg("node").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => "node".to_string(),
    };

    let mut cmd = Command::new(node_bin);
    cmd.arg("RelayHost.mjs")
        .arg(hook_path) // Path relative to current_dir which is tmp_dir
        .current_dir(&tmp_dir_path)
        .env("GIT_DIR", &ctx.repo_path)
        .env("OLD_COMMIT", &ctx.old_commit)
        .env("NEW_COMMIT", &ctx.new_commit)
        .env("REFNAME", &ctx.refname)
        .env("BRANCH", &ctx.branch);

    // Provide the extracted files and context as JSON via stdin
    let context_json = serde_json::json!({
        "old_commit": ctx.old_commit,
        "new_commit": ctx.new_commit,
        "refname": ctx.refname,
        "branch": ctx.branch,
        "files": ctx.files,
        "repo_path": ctx.repo_path,
        "is_verified": ctx.is_verified
    });

    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(context_json.to_string().as_bytes())?;
    }

    let status = child.wait()?;

    Ok(status.success())
}
