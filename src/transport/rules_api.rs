//! 007 US3 规则扩展 HTTP API(T039,contracts §3):
//! 关键词回复 CRUD(/accounts/{id}/reply-rules)与账号默认回复
//! (/accounts/{id}/default-reply,含清空回复记录)。
//! ai-settings 端点依赖 0006 迁移的 accounts 列,本轮不做(T078)。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::application::catalog::replies::{
    DefaultReplyDraft, DefaultReplyService, RepliesError, ReplyRuleDraft, ReplyRulesService,
};
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

fn replies_err(e: RepliesError) -> Response {
    match e {
        RepliesError::Invalid(_) => err_shared(ErrorCode::InvalidRequest, &e.to_string()),
        RepliesError::NotFound => err_shared(ErrorCode::ResourceNotFound, &e.to_string()),
        RepliesError::Db(_) | RepliesError::Sqlite(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "回复规则服务不可用")
        }
    }
}

fn reply_rule_dto(row: &crate::adapters::sqlite::repos::rules_ext::ReplyRuleRow) -> serde_json::Value {
    json!({
        "id": row.id,
        "account_id": row.account_id,
        "keyword": row.keyword,
        "reply_kind": row.reply_kind,
        "reply_text": row.reply_text,
        "reply_image_url": row.reply_image_url,
        "enabled": row.enabled,
        "item_ids": row.item_ids,
    })
}

#[derive(Deserialize)]
pub struct ReplyRuleRequest {
    pub keyword: String,
    pub reply_kind: String,
    #[serde(default)]
    pub reply_text: Option<String>,
    #[serde(default)]
    pub reply_image_url: Option<String>,
    pub enabled: bool,
    #[serde(default)]
    pub item_ids: Vec<String>,
}

fn reply_draft(body: &ReplyRuleRequest) -> ReplyRuleDraft {
    ReplyRuleDraft {
        keyword: body.keyword.clone(),
        reply_kind: body.reply_kind.clone(),
        reply_text: body.reply_text.clone(),
        reply_image_url: body.reply_image_url.clone(),
        enabled: body.enabled,
        item_ids: body.item_ids.clone(),
    }
}

pub async fn list_reply_rules(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let svc = ReplyRulesService::new(state.inner.db.clone());
    match svc.list(&account_id).await {
        Ok(rows) => Json(json!({
            "items": rows.iter().map(reply_rule_dto).collect::<Vec<_>>(),
            "next_cursor": null,
        }))
        .into_response(),
        Err(e) => replies_err(e),
    }
}

pub async fn create_reply_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<ReplyRuleRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let svc = ReplyRulesService::new(state.inner.db.clone());
    match svc.create(&account_id, reply_draft(&body)).await {
        Ok(row) => (StatusCode::CREATED, Json(reply_rule_dto(&row))).into_response(),
        Err(e) => replies_err(e),
    }
}

pub async fn update_reply_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, reply_rule_id)): Path<(String, String)>,
    Json(body): Json<ReplyRuleRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let svc = ReplyRulesService::new(state.inner.db.clone());
    match svc.update(&account_id, &reply_rule_id, reply_draft(&body)).await {
        Ok(row) => Json(reply_rule_dto(&row)).into_response(),
        Err(e) => replies_err(e),
    }
}

pub async fn delete_reply_rule(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, reply_rule_id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let svc = ReplyRulesService::new(state.inner.db.clone());
    match svc.delete(&account_id, &reply_rule_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err_shared(ErrorCode::ResourceNotFound, "关键词规则不存在"),
        Err(e) => replies_err(e),
    }
}

#[derive(Deserialize)]
pub struct DefaultReplyRequest {
    pub enabled: bool,
    #[serde(default)]
    pub reply_text: Option<String>,
    #[serde(default)]
    pub reply_image_url: Option<String>,
    #[serde(default = "default_reply_once")]
    pub reply_once: bool,
}

fn default_reply_once() -> bool {
    true
}

pub async fn get_default_reply(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let svc = DefaultReplyService::new(state.inner.db.clone());
    let records = svc.records(&account_id, 20).await.unwrap_or_default();
    match svc.get(&account_id).await {
        Ok(row) => Json(json!({
            "account_id": row.account_id,
            "enabled": row.enabled,
            "reply_text": row.reply_text,
            "reply_image_url": row.reply_image_url,
            "reply_once": row.reply_once,
            "updated_at": row.updated_at,
            "recent_records": records.iter().map(|r| json!({
                "id": r.id, "buyer_id": r.buyer_id, "state": r.state, "sent_at": r.sent_at,
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => replies_err(e),
    }
}

pub async fn put_default_reply(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Json(body): Json<DefaultReplyRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let svc = DefaultReplyService::new(state.inner.db.clone());
    let draft = DefaultReplyDraft {
        enabled: body.enabled,
        reply_text: body.reply_text.clone(),
        reply_image_url: body.reply_image_url.clone(),
        reply_once: body.reply_once,
    };
    match svc.put(&account_id, draft).await {
        Ok(row) => Json(json!({
            "account_id": row.account_id,
            "enabled": row.enabled,
            "reply_text": row.reply_text,
            "reply_image_url": row.reply_image_url,
            "reply_once": row.reply_once,
            "updated_at": row.updated_at,
        }))
        .into_response(),
        Err(e) => replies_err(e),
    }
}

/// 清空默认回复记录(FR-034;破坏性操作走既有确认流,由前端承担)。
pub async fn clear_default_reply_records(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let svc = DefaultReplyService::new(state.inner.db.clone());
    match svc.clear_records(&account_id).await {
        Ok(cleared) => Json(json!({ "cleared": cleared })).into_response(),
        Err(e) => replies_err(e),
    }
}
