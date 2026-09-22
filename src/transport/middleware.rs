//! 中间件:请求 ID、Host/Origin 校验、no-store。
//! 本机 Host/Origin/CSRF 校验不能以关闭校验解决开发连通问题(quickstart §3)。

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderName, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::transport::error::ErrorCode;
use crate::transport::state::AppState;

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// 为每个请求生成 request_id,进入扩展与响应头;错误信封引用同一 ID。
pub async fn request_id_middleware(mut req: Request<axum::body::Body>, next: Next) -> Response {
    let request_id = crate::domain::ids::new_request_key();
    req.extensions_mut().insert(request_id.clone());
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(X_REQUEST_ID, request_id.parse().unwrap());
    resp
}

fn rejection(code: ErrorCode, message: &str) -> Response {
    let request_id = crate::domain::ids::new_request_key();
    (StatusCode::FORBIDDEN, Json(serde_json::json!({
        "error": { "code": code_str(code), "message": message, "request_id": request_id, "retryable": false }
    })))
    .into_response()
}

fn code_str(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::CsrfRejected => "csrf_rejected",
        _ => "origin_rejected",
    }
}

/// Host 允许回环与显式开发来源;浏览器变更请求必须来自相同 Origin,无通配 CORS。
pub async fn host_origin_middleware(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let cfg = &state.inner.config;
    let host_ok = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| {
            cfg.allowed_hosts
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(h))
        })
        .unwrap_or(false);
    if !host_ok {
        return rejection(ErrorCode::OriginRejected, "Host 不在允许的本地地址列表中");
    }
    let method = req.method().clone();
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .map(|s| s.to_string());
    if let Some(origin) = origin {
        let origin_ok = cfg.allowed_origins.contains(&origin);
        if !origin_ok {
            return rejection(ErrorCode::OriginRejected, "Origin 不在允许的本地来源列表中");
        }
    } else if method != axum::http::Method::GET && method != axum::http::Method::HEAD {
        // 变更请求缺少 Origin(非浏览器客户端):拒绝,避免跨站表单直投
        return rejection(ErrorCode::OriginRejected, "变更请求缺少 Origin 头");
    }
    let mut resp = next.run(req).await;
    if resp.status().is_success() || resp.status().is_redirection() {
        resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
    }
    resp
}
