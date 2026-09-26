//! 007 US7 系统与 AI HTTP API(T078,contracts §7):
//! - GET/PUT /settings/system:AI 地址/模型明文读写,Key 只回 *_configured
//!   (留空=不修改);smtp 部分由既有 /settings/system-smtp 端点承担,不重复;
//! - POST /settings/ai/models、POST /settings/ai/test:拉取模型列表与测试连接
//!   (请求带 base_url 则优先;AI 上游失败 → 422 invalid_request 带结构化原因
//!   ——无 502/504 码可用,选择 422 与契约"结构化原因"近似);
//! - PUT /auth/credentials:改密后除当前会话(CSRF 鉴权自动保留)外全部失效;
//! - PUT /accounts/{id}/ai-settings:账号 AI 开关+提示词(≤2000 字)。
//! 薄 handler:鉴权 + DTO 解析 + 调用应用服务(宪章 II);
//! API Key 明文绝不进入响应/日志/错误信封。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::adapters::aiclient::{AiConfig, AiHttpClient};
use crate::application::auth::AuthError;
use crate::application::settings_sys::{SettingsError, SettingsService};
use crate::transport::error::ErrorCode;
use crate::transport::routes::{SESSION_COOKIE, err_shared, require_admin_public, session_token_from_headers};
use crate::transport::state::{AppState, SharedState};

fn settings_service(state: &AppState) -> SettingsService {
    // 服务无状态:经 DbThread + 数据密钥即取即用(与通知 API 同模式)
    SettingsService::new(state.inner.db.clone(), state.inner.key.clone())
}

fn settings_err(e: SettingsError) -> Response {
    match e {
        SettingsError::Invalid(_) => err_shared(ErrorCode::InvalidRequest, &e.to_string()),
        SettingsError::Crypto => err_shared(ErrorCode::PersistenceUnavailable, "系统机密不可用"),
        SettingsError::Db(_) | SettingsError::Sqlite(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "系统设置不可用")
        }
    }
}

/// AI 上游失败:无 502/504 错误码可用 → 422 invalid_request 带脱敏结构化原因。
fn ai_err(e: crate::adapters::aiclient::AiError) -> Response {
    err_shared(ErrorCode::InvalidRequest, &e.to_string())
}

// ---------- 系统设置(contracts §7) ----------

/// GET /settings/system → {ai_api_url, ai_model, ai_key_configured}
pub async fn get_system(State(state): SharedState, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    match settings_service(&state).get_system().await {
        Ok(summary) => Json(json!({
            "ai_api_url": summary.ai_api_url,
            "ai_model": summary.ai_model,
            "ai_key_configured": summary.ai_key_configured,
        }))
        .into_response(),
        Err(e) => settings_err(e),
    }
}

#[derive(Deserialize)]
pub struct SystemWriteRequest {
    /// None = 不修改;空串 = 清除
    pub ai_api_url: Option<String>,
    pub ai_model: Option<String>,
    /// None/空串 = 不修改(脱敏编辑模式);非空 = 信封重封
    pub ai_api_key: Option<String>,
}

/// PUT /settings/system(Key 留空=不修改;非法地址/超长 422)
pub async fn put_system(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<SystemWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match settings_service(&state)
        .put_system(
            body.ai_api_url.as_deref(),
            body.ai_model.as_deref(),
            body.ai_api_key.as_deref(),
        )
        .await
    {
        Ok(summary) => Json(json!({
            "ai_api_url": summary.ai_api_url,
            "ai_model": summary.ai_model,
            "ai_key_configured": summary.ai_key_configured,
        }))
        .into_response(),
        Err(e) => settings_err(e),
    }
}

// ---------- 模型列表 / 测试连接 ----------

#[derive(Deserialize)]
pub struct AiProbeRequest {
    /// 请求级地址覆盖(优先于系统存地址;如"读取模型"前先换地址试探)
    pub base_url: Option<String>,
}

/// 装配探测配置:系统存 Key + 地址(请求 base_url 优先);model 可空(models 不用)。
async fn probe_config(
    state: &AppState,
    base_url_override: Option<&str>,
) -> Result<AiConfig, Response> {
    let svc = settings_service(state);
    let summary = svc.get_system().await.map_err(settings_err)?;
    let override_url = base_url_override
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    let url = override_url.unwrap_or_else(|| summary.ai_api_url.trim().to_string());
    if url.is_empty() {
        return Err(err_shared(ErrorCode::InvalidRequest, "尚未配置 AI 服务地址"));
    }
    if !summary.ai_key_configured {
        return Err(err_shared(ErrorCode::InvalidRequest, "尚未配置 API Key"));
    }
    let key = svc
        .read_ai_api_key()
        .await
        .map_err(settings_err)?
        .unwrap_or_default();
    Ok(AiConfig {
        api_url: url,
        api_key: key,
        model: summary.ai_model.trim().to_string(),
    })
}

/// POST /settings/ai/models {base_url?} → {models: [...]}
pub async fn ai_models(
    State(state): SharedState,
    headers: HeaderMap,
    body: Option<Json<AiProbeRequest>>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let base_url = body.and_then(|Json(b)| b.base_url);
    let cfg = match probe_config(&state, base_url.as_deref()).await {
        Ok(cfg) => cfg,
        Err(resp) => return resp,
    };
    match AiHttpClient::new().models(&cfg).await {
        Ok(models) => Json(json!({ "models": models })).into_response(),
        Err(e) => ai_err(e),
    }
}

