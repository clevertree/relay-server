use std::path::PathBuf;
use std::net::SocketAddr;
use std::str::FromStr;
use crate::types::AppState;
use crate::cli::{Cli, Commands};
use axum_server::tls_rustls::RustlsConfig;
use anyhow::Result;

pub struct Config {
    pub state: AppState,
    pub http_addr: SocketAddr,
    pub https_port: u16,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub acme_dir: String,
}

impl Config {
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let (repo_path, mut static_paths, bind_cli): (PathBuf, Vec<PathBuf>, Option<String>) =
            match &cli.command {
                Some(Commands::Serve(sa)) => {
                    let rp = sa
                        .repo
                        .clone()
                        .or_else(|| std::env::var("RELAY_REPO_PATH").ok().map(PathBuf::from))
                        .unwrap_or_else(|| PathBuf::from("data"));
                    (rp, sa.static_paths.clone(), sa.bind.clone())
                }
                _ => {
                    let rp = std::env::var("RELAY_REPO_PATH")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("data"));
                    (rp, Vec::new(), None)
                }
            };

        if let Ok(extra) = std::env::var("RELAY_STATIC_DIR") {
            for p in extra.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                static_paths.push(PathBuf::from(p));
            }
        }

        let http_addr: SocketAddr = if let Some(bind) = bind_cli.or_else(|| std::env::var("RELAY_BIND").ok()) {
            SocketAddr::from_str(&bind)?
        } else {
            let port = std::env::var("RELAY_HTTP_PORT").ok().and_then(|s| s.parse::<u16>().ok()).unwrap_or(80);
            SocketAddr::from_str(&format!("0.0.0.0:{}", port))?
        };

        let https_port = std::env::var("RELAY_HTTPS_PORT").ok().and_then(|s| s.parse::<u16>().ok()).unwrap_or(443);
        let tls_cert = std::env::var("RELAY_TLS_CERT").ok();
        let tls_key = std::env::var("RELAY_TLS_KEY").ok();
        let acme_dir = std::env::var("RELAY_ACME_DIR").unwrap_or_else(|_| "/var/www/certbot".to_string());

        Ok(Config {
            state: AppState {
                repo_path,
                static_paths,
            },
            http_addr,
            https_port,
            tls_cert,
            tls_key,
            acme_dir,
        })
    }

    pub fn initialize_repos(&self) {
        let repo_path = &self.state.repo_path;
        let _ = std::fs::create_dir_all(repo_path);
        // Repo cloning and updates are handled by the docker-entrypoint.sh
        // or external processes to keep the server lightweight.
    }
}

pub async fn load_rustls_config(cert_path: &str, key_path: &str) -> Result<RustlsConfig> {
    let cert_bytes = tokio::fs::read(cert_path).await?;
    let key_bytes = tokio::fs::read(key_path).await?;
    let config = RustlsConfig::from_pem(cert_bytes, key_bytes).await?;
    Ok(config)
}
