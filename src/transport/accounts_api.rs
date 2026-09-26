//! 账号与授权 HTTP API(T042/T049):QR 会话、账号列表/详情、控制开关(202)。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::adapters::sqlite::repos::accounts as repo;
use crate::application::accounts::qr::QrFlowError;
use crate::domain::accounts::AccountStatus;
use crate::domain::time_util::format_rfc3339;
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

#[derive(Deserialize)]
pub struct AccountListQuery {
    pub limit: Option<i64>,
}

fn account_summary(row: &repo::AccountRow) -> serde_json::Value {
    json!({
        "id": row.id,
        "platform": row.platform,
        "platform_user_id": row.external_user_id,
        "display_name": row.display_name,
        "avatar_url": row.avatar_url,
        "remark": row.remark,
        "connection_state": AccountStatus::parse(&row.status).dto_str(),
        "control": {
            "run_enabled": row.runtime_enabled,
            "auto_delivery_enabled": row.auto_delivery_enabled,
            "auto_confirm_enabled": row.auto_confirm_enabled,
            "transition": "stable",
        },
        "control_version": row.version,
        "monitoring_since": row.monitor_since.map(format_rfc3339),
        // 007 US7:账号级 AI 开关(徽标用)与提示词(编辑弹窗预填;明文非机密)
        "ai_reply_enabled": row.ai_reply_enabled,
        "ai_prompt": row.ai_prompt,
        "restore_quarantined": false,
    })
}

pub async fn list_accounts(
    State(state): SharedState,
    headers: HeaderMap,
    Query(q): Query<AccountListQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let rows = state
        .inner
        .db
        .call(move |conn| repo::list(conn, limit))
        .await;
    match rows {
        Ok(Ok(rows)) => Json(json!({
            "items": rows.iter().map(account_summary).collect::<Vec<_>>(),
            "next_cursor": null,
        }))
        .into_response(),
        _ => err_shared(ErrorCode::PersistenceUnavailable, "账号查询不可用"),
    }
}

pub async fn get_account(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let row = state
        .inner
        .db
        .call(move |conn| repo::get(conn, &account_id))
        .await;
    match row {
        Ok(Ok(Some(row))) => Json(account_summary(&row)).into_response(),
        _ => err_shared(ErrorCode::ResourceNotFound, "账号不存在"),
    }
}

fn qr_session_dto(s: &crate::application::accounts::qr::QrSession) -> serde_json::Value {
    json!({
        "id": s.id,
        "state": s.status.dto_str(),
        "expires_at": format_rfc3339(s.expires_at),
        "generation": s.generation,
        "image_path": format!("/api/v1/accounts/qr-sessions/{}/image", s.id),
        "account_id": s.account_id,
    })
}

#[derive(Deserialize)]
pub struct QrCreateRequest {
    #[allow(dead_code)]
    pub platform: String,
    pub replace_account_id: Option<String>,
}

pub async fn create_qr_session(
    State(state): SharedState,
    headers: HeaderMap,
    body: Option<Json<QrCreateRequest>>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let replace = body.and_then(|Json(b)| b.replace_account_id);
    // live:先创建本地状态机,再向平台取真实二维码;取码失败回滚本地会话(取消)
    let created = state.inner.qr_flows.create(replace.as_deref()).await;
    let Ok(session) = created else {
        return err_shared(
            ErrorCode::PersistenceUnavailable,
            &created.err().map(|e| e.to_string()).unwrap_or_default(),
        );
    };
    if let Some(driver) = state.inner.authorization.as_ref() {
        match driver.create(&session.id).await {
            Ok(_) => {}
            Err(message) => {
                let _ = state.inner.qr_flows.cancel(&session.id).await;
                return err_shared(ErrorCode::UnsupportedCapability, &message);
            }
        }
    }
    (StatusCode::CREATED, Json(qr_session_dto(&session))).into_response()
}

