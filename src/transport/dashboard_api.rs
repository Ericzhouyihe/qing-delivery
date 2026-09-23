//! Dashboard 与订单详情内容揭示(T075/T076):
//! 3 秒轮询摘要;完整正文只在授权订单详情出现(FR-026)。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::adapters::sqlite::repos::{accounts, orders};
use crate::domain::crypto::{self, Aad};
use crate::transport::error::ErrorCode;
/// 内容快照载荷:(snapshot_id, ciphertext, nonce, digest)
type SnapshotPayload = (String, Vec<u8>, [u8; 12], String);

use crate::transport::orders_api::map_delivery_state;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

pub async fn dashboard(State(state): SharedState, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let stopping_now = state.stopping();
    let summary = state
        .inner
        .db
        .call(move |conn| -> rusqlite::Result<serde_json::Value> {
            let accounts_rows = accounts::list(conn, 100)?;
            let open_issues: i64 = conn.query_row(
                "SELECT COUNT(*) FROM issues WHERE state = 'open'",
                [],
                |r| r.get(0),
            )?;
            let active_jobs: i64 = conn.query_row(
                "SELECT COUNT(*) FROM operation_jobs WHERE state IN ('queued','running','cancel_requested')",
                [],
                |r| r.get(0),
            )?;
            // 002 增量字段(contracts/dashboard-api.md):本地自然日计数,additive 不破坏旧消费者
            let today = orders::count_today(
                conn,
                crate::domain::time_util::local_midnight_ms(crate::domain::time_util::utc_now_ms()),
            )?;
            let restore_epoch: i64 = conn.query_row(
                "SELECT restore_epoch FROM installation WHERE id='singleton'",
                [],
                |r| r.get(0),
            )?;
            let restore = if restore_epoch > 0 {
                let (started, unresolved): (Option<i64>, i64) = conn.query_row(
                    "SELECT quarantine_started_at,
                            (SELECT COUNT(*) FROM restore_reviews WHERE status='unresolved')
                     FROM installation WHERE id='singleton'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                Some(json!({
                    "state": "quarantined",
                    "quarantine_started_at": started.map(crate::domain::time_util::format_rfc3339),
                    "unresolved_count": unresolved,
                    "restore_epoch": restore_epoch,
                }))
            } else {
                None
            };
            Ok(json!({
                "accounts": accounts_rows.iter().map(|a| json!({
                    "id": a.id,
                    "display_name": a.display_name,
                    "connection_state": crate::domain::accounts::AccountStatus::parse(&a.status).dto_str(),
                    "run_enabled": a.runtime_enabled,
                    "auto_delivery_enabled": a.auto_delivery_enabled,
                })).collect::<Vec<_>>(),
                "open_issue_count": open_issues,
                "active_job_count": active_jobs,
                "orders_today": today.orders_today,
                "delivered_today": today.delivered_today,
                "persistence": "healthy",
                "stopping": stopping_now,
                "restore": restore,
            }))
        })
        .await;
    match summary {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => err_shared(ErrorCode::PersistenceUnavailable, "摘要不可用"),
    }
}

/// 授权订单详情:解密冻结快照原文(FR-026:列表/日志/导出不含正文)。
pub async fn reveal_content(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let probe = (account_id.clone(), order_id.clone());
    let loaded = state
        .inner
        .db
        .call(move |conn| -> rusqlite::Result<Option<SnapshotPayload>> {
            let Some(order) = orders::get_manual_ctx(conn, &probe.1)? else {
                return Ok(None);
            };
            if order.account_id != probe.0 {
                return Ok(None);
            }
            let row = conn.query_row(
                "SELECT s.id, s.ciphertext, s.nonce, s.digest
                 FROM deliveries d JOIN content_snapshots s ON s.id = d.content_snapshot_id
                 WHERE d.order_id = ?1 AND d.kind = 'initial'",
                rusqlite::params![probe.1],
                |r| {
                    let nonce_vec: Vec<u8> = r.get(2)?;
                    let mut nonce = [0u8; 12];
                    if nonce_vec.len() == 12 {
                        nonce.copy_from_slice(&nonce_vec);
                    }
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Vec<u8>>(1)?,
                        nonce,
                        r.get::<_, String>(3)?,
                    ))
                },
            );
            match row {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await;
    let payload = match loaded {
        Ok(Ok(Some((snapshot_id, ciphertext, nonce, digest)))) => {
            let aad = Aad {
                purpose: "content_snapshot".into(),
                entity_id: snapshot_id,
                content_version: None,
            };
            let envelope = crate::domain::crypto::Envelope {
                ciphertext,
                nonce,
                key_id: state.inner.key.key_id.clone(),
                format_version: 1,
            };
            match crypto::open(&state.inner.key.key, &aad, &envelope) {
                Ok(plain) => Some((String::from_utf8(plain).unwrap_or_default(), digest)),
                Err(_) => return err_shared(ErrorCode::PersistenceUnavailable, "快照解密失败"),
            }
        }
        Ok(Ok(None)) => None,
        _ => return err_shared(ErrorCode::PersistenceUnavailable, "详情查询不可用"),
    };
    match payload {
        Some((text, digest)) => Json(json!({
            "content": text,
            "content_hash": digest,
            "content_kind": "fixed_text",
        }))
        .into_response(),
        None => err_shared(ErrorCode::ResourceNotFound, "订单无冻结内容"),
    }
}

/// 详情页用的展示态聚合(列表字段 + delivery 状态双轴)。
pub async fn order_display_state(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, order_id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let probe = order_id.clone();
    let row = state
        .inner
        .db
        .call(move |conn| orders::get_manual_ctx(conn, &probe))
        .await;
    let Ok(Ok(Some(ctx))) = row else {
        return err_shared(ErrorCode::ResourceNotFound, "订单不存在");
    };
    if ctx.account_id != account_id {
        return err_shared(ErrorCode::ResourceNotFound, "订单不存在");
    }
    let _ = map_delivery_state;
    Json(json!({
        "order_id": ctx.id,
        "platform_state": ctx.platform_status,
        "content_endpoint": format!("/api/v1/accounts/{account_id}/orders/{order_id}/content"),
    }))
    .into_response()
}
