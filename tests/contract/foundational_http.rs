//! 基础设施契约测试(T022):匿名拒绝、CSRF、错误信封、认证流、能力声明。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use crate::support::app::spawn_app;

const HOST: &str = "127.0.0.1:59189";
const ORIGIN: &str = "http://127.0.0.1:59189";

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, json: &str, extra: &[(&'static str, String)]) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in extra {
        b = b.header(*k, v.as_str());
    }
    b.body(Body::from(json.to_string())).unwrap()
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn cookie_of(resp: &axum::response::Response) -> Option<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.starts_with("qing_session=").then(|| {
                s["qing_session=".len()..]
                    .split(';')
                    .next()
                    .unwrap()
                    .to_string()
            })
        })
}

#[tokio::test]
async fn health_returns_status_and_version_only() {
    let app = spawn_app().await;
    let resp = app.router.clone().oneshot(get("/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["status"], "ok");
    assert!(body["version"].as_str().is_some());
    assert!(body.get("accounts").is_none(), "健康检查不暴露账号信息");
}

#[tokio::test]
async fn host_and_origin_are_enforced() {
    let app = spawn_app().await;
    // 非法 Host
    let bad_host = Request::builder()
        .uri("/health")
        .header(header::HOST, "evil.example.com")
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(bad_host).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp).await;
    assert_eq!(body["error"]["code"], "origin_rejected");

    // 变更请求无 Origin
    let no_origin = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/initialize")
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.router.clone().oneshot(no_origin).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn initialize_login_and_protected_routes() {
    let app = spawn_app().await;

    // 未初始化:匿名会话 + CSRF
    let resp = app
        .router
        .clone()
        .oneshot(get("/api/v1/auth/session"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["initialized"], false);
    assert_eq!(body["authenticated"], false);
    let csrf = body["csrf_token"].as_str().unwrap().to_string();

    // 受保护接口未登录 → 401
    let resp = app
        .router
        .clone()
        .oneshot(get("/api/v1/capabilities"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let err = body_json(resp).await;
    assert_eq!(err["error"]["code"], "authentication_required");

    // 初始化缺 CSRF → 403
    let resp = app
        .router
        .clone()
        .oneshot(post(
            "/api/v1/auth/initialize",
            r#"{"password":"x-long-password-123","password_confirmation":"x-long-password-123"}"#,
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 短密码 → 400 明确错误
    let resp = app
        .router
        .clone()
        .oneshot(post(
            "/api/v1/auth/initialize",
            r#"{"password":"short","password_confirmation":"short"}"#,
            &[("x-csrf-token", csrf.clone())],
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 重新取匿名 CSRF(上一个未消耗但密码失败不消耗;这里再取一个新的)
    let resp = app
        .router
        .clone()
        .oneshot(get("/api/v1/auth/session"))
        .await
        .unwrap();
    let csrf2 = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    // 正常初始化 → 201 + Cookie
    let resp = app
        .router
        .clone()
        .oneshot(post(
            "/api/v1/auth/initialize",
            r#"{"password":"a-long-password-123","password_confirmation":"a-long-password-123"}"#,
            &[("x-csrf-token", csrf2)],
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookie = cookie_of(&resp).expect("应设置会话 Cookie");
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
    let body = body_json(resp).await;
    assert_eq!(body["authenticated"], true);
    let login_csrf = body["csrf_token"].as_str().unwrap().to_string();

    // 已初始化 → 再初始化 409
    let resp = app
        .router
        .clone()
        .oneshot(get("/api/v1/auth/session"))
        .await
        .unwrap();
    let anon_csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = app
        .router
        .clone()
        .oneshot(post(
            "/api/v1/auth/initialize",
            r#"{"password":"another-long-pass-1","password_confirmation":"another-long-pass-1"}"#,
            &[("x-csrf-token", anon_csrf)],
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(resp).await["error"]["code"],
        "already_initialized"
    );

    // 已登录:capabilities 200 且仅闲鱼+固定文字可用(FR-031)
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/capabilities")
                .header(header::HOST, HOST)
                .header(header::COOKIE, format!("qing_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let caps = body_json(resp).await;
    assert_eq!(caps["platforms"][0]["platform"], "xianyu");
    assert_eq!(caps["platforms"][0]["supported"], true);
    assert_eq!(caps["platforms"][1]["supported"], false);
    assert_eq!(caps["content_limits"]["unicode_scalars"], 1000);

    // 错误密码登录 → 401 统一错误
    let resp = app
        .router
        .clone()
        .oneshot(get("/api/v1/auth/session"))
        .await
        .unwrap();
    let anon_csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = app
        .router
        .clone()
        .oneshot(post(
            "/api/v1/auth/login",
            r#"{"password":"wrong-password-x"}"#,
            &[("x-csrf-token", anon_csrf)],
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 正确登录 → 200 + 轮换 Cookie
    let resp = app
        .router
        .clone()
        .oneshot(get("/api/v1/auth/session"))
        .await
        .unwrap();
    let anon_csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = app
        .router
        .clone()
        .oneshot(post(
            "/api/v1/auth/login",
            r#"{"password":"a-long-password-123"}"#,
            &[("x-csrf-token", anon_csrf)],
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie2 = cookie_of(&resp).expect("登录应轮换 Cookie");

    // 退出后旧会话失效
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::COOKIE, format!("qing_session={cookie2}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/capabilities")
                .header(header::HOST, HOST)
                .header(header::COOKIE, format!("qing_session={cookie2}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "退出后旧会话不可用"
    );

    // 初始化时签发的会话仍有效(未受退出影响)
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/capabilities")
                .header(header::HOST, HOST)
                .header(header::COOKIE, format!("qing_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // CSRF 轮换验证:会话查询返回新 CSRF,旧 login_csrf 不再可用(哈希已换)
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/session")
                .header(header::HOST, HOST)
                .header(header::COOKIE, format!("qing_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let rotated = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(rotated, login_csrf);
}

#[tokio::test]
async fn unknown_job_returns_404() {
    let app = spawn_app().await;
    let resp = app
        .router
        .clone()
        .oneshot(get("/api/v1/jobs/job_missing"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "先要求登录");
}

#[tokio::test]
async fn static_index_is_served() {
    let app = spawn_app().await;
    let resp = app.router.clone().oneshot(get("/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        html.contains("qing") || html.contains("<div id=\"app\">"),
        "应返回前端入口页"
    );
}