pub async fn get_qr_session(
    State(state): SharedState,
    headers: HeaderMap,
    Path(qr_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    match state.inner.qr_flows.observe(&qr_id).await {
        Ok(s) => Json(qr_session_dto(&s)).into_response(),
        Err(QrFlowError::NotFound) => err_shared(ErrorCode::ResourceNotFound, "授权会话不存在"),
        Err(e) => err_shared(ErrorCode::PersistenceUnavailable, &e.to_string()),
    }
}

/// 二维码图片:当前为占位 PNG;真实二维码渲染随协议适配(T041 实测)接入。
const PLACEHOLDER_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

pub async fn get_qr_image(
    State(state): SharedState,
    headers: HeaderMap,
    Path(qr_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    if state.inner.qr_flows.observe(&qr_id).await.is_err() {
        return err_shared(ErrorCode::ResourceNotFound, "授权会话不存在");
    }
    // live:真实二维码(SVG data URL);mock/无驱动回退占位图
    if let Some(driver) = state.inner.authorization.as_ref()
        && let Some(data_url) = driver.image(&qr_id).await
        && let Some((content_type, bytes)) = decode_data_url(&data_url)
    {
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        PLACEHOLDER_PNG.to_vec(),
    )
        .into_response()
}

/// data:image/svg+xml;base64,xxx → (content_type, bytes)
fn decode_data_url(data_url: &str) -> Option<(&'static str, Vec<u8>)> {
    use base64::Engine;
    let rest = data_url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let content_type = match meta.split(';').next()?.trim() {
        "image/svg+xml" => "image/svg+xml",
        "image/png" => "image/png",
        _ => return None,
    };
    if !meta.contains("base64") {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .ok()?;
    Some((content_type, bytes))
}

pub async fn cancel_qr_session(
    State(state): SharedState,
    headers: HeaderMap,
    Path(qr_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match state.inner.qr_flows.cancel(&qr_id).await {
        Ok(s) => Json(qr_session_dto(&s)).into_response(),
        Err(QrFlowError::AlreadyAuthorized) => {
            err_shared(ErrorCode::ActionInProgress, "已授权的会话不能取消")
        }
        Err(QrFlowError::NotFound) => err_shared(ErrorCode::ResourceNotFound, "授权会话不存在"),
        Err(e) => err_shared(ErrorCode::PersistenceUnavailable, &e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct ControlRequest {
    pub expected_version: i64,
    pub run_enabled: Option<bool>,
    pub auto_delivery_enabled: Option<bool>,
    pub auto_confirm_enabled: Option<bool>,
    pub acknowledge_monitoring_scope: Option<bool>,
}

/// 控制开关:幂等键必需;首次启用自动交付必须确认监控范围;202 任务句柄。
pub async fn control_account(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<ControlRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match state
        .inner
        .account_control
        .set_switches(
            &account_id,
            body.expected_version,
            body.run_enabled,
            body.auto_delivery_enabled,
            body.auto_confirm_enabled,
            body.acknowledge_monitoring_scope.unwrap_or(false),
        )
        .await
    {
        Ok(change) => {
            let job = state
                .inner
                .jobs
                .create("account_control", Some(&account_id))
                .await
                .ok()
                .map(|j| json!({
                    "id": j.id, "kind": j.kind, "state": j.state, "version": j.version,
                    "created_at": format_rfc3339(j.created_at), "updated_at": format_rfc3339(j.updated_at),
                    "target_id": j.target_id,
                }));
            (
                StatusCode::ACCEPTED,
                Json(json!({
                    "operation_id": crate::domain::ids::new_id("op"),
                    "job": job,
                    "new_status": change.new_status.dto_str(),
                })),
            )
                .into_response()
        }
        Err(crate::application::accounts::control::ControlError::ScopeAcknowledgementRequired) => {
            err_shared(
                ErrorCode::InvalidRequest,
                "首次启用自动交付必须确认监控范围(acknowledge_monitoring_scope=true)",
            )
        }
        Err(crate::application::accounts::control::ControlError::VersionConflict) => {
            err_shared(ErrorCode::VersionConflict, "版本冲突,请刷新后重试")
        }
        Err(crate::application::accounts::control::ControlError::NotFound) => {
            err_shared(ErrorCode::ResourceNotFound, "账号不存在")
        }
        Err(_) => err_shared(ErrorCode::PersistenceUnavailable, "控制服务不可用"),
    }
}

/// 官方验证入口(T050 浏览器流程接入前:受理任务并标记需浏览器)。
pub async fn start_verification(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    // 003:真实处置管道(手动触发=同管道;无驱动/活跃会话冲突如实返回)
    let Some(service) = state.inner.verification.clone() else {
        return err_shared(
            ErrorCode::UnsupportedCapability,
            "浏览器不可用或本构建未接入验证自动化;请安装 Chrome/Edge 后重试,或到平台手动完成验证",
        );
    };
    if let Some(active) = service.snapshot(&account_id) {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "operation_id": crate::domain::ids::new_id("op"),
                "session": {
                    "id": active.id, "state": active.state.as_str(),
                    "trigger": active.trigger, "retry_count": active.retry_count,
                },
                "note": "已有活跃验证会话",
            })),
        )
            .into_response();
    }
    service.handle_signal(crate::application::verification::VerificationSignal {
        account_id: account_id.clone(),
        source: crate::application::verification::SignalSource::Manual,
        url: None,
        raw: "手动触发".into(),
    });
    let session = service.snapshot(&account_id);
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "operation_id": crate::domain::ids::new_id("op"),
            "session": session.map(|s| json!({
                "id": s.id, "state": s.state.as_str(),
                "trigger": s.trigger, "retry_count": s.retry_count,
                "started_at": format_rfc3339(s.started_at),
            })),
            "verification": {
                "kind": "official_browser",
                "browser_available": true,
            },
        })),
    )
        .into_response()
}

