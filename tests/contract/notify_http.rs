//! 007 US5 通知渠道 HTTP 契约测试(T063,contracts §5 端点子集):
//! 渠道 CRUD(secrets_configured 脱敏回显、留空不修改、409 乐观锁、
//! 非法 kind 422)、绑定覆盖式 PUT/GET 回环、系统 SMTP 密码只回
//! password_configured。测试投递端点涉及真实外呼,由集成测试以
//! 假发送方覆盖(tests/integration/notify.rs),此处不触网。

use crate::support::app::spawn_app;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:59189";
const ORIGIN: &str = "http://127.0.0.1:59189";

struct Session {
    cookie: String,
    csrf: String,
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

async fn login(app: &axum::Router) -> Session {
    let anon = app
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
    let csrf1 = body_json(anon).await["csrf_token"].as_str().unwrap().to_string();
    let init = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/initialize")
                .header(header::HOST, HOST)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", csrf1)
                .body(Body::from(
                    r#"{"password":"a-long-password-123","password_confirmation":"a-long-password-123"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(init.status(), StatusCode::CREATED);
    let set_cookie = init
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    let cookie = set_cookie["qing_session=".len()..]
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let csrf = body_json(init).await["csrf_token"].as_str().unwrap().to_string();
    Session { cookie, csrf }
}

async fn req(
    app: &axum::Router,
    s: &Session,
    method: &str,
    uri: &str,
    payload: Option<String>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, format!("qing_session={}", s.cookie))
        .header("x-csrf-token", &s.csrf);
    if payload.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(payload.unwrap_or_default())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp).await)
}

