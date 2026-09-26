//! 007 US3 规则扩展 HTTP 契约测试(T039,contracts §3 端点子集):
//! 规则列表扩展字段 + trigger_counts、创建扩展体(优先级冲突 422 rule_conflict、
//! buyer_reviewed 422 unsupported_capability)、关键词回复 CRUD、默认回复 PUT 与
//! 清空记录、鉴权与 CSRF 门禁。ai-settings 端点(依赖 0006)不在本轮。

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

fn authed(builder: axum::http::request::Builder, s: &Session) -> axum::http::request::Builder {
    builder
        .header(header::COOKIE, format!("qing_session={}", s.cookie))
        .header("x-csrf-token", &s.csrf)
}

async fn post_json(
    app: &axum::Router,
    s: &Session,
    uri: &str,
    payload: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::CONTENT_TYPE, "application/json"),
                s,
            )
            .body(Body::from(payload.to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp).await)
}

async fn put_json(
    app: &axum::Router,
    s: &Session,
    uri: &str,
    payload: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::CONTENT_TYPE, "application/json"),
                s,
            )
            .body(Body::from(payload.to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp).await)
}

async fn get_json(app: &axum::Router, s: &Session, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .uri(uri)
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN),
                s,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp).await)
}

/// 预置账号与商品(直写仓储,与路由同一 DB 线程)。
async fn seed(app: &crate::support::app::TestApp) {
    app.db
        .call(|conn| {
            qing_delivery::adapters::sqlite::repos::accounts::upsert_identity(
                conn, "acct-1", "xianyu", "seller-1", "卖家",
            )?;
            qing_delivery::adapters::sqlite::repos::items::upsert(
                conn, "item-1", "acct-1", "EXT-1", "考研资料", "on_sale", "[]", "single",
            )
        })
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn 未认证访问规则扩展端点被拒() {
    let app = spawn_app().await;
    for uri in [
        "/api/v1/accounts/acct-1/reply-rules",
        "/api/v1/accounts/acct-1/default-reply",
    ] {
        let resp = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::HOST, HOST)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn 规则列表扩展字段与优先级冲突契约() {
    let app = spawn_app().await;
    seed(&app).await;
    let s = login(&app.router).await;

    // 创建扩展规则(card_pool 来源 + 变体 + 优先级)→ 201
    let create = r#"{
        "item_id": "item-1", "sku_key": "", "enabled": true,
        "content_kind": "card_pool", "content": "",
        "card_pool_id": "pool-none",
        "trigger_type": "order_paid", "priority": 42,
        "variants": [{
            "spec_name": "颜色", "spec_values": ["红色"],
            "source": "card_pool", "card_pool_id": "pool-none",
            "units_per_item": 2, "delay_override_seconds": 120
        }]
    }"#;
    let (status, body) = post_json(&app.router, &s, "/api/v1/accounts/acct-1/rules", create).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // 列表:RuleDto 扩展字段 + trigger_counts 汇总;引用缺失 → needs_reconfiguration
    let (status, list) =
        get_json(&app.router, &s, "/api/v1/accounts/acct-1/rules?trigger_type=order_paid").await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["priority"], 42);
    assert_eq!(items[0]["trigger_type"], "order_paid");
    assert_eq!(items[0]["content_source"], "card_pool");
    assert!(items[0]["needs_reconfiguration"].as_bool().unwrap(), "引用缺失标记");
    let variants = items[0]["variants"].as_array().unwrap();
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0]["units_per_item"], 2);
    assert_eq!(
        list["trigger_counts"]["order_paid"].as_i64(),
        Some(1),
        "trigger_counts 汇总"
    );

    // 同优先级第二条 → 422 rule_conflict;不同优先级 → 201
    let (status, err) = post_json(&app.router, &s, "/api/v1/accounts/acct-1/rules", create).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["error"]["code"], "rule_conflict");
    let other = r#"{
        "item_id": "item-1", "sku_key": "", "enabled": true,
        "content_kind": "fixed_text", "content": "备用内容", "priority": 43
    }"#;
    let (status, _) = post_json(&app.router, &s, "/api/v1/accounts/acct-1/rules", other).await;
    assert_eq!(status, StatusCode::CREATED);

    // buyer_reviewed 触发 → 422 unsupported_capability(FR-036 能力门禁)
    let gated = r#"{
        "item_id": "item-1", "sku_key": "single", "enabled": false,
        "content": "赠品", "trigger_type": "buyer_reviewed"
    }"#;
    let (status, err) = post_json(&app.router, &s, "/api/v1/accounts/acct-1/rules", gated).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["error"]["code"], "unsupported_capability");
}

#[tokio::test]
async fn 关键词回复与默认回复契约() {
    let app = spawn_app().await;
    seed(&app).await;
    let s = login(&app.router).await;

    // 关键词回复:创建(关联商品)→ 列表 → 更新(转账号级)→ 删除
    let create = r#"{
        "keyword": "发货", "reply_kind": "text", "reply_text": "24 小时内发出",
        "enabled": true, "item_ids": ["item-1"]
    }"#;
    let (status, body) = post_json(&app.router, &s, "/api/v1/accounts/acct-1/reply-rules", create).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let rid = body["id"].as_str().unwrap().to_string();

    let (status, list) = get_json(&app.router, &s, "/api/v1/accounts/acct-1/reply-rules").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["item_ids"], serde_json::json!(["item-1"]));

    let update = r#"{
        "keyword": "已发货", "reply_kind": "text", "reply_text": "请查收",
        "enabled": false, "item_ids": []
    }"#;
    let (status, body) = put_json(
        &app.router,
        &s,
        &format!("/api/v1/accounts/acct-1/reply-rules/{rid}"),
        update,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["keyword"], "已发货");
    assert!(!body["enabled"].as_bool().unwrap());

    // 校验拒绝:文字回复缺文案 → 400 invalid_request(与 cards 既有约定一致)
    let bad = r#"{"keyword":"空","reply_kind":"text","enabled":true,"item_ids":[]}"#;
    let (status, err) = post_json(&app.router, &s, "/api/v1/accounts/acct-1/reply-rules", bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid_request");

    // 删除 → 204;再删 → 404
    let resp = app
        .router
        .clone()
        .oneshot(
            authed(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/accounts/acct-1/reply-rules/{rid}"))
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN),
                &s,
            )
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
            authed(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/accounts/acct-1/reply-rules/{rid}"))
                    .header(header::HOST, HOST)
                    .header(header::ORIGIN, ORIGIN),
                &s,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // 默认回复:GET 空态关闭 → PUT 启用 → 清空记录
    let (status, body) = get_json(&app.router, &s, "/api/v1/accounts/acct-1/default-reply").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["enabled"].as_bool().unwrap(), "未配置返回关闭态");

    let put = r#"{"enabled": true, "reply_text": "亲,稍后回复您", "reply_once": true}"#;
    let (status, body) = put_json(&app.router, &s, "/api/v1/accounts/acct-1/default-reply", put).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["enabled"].as_bool().unwrap());
    assert_eq!(body["reply_once"], true);

    // 启用但无内容 → 400 invalid_request(同上约定)
    let bad = r#"{"enabled": true, "reply_once": true}"#;
    let (status, err) = put_json(&app.router, &s, "/api/v1/accounts/acct-1/default-reply", bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");

    // 清空记录 → 200 {cleared: 0}(无记录时)
    let (status, body) = post_json(
        &app.router,
        &s,
        "/api/v1/accounts/acct-1/default-reply/clear-records",
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["cleared"], 0);
}
