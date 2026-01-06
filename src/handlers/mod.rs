pub mod file;
pub mod general;
pub mod head;
pub mod helpers;
pub mod write;
pub mod query;

pub use file::{handle_get_file, try_static};
pub use general::{
    get_api_config, get_openapi_yaml, get_root, get_swagger_ui, options_capabilities,
    post_git_pull, post_github_hook, serve_acme_challenge,
};
pub use head::{head_file, head_root};
pub use write::{delete_file, put_file};
pub use query::handle_query;
