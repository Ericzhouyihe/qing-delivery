//! US7 系统与 AI 集成测试(T074 先红,T075~T078 转绿):
//! 1) 改密:当前密码错误 → 422 credential_change_failed;
//! 2) 改密成功:其他会话全部失效(旧 token 401)、当前会话保留、
//!    新密码可登录、旧密码失效(FR-072);
//! 3) 系统设置:AI 地址/模型明文读写、Key 只回 *_configured 不回明文;
//! 4) 读取模型/测试连接:走本地假 OpenAI 兼容服务(校验 Bearer key、
//!    模型列表、耗时与回复摘要);不可达地址 → 422 结构化原因;
//! 5) AI 分流:关键词未命中 → AI 文案发出、账号提示词注入 system 消息、
//!    订单/交付零写入(宪章红线);
//! 6) AI 上游故障 → 静默降级默认回复(默认回复仍发送、无 panic、零订单写入);
//! 7) 账号 ai_reply_enabled=false → 不调 AI(假服务零请求)直落默认回复;
//! 8) 账号 AI 设置 PUT:开关/提示词落库回读、prompt ≤2000 校验、账号不存在 404。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tower::ServiceExt;

use qing_delivery::application::chat::{IncomingChatMessage};
use qing_delivery::domain::chat::ChatMsgKind;

use crate::support::app::{TestApp, spawn_app};

const HOST: &str = "127.0.0.1:59189";
const ORIGIN: &str = "http://127.0.0.1:59189";
const INITIAL_PASSWORD: &str = "initial-long-password-123";
const NEW_PASSWORD: &str = "rotated-long-password-456";

// ---------- HTTP 辅助(与 contract 测试同模式) ----------

fn req(method: &str, path: &str, json_body: Option<&str>, extra: &[(&'static str, String)]) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN);
    if json_body.is_some() {
        b = b.header(header::CONTENT_TYPE, "application/json");
    }
    for (k, v) in extra {
        b = b.header(*k, v.as_str());
    }
    b.body(Body::from(json_body.unwrap_or("").to_string())).unwrap()
}

async fn send(app: &TestApp, request: Request<Body>) -> (StatusCode, Value, String) {
    let resp = app.router.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let raw_cookie = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.starts_with("qing_session=").then(|| {
                s["qing_session=".len()..].split(';').next().unwrap().to_string()
            })
        })
        .unwrap_or_default();
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    // 空响应(204 等)以 Null 兜底:字段断言自然失败,便于定位
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, json, raw_cookie)
}

#[derive(Debug)]
struct Session {
    cookie: String,
    csrf: String,
}

