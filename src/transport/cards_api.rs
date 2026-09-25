//! 卡密库存 HTTP API(T013,contracts §1 全部 9 端点):
//! 组列表/创建/详情/更新/删除、追加、批量导入(multipart,413 上限)、
//! test-api(不落库)、条目审计分页(永不返回明文)。
//! 薄 handler:鉴权+参数解析+调用应用服务,不含 SQL(宪章 II)。

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

use crate::adapters::cardsupplier::http::HttpCardSupplier;
use crate::application::cards::{
    CardPoolDraft, CardPoolSummary, CardService, CardsError, IMPORT_MAX_BYTES,
};
use crate::domain::cards::{ApiCardConfig, CardPoolKind};
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::{AppState, SharedState};

fn service(state: &AppState) -> CardService {
    // 服务无状态:经 DbThread + 数据密钥句柄即取即用(与 StatsService 同模式)
    CardService::new(state.inner.db.clone(), state.inner.key.clone())
}

fn cards_err(e: CardsError) -> Response {
    match e {
        CardsError::InvalidRequest(_) | CardsError::Import(_) => {
            err_shared(ErrorCode::InvalidRequest, &e.to_string())
        }
        CardsError::NotFound => err_shared(ErrorCode::ResourceNotFound, &e.to_string()),
        CardsError::VersionConflict => err_shared(ErrorCode::VersionConflict, &e.to_string()),
        CardsError::Referenced { .. } => err_shared(ErrorCode::ReferencedResource, &e.to_string()),
        CardsError::PayloadTooLarge => err_shared(ErrorCode::PayloadTooLarge, &e.to_string()),
        CardsError::Db(_) | CardsError::Sqlite(_) | CardsError::Crypto(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "卡密服务不可用")
        }
    }
}

fn summary_json(s: &CardPoolSummary) -> serde_json::Value {
    json!({
        "id": s.id,
        "name": s.name,
        "kind": s.kind,
        "enabled": s.enabled,
        "delay_seconds": s.delay_seconds,
        "description": s.description,
        "version": s.version,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
        // data 组带库存计数(CardPoolDto.stock)
        "stock": s.stock.map(|c| json!({
            "available": c.available, "reserved": c.reserved, "used": c.used,
        })),
        "content_set": s.content_set,
        // api 组脱敏摘要:headers/params/body 只回"是否已配置"
        "api_config": s.api_config.as_ref().map(|a| json!({
            "url": a.url,
            "method": a.method,
            "timeout_ms": a.timeout_ms,
            "content_type": a.content_type,
            "response_path": a.response_path,
            "retry_enabled": a.retry_enabled,
            "headers_configured": a.headers_configured,
            "params_configured": a.params_configured,
            "body_configured": a.body_configured,
        })),
    })
}

/// GET /card-pools?kind=&search=
pub async fn list_card_pools(
    State(state): SharedState,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let kind = match params.get("kind").map(|s| s.as_str()) {
        None | Some("") => None,
        Some(raw) => match CardPoolKind::parse(raw) {
            Some(k) => Some(k),
            None => {
                return err_shared(ErrorCode::InvalidRequest, "kind 过滤值非法(data/text/image/api)")
            }
        },
    };
    let search = params.get("search").cloned();
    match service(&state).list_pools(kind, search).await {
        Ok(pools) => Json(json!({
            "items": pools.iter().map(summary_json).collect::<Vec<_>>(),
            "next_cursor": null,
        }))
        .into_response(),
        Err(e) => cards_err(e),
    }
}

#[derive(Deserialize)]
pub struct PoolWriteRequest {
    name: String,
    kind: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    delay_seconds: i64,
    #[serde(default)]
    description: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    entries: Vec<String>,
    #[serde(default)]
    api_config: Option<ApiCardConfig>,
}

fn default_true() -> bool {
    true
}

fn draft_of(body: &PoolWriteRequest) -> Result<CardPoolDraft, Response> {
    let kind = CardPoolKind::parse(&body.kind).ok_or_else(|| {
        err_shared(
            ErrorCode::InvalidRequest,
            "kind 必须为 data/text/image/api 之一",
        )
    })?;
    Ok(CardPoolDraft {
        name: body.name.clone(),
        kind,
        enabled: body.enabled,
        delay_seconds: body.delay_seconds,
        description: body.description.clone(),
        content: body.content.clone(),
        entries: body.entries.clone(),
        api_config: body.api_config.clone(),
    })
}

/// POST /card-pools(201)
pub async fn create_card_pool(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<PoolWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let draft = match draft_of(&body) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    match service(&state).create_pool(draft).await {
        Ok(summary) => (StatusCode::CREATED, Json(summary_json(&summary))).into_response(),
        Err(e) => cards_err(e),
    }
}

