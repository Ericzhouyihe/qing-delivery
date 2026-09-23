//! 业务路由:/health、/api/v1/auth/*、/api/v1/capabilities、/api/v1/jobs/*。

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::application::auth::{AuthError, IssuedSession, SessionIdentity};
use crate::application::jobs::JobState;
use crate::domain::time_util::format_rfc3339;
use crate::transport::error::{ApiError, ErrorCode};
use crate::transport::middleware::{host_origin_middleware, request_id_middleware};
use crate::transport::state::{AppState, ExecutionProfile, SharedState};

pub const SESSION_COOKIE: &str = "qing_session";

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/auth/session", get(session_handler))
        .route("/auth/initialize", post(initialize_handler))
        .route("/auth/login", post(login_handler))
        .route("/auth/logout", post(logout_handler))
        .route("/capabilities", get(capabilities_handler))
        .route("/jobs/{job_id}", get(job_handler))
        .route("/jobs/{job_id}/cancel", post(job_cancel_handler))
        .route("/orders", get(crate::transport::orders_api::list_orders))
        .route(
            "/accounts/{account_id}/orders/{order_id}",
            get(crate::transport::orders_api::order_detail),
        )
        .route(
            "/accounts",
            get(crate::transport::accounts_api::list_accounts),
        )
        .route(
            "/accounts/{account_id}",
            get(crate::transport::accounts_api::get_account),
        )
        .route(
            "/accounts/{account_id}/control",
            post(crate::transport::accounts_api::control_account),
        )
        .route(
            "/accounts/{account_id}/verification",
            post(crate::transport::accounts_api::start_verification),
        )
        .route(
            "/accounts/qr-sessions",
            post(crate::transport::accounts_api::create_qr_session),
        )
        .route(
            "/accounts/qr-sessions/{qr_id}",
            get(crate::transport::accounts_api::get_qr_session),
        )
        .route(
            "/accounts/qr-sessions/{qr_id}/image",
            get(crate::transport::accounts_api::get_qr_image),
        )
        .route(
            "/accounts/qr-sessions/{qr_id}/cancel",
            post(crate::transport::accounts_api::cancel_qr_session),
        )
        .route(
            "/accounts/{account_id}/items",
            get(crate::transport::catalog_api::list_items),
        )
        .route(
            "/accounts/{account_id}/item-syncs",
            post(crate::transport::catalog_api::start_item_sync),
        )
        .route(
            "/accounts/{account_id}/rules",
            get(crate::transport::catalog_api::list_rules),
        )
        .route(
            "/accounts/{account_id}/rules",
            post(crate::transport::catalog_api::create_rule),
        )
        .route(
            "/accounts/{account_id}/rules/preview",
            post(crate::transport::catalog_api::preview_rule),
        )
        .route(
            "/accounts/{account_id}/rules/match-preview",
            post(crate::transport::catalog_api::match_preview_rule),
        )
        .route(
            "/accounts/{account_id}/rules/{rule_id}",
            put(crate::transport::catalog_api::update_rule),
        )
        .route("/issues", get(crate::transport::manual_api::list_issues))
        .route(
            "/dashboard",
            get(crate::transport::dashboard_api::dashboard),
        )
        .route(
            "/restore",
            get(crate::transport::restore_api::restore_summary),
        )
        .route(
            "/restore/reviews",
            get(crate::transport::restore_api::list_reviews),
        )
        .route(
            "/restore/reviews/{review_id}/decisions",
            post(crate::transport::restore_api::decide),
        )
        .route(
            "/accounts/{account_id}/orders/{order_id}/content",
            post(crate::transport::dashboard_api::reveal_content),
        )
        .route(
            "/accounts/{account_id}/orders/{order_id}/resends",
            post(crate::transport::manual_api::resend),
        )
        .route(
            "/accounts/{account_id}/orders/{order_id}/mark-received",
            post(crate::transport::manual_api::mark_received),
        )
        .route(
            "/accounts/{account_id}/orders/{order_id}/terminate",
            post(crate::transport::manual_api::terminate),
        )
        .route(
            "/accounts/{account_id}/orders/{order_id}/takeovers",
            post(crate::transport::manual_api::takeover),
        );
    #[cfg(feature = "dev-fixtures")]
    let api = api
        .route("/dev/scenarios", get(crate::transport::dev::list_scenarios))
        .route(
            "/dev/scenarios/{scenario}/runs",
            post(crate::transport::dev::run_scenario),
        );
    #[cfg(not(feature = "dev-fixtures"))]
    let api = api;
    Router::new()
        .route("/health", get(health_handler))
        .nest("/api/v1", api.with_state(state.clone()))
        .fallback(crate::transport::webui::serve_static)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            host_origin_middleware,
        ))
        .layer(axum::middleware::from_fn(request_id_middleware))
        .with_state(state)
}

