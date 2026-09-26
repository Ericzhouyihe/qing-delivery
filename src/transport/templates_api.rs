//! 发货模板 HTTP API(T025,contracts §2 四端点):
//! GET 列表(含 used_by_rules 计数)、POST 创建、PUT 更新(整体替换消息列表,
//! 乐观锁)、DELETE 删除(被规则/变体引用 409 referenced_resource)。
//! TemplateDto:{id,name,enabled,messages[],keys:{cards[],custom[]},
//! used_by_rules,version}。薄 handler:鉴权+参数解析+调用应用服务(宪章 II)。
//! 校验失败映射 invalid_request(本项目错误表,与 US1 卡密 API 同口径)。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

use crate::application::templates::{
    TemplateDraft, TemplateService, TemplateSummary, TemplatesError,
};
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::{AppState, SharedState};

fn service(state: &AppState) -> TemplateService {
    // 服务无状态:经 DbThread 句柄即取即用(与卡密 API 同模式)
    TemplateService::new(state.inner.db.clone())
}

fn templates_err(e: TemplatesError) -> Response {
    match e {
        TemplatesError::InvalidRequest(_) => {
            err_shared(ErrorCode::InvalidRequest, &e.to_string())
        }
        TemplatesError::NotFound => err_shared(ErrorCode::ResourceNotFound, &e.to_string()),
        TemplatesError::VersionConflict => err_shared(ErrorCode::VersionConflict, &e.to_string()),
        TemplatesError::Referenced(_) => err_shared(ErrorCode::ReferencedResource, &e.to_string()),
        TemplatesError::Db(_) | TemplatesError::Sqlite(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "模板服务不可用")
        }
    }
}

fn dto_json(t: &TemplateSummary) -> serde_json::Value {
    json!({
        "id": t.id,
        "name": t.name,
        "enabled": t.enabled,
        "messages": t.messages,
        // 占位符引用的变量名(前端绑定提示;data-model 语法约定)
        "keys": {
            "cards": t.keys.cards,
            "custom": t.keys.custom,
        },
        "used_by_rules": t.used_by_rules,
        "version": t.version,
    })
}

/// GET /delivery-templates?search=(列表含 used_by_rules 计数)
pub async fn list_templates(
    State(state): SharedState,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let search = params.get("search").cloned();
    match service(&state).list(search).await {
        Ok(items) => Json(json!({
            "items": items.iter().map(dto_json).collect::<Vec<_>>(),
            "next_cursor": null,
        }))
        .into_response(),
        Err(e) => templates_err(e),
    }
}

#[derive(Deserialize)]
pub struct TemplateWriteRequest {
    name: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    messages: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// POST /delivery-templates(201;占位符/条数/长度校验失败拒绝)
pub async fn create_template(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<TemplateWriteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let draft = TemplateDraft {
        name: body.name.clone(),
        enabled: body.enabled,
        messages: body.messages.clone(),
    };
    match service(&state).create(draft).await {
        Ok(summary) => (StatusCode::CREATED, Json(dto_json(&summary))).into_response(),
        Err(e) => templates_err(e),
    }
}

/// 更新请求 = 模板字段 + 乐观锁版本(写接口约定)
#[derive(Deserialize)]
pub struct UpdateTemplateRequest {
    #[serde(flatten)]
    template: TemplateWriteRequest,
    expected_version: i64,
}

/// PUT /delivery-templates/{id}(消息整体替换;409 version_conflict)
pub async fn update_template(
    State(state): SharedState,
    headers: HeaderMap,
    Path(template_id): Path<String>,
    Json(body): Json<UpdateTemplateRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state)
        .update(
            &template_id,
            body.expected_version,
            Some(body.template.name.clone()),
            Some(body.template.enabled),
            Some(body.template.messages.clone()),
        )
        .await
    {
        Ok(summary) => Json(dto_json(&summary)).into_response(),
        Err(e) => templates_err(e),
    }
}

/// DELETE /delivery-templates/{id}(204;被引用 409 referenced_resource)
pub async fn delete_template(
    State(state): SharedState,
    headers: HeaderMap,
    Path(template_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    match service(&state).delete(&template_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => templates_err(e),
    }
}
