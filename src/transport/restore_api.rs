//! 恢复核对 HTTP API(T081):隔离摘要、逐单核对事项、决定端点。
//! 总开关不能清除逐单隔离;决定只记录证据与授权,不直接发送。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::domain::time_util::{format_rfc3339, utc_now_ms};
/// 核对行:(id, order_id, kind, status, reason)
type ReviewRow = (String, Option<String>, String, String, String);

use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

pub async fn restore_summary(State(state): SharedState, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let row = state
        .inner
        .db
        .call(
            |conn| -> rusqlite::Result<Option<(i64, Option<i64>, i64)>> {
                conn.query_row(
                    "SELECT restore_epoch, quarantine_started_at,
                        (SELECT COUNT(*) FROM restore_reviews WHERE status='unresolved')
                 FROM installation WHERE id='singleton'",
                    [],
                    |r| Ok(Some((r.get(0)?, r.get(1)?, r.get(2)?))),
                )
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })
            },
        )
        .await;
    match row {
        Ok(Ok(Some((epoch, started, unresolved)))) if epoch > 0 => Json(json!({
            "id": format!("restore-{epoch}"),
            "state": "quarantined",
            "quarantine_started_at": started.map(format_rfc3339),
            "unresolved_count": unresolved,
            "coverage_complete": true,
        }))
        .into_response(),
        Ok(Ok(_)) => Json(serde_json::Value::Null).into_response(),
        _ => err_shared(ErrorCode::PersistenceUnavailable, "恢复状态不可用"),
    }
}

#[derive(Deserialize)]
pub struct ReviewQuery {
    pub state: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_reviews(
    State(state): SharedState,
    headers: HeaderMap,
    Query(q): Query<ReviewQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let st = q.state.clone();
    let rows = state
        .inner
        .db
        .call(move |conn| -> rusqlite::Result<Vec<ReviewRow>> {
            let sql = match &st {
                Some(_) => {
                    "SELECT id, order_id, kind, status, reason FROM restore_reviews
                     WHERE status = ?1 ORDER BY created_at LIMIT ?2"
                }
                None => {
                    "SELECT id, order_id, kind, status, reason FROM restore_reviews
                     ORDER BY created_at LIMIT ?2"
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
                ))
            };
            match &st {
                Some(s) => Ok(stmt
                    .query_map(rusqlite::params![s, limit], map)?
                    .collect::<Result<Vec<_>, _>>()?),
                None => Ok(stmt
                    .query_map(rusqlite::params![limit], map)?
                    .collect::<Result<Vec<_>, _>>()?),
            }
        })
        .await;
    match rows {
        Ok(Ok(rows)) => Json(json!({
            "items": rows.iter().map(|(id, order, kind, status, reason)| json!({
                "id": id, "order_id": order, "kind": kind, "status": status, "reason": reason,
            })).collect::<Vec<_>>(),
            "next_cursor": null,
        }))
        .into_response(),
        _ => err_shared(ErrorCode::PersistenceUnavailable, "核对列表不可用"),
    }
}

#[derive(Deserialize)]
pub struct DecisionBody {
    pub expected_version: i64,
    /// received | approved_not_sent | terminated
    pub decision: String,
    pub reason: String,
    pub evidence_note: String,
    #[serde(default)]
    pub acknowledge_duplicate_risk: bool,
}

/// 核对决定(T081):原子更新 review + 相关 delivery/guard/审计;
/// received/approved_not_sent 保留 manual_only 门槛,terminated 终结旧路径。
pub async fn decide(
    State(state): SharedState,
    headers: HeaderMap,
    Path(review_id): Path<String>,
    Json(body): Json<DecisionBody>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    if !matches!(
        body.decision.as_str(),
        "received" | "approved_not_sent" | "terminated"
    ) {
        return err_shared(
            ErrorCode::InvalidRequest,
            "决定必须是 received/approved_not_sent/terminated",
        );
    }
    if body.decision == "received" && !body.acknowledge_duplicate_risk {
        return err_shared(ErrorCode::InvalidRequest, "received 必须确认重复风险");
    }
    let admin = match require_admin_public(&state, &headers, true).await {
        Ok(id) => id.admin_id,
        Err(_) => String::new(),
    };
    let review = review_id.clone();
    let decision = body.decision.clone();
    let reason = format!("{}|{}", body.reason, body.evidence_note);
    let admin_id = admin;
    let updated = state
        .inner
        .db
        .call(move |conn| -> rusqlite::Result<Option<()>> {
            let row: Option<(String, Option<String>)> = conn
                .query_row(
                    "SELECT status, order_id FROM restore_reviews WHERE id = ?1",
                    [&review],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })?;
            let Some((status, order_id)) = row else {
                return Ok(None);
            };
            if status != "unresolved" {
                return Ok(Some(())); // 幂等重放
            }
            // 同一事务原子更新:review、delivery、guard、人工审计
            conn.execute(
                "UPDATE restore_reviews SET status=?2, evidence_origin='manual',
                        reason=?3, reviewer=?4, reviewed_at=?5 WHERE id=?1",
                rusqlite::params![review, decision, reason, admin_id, utc_now_ms()],
            )?;
            if let Some(order) = &order_id {
                // received:追加人工证明并终结旧自动路径;terminated:终止旧路径
                match decision.as_str() {
                    "received" => {
                        conn.execute(
                            "UPDATE deliveries SET content_state='accepted', review_state='resolved',
                                    evidence_origin='manual', version=version+1, updated_at=?2
                             WHERE order_id=?1 AND content_state IN ('unknown','not_sent','queued',
                                        'pending_verification','dispatching')",
                            rusqlite::params![order, utc_now_ms()],
                        )?;
                    }
                    "terminated" => {
                        conn.execute(
                            "UPDATE deliveries SET content_state='terminated', review_state='resolved',
                                    version=version+1, updated_at=?2
                             WHERE order_id=?1 AND content_state NOT IN ('accepted')",
                            rusqlite::params![order, utc_now_ms()],
                        )?;
                    }
                    _ => {} // approved_not_sent:保留 manual_only,显式接管/补发才建可执行动作
                }
                conn.execute(
                    "UPDATE order_execution_guards SET state='terminal' WHERE order_id=?1",
                    [order],
                )?;
                conn.execute(
                    "INSERT INTO manual_actions(id, admin_id, order_id, delivery_id, action, reason,
                         risk_confirmed, request_key, created_at)
                     VALUES (?1,?2,?3,NULL,'restore_review',?4,?5,?6,?7)",
                    rusqlite::params![
                        crate::domain::ids::new_id("man"),
                        admin_id,
                        order,
                        reason,
                        body.acknowledge_duplicate_risk,
                        crate::domain::ids::new_request_key(),
                        utc_now_ms()
                    ],
                )?;
            }
            Ok(Some(()))
        })
        .await;
    match updated {
        Ok(Ok(Some(()))) => (
            StatusCode::OK,
            Json(json!({ "id": review_id, "status": body.decision })),
        )
            .into_response(),
        Ok(Ok(None)) => err_shared(ErrorCode::ResourceNotFound, "核对事项不存在"),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "核对决定失败");
            err_shared(ErrorCode::PersistenceUnavailable, "核对决定不可用")
        }
        Err(_) => err_shared(ErrorCode::PersistenceUnavailable, "数据库不可用"),
    }
}

/// T082:CLI 用的同步窄操作帮助(不经过 DbThread;init-admin 停机独占)。
pub fn admin_exists_sync(conn: &rusqlite::Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM admin", [], |r| r.get(0))?;
    Ok(n > 0)
}
