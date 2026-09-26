//! 007 US4 在线聊天 HTTP API(T052,contracts §4 全部端点):
//! unread-summary / 会话分页 / 消息游标(新→旧)/ 发送文本(同步等待 ≤15s)/
//! 图片上传(能力门禁+multipart ≤10MB)/ 人工重试 / 未读清零 / 删除会话 /
//! 快捷回复(≤50)/ 买家备注。错误映射沿用 err_shared;SQL 在仓储层(宪章 II)。

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::application::chat::{ChatError, IMAGE_MAX_BYTES};
use crate::domain::time_util::format_rfc3339;
use crate::transport::error::ErrorCode;
use crate::transport::routes::{err_shared, require_admin_public};
use crate::transport::state::SharedState;

fn chat_err(e: ChatError) -> Response {
    match e {
        ChatError::ConversationNotFound | ChatError::MessageNotFound => {
            err_shared(ErrorCode::ResourceNotFound, &e.to_string())
        }
        ChatError::AccountUnavailable => {
            err_shared(ErrorCode::AccountUnavailable, "账号离线,暂不能发送消息")
        }
        ChatError::ImageUnsupported => err_shared(
            ErrorCode::UnsupportedCapability,
            "当前适配器不支持发送图片消息",
        ),
        ChatError::UnsafeRetry => err_shared(ErrorCode::UnsafeRetry, &e.to_string()),
        ChatError::ContentTooLong => {
            err_shared(ErrorCode::ContentTooLong, "消息内容超过 2000 字")
        }
        ChatError::PayloadTooLarge => {
            err_shared(ErrorCode::PayloadTooLarge, "上传文件超过 10MB 上限")
        }
        ChatError::UnsupportedImageType => {
            err_shared(ErrorCode::InvalidRequest, &e.to_string())
        }
        ChatError::ReplyLimitReached => {
            err_shared(ErrorCode::ReplyLimitReached, "快捷回复已达 50 条上限")
        }
        ChatError::InvalidCursor => err_shared(ErrorCode::InvalidCursor, "游标格式非法"),
        ChatError::UploadIo(_) | ChatError::Invalid(_) => {
            err_shared(ErrorCode::InvalidRequest, &e.to_string())
        }
        ChatError::Db(_) | ChatError::Sqlite(_) => {
            err_shared(ErrorCode::PersistenceUnavailable, "聊天服务暂不可用")
        }
    }
}

/// 无 ChatService 句柄(构建异常)时的统一降级。
fn unavailable() -> Response {
    err_shared(ErrorCode::PersistenceUnavailable, "聊天服务不可用")
}

// ---------- DTO ----------

fn conversation_dto(c: &crate::adapters::sqlite::repos::chat::ConversationRow) -> serde_json::Value {
    json!({
        "id": c.id,
        "account_id": c.account_id,
        "peer_buyer_id": c.peer_buyer_id,
        "peer_nickname": c.peer_nickname,
        "chat_id": c.chat_id,
        "last_item_id": c.last_item_id,
        "last_message_at": c.last_message_at,
        "last_message_preview": c.last_message_preview,
        "unread_count": c.unread_count,
    })
}

fn message_dto(m: &crate::adapters::sqlite::repos::chat::MessageRow) -> serde_json::Value {
    json!({
        "id": m.id,
        "conversation_id": m.conversation_id,
        "direction": m.direction,
        "msg_kind": m.msg_kind,
        "body_text": m.body_text,
        "image_path": m.image_path,
        "status": m.status,
        "error_hint": m.error_hint,
        "item_snapshot": m.item_snapshot.as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
        "created_at": m.created_at,
        "read_at": m.read_at,
    })
}

fn quick_reply_dto(r: &crate::adapters::sqlite::repos::chat::QuickReplyRow) -> serde_json::Value {
    json!({ "id": r.id, "body": r.body, "created_at": r.created_at })
}

// ---------- 会话与消息 ----------

/// GET /chat/unread-summary:{total, by_account}(侧边栏/Tab 徽标)。
pub async fn unread_summary(
    State(state): SharedState,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.unread_summary().await {
        Ok((total, by_account)) => {
            let mut map = serde_json::Map::new();
            for (aid, n) in by_account {
                map.insert(aid, json!(n));
            }
            Json(json!({ "total": total, "by_account": map })).into_response()
        }
        Err(e) => chat_err(e),
    }
}

