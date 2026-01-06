use std::collections::HashMap;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use tracing::error;
use crate::{AppState, helpers};

pub async fn handle_query(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(path): axum::extract::Path<String>,
    _query: Option<Query<HashMap<String, String>>>,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let branch = helpers::branch_from(&headers);
    let repo_name_opt = helpers::strict_repo_from(&state.repo_path, &headers);
    
    let repo_name = match repo_name_opt {
        Some(name) => name,
        None => return (StatusCode::BAD_REQUEST, "X-Relay-Repo header required").into_response(),
    };

    // Use path as query if it's not "query" (legacy) and not empty
    let mut query_val = if !path.is_empty() && path != "query" {
        Some(serde_json::Value::String(path))
    } else {
        None
    };

    let mut collection_storage = "index".to_string();

    // Override or refine with body if present
    if let Some(Json(b)) = body {
        if let Some(q) = b.get("query") {
            query_val = Some(q.clone());
        }
        if let Some(c) = b.get("collection").and_then(|v| v.as_str()) {
            collection_storage = c.to_string();
        }
    }

    match crate::git::query::execute_query(
        &state.repo_path,
        &repo_name,
        &branch,
        query_val,
        &collection_storage,
    ) {
        Ok(results) => (StatusCode::OK, Json(serde_json::json!({ "results": results }))).into_response(),
        Err(e) => {
            error!(?e, "Query failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "Query execution failed").into_response()
        }
    }
}
