//! 嵌入式静态页面:编译产物经 include_dir 进入二进制,普通用户无需 Node。
//! CI/release 严格要求本次构建的前端产物,不能静默使用上一次页面(quickstart §2)。

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

static WEBUI: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/webui");

pub async fn serve_static(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let wanted = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = WEBUI.get_file(wanted) {
        return file_response(wanted, file.contents());
    }
    // SPA 路由回退:非资产路径返回入口页,由前端路由解析
    if !wanted.contains('.')
        && let Some(index) = WEBUI.get_file("index.html")
    {
        return file_response("index.html", index.contents());
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn file_response(path: &str, contents: &[u8]) -> Response {
    let mime = match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "json" => "application/json",
        _ => "application/octet-stream",
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        contents.to_vec(),
    )
        .into_response()
}
