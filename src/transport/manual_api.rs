//! 人工动作与待处理事项 HTTP API(T068—T071):
//! resends/mark-received/terminate/takeovers(幂等键+CSRF+版本)、GET /issues。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::application::manual::actions::ManualError;
use crate::domain::time_util::{format_rfc3339, utc_now_ms};
use crate::transport::error::ErrorCode;
/// issues 列表行:(id, order_id, account_id, kind, reason_code, allowed, state, created_at)
type IssueRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    i64,
);

use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

fn manual_err(e: ManualError) -> Response {
    match e {
        ManualError::OrderNotFound | ManualError::NoDelivery => {
            err_shared(ErrorCode::ResourceNotFound, &e.to_string())
        }
        ManualError::NotAllowed(msg) => err_shared(ErrorCode::IneligibleOrder, &msg),
        ManualError::MissingFacts(fields) => err_shared(
            ErrorCode::IncompleteOrder,
            &format!("付款事实不完整,缺失:{fields:?};请先经订单同步或等待平台事件补全"),
        ),
        ManualError::RiskConfirmationRequired | ManualError::InvalidReason => {
            err_shared(ErrorCode::InvalidRequest, &e.to_string())
        }
        ManualError::InFlight => err_shared(ErrorCode::ActionInProgress, "已有同键操作在途"),
        ManualError::AutoDeliveryOff => err_shared(ErrorCode::IneligibleOrder, &e.to_string()),
        ManualError::RefundedOrCompleted => err_shared(ErrorCode::IneligibleOrder, &e.to_string()),
        ManualError::Db(_) | ManualError::Sqlite(_) | ManualError::Platform(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "人工动作服务不可用")
        }
        ManualError::Crypto(_) => err_shared(ErrorCode::PersistenceUnavailable, "内容解密失败"),
        ManualError::Delivery(_) => err_shared(ErrorCode::PersistenceUnavailable, "交付服务不可用"),
    }
}

#[derive(Deserialize)]
pub struct ManualBody {
    pub expected_order_version: Option<i64>,
    pub expected_delivery_version: Option<i64>,
    pub reason: String,
    #[serde(default)]
    pub acknowledge_duplicate_risk: bool,
    #[serde(default)]
    pub acknowledge_historical_scope: bool,
    /// 未提供时由服务端生成(浏览器一次性意图);显式提供用于超时重放
    pub idempotency_key: Option<String>,
}

async fn admin_id(state: &crate::transport::state::AppState, headers: &HeaderMap) -> String {
    match require_admin_public(state, headers, true).await {
        Ok(identity) => identity.admin_id,
        Err(_) => String::new(),
    }
}

pub async fn resend(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
    Json(body): Json<ManualBody>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let admin = admin_id(&state, &headers).await;
    let Some(manual) = state.inner.manual.as_ref() else {
        return err_shared(
            ErrorCode::ServiceStopping,
            "运行时适配器不可用(live 协议接入前)",
        );
    };
    let _ = body.expected_order_version;
    let _ = body.expected_delivery_version;
    let outcome = manual
        .resend(crate::application::manual::actions::ManualRequest {
            admin_id: admin,
            account_id,
            order_db_id: order_id,
            reason: body.reason,
            risk_confirmed: body.acknowledge_duplicate_risk,
            idempotency_key: body
                .idempotency_key
                .unwrap_or_else(|| format!("auto-{}", crate::domain::ids::new_request_key())),
        })
        .await;
    match outcome {
        Ok(_) => (
            StatusCode::ACCEPTED,
            Json(json!({ "operation_id": crate::domain::ids::new_id("op") })),
        )
            .into_response(),
        Err(e) => manual_err(e),
    }
}

pub async fn mark_received(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
    Json(body): Json<ManualBody>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let admin = admin_id(&state, &headers).await;
    let Some(manual) = state.inner.manual.as_ref() else {
        return err_shared(ErrorCode::ServiceStopping, "运行时适配器不可用");
    };
    match manual
        .mark_received(crate::application::manual::actions::ManualRequest {
            admin_id: admin,
            account_id,
            order_db_id: order_id,
            reason: body.reason,
            risk_confirmed: false,
            idempotency_key: body
                .idempotency_key
                .unwrap_or_else(|| format!("auto-{}", crate::domain::ids::new_request_key())),
        })
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => manual_err(e),
    }
}

pub async fn terminate(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
    Json(body): Json<ManualBody>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let admin = admin_id(&state, &headers).await;
    let Some(manual) = state.inner.manual.as_ref() else {
        return err_shared(ErrorCode::ServiceStopping, "运行时适配器不可用");
    };
    match manual
        .terminate(crate::application::manual::actions::ManualRequest {
            admin_id: admin,
            account_id,
            order_db_id: order_id,
            reason: body.reason,
            risk_confirmed: false,
            idempotency_key: body
                .idempotency_key
                .unwrap_or_else(|| format!("auto-{}", crate::domain::ids::new_request_key())),
        })
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => manual_err(e),
    }
}

