use std::net::SocketAddr;
use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path as AxPath, State},
    http::{header, Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_appender::rolling;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

use relay_server::{
    cli::Cli,
    config::{self, Config},
    handlers, transpiler, AppState,
};

/// Axum's `MethodFilter` does not support the custom `QUERY` verb; unhandled methods hit fallback.
async fn relay_path_fallback(
    State(state): State<AppState>,
    req: Request<Body>,
) -> axum::response::Response {
    if req.method().as_str() != "QUERY" {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET, HEAD, PUT, DELETE, OPTIONS, QUERY")],
            "Method Not Allowed",
        )
            .into_response();
    }
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().trim_start_matches('/').to_string();
    let headers = parts.headers;
    let json = match axum::body::to_bytes(body, 2 * 1024 * 1024).await {
        Ok(b) if b.is_empty() => None,
        Ok(b) => serde_json::from_slice::<serde_json::Value>(&b)
            .ok()
            .map(Json),
        Err(_) => None,
    };
    handlers::handle_query(State(state), headers, AxPath(path), None, json)
        .await
        .into_response()
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

    let config = Config::from_cli(&cli)?;
    config.initialize_repos();

    if let Some(relay_server::cli::Commands::Query(args)) = cli.command {
        let query_val = args.query.map(serde_json::Value::String);
        match relay_server::git::query::execute_query(
            &config.state.repo_path,
            &args.repo,
            &args.branch,
            query_val,
            &args.collection,
        ) {
            Ok(results) => {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "results": results }))?);
                return Ok(());
            }
            Err(e) => {
                error!("Query failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    info!(repo_path = %config.state.repo_path.display(), "Repository path resolved");

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(Any);

    let app = Router::new()
        .route("/openapi.yaml", get(handlers::get_openapi_yaml))
        .route("/swagger-ui", get(handlers::get_swagger_ui))
        .route("/api/config", get(handlers::get_api_config))
        .route("/git-pull", post(handlers::post_git_pull))
        .route("/hooks/github", post(handlers::post_github_hook))
        .route("/transpile", post(transpiler::post_transpile))
        .route(
            "/.well-known/acme-challenge/*path",
            get({
                let dir = config.acme_dir.clone();
                move |AxPath(path): AxPath<String>| async move {
                    handlers::serve_acme_challenge(&dir, &path).await
                }
            }),
        )
        .route(
            "/",
            get(handlers::get_root)
                .head(handlers::head_root)
                .options(handlers::options_capabilities),
        )
        .route(
            "/*path",
            get(handlers::handle_get_file)
                .head(handlers::head_file)
                .put(handlers::put_file)
                .delete(handlers::delete_file)
                .options(handlers::options_capabilities)
                .fallback(relay_path_fallback),
        )
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(config.state.clone());

    let http_addr = config.http_addr;
    let app_http = app.clone();
    let http_task = tokio::spawn(async move {
        info!(%http_addr, "HTTP listening");
        let listener = TcpListener::bind(&http_addr).await.expect("bind http");
        if let Err(e) = axum::serve(listener, app_http.into_make_service()).await {
            error!(?e, "HTTP server error");
        }
    });

    // HTTPS optional
    let https_task = if let (Some(cert_path), Some(key_path)) = (config.tls_cert, config.tls_key) {
        let https_addr: SocketAddr = format!("0.0.0.0:{}", config.https_port).parse()?;
        let tls_config = config::load_rustls_config(&cert_path, &key_path).await?;
        let app_https = app;
        Some(tokio::spawn(async move {
            info!(%https_addr, cert=%cert_path, key=%key_path, "HTTPS listening");
            if let Err(e) = axum_server::bind_rustls(https_addr, tls_config)
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