// ---- 供 dev 场景端点复用的公共辅助 ----

pub async fn require_admin_public(
    state: &AppState,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<crate::application::auth::SessionIdentity, Response> {
    require_admin(state, headers, mutation)
        .await
        .map_err(|e| e.into_response())
}

pub fn job_summary_dto_public(row: &crate::application::jobs::JobRow) -> serde_json::Value {
    serde_json::to_value(job_summary_dto(row)).unwrap_or_default()
}

pub fn not_found_public(msg: &str) -> Response {
    err(ErrorCode::ResourceNotFound, msg).into_response()
}

// ---------- DTO ----------

#[derive(serde::Serialize)]
struct SessionResponse {
    initialized: bool,
    authenticated: bool,
    csrf_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    administrator: Option<AdministratorSummary>,
}

#[derive(serde::Serialize)]
struct AdministratorSummary {
    id: String,
    display_name: String,
}

#[derive(Deserialize)]
struct InitializeRequest {
    password: String,
    password_confirmation: String,
}

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

#[derive(serde::Serialize)]
struct JobSummaryDto {
    id: String,
    kind: String,
    state: String,
    version: i64,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_id: Option<String>,
    // 002 商品同步结果摘要/安全错误(不含凭证等敏感信息);旧消费者忽略
    #[serde(skip_serializing_if = "Option::is_none")]
    result_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    safe_error: Option<String>,
}

#[derive(serde::Serialize)]
struct AcceptedOperationDto {
    operation_id: String,
    job: JobSummaryDto,
}

#[derive(Deserialize)]
struct CancelJobRequest {
    expected_version: i64,
    #[allow(dead_code)]
    reason: String,
}

// ---------- 辅助 ----------

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let mut it = pair.trim().splitn(2, '=');
        if it.next() == Some(name) {
            return it.next().map(|v| v.to_string());
        }
    }
    None
}

fn csrf_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn err(code: ErrorCode, msg: impl Into<String>) -> ApiError {
    let request_id = crate::domain::ids::new_request_key();
    ApiError::new(code, msg, request_id)
}

fn from_auth_error(e: AuthError) -> ApiError {
    match e {
        AuthError::AlreadyInitialized => err(ErrorCode::AlreadyInitialized, "管理员已初始化"),
        AuthError::PasswordTooShort => err(ErrorCode::InvalidRequest, "密码至少 12 个字符"),
        AuthError::PasswordMismatch => err(ErrorCode::InvalidRequest, "两次输入的密码不一致"),
        AuthError::InvalidCredentials | AuthError::NotInitialized => {
            err(ErrorCode::InvalidCredentials, "认证失败")
        }
        AuthError::SessionInvalid => err(ErrorCode::AuthenticationRequired, "需要管理员登录"),
        AuthError::CsrfRejected => err(ErrorCode::CsrfRejected, "CSRF 校验失败"),
        AuthError::Db(_) | AuthError::Hash(_) | AuthError::Sqlite(_) => {
            err(ErrorCode::PersistenceUnavailable, "服务暂不可用")
        }
    }
}

/// 要求有效管理员会话;变更方法同时校验 CSRF。
async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<SessionIdentity, ApiError> {
    let Some(token) = cookie_value(headers, SESSION_COOKIE) else {
        return Err(err(ErrorCode::AuthenticationRequired, "需要管理员登录"));
    };
    if mutation {
        // 变更请求必须携带 CSRF 头:缺失即拒绝,不得静默跳过校验
        let Some(csrf) = csrf_header(headers) else {
            return Err(err(ErrorCode::CsrfRejected, "缺少 X-CSRF-Token"));
        };
        return state
            .inner
            .auth
            .validate(&token, Some(&csrf))
            .await
            .map_err(from_auth_error);
    }
    state
        .inner
        .auth
        .validate(&token, None)
        .await
        .map_err(from_auth_error)
}

