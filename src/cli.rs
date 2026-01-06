use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "relay-server",
    version,
    about = "Relay Server and CLI utilities",
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run the HTTP server
    Serve(ServeArgs),
    /// Query a repository branch
    Query(QueryArgs),
}

#[derive(Args, Debug)]
pub struct QueryArgs {
    /// Repository name
    pub repo: String,
    /// Branch name (default: main)
    #[arg(short, long, default_value = "main")]
    pub branch: String,
    /// Search query string
    #[arg(short, long)]
    pub query: Option<String>,
    /// Collection name (default: index)
    #[arg(short, long, default_value = "index")]
    pub collection: String,
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Bare Git repository path
    #[arg(long)]
    pub repo: Option<PathBuf>,
    /// Additional static directory to serve files from (may be repeated)
    #[arg(long = "static", value_name = "DIR")]
    pub static_paths: Vec<PathBuf>,
    /// Bind address (host:port) for HTTP (overrides RELAY_HTTP_PORT if set)
    #[arg(long)]
    pub bind: Option<String>,
}
