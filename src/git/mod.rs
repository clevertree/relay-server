pub mod repo;
pub mod resolve;
pub mod hooks;
pub mod indexing;
pub mod query;

#[cfg(test)]
mod tests;

pub use repo::{
    bare_repo_names, get_branch_commit_info, list_branches, open_repo, read_file_from_repo,
    read_relay_config, read_git_config,
};
pub use resolve::git_resolve_and_respond;
pub use hooks::{execute_repo_hook, HookContext};
pub use indexing::ensure_indexed;