/// 匿名预会话 CSRF:初始化/登录前必须先 GET /auth/session 获取。
fn require_anonymous_csrf(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(csrf) = csrf_header(headers) else {
        return Err(err(ErrorCode::CsrfRejected, "缺少 X-CSRF-Token"));
    };
    if state.inner.anonymous_csrf.consume(&csrf) {
        Ok(())
    } else {
        Err(err(ErrorCode::CsrfRejected, "CSRF 校验失败"))
    }
}

fn session_cookie_header(token: &str, max_age_secs: i64) -> String {
    // 本版本机 HTTP 不依赖 Secure Cookie(契约 §1);HttpOnly+SameSite=Strict+Path=/
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_secs}")
}

fn clear_cookie_header() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
}

fn rate_limited() -> Response {
    let mut resp = err(ErrorCode::RateLimited, "尝试过于频繁,请稍后再试").into_response();
    resp.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    resp
}

fn issued_session_response(status: StatusCode, issued: &IssuedSession) -> Response {
    let body = SessionResponse {
        initialized: true,
        authenticated: true,
        csrf_token: issued.csrf_token.clone(),
        administrator: Some(AdministratorSummary {
            id: issued.admin_id.clone(),
            display_name: "管理员".into(),
        }),
    };
    let mut resp = (status, Json(body)).into_response();
    let max_age = (issued.expires_at_ms - crate::domain::time_util::utc_now_ms()) / 1000;
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie_header(&issued.cookie_token, max_age)).unwrap(),
    );
    resp
}

// ---------- handlers ----------