/// 初始化管理员并返回首个会话(带 CSRF)。
async fn initialize(app: &TestApp, password: &str) -> Session {
    let (_, body, _) = send(
        app,
        req("GET", "/api/v1/auth/session", None, &[]),
    )
    .await;
    let anon_csrf = body["csrf_token"].as_str().unwrap().to_string();
    let (status, body, cookie) = send(
        app,
        req(
            "POST",
            "/api/v1/auth/initialize",
            Some(&json!({"password": password, "password_confirmation": password}).to_string()),
            &[("x-csrf-token", anon_csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "初始化失败:{body:?}");
    Session {
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_string(),
    }
}

/// 用密码再开一个会话(匿名 CSRF → 登录)。
async fn login(app: &TestApp, password: &str) -> Result<Session, StatusCode> {
    let (_, body, _) = send(app, req("GET", "/api/v1/auth/session", None, &[])).await;
    let anon_csrf = body["csrf_token"].as_str().unwrap().to_string();
    let (status, body, cookie) = send(
        app,
        req(
            "POST",
            "/api/v1/auth/login",
            Some(&json!({"password": password}).to_string()),
            &[("x-csrf-token", anon_csrf)],
        ),
    )
    .await;
    if status != StatusCode::OK {
        return Err(status);
    }
    Ok(Session {
        cookie,
        csrf: body["csrf_token"].as_str().unwrap_or_default().to_string(),
    })
}

fn authed(s: &Session) -> Vec<(&'static str, String)> {
    vec![
        ("cookie", format!("qing_session={}", s.cookie)),
        ("x-csrf-token", s.csrf.clone()),
    ]
}

/// 受保护 GET 是否仍通过(会话有效性探针)。
async fn session_alive(app: &TestApp, s: &Session) -> bool {
    let (status, _, _) = send(
        app,
        req(
            "GET",
            "/api/v1/capabilities",
            None,
            &[("cookie", format!("qing_session={}", s.cookie))],
        ),
    )
    .await;
    status == StatusCode::OK
}

// ---------- 本地假 OpenAI 兼容服务 ----------

struct FakeAiState {
    chat_calls: Mutex<Vec<Value>>,
    models_calls: Mutex<usize>,
    auth_headers: Mutex<Vec<String>>,
    reply_text: Mutex<String>,
    /// true:chat/completions 一律 500(故障注入)
    fail_chat: AtomicBool,
}

impl FakeAiState {
    fn new(reply: &str) -> Arc<Self> {
        Arc::new(Self {
            chat_calls: Mutex::new(Vec::new()),
            models_calls: Mutex::new(0),
            auth_headers: Mutex::new(Vec::new()),
            reply_text: Mutex::new(reply.to_string()),
            fail_chat: AtomicBool::new(false),
        })
    }

    fn chat_calls(&self) -> Vec<Value> {
        self.chat_calls.lock().unwrap().clone()
    }
}

async fn fake_chat(
    State(st): State<Arc<FakeAiState>>,
    headers: header::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    st.auth_headers.lock().unwrap().push(auth);
    st.chat_calls.lock().unwrap().push(body);
    if st.fail_chat.load(Ordering::SeqCst) {
        return (StatusCode::INTERNAL_SERVER_ERROR, "upstream down").into_response();
    }
    let reply = st.reply_text.lock().unwrap().clone();
    Json(json!({
        "choices": [{"message": {"role": "assistant", "content": reply}}]
    }))
    .into_response()
}

async fn fake_models(State(st): State<Arc<FakeAiState>>, headers: header::HeaderMap) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    st.auth_headers.lock().unwrap().push(auth);
    *st.models_calls.lock().unwrap() += 1;
    Json(json!({
        "data": [{"id": "gpt-test"}, {"id": "gpt-mini"}, {"id": "gpt-large"}]
    }))
    .into_response()
}

async fn spawn_fake_ai(state: Arc<FakeAiState>) -> SocketAddr {
    let app = axum::Router::new()
        .route("/v1/chat/completions", post(fake_chat))
        .route("/v1/models", get(fake_models))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

// ---------- 数据辅助 ----------

async fn seed_online_account(app: &TestApp) {
    app.db
        .call(|conn| {
            let acct = qing_delivery::adapters::sqlite::repos::accounts::upsert_identity(
                conn, "acct-1", "xianyu", "seller-1", "测试店铺",
            )?;
            qing_delivery::adapters::sqlite::repos::accounts::set_control(
                conn,
                &acct.id,
                "online",
                Some(true),
                Some(true),
                Some(false),
                Some(1_600_000_000_000),
            )
        })
        .await
        .unwrap()
        .unwrap();
}

async fn seed_default_reply(app: &TestApp, text: &str) {
    let text = text.to_string();
    app.db
        .call(move |conn| {
            qing_delivery::adapters::sqlite::repos::rules_ext::upsert_default_reply(
                conn, "acct-1", true, Some(&text), None, true,
            )
        })
        .await
        .unwrap()
        .unwrap();
}

async fn count_rows(app: &TestApp, sql: &str) -> i64 {
    let sql = sql.to_string();
    app.db
        .call(move |conn| {
            Ok::<_, rusqlite::Error>(conn.query_row(&sql, [], |r| r.get::<_, i64>(0))?)
        })
        .await
        .unwrap()
        .unwrap()
}

/// 摄取一条买家文本消息(经应用 ChatService,与事件管道同路径)。
async fn ingest_buyer_text(app: &TestApp, pmid: &str, buyer: &str, text: &str) {
    app.chat
        .ingest(IncomingChatMessage {
            account_id: "acct-1".into(),
            platform_message_id: pmid.into(),
            chat_id: Some("chat-ai-1".into()),
            buyer_id: buyer.into(),
            msg_kind: ChatMsgKind::Text,
            text: Some(text.into()),
            image_url: None,
            item_id: Some("item-1".into()),
        })
        .await
        .unwrap();
}

/// 某买家会话中的出站文本(自动回复留痕行)。
async fn outgoing_texts(app: &TestApp, buyer: &str) -> Vec<(String, String)> {
    let buyer = buyer.to_string();
    app.db
        .call(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT m.body_text, m.status FROM chat_messages m
                     JOIN conversations c ON c.id = m.conversation_id
                     WHERE m.direction = 'out' AND c.peer_buyer_id = ?1",
                )
                .unwrap();
            let rows = stmt
                .query_map(rusqlite::params![buyer], |r| {
                    Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default()))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            Ok::<_, rusqlite::Error>(rows)
        })
        .await
        .unwrap()
        .unwrap()
}

/// 写入系统 AI 配置(经 HTTP 端点,与真实管理页同路径)。
async fn put_ai_settings(app: &TestApp, s: &Session, url: &str, key: &str, model: &str) {
    let (status, body, _) = send(
        app,
        req(
            "PUT",
            "/api/v1/settings/system",
            Some(
                &json!({"ai_api_url": url, "ai_model": model, "ai_api_key": key}).to_string(),
            ),
            &authed(s),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写入系统 AI 配置失败:{body:?}");
}

/// 开启账号 AI 自动回复并设置提示词。
async fn put_account_ai(app: &TestApp, s: &Session, enabled: bool, prompt: Option<&str>) -> (StatusCode, Value, String) {
    let mut body = json!({"ai_reply_enabled": enabled});
    if let Some(p) = prompt {
        body["ai_prompt"] = json!(p);
    }
    send(
        app,
        req(
            "PUT",
            "/api/v1/accounts/acct-1/ai-settings",
            Some(&body.to_string()),
            &authed(s),
        ),
    )
    .await
}

// ---------- 1/2) 改密(FR-072) ----------

/// 当前密码错误 → 422 credential_change_failed;原密码仍可登录。
#[tokio::test]
async fn change_credentials_rejects_wrong_current_password() {
    let app = spawn_app().await;
    let s = initialize(&app, INITIAL_PASSWORD).await;
    let (status, body, _) = send(
        &app,
        req(
            "PUT",
            "/api/v1/auth/credentials",
            Some(
                &json!({
                    "current_password": "totally-wrong-password",
                    "new_password": NEW_PASSWORD
                })
                .to_string(),
            ),
            &authed(&s),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"]["code"], "credential_change_failed");
    // 原密码未变:仍可登录
    assert!(login(&app, INITIAL_PASSWORD).await.is_ok());
}

/// 改密成功:其他会话失效、当前会话保留、新密码可登录、旧密码失效。
#[tokio::test]
async fn change_credentials_revokes_other_sessions_keeps_current() {
    let app = spawn_app().await;
    let current = initialize(&app, INITIAL_PASSWORD).await;
    let other = login(&app, INITIAL_PASSWORD).await.expect("第二会话登录");
    assert!(session_alive(&app, &other).await, "改密前两会话均有效");

    let (status, body, _) = send(
        &app,
        req(
            "PUT",
            "/api/v1/auth/credentials",
            Some(
                &json!({
                    "current_password": INITIAL_PASSWORD,
                    "new_username": "admin",
                    "new_password": NEW_PASSWORD
                })
                .to_string(),
            ),
            &authed(&current),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    // 其他会话:立即失效(旧 token 校验失败)
    assert!(
        !session_alive(&app, &other).await,
        "改密后其他会话必须全部失效"
    );
    // 当前会话:保留(CSRF 鉴权自动保留,变更仍可用)
    assert!(session_alive(&app, &current).await, "当前会话必须保留");
    let (status, body, _) = send(
        &app,
        req(
            "GET",
            "/api/v1/settings/system",
            None,
            &[("cookie", format!("qing_session={}", current.cookie))],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "当前会话仍可访问:{body:?}");

    // 新密码可登录、旧密码失效
    assert!(login(&app, NEW_PASSWORD).await.is_ok());
    assert_eq!(
        login(&app, INITIAL_PASSWORD).await.unwrap_err(),
        StatusCode::UNAUTHORIZED
    );
}

// ---------- 3) 系统设置读写与脱敏 ----------

#[tokio::test]
async fn system_settings_roundtrip_and_key_never_echoed() {
    let app = spawn_app().await;
    let s = initialize(&app, INITIAL_PASSWORD).await;

    // 初始态:未配置
    let (status, body, _) = send(
        &app,
        req("GET", "/api/v1/settings/system", None, &[(
            "cookie",
            format!("qing_session={}", s.cookie),
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["ai_api_url"], "");
    assert_eq!(body["ai_model"], "");
    assert_eq!(body["ai_key_configured"], false);

    // 非法地址 → 422
    let (status, body, _) = send(
        &app,
        req(
            "PUT",
            "/api/v1/settings/system",
            Some(&json!({"ai_api_url": "ftp://not-http"}).to_string()),
            &authed(&s),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["error"]["code"], "invalid_request");

    // 正常写入
    put_ai_settings(&app, &s, "https://ai.example.com/v1", "sk-secret-abcdef", "gpt-test").await;
    let (_, body, _) = send(
        &app,
        req("GET", "/api/v1/settings/system", None, &[(
            "cookie",
            format!("qing_session={}", s.cookie),
        )]),
    )
    .await;
    assert_eq!(body["ai_api_url"], "https://ai.example.com/v1");
    assert_eq!(body["ai_model"], "gpt-test");
    assert_eq!(body["ai_key_configured"], true);
    let raw = body.to_string();
    assert!(!raw.contains("sk-secret-abcdef"), "API Key 明文绝不回显");
}

// ---------- 4) 读取模型 / 测试连接 ----------

#[tokio::test]
async fn ai_models_and_test_hit_fake_openai_with_stored_key() {
    let app = spawn_app().await;
    let s = initialize(&app, INITIAL_PASSWORD).await;
    seed_online_account(&app).await;
    let state = FakeAiState::new("你好,我是测试助手的回复");
    let addr = spawn_fake_ai(state.clone()).await;
    put_ai_settings(&app, &s, &format!("http://{addr}/v1"), "sk-local-test-key", "gpt-test").await;

    // 读取模型:系统存 key(请求带 base_url 则优先)
    let (status, body, _) = send(
        &app,
        req(
            "POST",
            "/api/v1/settings/ai/models",
            Some(&json!({"base_url": format!("http://{addr}/v1")}).to_string()),
            &authed(&s),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let models: Vec<&str> = body["models"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(models.contains(&"gpt-test") && models.contains(&"gpt-mini") && models.len() == 3);

    // 测试连接:模型/耗时/回复摘要
    let (status, body, _) = send(
        &app,
        req("POST", "/api/v1/settings/ai/test", Some("{}"), &authed(&s)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["model"], "gpt-test");
    assert!(body["latency_ms"].as_u64().is_some());
    assert_eq!(body["reply"], "你好,我是测试助手的回复");

    // 假服务看到 Bearer 系统 key(明文只在内存/信封,不出现在响应)
    let auths = state.auth_headers.lock().unwrap().clone();
    assert!(auths.iter().all(|a| a == "Bearer sk-local-test-key"), "{auths:?}");
    assert_eq!(*state.models_calls.lock().unwrap(), 1);

    // 不可达地址(请求级 base_url 覆盖)→ 422 结构化原因
    let (status, body, _) = send(
        &app,
        req(
            "POST",
            "/api/v1/settings/ai/models",
            Some(&json!({"base_url": "http://127.0.0.1:1/unreachable"}).to_string()),
            &authed(&s),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["error"]["code"], "invalid_request");
    assert!(
        body["error"]["message"].as_str().unwrap().len() > 5,
        "错误需带结构化原因"
    );
}

// ---------- 5) AI 分流主路径(FR-071) ----------

#[tokio::test]
async fn ingest_without_keyword_hit_sends_ai_reply_with_account_prompt() {
    let app = spawn_app().await;
    let s = initialize(&app, INITIAL_PASSWORD).await;
    seed_online_account(&app).await;
    let state = FakeAiState::new("您好,这份资料包含近三年真题与解析。");
    let addr = spawn_fake_ai(state.clone()).await;
    put_ai_settings(&app, &s, &format!("http://{addr}/v1"), "sk-local-test-key", "gpt-test").await;
    let (status, body, _) = put_account_ai(&app, &s, true, Some("你是考研资料店铺的金牌客服。")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    // 不配置关键词与默认回复:未命中即 AI
    ingest_buyer_text(&app, "pm-ai-1", "buyer-ai-1", "请问这份资料都有哪些内容呀").await;

    // 出站行 = AI 文案(sent)
    let out = outgoing_texts(&app, "buyer-ai-1").await;
    assert_eq!(out.len(), 1, "AI 回复应发出一条:{out:?}");
    assert_eq!(out[0].0, "您好,这份资料包含近三年真题与解析。");
    assert_eq!(out[0].1, "sent");

    // 假服务收到一次 chat 请求:模型正确、system 注入账号提示词、user 为买家消息
    let calls = state.chat_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["model"], "gpt-test");
    let messages = calls[0]["messages"].as_array().unwrap();
    let system = messages.iter().find(|m| m["role"] == "system").expect("system 消息");
    assert!(system["content"].as_str().unwrap().contains("考研资料店铺"));
    let user = messages.iter().find(|m| m["role"] == "user").expect("user 消息");
    assert_eq!(user["content"], "请问这份资料都有哪些内容呀");

    // 宪章红线:订单/交付零写入
    assert_eq!(count_rows(&app, "SELECT COUNT(*) FROM orders").await, 0);
    assert_eq!(count_rows(&app, "SELECT COUNT(*) FROM deliveries").await, 0);
}

// ---------- 6) AI 故障静默降级 ----------

#[tokio::test]
async fn ai_upstream_failure_silently_falls_back_to_default_reply() {
    let app = spawn_app().await;
    let s = initialize(&app, INITIAL_PASSWORD).await;
    seed_online_account(&app).await;
    seed_default_reply(&app, "亲,稍后回复您").await;
    let state = FakeAiState::new("不应出现");
    state.fail_chat.store(true, Ordering::SeqCst);
    let addr = spawn_fake_ai(state.clone()).await;
    put_ai_settings(&app, &s, &format!("http://{addr}/v1"), "sk-local-test-key", "gpt-test").await;
    let (status, _, _) = put_account_ai(&app, &s, true, None).await;
    assert_eq!(status, StatusCode::OK);

    // 摄取不 panic、默认回复仍发送
    ingest_buyer_text(&app, "pm-deg-1", "buyer-deg-1", "在吗,有人吗").await;
    let out = outgoing_texts(&app, "buyer-deg-1").await;
    assert_eq!(out.len(), 1, "静默降级后默认回复应发送:{out:?}");
    assert_eq!(out[0].0, "亲,稍后回复您");
    assert_eq!(out[0].1, "sent");
    // AI 确实被尝试过(上游 500)
    assert_eq!(state.chat_calls().len(), 1);
    // 订单/交付零写入
    assert_eq!(count_rows(&app, "SELECT COUNT(*) FROM orders").await, 0);
    assert_eq!(count_rows(&app, "SELECT COUNT(*) FROM deliveries").await, 0);
}

// ---------- 7) 账号开关关闭 → 不调 AI ----------

#[tokio::test]
async fn ai_disabled_account_never_calls_ai_and_uses_default() {
    let app = spawn_app().await;
    let s = initialize(&app, INITIAL_PASSWORD).await;
    seed_online_account(&app).await;
    seed_default_reply(&app, "默认文案兜底").await;
    let state = FakeAiState::new("AI 不应被调用");
    let addr = spawn_fake_ai(state.clone()).await;
    put_ai_settings(&app, &s, &format!("http://{addr}/v1"), "sk-local-test-key", "gpt-test").await;
    // 账号开关保持默认 false(0006 迁移默认 0),显式写一次断言语义
    let (status, _, _) = put_account_ai(&app, &s, false, None).await;
    assert_eq!(status, StatusCode::OK);

    ingest_buyer_text(&app, "pm-off-1", "buyer-off-1", "随便聊聊").await;
    let out = outgoing_texts(&app, "buyer-off-1").await;
    assert_eq!(out.len(), 1, "默认回复兜底:{out:?}");
    assert_eq!(out[0].0, "默认文案兜底");
    // 关键证据:AI 零请求
    assert!(state.chat_calls().is_empty(), "ai_reply_enabled=false 不得调用 AI");
    assert_eq!(count_rows(&app, "SELECT COUNT(*) FROM orders").await, 0);
}

// ---------- 8) 账号 AI 设置落库回读 ----------

#[tokio::test]
async fn account_ai_settings_put_persists_and_validates() {
    let app = spawn_app().await;
    let s = initialize(&app, INITIAL_PASSWORD).await;
    seed_online_account(&app).await;

    // 落库 + 回读(响应回显写入值)
    let (status, body, _) = put_account_ai(&app, &s, true, Some("定制提示词")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["ai_reply_enabled"], true);
    assert_eq!(body["ai_prompt"], "定制提示词");
    let row: (i64, Option<String>) = app
        .db
        .call(|conn| {
            Ok::<_, rusqlite::Error>(conn.query_row(
                "SELECT ai_reply_enabled, ai_prompt FROM accounts WHERE id = 'acct-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row, (1, Some("定制提示词".to_string())));

    // prompt 超长 → 422
    let long_prompt = "长".repeat(2001);
    let (status, body, _) = put_account_ai(&app, &s, true, Some(&long_prompt)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["error"]["code"], "invalid_request");

    // prompt 省略 = 不修改
    let (status, _, _) = put_account_ai(&app, &s, false, None).await;
    assert_eq!(status, StatusCode::OK);
    let prompt: Option<String> = app
        .db
        .call(|conn| {
            Ok::<_, rusqlite::Error>(conn.query_row(
                "SELECT ai_prompt FROM accounts WHERE id = 'acct-1'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prompt.as_deref(), Some("定制提示词"), "省略 prompt 保留原值");

    // 账号不存在 → 404
    let (status, body, _) = send(
        &app,
        req(
            "PUT",
            "/api/v1/accounts/acct-nope/ai-settings",
            Some(&json!({"ai_reply_enabled": true}).to_string()),
            &authed(&s),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
}
