//! dashboard 摘要契约测试(T021/002):增量字段 orders_today/delivered_today
//! 的存在、类型、不变量与既有字段兼容(contracts/dashboard-api.md)。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use crate::support::app::spawn_app;

const HOST: &str = "127.0.0.1:59189";
const ORIGIN: &str = "http://127.0.0.1:59189";

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn cookie_of(resp: &axum::response::Response) -> String {
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
        .unwrap()
}

/// 初始化管理员并返回会话 Cookie。
async fn login_session(app: &crate::support::app::TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/session")
                .header(header::HOST, HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/initialize")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", csrf)
                .body(Body::from(
                    r#"{"password":"a-long-password-123","password_confirmation":"a-long-password-123"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    cookie_of(&resp)
}

async fn get_dashboard(app: &crate::support::app::TestApp, cookie: &str) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/dashboard")
                .header(header::HOST, HOST)
                .header(header::COOKIE, format!("qing_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp).await
}

#[tokio::test]
async fn dashboard_增量字段_空库为零且不变量成立() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;
    let body = get_dashboard(&app, &cookie).await;

    // 既有字段不受增量影响(additive,SC-206)
    for key in [
        "accounts",
        "open_issue_count",
        "active_job_count",
        "persistence",
        "stopping",
    ] {
        assert!(body.get(key).is_some(), "既有字段缺失:{key}");
    }
    assert_eq!(body["orders_today"], 0);
    assert_eq!(body["delivered_today"], 0);
    assert!(body["delivered_today"].as_i64() <= body["orders_today"].as_i64());
}

#[tokio::test]
async fn dashboard_增量字段_播种后计数正确() {
    let app = spawn_app().await;
    let cookie = login_session(&app).await;

    let now = qing_delivery::domain::time_util::utc_now_ms();
    let today = now - 60_000; // 一分钟前,稳落在本地自然日内

    app.db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO accounts (id, platform, external_user_id, display_name, created_at, updated_at)
                 VALUES ('acct-1', 'xianyu', 'u1', '播种账号', 0, 0)",
                [],
            )?;
            for (id, paid) in [("o1", today), ("o2", today + 1), ("o3", today + 2)] {
                conn.execute(
                    "INSERT INTO orders (id, platform, account_id, external_order_id, paid_at, created_at, updated_at)
                     VALUES (?1, 'xianyu', 'acct-1', ?1, ?2, 0, 0)",
                    rusqlite::params![id, paid],
                )?;
            }
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();

    // o1 已交付,o2 补发后交付,o3 未交付
    let deliveries: Vec<(&str, &str, &str)> = vec![
        ("d1", "o1", "initial"),
        ("d2", "o2", "initial"),
        ("d3", "o2", "resend"),
        ("d4", "o3", "initial"),
    ];
    for (id, order, kind) in deliveries {
        let state = match id {
            "d1" | "d3" => "accepted",
            "d2" => "unknown",
            _ => "dispatching",
        };
        app.db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO deliveries (id, order_id, kind, content_state, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 0, 0)",
                    rusqlite::params![id, order, kind, state],
                )?;
                Ok::<(), rusqlite::Error>(())
            })
            .await
            .unwrap()
            .unwrap();
    }

    let body = get_dashboard(&app, &cookie).await;
    assert_eq!(body["orders_today"], 3, "今日付款 3 单");
    assert_eq!(body["delivered_today"], 2, "已交付 2 单(含补发路径)");
    assert!(body["delivered_today"].as_i64() <= body["orders_today"].as_i64());
}