/// GET /card-pools/{id}
pub async fn get_card_pool(
    State(state): SharedState,
    headers: HeaderMap,
    Path(pool_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    match service(&state).get_pool(&pool_id).await {
        Ok(summary) => Json(summary_json(&summary)).into_response(),
        Err(e) => cards_err(e),
    }
}

/// PUT /card-pools/{id}(乐观锁;text/image 换内容=整体替换)
pub async fn update_card_pool(
    State(state): SharedState,
    headers: HeaderMap,
    Path(pool_id): Path<String>,
    Json(body): Json<UpdatePoolRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let p = &body.pool;
    match service(&state)
        .update_pool(
            &pool_id,
            body.expected_version,
            Some(p.name.clone()),
            Some(p.enabled),
            Some(p.delay_seconds),
            Some(p.description.clone()),
            p.content.clone(),
            p.api_config.clone(),
        )
        .await
    {
        Ok(summary) => Json(summary_json(&summary)).into_response(),
        Err(e) => cards_err(e),
    }
}

/// 更新请求 = 组字段 + 乐观锁版本(写接口约定)
#[derive(Deserialize)]
pub struct UpdatePoolRequest {
    #[serde(flatten)]
    pool: PoolWriteRequest,
    expected_version: i64,
}

/// DELETE /card-pools/{id}(204;被引用 409)
pub async fn delete_card_pool(
    State(state): SharedState,
    headers: HeaderMap,
    Path(pool_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state).delete_pool(&pool_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => cards_err(e),
    }
}

#[derive(Deserialize)]
pub struct AppendRequest {
    lines: Vec<String>,
}

/// POST /card-pools/{id}/append-data
pub async fn append_data(
    State(state): SharedState,
    headers: HeaderMap,
    Path(pool_id): Path<String>,
    Json(body): Json<AppendRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state).append_data(&pool_id, body.lines).await {
        Ok(result) => Json(json!({
            "appended": result.appended,
            "skipped_empty": result.skipped_empty,
            "skipped_duplicate": result.skipped_duplicate,
        }))
        .into_response(),
        Err(e) => cards_err(e),
    }
}

/// POST /card-pools/batch-import(multipart file 字段;xlsx/csv/tsv)
/// 同步解析逐行返回结果(无长任务;契约 §1)。
pub async fn batch_import(
    State(state): SharedState,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    // Content-Length 预检:超限立即 413,不进入流式读取(multipart 开销按 64 KiB 余量)
    if let Some(len) = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        && len > IMPORT_MAX_BYTES + 64 * 1024
    {
        return err_shared(ErrorCode::PayloadTooLarge, "导入文件超过大小限制(2 MiB)");
    }
    let mut filename: Option<String> = None;
    let mut data: Vec<u8> = Vec::new();
    loop {
        let field = match multipart.next_field().await {
            Ok(f) => f,
            Err(_) => return err_shared(ErrorCode::InvalidRequest, "multipart 请求体非法"),
        };
        let Some(mut field) = field else { break };
        if field.name() != Some("file") {
            continue; // 忽略其他字段(注释等)
        }
        filename = field.file_name().map(|s| s.to_string());
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    data.extend_from_slice(&chunk);
                    if data.len() > IMPORT_MAX_BYTES {
                        return err_shared(
                            ErrorCode::PayloadTooLarge,
                            "导入文件超过大小限制(2 MiB)",
                        );
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    // 体积超限在传输层以长度限制错误浮现:按 413 上报;其余按非法请求
                    let msg = e.to_string();
                    if msg.to_ascii_lowercase().contains("limit") {
                        return err_shared(
                            ErrorCode::PayloadTooLarge,
                            "导入文件超过大小限制(2 MiB)",
                        );
                    }
                    return err_shared(ErrorCode::InvalidRequest, "文件读取中断");
                }
            }
        }
    }
    let Some(filename) = filename else {
        return err_shared(ErrorCode::InvalidRequest, "缺少 file 字段(文件名后缀决定解析器)");
    };
    match service(&state).batch_import(data, &filename).await {
        Ok(report) => Json(json!({
            "total": report.total,
            "succeeded": report.succeeded,
            "failed": report.failed.iter().map(|f| json!({
                "row": f.row, "error": f.error,
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => cards_err(e),
    }
}

/// POST /card-pools/test-api(API 配置一次性测试,不落库)
pub async fn test_api(
    State(state): SharedState,
    headers: HeaderMap,
    Json(cfg): Json<ApiCardConfig>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let supplier = HttpCardSupplier::new();
    match service(&state).test_api(&supplier, &cfg).await {
        Ok(result) => Json(json!({
            "ok": result.ok,
            "sample": result.sample,
            "latency_ms": result.latency_ms,
            "error": result.error,
        }))
        .into_response(),
        Err(e) => cards_err(e),
    }
}

#[derive(Deserialize)]
pub struct EntriesQuery {
    state: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

/// GET /card-pools/{id}/entries?state=&cursor=(永不返回明文)
pub async fn list_entries(
    State(state): SharedState,
    headers: HeaderMap,
    Path(pool_id): Path<String>,
    Query(q): Query<EntriesQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    match service(&state)
        .list_entries(&pool_id, q.state, q.cursor, limit)
        .await
    {
        Ok((items, next_cursor)) => Json(json!({
            "items": items.iter().map(|e| json!({
                "id": e.id,
                "state": e.state,
                "origin": e.origin,
                "content_digest_prefix": e.content_digest_prefix,
                "reserved_order_id": e.reserved_order_id,
                "reserved_delivery_id": e.reserved_delivery_id,
                "request_key": e.request_key,
                "reserved_at": e.reserved_at,
                "used_at": e.used_at,
                "created_at": e.created_at,
            })).collect::<Vec<_>>(),
            "next_cursor": next_cursor,
        }))
        .into_response(),
        Err(e) => cards_err(e),
    }
}