#[tokio::test]
async fn notify_channels_contract_subset() {
    let app = spawn_app().await;
    let s = login(&app.router).await;

    // 创建 webhook:201;回读无 secrets_configured
    let (status, ch) = req(
        &app.router,
        &s,
        "POST",
        "/api/v1/notification-channels",
        Some(
            r#"{"kind":"webhook","name":"本地收集","enabled":true,
                "config":{"url":"https://example.com/hook"},"secrets":{},"event_types":[]}"#
                .into(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{ch}");
    assert_eq!(ch["kind"], "webhook");
    assert_eq!(ch["secrets_configured"].as_array().map(Vec::len), Some(0));
    let webhook_id = ch["id"].as_str().unwrap().to_string();
    let webhook_version = ch["version"].as_i64().unwrap();

    // 创建 telegram(秘密):secrets_configured 只含键名,无明文
    let (status, tg) = req(
        &app.router,
        &s,
        "POST",
        "/api/v1/notification-channels",
        Some(
            r#"{"kind":"telegram","name":"机器人","config":{"chat_id":"-100123"},
                "secrets":{"bot_token":"PLAIN-TOKEN-1"},"event_types":["account_offline"]}"#
                .into(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{tg}");
    assert_eq!(
        tg["secrets_configured"],
        serde_json::json!(["bot_token"]),
        "脱敏回显:只键名"
    );
    let serialized = tg.to_string();
    assert!(!serialized.contains("PLAIN-TOKEN-1"), "API 永不回显秘密明文");
    assert_eq!(tg["event_types"], serde_json::json!(["account_offline"]));

    // 非法 kind / 非法 event_types → 422
    let (status, body) = req(
        &app.router,
        &s,
        "POST",
        "/api/v1/notification-channels",
        Some(r#"{"kind":"sms","name":"x","config":{}}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = req(
        &app.router,
        &s,
        "POST",
        "/api/v1/notification-channels",
        Some(
            r#"{"kind":"webhook","name":"x","config":{"url":"https://a.com"},
                "event_types":["order_paid"]}"#
                .into(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // 列表包含两条
    let (status, list) = req(
        &app.router,
        &s,
        "GET",
        "/api/v1/notification-channels",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().map(Vec::len), Some(2));

    // 乐观锁:错误版本 409 version_conflict
    let (status, body) = req(
        &app.router,
        &s,
        "PUT",
        &format!("/api/v1/notification-channels/{webhook_id}"),
        Some(
            r#"{"kind":"webhook","name":"改名","enabled":true,
                "config":{"url":"https://example.com/hook"},"event_types":[],"expected_version":999}"#
                .into(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // 更新:secrets 留空 = 不修改(telegram 秘密保持已配置)
    let tg_id = tg["id"].as_str().unwrap().to_string();
    let tg_version = tg["version"].as_i64().unwrap();
    let (status, updated) = req(
        &app.router,
        &s,
        "PUT",
        &format!("/api/v1/notification-channels/{tg_id}"),
        Some(
            format!(
                r#"{{"kind":"telegram","name":"机器人2","config":{{"chat_id":"-100123"}},
                    "secrets":{{}},"event_types":[],"expected_version":{tg_version}}}"#
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(
        updated["secrets_configured"],
        serde_json::json!(["bot_token"]),
        "留空保存不覆盖"
    );

    // 详情 404 / 删除 204
    let (status, _) = req(
        &app.router,
        &s,
        "DELETE",
        &format!("/api/v1/notification-channels/{webhook_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = req(
        &app.router,
        &s,
        "GET",
        &format!("/api/v1/notification-channels/{webhook_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 未登录 401
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/notification-channels")
                .header(header::HOST, HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let _ = webhook_version;
}

#[tokio::test]
async fn bindings_and_system_smtp_contract() {
    let app = spawn_app().await;
    let s = login(&app.router).await;

    // 渠道 + 账号种子
    let (status, ch) = req(
        &app.router,
        &s,
        "POST",
        "/api/v1/notification-channels",
        Some(
            r#"{"kind":"webhook","name":"A","config":{"url":"https://example.com/hook"}}"#.into(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{ch}");
    let ch_id = ch["id"].as_str().unwrap().to_string();
    app.db
        .call(move |conn| {
            qing_delivery::adapters::sqlite::repos::accounts::upsert_identity(
                conn, "acct-1", "xianyu", "s1", "店铺",
            )
        })
        .await
        .unwrap()
        .unwrap();

    // 绑定覆盖式 PUT → GET 回环;未知渠道 422
    let (status, body) = req(
        &app.router,
        &s,
        "PUT",
        "/api/v1/accounts/acct-1/notification-bindings",
        Some(format!(r#"{{"channel_ids":["{ch_id}"]}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = req(
        &app.router,
        &s,
        "GET",
        "/api/v1/accounts/acct-1/notification-bindings",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["channel_ids"], serde_json::json!([ch_id]));
    let (status, body) = req(
        &app.router,
        &s,
        "PUT",
        "/api/v1/accounts/acct-1/notification-bindings",
        Some(r#"{"channel_ids":["nch-none"]}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // 系统 SMTP:PUT(带密码)→ GET 只回 password_configured;留空不覆盖
    let (status, body) = req(
        &app.router,
        &s,
        "PUT",
        "/api/v1/settings/system-smtp",
        Some(
            r#"{"host":"smtp.example.com","port":465,"encryption":"ssl",
                "from_address":"shop@example.com","username":"shop",
                "password":"SECRET-SMTP-PW"}"#
                .into(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["password_configured"], true);
    assert!(!body.to_string().contains("SECRET-SMTP-PW"), "密码不回显");
    assert_eq!(body["port"], 465);
    let (status, body) = req(
        &app.router,
        &s,
        "PUT",
        "/api/v1/settings/system-smtp",
        Some(
            r#"{"host":"smtp2.example.com","port":587,"encryption":"starttls",
                "from_address":"shop2@example.com"}"#
                .into(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["host"], "smtp2.example.com");
    assert_eq!(body["password_configured"], true, "留空密码不覆盖");
    // 非法加密方式 422
    let (status, body) = req(
        &app.router,
        &s,
        "PUT",
        "/api/v1/settings/system-smtp",
        Some(
            r#"{"host":"s.example.com","port":25,"encryption":"magic",
                "from_address":"a@b.c"}"#
                .into(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
