//! 通知渠道 HTTP API(T063,contracts §5 全部端点):
//! 渠道 CRUD(创建/详情/更新/删除;secrets_configured 脱敏模式,永不回显
//! 明文)、测试投递(即时发送一次,返回 {state,error_hint})、账号绑定
//! 覆盖式 GET/PUT、系统 SMTP GET/PUT(密码只回 password_configured)。
//! 薄 handler:鉴权 + DTO 解析 + 调用应用服务(宪章 II);
//! 校验失败沿用 invalid_request / 409 version_conflict 错误口径。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

use crate::application::notify::{
    ChannelDraft, NotifyError, NotifyService, SmtpConfig,
};
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::{AppState, SharedState};

fn service(state: &AppState) -> NotifyService {
    // 服务无状态:经 DbThread + 数据密钥即取即用(与模板/卡密 API 同模式)
    NotifyService::new(state.inner.db.clone(), state.inner.key.clone())
}

fn notify_err(e: NotifyError) -> Response {
    match e {
        NotifyError::Invalid(_) => err_shared(ErrorCode::InvalidRequest, &e.to_string()),
        NotifyError::NotFound => err_shared(ErrorCode::ResourceNotFound, &e.to_string()),
        NotifyError::VersionConflict => err_shared(ErrorCode::VersionConflict, &e.to_string()),
        NotifyError::Unavailable(_)
        | NotifyError::Db(_)
        | NotifyError::Sqlite(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "通知服务不可用")
        }
    }
}

// ---------- 渠道 CRUD ----------

/// GET /notification-channels(列表;secrets_configured 脱敏)
pub async fn list_channels(State(state): SharedState, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    match service(&state).list_channels().await {
        Ok(items) => Json(json!({
            "items": items,
            "next_cursor": null,
        }))
        .into_response(),
        Err(e) => notify_err(e),
    }
}

#[derive(Deserialize)]
pub struct ChannelWriteRequest {
    kind: String,
    name: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    config: serde_json::Value,
    /// 缺省/空对象 = 无秘密(创建)或不修改(编辑)
    #[serde(default)]
    secrets: BTreeMap<String, String>,
    #[serde(default)]
    event_types: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn draft_of(body: &ChannelWriteRequest) -> ChannelDraft {
    ChannelDraft {
        kind: body.kind.clone(),
        name: body.name.clone(),
        enabled: body.enabled,
        config: body.config.clone(),
        secrets: body.secrets.clone(),
        event_types: body.event_types.clone(),
    }
}

/// POST /notification-channels(201;kind/config/secrets/event_types 校验 422)
pub async fn create_channel(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<ChannelWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state).create_channel(draft_of(&body)).await {
        Ok(summary) => (StatusCode::CREATED, Json(summary)).into_response(),
        Err(e) => notify_err(e),
    }
}

/// GET /notification-channels/{id}(详情)
pub async fn get_channel(
    State(state): SharedState,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    match service(&state).get_channel(&channel_id).await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => notify_err(e),
    }
}

#[derive(Deserialize)]
pub struct ChannelUpdateRequest {
    #[serde(flatten)]
    channel: ChannelWriteRequest,
    expected_version: i64,
}

/// PUT /notification-channels/{id}(secrets 留空=不修改;409 version_conflict)
pub async fn update_channel(
    State(state): SharedState,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(body): Json<ChannelUpdateRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state)
        .update_channel(&channel_id, body.expected_version, draft_of(&body.channel))
        .await
    {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => notify_err(e),
    }
}

/// DELETE /notification-channels/{id}(204;绑定经 FK CASCADE 联清)
pub async fn delete_channel(
    State(state): SharedState,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state).delete_channel(&channel_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err_shared(ErrorCode::ResourceNotFound, "通知渠道不存在"),
        Err(e) => notify_err(e),
    }
}

// ---------- 测试投递 ----------

/// POST /notification-channels/{id}/test
/// 即时发送一次测试事件 → {state: accepted|not_sent|unknown, error_hint?}
pub async fn test_channel(
    State(state): SharedState,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state).test_send(&channel_id).await {
        Ok(send_state) => {
            use crate::adapters::notify::SendState;
            let body = match &send_state {
                SendState::Accepted => json!({ "state": "accepted" }),
                SendState::NotSent(hint) => json!({ "state": "not_sent", "error_hint": hint }),
                SendState::Unknown(hint) => json!({ "state": "unknown", "error_hint": hint }),
            };
            Json(body).into_response()
        }
        Err(e) => notify_err(e),
    }
}

// ---------- 账号绑定(FR-052 覆盖式) ----------

/// GET /accounts/{account_id}/notification-bindings → {channel_ids: []}
pub async fn get_bindings(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    match service(&state).get_bindings(&account_id).await {
        Ok(ids) => Json(json!({ "channel_ids": ids })).into_response(),
        Err(e) => notify_err(e),
    }
}

#[derive(Deserialize)]
pub struct BindingsRequest {
    channel_ids: Vec<String>,
}

/// PUT /accounts/{account_id}/notification-bindings(覆盖式)
pub async fn put_bindings(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<BindingsRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state)
        .put_bindings(&account_id, &body.channel_ids)
        .await
    {
        Ok(()) => Json(json!({ "channel_ids": body.channel_ids })).into_response(),
        Err(e) => notify_err(e),
    }
}

// ---------- 系统 SMTP(FR-053;密码脱敏) ----------

/// GET /settings/system-smtp(密码只回 password_configured)
pub async fn get_system_smtp(State(state): SharedState, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    match service(&state).get_system_smtp().await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => notify_err(e),
    }
}

#[derive(Deserialize)]
pub struct SmtpWriteRequest {
    host: String,
    port: u16,
    encryption: String,
    from_address: String,
    from_name: Option<String>,
    username: Option<String>,
    /// 缺省/空 = 不修改(脱敏编辑模式)
    password: Option<String>,
}

/// PUT /settings/system-smtp(密码留空=不修改;422 校验)
pub async fn put_system_smtp(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<SmtpWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let cfg = SmtpConfig {
        host: body.host,
        port: body.port,
        encryption: body.encryption,
        from_address: body.from_address,
        from_name: body.from_name,
        username: body.username,
        password: body.password,
    };
    match service(&state).put_system_smtp(&cfg).await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => notify_err(e),
    }
}
