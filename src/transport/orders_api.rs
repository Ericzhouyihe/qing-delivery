//! 订单查询 API(T040 最小实现;US5 补全筛选/游标/性能验证)。
//! DTO 状态映射按 http-api §4:持久模型不得直接序列化。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::adapters::sqlite::repos::orders as repo;
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

#[derive(Deserialize)]
pub struct ListQuery {
    pub account_id: Option<String>,
    pub platform_order_id: Option<String>,
    pub platform_state: Option<String>,
    /// 付款时间范围(RFC3339)
    pub from: Option<String>,
    pub to: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

/// 游标编码:base64("updated_at|id")。
fn encode_cursor(updated_at: i64, id: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{updated_at}|{id}"))
}

fn decode_cursor(raw: &str) -> Option<(i64, String)> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (ts, id) = text.split_once('|')?;
    Some((ts.parse().ok()?, id.to_string()))
}

/// content_state → 展示态;review=required 时显示 needs_review 但详情保留底层分类。
pub fn map_delivery_state(content: &str, review: &str) -> &'static str {
    if review == "required" {
        return "needs_review";
    }
    map_raw_content(content)
}

/// 底层分类映射(详情/时间线保留,http-api §4)。
pub fn map_raw_content(content: &str) -> &'static str {
    match content {
        "pending_verification" => "awaiting_verification",
        "queued" => "ready",
        "dispatching" => "sending",
        "accepted" => "delivered",
        "not_sent" => "definitely_not_sent",
        "unknown" => "unknown",
        "terminated" => "terminated",
        _ => "unknown",
    }
}

pub fn map_confirmation(state: &str) -> &'static str {
    match state {
        "pending" => "pending",
        "dispatching" => "dispatching",
        "accepted" => "succeeded",
        "rejected" => "failed",
        "unknown" => "unknown",
        "terminated" => "terminated",
        _ => "disabled",
    }
}

fn summary_json(r: &repo::OrderSummaryRow) -> serde_json::Value {
    json!({
        "id": r.id,
        "platform": "xianyu",
        "account_id": r.account_id,
        "platform_order_id": r.platform_order_id,
        "paid_at": r.paid_at.map(crate::domain::time_util::format_rfc3339),
        "quantity": r.quantity,
        "amount": r.amount_minor.map(|m| json!({ "minor_units": m, "currency": r.currency.clone().unwrap_or_default() })),
        "trade_type": r.trade_type,
        "platform_state": r.platform_state,
        "delivery_state": r.delivery_state.as_deref()
            .map(|c| map_delivery_state(c, r.review_state.as_deref().unwrap_or("none"))),
        "raw_content_state": r.delivery_state.as_deref().map(map_raw_content),
        "confirmation_state": r.confirmation_state.as_deref().map(map_confirmation),
        "version": r.version,
        "updated_at": crate::domain::time_util::format_rfc3339(r.updated_at),
    })
}

pub async fn list_orders(
    State(state): SharedState,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    // 游标:base64("updated_at|id"),绑定筛选条件由调用方保证一致(契约 §1)
    let cursor_pair = q.cursor.as_deref().and_then(decode_cursor);
    let account = q.account_id.clone();
    let ext = q.platform_order_id.clone();
    let pstate = q.platform_state.clone();
    let paid_from = q
        .from
        .as_deref()
        .and_then(crate::domain::time_util::parse_rfc3339_ms);
    let paid_to =
        q.to.as_deref()
            .and_then(crate::domain::time_util::parse_rfc3339_ms);
    let cursor = cursor_pair.clone();
    let rows = match state
        .inner
        .db
        .call(move |conn| {
            let filter = repo::OrderFilter {
                account_id: account.as_deref(),
                platform_order_id: ext.as_deref(),
                platform_state: pstate.as_deref(),
                paid_from,
                paid_to,
                cursor: cursor.as_ref().map(|(ts, id)| (ts, id.as_str())),
                limit,
            };
            repo::list_filtered(conn, &filter)
        })
        .await
    {
        Ok(Ok(rows)) => rows,
        Ok(Err(e)) => {
            // 原始 SQL 错误不进错误信封(脱敏);记录日志排查
            tracing::warn!(error = %e, "订单列表查询失败");
            return err_shared(ErrorCode::PersistenceUnavailable, "查询不可用");
        }
        Err(e) => {
            tracing::warn!(error = %e, "数据库线程不可用");
            return err_shared(ErrorCode::PersistenceUnavailable, "查询不可用");
        }
    };
    let next_cursor = if rows.len() as i64 >= limit.clamp(1, 100) {
        rows.last().map(|r| encode_cursor(r.updated_at, &r.id))
    } else {
        None
    };
    Json(json!({
        "items": rows.iter().map(summary_json).collect::<Vec<_>>(),
        "next_cursor": next_cursor,
    }))
    .into_response()
}

