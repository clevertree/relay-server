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