/// POST /settings/ai/test {base_url?} → {model, latency_ms, reply}
pub async fn ai_test(
    State(state): SharedState,
    headers: HeaderMap,
    body: Option<Json<AiProbeRequest>>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let base_url = body.and_then(|Json(b)| b.base_url);
    let cfg = match probe_config(&state, base_url.as_deref()).await {
        Ok(cfg) => cfg,
        Err(resp) => return resp,
    };
    if cfg.model.is_empty() {
        return err_shared(ErrorCode::InvalidRequest, "尚未配置模型名");
    }
    match AiHttpClient::new().test(&cfg).await {
        Ok(report) => Json(json!({
            "model": report.model,
            "latency_ms": report.latency_ms,
            "reply": report.reply,
        }))
        .into_response(),
        Err(e) => ai_err(e),
    }
}

// ---------- 管理员凭据修改(FR-072) ----------

#[derive(Deserialize)]
pub struct CredentialsRequest {
    pub current_password: String,
    pub new_username: Option<String>,
    pub new_password: Option<String>,
}

/// PUT /auth/credentials:成功后除当前会话(CSRF 鉴权)外其他会话全部失效。
pub async fn put_credentials(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<CredentialsRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let has_username = body
        .new_username
        .as_deref()
        .is_some_and(|u| !u.trim().is_empty());
    let has_password = body
        .new_password
        .as_deref()
        .is_some_and(|p| !p.trim().is_empty());
    if !has_username && !has_password {
        return err_shared(
            ErrorCode::InvalidRequest,
            "至少提供新用户名或新密码之一",
        );
    }
    // 当前会话 token:改密后唯一保留的会话
    let Some(token) = session_token_from_headers(&headers, SESSION_COOKIE) else {
        return err_shared(ErrorCode::AuthenticationRequired, "需要管理员登录");
    };
    match state
        .inner
        .auth
        .change_credentials(
            &body.current_password,
            body.new_username.as_deref(),
            body.new_password.as_deref(),
            &token,
        )
        .await
    {
        Ok(()) => Json(json!({
            "message": "凭据已更新,除当前会话外其他会话已全部失效"
        }))
        .into_response(),
        Err(e) => match e {
            AuthError::CredentialChangeFailed => err_shared(
                ErrorCode::CredentialChangeFailed,
                "当前密码错误或没有可变更项",
            ),
            AuthError::PasswordTooShort => {
                err_shared(ErrorCode::InvalidRequest, "密码至少 12 个字符")
            }
            AuthError::NotInitialized => {
                err_shared(ErrorCode::InvalidCredentials, "认证失败")
            }
            AuthError::Db(_) | AuthError::Hash(_) | AuthError::Sqlite(_) => {
                err_shared(ErrorCode::PersistenceUnavailable, "服务暂不可用")
            }
            other => err_shared(ErrorCode::PersistenceUnavailable, &other.to_string()),
        },
    }
}

// ---------- 账号 AI 设置(FR-071) ----------

/// 提示词长度上限(与 AI 回复输出截断同量级)。
const AI_PROMPT_MAX_CHARS: usize = 2000;

#[derive(Deserialize)]
pub struct AccountAiSettingsRequest {
    pub ai_reply_enabled: bool,
    /// 省略/None = 不修改;空串 = 清除;非空 = 写入
    pub ai_prompt: Option<String>,
}

/// PUT /accounts/{account_id}/ai-settings(开关+提示词落库;prompt ≤2000)
pub async fn put_account_ai_settings(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<AccountAiSettingsRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    if let Some(prompt) = body.ai_prompt.as_deref()
        && prompt.chars().count() > AI_PROMPT_MAX_CHARS
    {
        return err_shared(
            ErrorCode::InvalidRequest,
            "提示词不能超过 2000 字",
        );
    }
    // prompt 语义解析在闭包外(读库取现值:省略=保留)
    let result = state
        .inner
        .db
        .call({
            let account_id = account_id.clone();
            let enabled = body.ai_reply_enabled;
            let prompt = body.ai_prompt.clone();
            move |conn| -> rusqlite::Result<Option<Option<String>>> {
                let existing = crate::adapters::sqlite::repos::accounts::get_ai_settings(
                    conn, &account_id,
                )?;
                let Some((_, current_prompt)) = existing else {
                    return Ok(None);
                };
                let effective = match prompt {
                    None => current_prompt,
                    Some(p) if p.is_empty() => None,
                    Some(p) => Some(p),
                };
                crate::adapters::sqlite::repos::accounts::set_ai_settings(
                    conn,
                    &account_id,
                    enabled,
                    effective.as_deref(),
                )?;
                Ok(Some(effective))
            }
        })
        .await;
    match result {
        Ok(Ok(Some(effective_prompt))) => Json(json!({
            "account_id": account_id,
            "ai_reply_enabled": body.ai_reply_enabled,
            "ai_prompt": effective_prompt,
        }))
        .into_response(),
        Ok(Ok(None)) => err_shared(ErrorCode::ResourceNotFound, "账号不存在"),
        Ok(Err(_)) | Err(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "账号设置不可用")
        }
    }
}
