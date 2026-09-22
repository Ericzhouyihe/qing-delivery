//! 商品与规则 HTTP API(T056/T059):items 列表、规则 CRUD、预览与匹配预览。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::adapters::sqlite::repos::{items, rules};
use crate::application::catalog::rules::{MatchPreview, RuleDraft, RuleError};
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    #[allow(dead_code)]
    pub status: Option<String>,
}

/// items 列表(T056):rule_state 徽标(configured/missing/conflict)由启用规则计数计算。
pub async fn list_items(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let account = account_id.clone();
    let rows = state
        .inner
        .db
        .call(move |conn| items::list_for_account(conn, &account, limit))
        .await;
    match rows {
        Ok(Ok(rows)) => {
            let counts = state
                .inner
                .db
                .call(move |conn| -> rusqlite::Result<Vec<(String, i64)>> {
                    let mut stmt = conn.prepare(
                        "SELECT item_id, COUNT(*) FROM rules
                         WHERE account_id = ?1 AND enabled = 1 GROUP BY item_id",
                    )?;
                    let out = stmt
                        .query_map([&account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(out)
                })
                .await;
            let badge = |item: &items::ItemRow| -> &'static str {
                match &counts {
                    Ok(Ok(list)) => match list.iter().find(|(id, _)| *id == item.id) {
                        Some((_, 1)) => "configured",
                        Some((_, _)) => "conflict",
                        None => "missing",
                    },
                    _ => "missing",
                }
            };
            Json(json!({
                "items": rows.iter().map(|r| json!({
                    "id": r.id,
                    "account_id": r.account_id,
                    "platform_item_id": r.external_item_id,
                    "title": r.title,
                    "status": r.listing_state,
                    "sku_definition": serde_json::from_str::<serde_json::Value>(&r.sku_definition)
                        .unwrap_or(serde_json::json!([])),
                    "rule_state": badge(r),
                    "version": r.version,
                })).collect::<Vec<_>>(),
                "next_cursor": null,
            }))
            .into_response()
        }
        _ => err_shared(ErrorCode::PersistenceUnavailable, "商品查询不可用"),
    }
}

#[derive(Deserialize)]
pub struct RuleWriteRequest {
    pub item_id: String,
    pub sku_key: String,
    pub content_kind: String,
    pub content: String,
    pub enabled: bool,
    pub expected_version: Option<i64>,
}

fn rule_err(e: RuleError) -> Response {
    match e {
        RuleError::EmptyContent | RuleError::ContentTooLong { .. } => {
            err_shared(ErrorCode::ContentTooLong, &e.to_string())
        }
        RuleError::EnabledConflict => err_shared(ErrorCode::RuleConflict, &e.to_string()),
        RuleError::NotFound | RuleError::ItemNotFound => {
            err_shared(ErrorCode::ResourceNotFound, &e.to_string())
        }
        RuleError::VersionConflict => err_shared(ErrorCode::VersionConflict, &e.to_string()),
        RuleError::Db(_) | RuleError::Sqlite(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "规则服务不可用")
        }
    }
}

/// 创建规则(T057):POST 不带 expected_version;正文详情随响应返回(授权编辑)。
pub async fn create_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<RuleWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    if body.content_kind != "fixed_text" {
        return err_shared(ErrorCode::UnsupportedCapability, "首版仅支持 fixed_text");
    }
    match state
        .inner
        .rule_service
        .create(
            &account_id,
            RuleDraft {
                item_id: body.item_id.clone(),
                sku_key: body.sku_key.clone(),
                content: body.content.clone(),
                enabled: body.enabled,
            },
        )
        .await
    {
        Ok(saved) => (
            StatusCode::CREATED,
            Json(json!({
                "summary": {
                    "id": saved.id, "account_id": account_id, "item_id": body.item_id,
                    "sku_key": body.sku_key, "enabled": body.enabled,
                    "version": saved.version, "content_length": body.content.chars().count(),
                    "content_version": saved.content_version,
                },
                "content": body.content,
                "content_kind": "fixed_text",
            })),
        )
            .into_response(),
        Err(e) => rule_err(e),
    }
}