#[derive(Deserialize)]
pub struct SessionsQuery {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub unread_only: Option<bool>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// GET /accounts/{aid}/chat/sessions:会话分页(隐藏除外;搜索/只看未读)。
pub async fn list_sessions(
    State(state): SharedState,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Query(q): Query<SessionsQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    match chat
        .list_conversations(
            &account_id,
            q.search.as_deref(),
            q.unread_only.unwrap_or(false),
            q.cursor.as_deref(),
            limit,
        )
        .await
    {
        Ok((items, next_cursor)) => Json(json!({
            "items": items.iter().map(conversation_dto).collect::<Vec<_>>(),
            "next_cursor": next_cursor,
        }))
        .into_response(),
        Err(e) => chat_err(e),
    }
}

#[derive(Deserialize)]
pub struct MessagesQuery {
    #[serde(default)]
    pub before_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// GET /chat/conversations/{cid}/messages:消息游标(新→旧,"加载更早")。
pub async fn list_messages(
    State(state): SharedState,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Query(q): Query<MessagesQuery>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    match chat
        .list_messages(&conversation_id, q.before_id.as_deref(), limit)
        .await
    {
        Ok((items, next_cursor)) => Json(json!({
            "items": items.iter().map(message_dto).collect::<Vec<_>>(),
            "next_cursor": next_cursor,
        }))
        .into_response(),
        Err(e) => chat_err(e),
    }
}

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub text: String,
}

/// POST /chat/conversations/{cid}/messages:{text} ≤2000 字,同步等待发送结果(≤15s)。
pub async fn send_message(
    State(state): SharedState,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(body): Json<SendMessageRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.send_text(&conversation_id, &body.text).await {
        Ok(row) => (StatusCode::CREATED, Json(message_dto(&row))).into_response(),
        Err(e) => chat_err(e),
    }
}

/// POST /chat/conversations/{cid}/images:multipart ≤10MB(能力门禁;文件不入库)。
/// Content-Length 预检超限立即 413;流式读取按应用层 10MB 上限中止(内存有界)。
pub async fn send_image(
    State(state): SharedState,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    // 能力门禁先于读体(FR-042:false 时 403,不接收上传)
    if !chat.chat_send_image_supported() {
        return err_shared(
            ErrorCode::UnsupportedCapability,
            "当前适配器未声明图片发送能力(chat_send_image=false)",
        );
    }
    if let Some(len) = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        && len > IMAGE_MAX_BYTES + 64 * 1024
    {
        return err_shared(ErrorCode::PayloadTooLarge, "上传图片超过 10MB 上限");
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
            continue;
        }
        filename = field.file_name().map(|s| s.to_string());
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    data.extend_from_slice(&chunk);
                    if data.len() > IMAGE_MAX_BYTES {
                        return err_shared(ErrorCode::PayloadTooLarge, "上传图片超过 10MB 上限");
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let msg = e.to_string();
                    if msg.to_ascii_lowercase().contains("limit") {
                        return err_shared(ErrorCode::PayloadTooLarge, "上传图片超过 10MB 上限");
                    }
                    return err_shared(ErrorCode::InvalidRequest, "文件读取中断");
                }
            }
        }
        break; // 只取第一个 file 字段
    }
    let Some(filename) = filename else {
        return err_shared(ErrorCode::InvalidRequest, "缺少 multipart 文件字段 file");
    };
    match chat.send_image(&conversation_id, &filename, &data).await {
        Ok(row) => (StatusCode::CREATED, Json(message_dto(&row))).into_response(),
        Err(e) => chat_err(e),
    }
}

/// POST /chat/messages/{mid}/retry:failed 重发(uncertain 拒绝 unsafe_retry)。
pub async fn retry_message(
    State(state): SharedState,
    headers: HeaderMap,
    Path(message_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.retry(&message_id).await {
        Ok(row) => (StatusCode::CREATED, Json(message_dto(&row))).into_response(),
        Err(e) => chat_err(e),
    }
}

/// POST /chat/conversations/{cid}/read:打开会话未读清零。
pub async fn mark_read(
    State(state): SharedState,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.mark_read(&conversation_id).await {
        Ok(()) => Json(json!({ "conversation_id": conversation_id, "unread_count": 0 })).into_response(),
        Err(e) => chat_err(e),
    }
}

/// DELETE /chat/conversations/{cid}:本机隐藏+清空展示消息(确认流由前端承担)。
pub async fn delete_conversation(
    State(state): SharedState,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.delete_conversation(&conversation_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => chat_err(e),
    }
}

// ---------- 快捷回复 ----------

pub async fn list_quick_replies(State(state): SharedState, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.quick_replies().await {
        Ok(rows) => Json(json!({
            "items": rows.iter().map(quick_reply_dto).collect::<Vec<_>>(),
            "next_cursor": null,
        }))
        .into_response(),
        Err(e) => chat_err(e),
    }
}

#[derive(Deserialize)]
pub struct QuickReplyRequest {
    pub body: String,
}

pub async fn create_quick_reply(
    State(state): SharedState,
    headers: HeaderMap,
    Json(body): Json<QuickReplyRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.add_quick_reply(&body.body).await {
        Ok(row) => (StatusCode::CREATED, Json(quick_reply_dto(&row))).into_response(),
        Err(e) => chat_err(e),
    }
}

pub async fn delete_quick_reply(
    State(state): SharedState,
    headers: HeaderMap,
    Path(reply_id): Path<String>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.delete_quick_reply(&reply_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err_shared(ErrorCode::ResourceNotFound, "快捷回复不存在"),
        Err(e) => chat_err(e),
    }
}

// ---------- 买家备注 ----------

pub async fn get_buyer_note(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, buyer_id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, false).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.buyer_note(&account_id, &buyer_id).await {
        Ok(note) => Json(json!({ "note": note })).into_response(),
        Err(e) => chat_err(e),
    }
}

#[derive(Deserialize)]
pub struct BuyerNoteRequest {
    pub note: String,
}

pub async fn put_buyer_note(
    State(state): SharedState,
    headers: HeaderMap,
    Path((account_id, buyer_id)): Path<(String, String)>,
    Json(body): Json<BuyerNoteRequest>,
) -> Response {
    if let Err(e) = require_admin_public(&state, &headers, true).await {
        return e.into_response();
    }
    let Some(chat) = state.inner.chat.as_ref() else {
        return unavailable();
    };
    match chat.save_buyer_note(&account_id, &buyer_id, &body.note).await {
        Ok(()) => Json(json!({
            "account_id": account_id,
            "buyer_id": buyer_id,
            "note": body.note,
            "updated_at": format_rfc3339(crate::domain::time_util::utc_now_ms()),
        }))
        .into_response(),
        Err(e) => chat_err(e),
    }
}