pub async fn takeover(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
    Json(body): Json<ManualBody>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let admin = admin_id(&state, &headers).await;
    let Some(manual) = state.inner.manual.as_ref() else {
        return err_shared(ErrorCode::ServiceStopping, "运行时适配器不可用");
    };
    if !body.acknowledge_historical_scope {
        return err_shared(ErrorCode::InvalidRequest, "必须确认历史接管范围");
    }
    match manual
        .takeover(crate::application::manual::actions::ManualRequest {
            admin_id: admin,
            account_id,
            order_db_id: order_id,
            reason: body.reason,
            risk_confirmed: body.acknowledge_historical_scope,
            idempotency_key: body
                .idempotency_key
                .unwrap_or_else(|| format!("auto-{}", crate::domain::ids::new_request_key())),
        })
        .await
    {
        Ok(_) => (
            StatusCode::ACCEPTED,
            Json(json!({ "operation_id": crate::domain::ids::new_id("op") })),
        )
            .into_response(),
        Err(e) => manual_err(e),
    }
}

/// 人工触发交付(007 US6,FR-061):付款事实齐备才走既有管线,幂等拒绝重复。
pub async fn trigger_delivery(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
    Json(body): Json<ManualBody>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let admin = admin_id(&state, &headers).await;
    let Some(manual) = state.inner.manual.as_ref() else {
        return err_shared(ErrorCode::ServiceStopping, "运行时适配器不可用");
    };
    match manual
        .trigger_delivery(crate::application::manual::actions::ManualRequest {
            admin_id: admin,
            account_id,
            order_db_id: order_id,
            reason: body.reason,
            risk_confirmed: false,
            idempotency_key: body
                .idempotency_key
                .unwrap_or_else(|| format!("auto-{}", crate::domain::ids::new_request_key())),
        })
        .await
    {
        Ok(outcome) => {
            let summary = match &outcome {
                crate::application::manual::actions::ManualOutcome::Triggered(o) => format!("{o:?}"),
                crate::application::manual::actions::ManualOutcome::TriggerNoOp(_) => {
                    "already_handled".into()
                }
                _ => "replay".into(),
            };
            (
                StatusCode::ACCEPTED,
                Json(json!({
                    "operation_id": crate::domain::ids::new_id("op"),
                    "outcome": summary,
                })),
            )
                .into_response()
        }
        Err(e) => manual_err(e),
    }
}

/// 人工确认平台已发货(007 US6,FR-062):确认轴独立,不影响消息交付记录。
pub async fn confirm_shipment(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
    Json(body): Json<ManualBody>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let admin = admin_id(&state, &headers).await;
    let Some(manual) = state.inner.manual.as_ref() else {
        return err_shared(ErrorCode::ServiceStopping, "运行时适配器不可用");
    };
    match manual
        .confirm_shipment(crate::application::manual::actions::ManualRequest {
            admin_id: admin,
            account_id,
            order_db_id: order_id,
            reason: body.reason,
            risk_confirmed: false,
            idempotency_key: body
                .idempotency_key
                .unwrap_or_else(|| format!("auto-{}", crate::domain::ids::new_request_key())),
        })
        .await
    {
        Ok(outcome) => {
            let platform_confirmed = matches!(
                outcome,
                crate::application::manual::actions::ManualOutcome::ConfirmShipment {
                    platform_confirmed: true
                }
            );
            (
                StatusCode::ACCEPTED,
                Json(json!({
                    "operation_id": crate::domain::ids::new_id("op"),
                    "platform_confirmed": platform_confirmed,
                })),
            )
                .into_response()
        }
        Err(e) => manual_err(e),
    }
}

#[derive(Deserialize)]
pub struct IssueQuery {
    pub state: Option<String>,
    pub limit: Option<i64>,
}

/// 待处理事项列表(T071/FR-024):持久化、刷新重开仍在。
pub async fn list_issues(
    State(state): SharedState,
    headers: HeaderMap,
    Query(q): Query<IssueQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let issue_state = q.state.clone();
    let rows = state
        .inner
        .db
        .call(move |conn| -> rusqlite::Result<Vec<IssueRow>> {
            let sql = match &issue_state {
                Some(_) => {
                    "SELECT id, order_id, account_id, kind, reason_code, allowed_actions, state, created_at
                     FROM issues WHERE state = ?1 ORDER BY created_at DESC LIMIT ?2"
                }
                None => {
                    "SELECT id, order_id, account_id, kind, reason_code, allowed_actions, state, created_at
                     FROM issues ORDER BY created_at DESC LIMIT ?1"
                }
            };
            let mut stmt = conn.prepare(sql)?;
            let map = |r: &rusqlite::Row<'_>| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            };
            match &issue_state {
                Some(s) => stmt.query_map(rusqlite::params![s, limit], map)?.collect(),
                None => stmt.query_map(rusqlite::params![limit], map)?.collect(),
            }
        })
        .await;
    match rows {
        Ok(Ok(rows)) => Json(json!({
            "items": rows.iter().map(|(id, order, account, kind, reason, allowed, state, created)| json!({
                "id": id,
                "order_id": order,
                "account_id": account,
                "category": kind,
                "reason": reason,
                "allowed_actions": serde_json::from_str::<serde_json::Value>(allowed)
                    .unwrap_or(serde_json::json!([])),
                "state": state,
                "created_at": format_rfc3339(*created),
            })).collect::<Vec<_>>(),
            "next_cursor": null,
            "generated_at": format_rfc3339(utc_now_ms()),
        }))
        .into_response(),
        _ => err_shared(ErrorCode::PersistenceUnavailable, "待处理查询不可用"),
    }
}