/// 版本化更新:expected_version 必填;禁用/启用/改内容均走此端点。
pub async fn update_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, rule_id)): Path<(String, String)>,
    Json(body): Json<RuleWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(expected) = body.expected_version else {
        return err_shared(ErrorCode::InvalidRequest, "更新必须携带 expected_version");
    };
    match state
        .inner
        .rule_service
        .update(
            &account_id,
            &rule_id,
            expected,
            Some(body.content.clone()),
            Some(body.enabled),
        )
        .await
    {
        Ok(saved) => Json(json!({
            "summary": {
                "id": saved.id, "account_id": account_id, "item_id": body.item_id,
                "sku_key": body.sku_key, "enabled": body.enabled,
                "version": saved.version, "content_length": body.content.chars().count(),
                "content_version": saved.content_version,
            },
            "content": body.content,
            "content_kind": "fixed_text",
        }))
        .into_response(),
        Err(e) => rule_err(e),
    }
}

pub async fn list_rules(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let rows = state
        .inner
        .db
        .call(move |conn| rules::list_for_account(conn, &account_id, limit))
        .await;
    match rows {
        Ok(Ok(rows)) => Json(json!({
            "items": rows.iter().map(|r| json!({
                "id": r.id, "account_id": r.account_id, "item_id": r.item_id,
                "sku_key": r.sku_key, "enabled": r.enabled, "version": r.version,
                "content_version": r.current_content_version,
            })).collect::<Vec<_>>(),
            "next_cursor": null,
        }))
        .into_response(),
        _ => err_shared(ErrorCode::PersistenceUnavailable, "规则查询不可用"),
    }
}

#[derive(Deserialize)]
pub struct PreviewRequest {
    pub item_id: String,
    pub sku_key: String,
    pub content: String,
}

/// 预览(T059):纯校验,不保存、不发送。
pub async fn preview_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path(_account_id): Path<String>,
    Json(body): Json<PreviewRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let check = state.inner.rule_service.preview(&body.content);
    Json(json!({
        "rendered_text": body.content,
        "unicode_scalars": check.unicode_scalars,
        "utf8_bytes": check.utf8_bytes,
        "valid": check.valid,
        "violations": check.violations.iter().map(|v| json!({
            "field": "content", "code": "content_invalid", "message": v
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct MatchPreviewRequest {
    pub item_id: String,
    pub sku_key: String,
}

pub async fn match_preview_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<MatchPreviewRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match state
        .inner
        .rule_service
        .match_preview(&account_id, &body.item_id, &body.sku_key)
        .await
    {
        Ok(MatchPreview::Matched {
            rule_id,
            rule_version,
        }) => Json(json!({
            "state": "matched", "rule_id": rule_id, "rule_version": rule_version,
        }))
        .into_response(),
        Ok(MatchPreview::None) => {
            Json(json!({ "state": "none", "reason": "无启用规则匹配完整规格组合" })).into_response()
        }
        Ok(MatchPreview::Ambiguous) => {
            Json(json!({ "state": "ambiguous", "reason": "多条启用规则,不得任选一条" }))
                .into_response()
        }
        Err(e) => rule_err(e),
    }
}

/// 商品同步入口:真实协议适配器接入前(US2 剩余),明确不支持而非伪成功。
pub async fn start_item_sync(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    if state.inner.config.profile == crate::transport::state::ExecutionProfile::Mock {
        // mock profile 的同步经 dev 场景驱动(scenario ordinary_payment 等)
        return err_shared(
            ErrorCode::UnsupportedCapability,
            "mock 同步经 /dev/scenarios 驱动",
        );
    }
    err_shared(
        ErrorCode::UnsupportedCapability,
        &format!("账号 {account_id}:真实平台同步待 US2 协议接入(SC-091),不伪成功"),
    )
}
