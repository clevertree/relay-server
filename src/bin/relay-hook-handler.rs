use std::io::{self, Read};
use std::path::PathBuf;
use relay_server::git::{execute_repo_hook, HookContext};
use tracing::{info, error, debug, Level};
use tracing_subscriber::FmtSubscriber;

fn main() -> anyhow::Result<()> {
    // Force write to stderr so we see it in git daemon logs
    eprintln!("[relay-hook-handler] Hook started");
    
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();

    let args: Vec<String> = std::env::args().collect();
    let hook_path = args.get(0).map(PathBuf::from).unwrap_or_default();
    let hook_name = hook_path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let git_dir = std::env::var("GIT_DIR").map(PathBuf::from)
        .or_else(|_| std::env::current_dir())?;

    // Git hooks like pre-receive and post-receive get input from stdin
    // Format: <old-value> SP <new-value> SP <ref-name> LF
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let absolute_git_dir = if git_dir.is_absolute() {
        git_dir.clone()
    } else {
        std::env::current_dir()?.join(&git_dir)
    };

    let repo_name = absolute_git_dir.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown.git");

    for line in input.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let old_commit = parts[0].to_string();
        let new_commit = parts[1].to_string();
        let refname = parts[2].to_string();
        let branch = refname.strip_prefix("refs/heads/").unwrap_or(&refname).to_string();

        let mut ctx = HookContext {
            repo_path: absolute_git_dir.clone(),
            old_commit: old_commit.clone(),
            new_commit: new_commit.clone(),
            refname: refname.clone(),
            branch: branch.clone(),
            is_verified: false,
            files: std::collections::HashMap::new(),
        };

        // Extract changed files using git CLI (quarantine-aware)
        if let Ok(fs) = extract_changed_files(&ctx.repo_path, &old_commit, &new_commit) {
            ctx.files = fs;
        }

        // Enforce branch rules for pre-receive BEFORE execute_repo_hook
        if hook_name == "pre-receive" {
            match enforce_branch_rules(&ctx) {
                Ok(_) => ctx.is_verified = true,
                Err(e) => {
                    error!("Branch rule violation: {}", e);
                    eprintln!("[relay-hook-handler] Branch rule violation: {}", e);
                    std::process::exit(1);
                }
            }
        }

        if !execute_repo_hook(&ctx, hook_name)? {
            error!("Hook {} failed for {}", hook_name, ctx.refname);
            std::process::exit(1);
        }

        // Special handling for post-receive (Auto-Push)
        if hook_name == "post-receive" {
            if let Err(e) = handle_auto_push(&ctx) {
                error!("Auto-push failed: {}", e);
            }
        }
    }

    Ok(())
}

fn enforce_branch_rules(ctx: &HookContext) -> anyhow::Result<()> {
    let repo = git2::Repository::open_bare(&ctx.repo_path)?;
    // Read from the new commit being pushed, as the branch ref hasn't moved yet
    let git_config = match relay_server::git::read_git_config(&repo, &ctx.new_commit) {
        Some(c) => c,
        None => return Ok(()),
    };

    let rules_config = match git_config.branch_rules {
        Some(r) => r,
        None => return Ok(()),
    };

    // Find applicable rule
    let mut active_rule = rules_config.default.clone();
    if let Some(branches) = rules_config.branches {
        for b in branches {
            if b.name == ctx.branch {
                active_rule = Some(b.rule);
                break;
            }
        }
    }

    let rule = match active_rule {
        Some(r) => r,
        None => return Ok(()),
    };

    // Check requireSigned
    if rule.require_signed.unwrap_or(false) && !rule.allow_unsigned.unwrap_or(false) {
        let verify_out = std::process::Command::new("git")
            .arg("-C").arg(&ctx.repo_path)
            .arg("verify-commit")
            .arg(&ctx.new_commit)
            .output()?;

        if !verify_out.status.success() {
            return Err(anyhow::anyhow!("Commit {} must be signed and verified", ctx.new_commit));
        }
    }

    Ok(())
}

fn handle_auto_push(ctx: &HookContext) -> anyhow::Result<()> {
    // Avoid infinite loops if we are already in a sync operation
    if std::env::var("RELAY_SYNC_IN_PROGRESS").is_ok() {
        debug!("Sync already in progress, skipping auto-push for {}", ctx.branch);
        return Ok(());
    }

    let repo = git2::Repository::open_bare(&ctx.repo_path)?;
    let config = match relay_server::git::read_relay_config(&repo, &ctx.new_commit) {
        Some(c) => c,
        None => return Ok(()),
    };

    let git_config = match config.git {
        Some(c) => c,
        None => return Ok(()),
    };

    let auto_push = match git_config.auto_push {
        Some(ap) => ap,
        None => return Ok(()),
    };

    if !auto_push.branches.contains(&ctx.branch) && !auto_push.branches.contains(&"*".to_string()) {
        return Ok(());
    }

    info!("Starting auto-push for branch {} to {} peers", ctx.branch, auto_push.origin_list.len());

    let repo_name = ctx.repo_path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown.git");

    for origin in auto_push.origin_list {
        let push_url = if origin.contains("://") || origin.contains("@") {
            origin.clone()
        } else {
            format!("git://{}/{}", origin, repo_name)
        };

        info!("Pushing {} to {}", ctx.branch, push_url);
        eprintln!("[relay-hook-handler] Pushing {} to {}", ctx.branch, push_url);
        
        // Construct git push command
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(&ctx.repo_path)
           .arg("push")
           .arg("--force") // Use force for peer sync
           .arg(&push_url)
           .arg(format!("{}:{}", ctx.branch, ctx.branch))
           .env("RELAY_SYNC_IN_PROGRESS", "1");

        match cmd.output() {
            Ok(output) => {
                if !output.status.success() {
                    error!("Failed to push to {}: {}", origin, String::from_utf8_lossy(&output.stderr));
                } else {
                    info!("Successfully pushed to {}", origin);
                }
            }
            Err(e) => {
                error!("Error executing git push to {}: {}", origin, e);
            }
        }
    }

    Ok(())
}

fn extract_changed_files(repo_path: &std::path::Path, old_rev: &str, new_rev: &str) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut files = std::collections::HashMap::new();
    
    // Get list of changed files
    let output = std::process::Command::new("git")
        .arg("-C").arg(repo_path)
        .arg("diff-tree")
        .arg("-r")
        .arg("--no-commit-id")
        .arg("--name-only")
        .arg(old_rev)
        .arg(new_rev)
        .output()?;
        
    if !output.status.success() {
        return Ok(files);
    }
    
    let paths = String::from_utf8_lossy(&output.stdout);
    for path in paths.lines().filter(|l| !l.is_empty()) {
        // Extract content for each path
        let content_out = std::process::Command::new("git")
            .arg("-C").arg(repo_path)
            .arg("show")
            .arg(format!("{}:{}", new_rev, path))
            .output()?;
            
        if content_out.status.success() {
            files.insert(path.to_string(), base64::encode(&content_out.stdout));
        }
    }
    
    Ok(files)
}