async fn health_handler(State(state): SharedState) -> Response {
    let stopping = state.stopping();
    let status = if stopping { "degraded" } else { "ok" };
    let code = if stopping {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (
        code,
        Json(json!({ "status": status, "version": env!("CARGO_PKG_VERSION") })),
    )
        .into_response()
}

async fn session_handler(State(state): SharedState, headers: HeaderMap) -> Response {
    let initialized = match state.inner.auth.is_initialized().await {
        Ok(v) => v,
        Err(e) => return from_auth_error(e).into_response(),
    };
    if let Some(token) = cookie_value(&headers, SESSION_COOKIE) {
        match state.inner.auth.validate(&token, None).await {
            Ok(identity) => {
                // CSRF 只存哈希:每次会话查询轮换新 token 交给当前页面
                let csrf = match state.inner.auth.rotate_csrf(&token).await {
                    Ok(c) => c,
                    Err(e) => return from_auth_error(e).into_response(),
                };
                return Json(SessionResponse {
                    initialized,
                    authenticated: true,
                    csrf_token: csrf,
                    administrator: Some(AdministratorSummary {
                        id: identity.admin_id,
                        display_name: "管理员".into(),
                    }),
                })
                .into_response();
            }
            Err(AuthError::SessionInvalid) => {}
            Err(e) => return from_auth_error(e).into_response(),
        }
    }
    Json(SessionResponse {
        initialized,
        authenticated: false,
        csrf_token: state.inner.anonymous_csrf.mint(),
        administrator: None,
    })
    .into_response()
}

/// 登录/初始化限流键:本工具仅本机回环单管理员,全局键即等价于按对端。
const LOGIN_LIMIT_KEY: &str = "local";

async fn initialize_handler(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<InitializeRequest>,
) -> Response {
    if state.inner.login_limiter.is_limited(LOGIN_LIMIT_KEY) {
        return rate_limited();
    }
    if let Err(e) = require_anonymous_csrf(&state, &headers) {
        return e.into_response();
    }
    match state
        .inner
        .auth
        .initialize(&body.password, &body.password_confirmation)
        .await
    {
        Ok(issued) => issued_session_response(StatusCode::CREATED, &issued).into_response(),
        Err(e) => {
            if matches!(e, AuthError::PasswordTooShort | AuthError::PasswordMismatch) {
                state.inner.login_limiter.record_failure(LOGIN_LIMIT_KEY);
            }
            from_auth_error(e).into_response()
        }
    }
}

async fn login_handler(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Response {
    if state.inner.login_limiter.is_limited(LOGIN_LIMIT_KEY) {
        return rate_limited();
    }
    if let Err(e) = require_anonymous_csrf(&state, &headers) {
        return e.into_response();
    }
    match state.inner.auth.login(&body.password).await {
        Ok(issued) => issued_session_response(StatusCode::OK, &issued).into_response(),
        Err(e) => {
            if matches!(e, AuthError::InvalidCredentials | AuthError::NotInitialized) {
                state.inner.login_limiter.record_failure(LOGIN_LIMIT_KEY);
            }
            from_auth_error(e).into_response()
        }
    }
}

async fn logout_handler(State(state): SharedState, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_value(&headers, SESSION_COOKIE) {
        let _ = state.inner.auth.logout(&token).await;
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&clear_cookie_header()).unwrap(),
    );
    resp
}

async fn capabilities_handler(State(state): SharedState, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin(&state, &headers, false).await {
        return e.into_response();
    }
    let profile = match state.inner.config.profile {
        ExecutionProfile::Live => "live",
        ExecutionProfile::Mock => "mock",
    };
    Json(json!({
        "platforms": [
            { "platform": "xianyu", "supported": true },
            { "platform": "taobao_qianniu", "supported": false, "reason": "首版不支持,后续验证接入" }
        ],
        "content_sources": [
            { "kind": "fixed_text", "supported": true },
            { "kind": "card_key", "supported": false, "reason": "首版不支持卡密" }
        ],
        "content_limits": {
            "unicode_scalars": 1000,
            "utf8_bytes": 4000,
            "effective_source": "product"
        },
        "execution_profile": profile
    }))
    .into_response()
}

fn parse_job_state(s: &str) -> JobState {
    match s {
        "queued" => JobState::Queued,
        "running" => JobState::Running,
        "cancel_requested" => JobState::CancelRequested,
        "succeeded" => JobState::Succeeded,
        "failed" => JobState::Failed,
        "cancelled" => JobState::Cancelled,
        _ => JobState::NeedsReview,
    }
}

fn job_summary_dto(row: &crate::application::jobs::JobRow) -> JobSummaryDto {
    JobSummaryDto {
        id: row.id.clone(),
        kind: row.kind.clone(),
        state: row.state.clone(),
        version: row.version,
        created_at: format_rfc3339(row.created_at),
        updated_at: format_rfc3339(row.updated_at),
        target_id: row.target_id.clone(),
        result_ref: row.result_ref.clone(),
        safe_error: row.safe_error.clone(),
    }
}

async fn job_handler(
    State(state): SharedState,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin(&state, &headers, false).await {
        return e.into_response();
    }
    match state.inner.jobs.get(&job_id).await {
        Ok(row) => Json(json!({
            "summary": job_summary_dto(&row),
            "stage": row.stage,
            "cancel_allowed": !parse_job_state(&row.state).is_terminal(),
        }))
        .into_response(),
        Err(_) => err(ErrorCode::ResourceNotFound, "任务不存在").into_response(),
    }
}

async fn job_cancel_handler(
    State(state): SharedState,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(body): Json<CancelJobRequest>,
) -> Response {
    if let Err(e) = require_admin(&state, &headers, true).await {
        return e.into_response();
    }
    let row = match state.inner.jobs.get(&job_id).await {
        Ok(r) => r,
        Err(_) => return err(ErrorCode::ResourceNotFound, "任务不存在").into_response(),
    };
    if parse_job_state(&row.state).is_terminal() {
        return err(ErrorCode::ActionInProgress, "任务已处于终态,不能取消").into_response();
    }
    match state
        .inner
        .jobs
        .request_cancel(&job_id, body.expected_version)
        .await
    {
        Ok(updated) => {
            let op = AcceptedOperationDto {
                operation_id: crate::domain::ids::new_id("op"),
                job: job_summary_dto(&updated),
            };
            (StatusCode::ACCEPTED, Json(op)).into_response()
        }
        Err(crate::application::jobs::JobError::VersionConflict) => {
            err(ErrorCode::VersionConflict, "版本冲突,请刷新后重试").into_response()
        }
        Err(_) => err(ErrorCode::PersistenceUnavailable, "任务状态不可用").into_response(),
    }
}

/// 共享错误构造(供 orders_api 等模块使用)。
pub fn err_shared(code: ErrorCode, msg: &str) -> axum::response::Response {
    err(code, msg).into_response()
}