/// 当前会话与最近尝试(003 契约 §2;前端 5s 轮询活跃会话)
pub async fn get_verification(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let Some(service) = state.inner.verification.clone() else {
        return err_shared(
            ErrorCode::UnsupportedCapability,
            "验证自动化未接入(浏览器不可用或 mock 构建)",
        );
    };
    let active = service.snapshot(&account_id).map(|s| {
        json!({
            "id": s.id, "state": s.state.as_str(), "trigger": s.trigger,
            "retry_count": s.retry_count, "started_at": format_rfc3339(s.started_at),
        })
    });
    let attempts: Vec<serde_json::Value> = service
        .recent_attempts(&account_id)
        .await
        .into_iter()
        .map(|a| {
            json!({
                "id": a.id, "trigger_source": a.trigger_source,
                "trigger_reason": a.trigger_reason, "verification_url": a.verification_url,
                "started_at": format_rfc3339(a.started_at),
                "finished_at": a.finished_at.map(format_rfc3339),
                "outcome": a.outcome, "duration_ms": a.duration_ms,
                "credential_updated": a.credential_updated, "failure_reason": a.failure_reason,
            })
        })
        .collect();
    Json(json!({ "active": active, "recent_attempts": attempts })).into_response()
}

/// 004:手动刷新资料(US1;风控→409 转验证提示,契约 §profile-fetch)。
pub async fn fetch_account_profile(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(_svc) = state.inner.verification.as_ref().map(|_| ()) else {
        // 资料服务与验证服务同生命周期(均依赖浏览器/协议);无验证服务时如实报不支持
        return err_shared(
            ErrorCode::UnsupportedCapability,
            "资料拉取需 live 协议(浏览器/协议未接入)",
        );
    };
    let db = state.inner.db.clone();
    let key = crate::adapters::windows::keys::DataKey {
        key_id: state.inner.key.key_id.clone(),
        key: state.inner.key.key,
    };
    let svc = std::sync::Arc::new(crate::application::accounts::profile::ProfileService::new(
        db,
        key,
        state.inner.verification.clone(),
    ));
    let result = svc.refresh(&account_id).await;
    match result {
        Ok(crate::application::accounts::profile::ProfileRefresh::Updated {
            nickname,
            avatar_url,
        }) => (
            StatusCode::OK,
            Json(json!({ "status": "updated", "nickname": nickname, "avatar_url": avatar_url })),
        )
            .into_response(),
        Ok(crate::application::accounts::profile::ProfileRefresh::VerificationTriggered) => (
            StatusCode::CONFLICT,
            Json(json!({ "status": "verification_required",
                "message": "资料拉取触发平台风控,已转入安全验证流程(见账号卡进度)" })),
        )
            .into_response(),
        Ok(other) => (
            StatusCode::OK,
            Json(json!({ "status": "kept", "detail": format!("{other:?}") })),
        )
            .into_response(),
        Err(e) => err_shared(ErrorCode::PersistenceUnavailable, &e),
    }
}

/// 004:删除账号(US2;409=活跃验证,404,204;契约 §DELETE)。
pub async fn delete_account(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let svc = crate::application::accounts::removal::RemovalService::new(
        state.inner.db.clone(),
        state.inner.verification.clone(),
    );
    match svc.remove(&account_id).await {
        Ok(crate::application::accounts::removal::RemovalOutcome::Removed) => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(crate::application::accounts::removal::RemovalOutcome::BlockedByVerification) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": { "code": "verification_active",
                "message": "该账号有进行中的安全验证,请等待处置结束后再删除" } })),
        )
            .into_response(),
        Ok(crate::application::accounts::removal::RemovalOutcome::NotFound) => {
            err_shared(ErrorCode::ResourceNotFound, "账号不存在")
        }
        Err(e) => err_shared(ErrorCode::PersistenceUnavailable, &e),
    }
}

/// 004:修改备注(US3;≤64 字符;null=清空;契约 §PATCH)。
pub async fn patch_account(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    body: axum::Json<serde_json::Value>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(remark_raw) = body.get("remark") else {
        return err_shared(ErrorCode::InvalidRequest, "缺少 remark 字段");
    };
    let remark: Option<String> = match remark_raw {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        _ => return err_shared(ErrorCode::InvalidRequest, "remark 须为字符串或 null"),
    };
    if let Some(r) = remark.as_ref()
        && r.chars().count() > 64
    {
        return err_shared(ErrorCode::InvalidRequest, "备注不得超过 64 字符");
    }
    let account = account_id.to_string();
    let updated = state
        .inner
        .db
        .call(move |conn| repo::set_remark(conn, &account, remark.as_deref()))
        .await;
    match updated {
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => err_shared(ErrorCode::ResourceNotFound, "账号不存在"),
        _ => err_shared(ErrorCode::PersistenceUnavailable, "备注保存不可用"),
    }
}
