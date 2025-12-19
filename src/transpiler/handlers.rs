use axum::{http::StatusCode, response::IntoResponse, Json};

use crate::{helpers, types::*};
use hook_transpiler::{transpile, TranspileOptions};

/// POST /transpile — transpile arbitrary source (used by tooling)
pub async fn post_transpile(Json(req): Json<TranspileRequest>) -> impl IntoResponse {
    let opts = TranspileOptions {
        filename: req.filename.clone(),
        react_dev: false,
        to_commonjs: req.to_common_js,
        pragma: Some("h".to_string()),
        pragma_frag: None,
    };
    match transpile(&req.code, opts) {
        Ok(out) => {
            let mut resp = (
                StatusCode::OK,
                Json(TranspileResponse {
                    code: Some(out.code),
                    map: out.map,
                    diagnostics: None,
                    ok: true,
                }),
            )
                .into_response();
            helpers::add_transpiler_version_header(&mut resp);
            resp
        }
        Err(err) => helpers::build_transpile_error_response(err, None, None),
    }
}