pub async fn order_detail(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let detail = state
        .inner
        .db
        .call(move |conn| repo::get_detail(conn, &order_id))
        .await;
    let Ok(Ok(Some(detail))) = detail else {
        return err_shared(ErrorCode::ResourceNotFound, "订单不存在");
    };
    // 路径账号必须匹配订单;不匹配按不存在处理,不泄露其他账号身份
    if detail.summary.account_id != account_id {
        return err_shared(ErrorCode::ResourceNotFound, "订单不存在");
    }
    let s = &detail.summary;
    let attempts: serde_json::Value =
        serde_json::from_str(&detail.attempts_json).unwrap_or(serde_json::Value::Array(vec![]));
    Json(json!({
        "summary": summary_json(s),
        "delivery": {
            "id": detail.delivery_id,
            "version": detail.delivery_version,
            "state": s.delivery_state.as_deref().map(|c| map_delivery_state(c, s.review_state.as_deref().unwrap_or("none"))),
            "raw_content_state": s.delivery_state.as_deref().map(map_raw_content),
            "review_state": s.review_state,
            "confirmation_state": s.confirmation_state.as_deref().map(map_confirmation),
            "accepted_evidence": detail.evidence_origin.as_deref().unwrap_or("none"),
            "automatic_retries_used": detail.retry_count.unwrap_or(0),
            "attempts": attempts,
        },
        "allowed_actions": [],
    }))
    .into_response()
}

// ---------- 007 US6:订单同步端点(contracts §6,T070) ----------

#[derive(Deserialize)]
pub struct SyncStartBody {
    pub account_ids: Option<Vec<String>>,
    pub window_days: Option<i64>,
}

/// 一键同步:创建 order_sync 后台任务,立即返回 202 + 任务句柄。
pub async fn start_order_sync(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<SyncStartBody>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(sync) = state.inner.order_sync.as_ref() else {
        return err_shared(ErrorCode::ServiceStopping, "同步服务不可用(运行时未接入)");
    };
    let window = body.window_days.unwrap_or(crate::application::orders_sync::DEFAULT_WINDOW_DAYS);
    if !(1..=92).contains(&window) {
        return err_shared(ErrorCode::InvalidRequest, "window_days 必须在 1—92 天");
    }
    match sync.start(body.account_ids.unwrap_or_default(), window).await {
        Ok(job) => (
            axum::http::StatusCode::ACCEPTED,
            Json(json!({
                "operation_id": crate::domain::ids::new_id("op"),
                "job": {
                    "id": job.id, "kind": job.kind, "state": job.state, "version": job.version,
                    "created_at": crate::domain::time_util::format_rfc3339(job.created_at),
                    "updated_at": crate::domain::time_util::format_rfc3339(job.updated_at),
                    "target_id": job.target_id,
                }
            })),
        )
            .into_response(),
        Err(_) => err_shared(ErrorCode::PersistenceUnavailable, "同步任务创建失败"),
    }
}

/// 单笔同步:直接走既有交付管线,同步返回结果摘要。
pub async fn sync_single_order(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(sync) = state.inner.order_sync.as_ref() else {
        return err_shared(ErrorCode::ServiceStopping, "同步服务不可用(运行时未接入)");
    };
    use crate::application::orders_sync::SingleSyncOutcome as S;
    match sync.sync_single(&account_id, &order_id).await {
        Ok(out) => Json(json!({ "outcome": match out {
            S::Delivered { .. } => "delivered",
            S::Ineligible { .. } => "ineligible",
            S::NotSent { .. } => "not_sent",
            S::Unknown { .. } => "unknown",
            S::AlreadyHandled => "already_handled",
            S::Busy => "busy",
        }}))
        .into_response(),
        Err(e) => err_shared(ErrorCode::AccountUnavailable, &e.to_string()),
    }
}
