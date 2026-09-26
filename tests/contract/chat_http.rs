//! 007 US4 在线聊天 HTTP 契约测试(T052,contracts §4 端点子集):
//! unread-summary 形状、快捷回复 CRUD(第 51 条 422 reply_limit_reached)、
//! 买家备注 PUT/GET 回环、会话/消息 404、发送门禁(离线 403 account_unavailable、
//! 超 2000 字 422 content_too_long)、鉴权与 CSRF。

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
        // 204 等空体响应:以 Null 表达(状态码断言足够)
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

fn authed(builder: axum::http::request::Builder, s: &Session) -> axum::http::request::Builder {
    builder
        .header(header::COOKIE, format!("qing_session={}", s.cookie))
        .header("x-csrf-token", &s.csrf)
}

async fn req(
    app: &axum::Router,
    s: &Session,
    method: &str,
    uri: &str,
    payload: Option<String>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = authed(
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, HOST)
            .header(header::ORIGIN, ORIGIN),
        s,
    );
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
async fn chat_endpoints_contract_subset() {
    let test_app = spawn_app().await;
    let app = &test_app.router;
    let s = login(app).await;

    // 未读汇总:空库形状 {total, by_account}
    let (status, body) = req(app, &s, "GET", "/api/v1/chat/unread-summary", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);
    assert_eq!(body["by_account"].as_object().map(|m| m.len()), Some(0));

    // 快捷回复:创建 201 → 列表 → 删除 204 → 再删 404
    let (status, created) = req(
        app,
        &s,
        "POST",
        "/api/v1/chat/quick-replies",
        Some(r#"{"body":"已发货,请查收"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["body"], "已发货,请查收");
    let (status, list) = req(app, &s, "GET", "/api/v1/chat/quick-replies", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().map(|a| a.len()), Some(1));
    let id = created["id"].as_str().unwrap().to_string();
    let (status, _) = req(
        app,
        &s,
        "DELETE",
        &format!("/api/v1/chat/quick-replies/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = req(
        app,
        &s,
        "DELETE",
        &format!("/api/v1/chat/quick-replies/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // 买家备注:PUT → GET 回环(空=未设置 note null)
    let (status, _) = req(
        app,
        &s,
        "GET",
        "/api/v1/chat/accounts/acct-x/buyer-notes/buyer-9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req(
        app,
        &s,
        "PUT",
        "/api/v1/chat/accounts/acct-x/buyer-notes/buyer-9",
        Some(r#"{"note":"老客户"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = req(
        app,
        &s,
        "GET",
        "/api/v1/chat/accounts/acct-x/buyer-notes/buyer-9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["note"], "老客户");

    // 会话列表(未知账号)= 空页;消息查询未知会话 404
    let (status, body) = req(
        app,
        &s,
        "GET",
        "/api/v1/accounts/acct-x/chat/sessions",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().map(|a| a.len()), Some(0));
    let (status, body) = req(
        app,
        &s,
        "GET",
        "/api/v1/chat/conversations/conv-none/messages",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = req(
        app,
        &s,
        "POST",
        "/api/v1/chat/conversations/conv-none/messages",
        Some(r#"{"text":"你好"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // 鉴权:未登录 401;变更缺 CSRF 403
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/chat/unread-summary")
                .header(header::HOST, HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// 发送门禁:离线账号 403 account_unavailable;超 2000 字 422 content_too_long;
/// 未知消息重试 404。账号与会话经共享 DB 线程直种(与生产同仓储路径)。
#[tokio::test]
async fn chat_send_gates_contract() {
    let test_app = spawn_app().await;
    let app = &test_app.router;
    let s = login(app).await;
    let db = test_app.db.clone();
    // 离线账号 + 会话
    db.call(|conn| {
        qing_delivery::adapters::sqlite::repos::accounts::upsert_identity(
            conn, "acct-1", "xianyu", "seller-1", "卖家",
        )?;
        qing_delivery::adapters::sqlite::repos::chat::upsert_conversation(
            conn,
            "conv-1",
            "acct-1",
            "buyer-1",
            Some("chat-1"),
            None,
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();
    let (status, body) = req(
        app,
        &s,
        "POST",
        "/api/v1/chat/conversations/conv-1/messages",
        Some(r#"{"text":"离线发不出"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "account_unavailable");

    // 账号置在线后,超长文本 422 content_too_long
    db.call(|conn| {
        conn.execute("UPDATE accounts SET status='online' WHERE id='acct-1'", [])
    })
    .await
    .unwrap()
    .unwrap();
    let long = "字".repeat(2001);
    let payload = format!(r#"{{"text":"{long}"}}"#);
    let (status, body) = req(
        app,
        &s,
        "POST",
        "/api/v1/chat/conversations/conv-1/messages",
        Some(payload),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "content_too_long");

    // 合法文本:mock 适配器默认 Accepted → 201 + status=sent(MessageDto 形状)
    let (status, body) = req(
        app,
        &s,
        "POST",
        "/api/v1/chat/conversations/conv-1/messages",
        Some(r#"{"text":"您好"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["direction"], "out");
    assert_eq!(body["status"], "sent");
    assert_eq!(body["msg_kind"], "text");
    let mid = body["id"].as_str().unwrap().to_string();
    // sent 消息重试 → 422 unsafe_retry(仅 failed 可重试)
    let (status, body) = req(
        app,
        &s,
        "POST",
        &format!("/api/v1/chat/messages/{mid}/retry"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "unsafe_retry");

    // 未知消息重试 404
    let (status, _) = req(app, &s, "POST", "/api/v1/chat/messages/msg-none/retry", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 未读清零 + 删除会话(隐藏后消息查询 404)
    let (status, _) = req(
        app,
        &s,
        "POST",
        "/api/v1/chat/conversations/conv-1/read",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = req(
        app,
        &s,
        "DELETE",
        "/api/v1/chat/conversations/conv-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = req(
        app,
        &s,
        "GET",
        "/api/v1/chat/conversations/conv-1/messages",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "删除(隐藏)后不可见");
}
