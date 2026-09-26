//! 007 US4 在线聊天用例(T050):入站摄取(幂等/未读/触发自动回复分流)、
//! 文本与图片发送(sending 行→SendOutcome 四分类落状态,同步等待 ≤15s)、
//! 人工重试(仅 failed;uncertain 拒绝 unsafe_retry)、未读清零、删除会话
//! (本机隐藏+清展示消息)、快捷回复(≤50)、买家备注、未读汇总与
//! OutgoingMessageEvidence 回填。
//! 宪章红线:网络不进 DB 事务;uncertain 不自动重发;聊天零订单/交付写入;
//! 明文仅限 chat body(data-model D15);上传文件不入库。
//!
//! OutgoingMessageEvidence 消费方式(T050 注明):不经事件管道改造,
//! ChatService 直接消费该事件——由 supervisor::dispatch_loop 分支调用
//! `apply_evidence`(与 ChatMessageReceived 同法);管道内它仍为 Observed。

use std::path::PathBuf;

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::chat::{
    self, ConversationRow, MessageRow, QuickReplyRow,
};
use crate::application::ports::platform::{
    CapabilitySet, ChatPeer, ChatSendKind, ContentForSend, PlatformAdapter, RequestContext,
    SendOutcome,
};
use crate::application::replies::{ReplyDispatch, ReplyService};
use crate::domain::chat::{
    self as chat_domain, ChatMsgKind, OutgoingStatus,
};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;
use sha2::Digest;

/// 同步等待发送结果的超时上限(contracts §4:≤15s)
pub const SEND_WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(15);
/// 图片上传上限(FR-041:≤10MB)
pub const IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;
/// 图片扩展名白名单(存数据目录 uploads/chat/,UUID 文件名+白名单扩展)
const IMAGE_EXTS: [&str; 5] = ["jpg", "jpeg", "png", "gif", "webp"];

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error("会话不存在")]
    ConversationNotFound,
    #[error("消息不存在")]
    MessageNotFound,
    #[error("账号不在线,无法发送")]
    AccountUnavailable,
    #[error("当前适配器不支持发送图片")]
    ImageUnsupported,
    #[error("仅失败消息可人工重试;结果未知(uncertain)消息禁止重发,请先人工核对")]
    UnsafeRetry,
    #[error("消息内容超过 2000 字")]
    ContentTooLong,
    #[error("上传文件超过 10MB 上限")]
    PayloadTooLarge,
    #[error("不支持的图片类型(仅 jpg/png/gif/webp)")]
    UnsupportedImageType,
    #[error("图片文件读写失败:{0}")]
    UploadIo(String),
    #[error("游标格式非法")]
    InvalidCursor,
    #[error("快捷回复已达 50 条上限")]
    ReplyLimitReached,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// 入站聊天消息(适配器事件 → 摄取用;dispatch 与测试共用)。
#[derive(Clone, Debug)]
pub struct IncomingChatMessage {
    pub account_id: String,
    pub platform_message_id: String,
    pub chat_id: Option<String>,
    pub buyer_id: String,
    pub msg_kind: ChatMsgKind,
    pub text: Option<String>,
    pub image_url: Option<String>,
    pub item_id: Option<String>,
}

/// 摄取结果(留痕/测试断言用)。
#[derive(Clone, Debug, PartialEq)]
pub enum IngestOutcome {
    /// 首次落库(并可能触发了自动回复分流)
    Stored,
    /// platform_message_id 重复:双路径防重(FR-046)
    Duplicate,
}

fn digest_of(text: &str) -> String {
    hex::encode(sha2::Sha256::digest(text.as_bytes()))
}

pub struct ChatService<A: PlatformAdapter + Clone + 'static> {
    db: DbThread,
    sender: A,
    replies: ReplyService<A>,
    /// 数据目录 uploads/ 根;None 时图片发送不可用(如实报错)
    uploads_root: Option<PathBuf>,
    /// 能力门禁(FR-042):chat_send_image=false 时图片端点 403
    capabilities: CapabilitySet,
}

impl<A: PlatformAdapter + Clone + 'static> ChatService<A> {
    pub fn new(db: DbThread, sender: A) -> Self {
        Self {
            replies: ReplyService::new(db.clone(), sender.clone()),
            db,
            sender,
            uploads_root: None,
            // 保守默认:live 未验证(false);mock 构造侧显式传 MOCK
            capabilities: CapabilitySet::LIVE,
        }
    }

    /// 注入 AI 提供方(US7 实装;测试用记录型假实现)。
    pub fn with_ai(mut self, ai: std::sync::Arc<dyn crate::application::replies::AiReplyProvider>) -> Self {
        self.replies = ReplyService::with_ai(self.db.clone(), self.sender.clone(), ai);
        self
    }

    /// 数据目录(uploads 根)与能力声明(mock 档两项 true;live 如实 false)。
    pub fn with_storage(mut self, uploads_root: PathBuf, capabilities: CapabilitySet) -> Self {
        self.uploads_root = Some(uploads_root);
        self.capabilities = capabilities;
        self
    }

    /// 入站摄取:platform_message_id 幂等(双路径防重)、会话 upsert、
    /// 未读按 domain 规则聚合、消息落库;入站文本触发 replies 分流
    /// (US3-B 通电;item_id 从消息透传;发送结果只写 chat 出站行,零订单写入)。
    pub async fn ingest(&self, msg: IncomingChatMessage) -> Result<IngestOutcome, ChatError> {
        let buyer_id = msg.buyer_id.trim().trim_end_matches("@goofish").to_string();
        if buyer_id.is_empty() {
            tracing::warn!(account = %msg.account_id, "聊天消息缺少买家标识,跳过摄取");
            return Ok(IngestOutcome::Stored);
        }
        let conv_id = ids::new_id("conv");
        let (stored, conversation_id) = self
            .db
            .call({
                let msg = msg.clone();
                let buyer_id = buyer_id.clone();
                let conv_id = conv_id.clone();
                move |conn| -> rusqlite::Result<(bool, String)> {
                    let conv = chat::upsert_conversation(
                        conn,
                        &conv_id,
                        &msg.account_id,
                        &buyer_id,
                        msg.chat_id.as_deref(),
                        None,
                    )?;
                    if chat::platform_message_seen(conn, &msg.platform_message_id)? {
                        return Ok((false, conv.id));
                    }
                    let message_id = ids::new_time_ordered_id("msg");
                    let created_at =
                        crate::domain::time_util::format_rfc3339(utc_now_ms());
                    chat::insert_incoming(
                        conn,
                        &chat::IncomingMessage {
                            id: &message_id,
                            conversation_id: &conv.id,
                            platform_message_id: &msg.platform_message_id,
                            kind: msg.msg_kind,
                            body_text: msg.text.as_deref(),
                            image_url: msg.image_url.as_deref(),
                            item_snapshot: None,
                            created_at: &created_at,
                        },
                    )?;
                    // 未读聚合 + 会话摘要(同闭包内串行,无竞态)
                    let unread =
                        chat_domain::unread_after(conv.unread_count, chat_domain::ChatDirection::In, msg.msg_kind);
                    let preview = chat_domain::preview_of(msg.msg_kind, msg.text.as_deref());
                    chat::apply_incoming_summary(
                        conn,
                        &conv.id,
                        &preview,
                        msg.item_id.as_deref(),
                        unread,
                    )?;
                    Ok((true, conv.id))
                }
            })
            .await??;
        if !stored {
            return Ok(IngestOutcome::Duplicate);
        }

        // 入站文本 → 自动回复分流(关键词 → AI → 默认;宪章:零订单/交付写入)
        if msg.msg_kind == ChatMsgKind::Text
            && msg.text.as_deref().is_some_and(|t| !t.trim().is_empty())
        {
            let dispatch = self
                .replies
                .resolve_and_send(
                    &msg.account_id,
                    msg.item_id.as_deref(),
                    &buyer_id,
                    msg.chat_id.as_deref(),
                    msg.text.as_deref().unwrap_or_default(),
                )
                .await;
            match dispatch {
                Ok(result) => {
                    self.record_reply_outgoing(&conversation_id, result).await?;
                }
                // 账号不存在/仓储异常:聊天消息已落库,分流失败仅留痕(不阻塞)
                Err(e) => {
                    tracing::warn!(account = %msg.account_id, error = %e, "自动回复分流失败(消息已落库)");
                }
            }
        }
        Ok(IngestOutcome::Stored)
    }

    /// 自动回复出站留痕:发送结果只写 chat_messages 出站行(Sent→sent、
    /// Uncertain→uncertain;NotSubmitted/Rejected/未命中不留行,与顺延语义一致)。
    async fn record_reply_outgoing(
        &self,
        conversation_id: &str,
        dispatch: ReplyDispatch,
    ) -> Result<(), ChatError> {
        let (kind, body, image, status, pmid, request_key, hint) = match &dispatch {
            ReplyDispatch::Sent {
                proof,
                payload,
                is_image,
                ..
            } => {
                let kind = if *is_image {
                    ChatMsgKind::Image
                } else {
                    ChatMsgKind::Text
                };
                (
                    kind,
                    (!*is_image).then(|| payload.clone()),
                    is_image.then(|| payload.clone()),
                    OutgoingStatus::Sent,
                    proof.platform_message_id.clone(),
                    Some(proof.request_id.clone()),
                    None,
                )
            }
            ReplyDispatch::Uncertain {
                hint,
                payload,
                is_image,
                ..
            } => {
                let kind = if *is_image {
                    ChatMsgKind::Image
                } else {
                    ChatMsgKind::Text
                };
                (
                    kind,
                    (!*is_image).then(|| payload.clone()),
                    is_image.then(|| payload.clone()),
                    OutgoingStatus::Uncertain,
                    None,
                    None,
                    hint.clone(),
                )
            }
            ReplyDispatch::None => return Ok(()),
        };
        let message_id = ids::new_time_ordered_id("msg");
        let request_key_fallback = ids::new_request_key();
        self.db
            .call({
                let mid = message_id.clone();
                let cid = conversation_id.to_string();
                let body = body.clone();
                let image = image.clone();
                let st = status.as_str().to_string();
                let pm = pmid.clone();
                let rk = request_key.clone().unwrap_or_else(|| request_key_fallback.clone());
                let h = hint.clone();
                move |conn| -> rusqlite::Result<()> {
                    chat::insert_outgoing(
                        conn,
                        &mid,
                        &cid,
                        kind,
                        body.as_deref(),
                        image.as_deref(),
                        &rk,
                    )?;
                    chat::update_outgoing_result(conn, &mid, &st, pm.as_deref(), h.as_deref(), None)?;
                    let preview = chat_domain::preview_of(kind, body.as_deref().or(image.as_deref()));
                    chat::apply_outgoing_summary(conn, &cid, &preview)
                }
            })
            .await??;
        Ok(())
    }

    /// 发送文本(FR-041:≤2000 字;账号须在线;同步等待 ≤15s)。
    /// 流程:校验 → 插 sending 行(发送前持久化)→ 网络发送(不进事务)→
    /// 按 SendOutcome 落终态(Accepted→sent+回执回填;NotSubmitted/Rejected→failed;
    /// Unknown→uncertain)。
    pub async fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
    ) -> Result<MessageRow, ChatError> {
        chat_domain::check_outgoing_text(text).map_err(|e| {
            if e.contains("超过") {
                ChatError::ContentTooLong
            } else {
                ChatError::Invalid(e)
            }
        })?;
        self.send_outgoing(conversation_id, ChatMsgKind::Text, Some(text), None)
            .await
    }

    /// 发送图片(能力门禁 FR-042):multipart 文件存数据目录 uploads/chat/
    /// (UUID 文件名+扩展白名单;≤10MB 超限 413;上传文件不入库)。
    pub async fn send_image(
        &self,
        conversation_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<MessageRow, ChatError> {
        if !self.capabilities.chat_send_image {
            return Err(ChatError::ImageUnsupported);
        }
        if bytes.len() > IMAGE_MAX_BYTES {
            return Err(ChatError::PayloadTooLarge);
        }
        let ext = filename
            .rsplit('.')
            .next()
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !IMAGE_EXTS.contains(&ext.as_str()) {
            return Err(ChatError::UnsupportedImageType);
        }
        let Some(root) = self.uploads_root.as_ref() else {
            return Err(ChatError::UploadIo("数据目录不可用,无法存储上传文件".into()));
        };
        let dir = root.join("chat");
        std::fs::create_dir_all(&dir).map_err(|e| ChatError::UploadIo(e.to_string()))?;
        let stored_name = format!("{}.{}", ids::new_request_key(), ext);
        let abs_path = dir.join(&stored_name);
        std::fs::write(&abs_path, bytes).map_err(|e| ChatError::UploadIo(e.to_string()))?;
        // image_path 存数据目录相对路径(uploads/chat/xxx)
        let rel_path = format!("uploads/chat/{stored_name}");
        self.send_outgoing(conversation_id, ChatMsgKind::Image, None, Some(&rel_path))
            .await
    }

    /// 人工重试(仅 failed;uncertain/sending/sent 拒绝 unsafe_retry)。
    /// 新 attempt 由新幂等键承载:新出站行(同正文),旧行保留审计。
    pub async fn retry(&self, message_id: &str) -> Result<MessageRow, ChatError> {
        let original = self
            .db
            .call({
                let id = message_id.to_string();
                move |conn| chat::get_message(conn, &id)
            })
            .await??
            .ok_or(ChatError::MessageNotFound)?;
        if original.direction != "out" {
            return Err(ChatError::Invalid("仅出站消息可重试".into()));
        }
        let status = OutgoingStatus::parse(original.status.as_deref().unwrap_or_default())
            .unwrap_or(OutgoingStatus::Cancelled);
        chat_domain::retry_guard(status).map_err(|_| ChatError::UnsafeRetry)?;
        self.send_outgoing(
            &original.conversation_id,
            ChatMsgKind::parse(&original.msg_kind).unwrap_or(ChatMsgKind::Text),
            original.body_text.as_deref(),
            original.image_path.as_deref(),
        )
        .await
    }

    /// 出站发送共用路径:插 sending 行 → 网络发送(事务外)→ 终态落库。
    async fn send_outgoing(
        &self,
        conversation_id: &str,
        kind: ChatMsgKind,
        body_text: Option<&str>,
        image_path: Option<&str>,
    ) -> Result<MessageRow, ChatError> {
        // 会话 + 账号核验(账号不在线 → account_unavailable 语义)
        let (conv, account) = self
            .db
            .call({
                let cid = conversation_id.to_string();
                move |conn| -> rusqlite::Result<Option<(ConversationRow, crate::adapters::sqlite::repos::accounts::AccountRow)>> {
                    let Some(conv) = chat::find_conversation(conn, &cid)? else {
                        return Ok(None);
                    };
                    if conv.hidden_at.is_some() {
                        return Ok(None); // 已删除(隐藏)会话不可再发送(404 语义)
                    }
                    let account =
                        crate::adapters::sqlite::repos::accounts::get(conn, &conv.account_id)?;
                    Ok(account.map(|a| (conv, a)))
                }
            })
            .await??
            .ok_or(ChatError::ConversationNotFound)?;
        if account.status != "online" {
            return Err(ChatError::AccountUnavailable);
        }

        // 发送前持久化:sending 行(网络不在事务内;崩溃后可见 sending 态)
        let message_id = ids::new_time_ordered_id("msg");
        let request_key = ids::new_request_key();
        self.db
            .call({
                let mid = message_id.clone();
                let cid = conversation_id.to_string();
                let body = body_text.map(|s| s.to_string());
                let img = image_path.map(|s| s.to_string());
                let rk = request_key.clone();
                move |conn| -> rusqlite::Result<()> {
                    chat::insert_outgoing(conn, &mid, &cid, kind, body.as_deref(), img.as_deref(), &rk)?;
                    let preview = chat_domain::preview_of(
                        kind,
                        body.as_deref().or(img.as_deref()),
                    );
                    chat::apply_outgoing_summary(conn, &cid, &preview)
                }
            })
            .await??;

        // 网络发送(事务外;正文/图片路径即发送内容,摘要覆盖全文)
        let payload = body_text
            .map(|s| s.to_string())
            .or_else(|| image_path.map(|s| s.to_string()))
            .unwrap_or_default();
        let peer = ChatPeer {
            buyer_id: conv.peer_buyer_id.trim_end_matches("@goofish").to_string(),
            chat_id: conv.chat_id.clone(),
        };
        let send_kind = match kind {
            ChatMsgKind::Image => ChatSendKind::Image { url: payload.clone() },
            _ => ChatSendKind::Text,
        };
        let content = ContentForSend {
            snapshot_id: format!("chat:{message_id}"),
            text: payload,
            text_digest: digest_of(&content_text_of(kind, body_text, image_path)),
        };
        let ctx = RequestContext {
            operation_id: ids::new_id("op"),
            account_id: account.id.clone(),
            credential_generation: account.credential_epoch,
            control_generation: account.control_epoch,
            deadline_ms: utc_now_ms() + 15_000,
        };
        let send_fut = self
            .sender
            .send_chat_message(&ctx, &peer, send_kind, &content);
        let outcome = match tokio::time::timeout(SEND_WAIT_LIMIT, send_fut).await {
            Ok(Ok(outcome)) => outcome,
            // Err(PlatformError)语义=未提交(与 ReplyService 同口径)→ failed 可人工重试
            Ok(Err(e)) => SendOutcome::NotSubmitted {
                retryable: !matches!(
                    e,
                    crate::application::ports::platform::PlatformError::BusinessRejected(_)
                ),
            },
            // 等待超时:可能已提交但无证据 → uncertain(禁止自动重发)
            Err(_) => SendOutcome::Unknown {
                hint: Some("等待发送结果超时(15s)".into()),
            },
        };

        // 终态落库(域层裁决 + 回执回填)
        let (status, pmid, hint, req) = match &outcome {
            SendOutcome::Accepted(proof) => (
                OutgoingStatus::Sent,
                proof.platform_message_id.clone(),
                None,
                Some(proof.request_id.clone()),
            ),
            SendOutcome::NotSubmitted { retryable } => (
                OutgoingStatus::Failed,
                None,
                Some(format!("未提交(可重试={retryable})")),
                None,
            ),
            SendOutcome::Rejected { safe_code } => (
                OutgoingStatus::Failed,
                None,
                Some(format!("平台拒绝:{safe_code}")),
                None,
            ),
            SendOutcome::Unknown { hint } => (OutgoingStatus::Uncertain, None, hint.clone(), None),
        };
        chat_domain::outcome_transition(OutgoingStatus::Sending).map_err(|e| ChatError::Invalid(e.to_string()))?;
        self.db
            .call({
                let mid = message_id.clone();
                let st = status.as_str().to_string();
                let pm = pmid.clone();
                let h = hint.clone();
                let rk = req.clone();
                move |conn| -> rusqlite::Result<()> {
                    chat::update_outgoing_result(conn, &mid, &st, pm.as_deref(), h.as_deref(), rk.as_deref())
                }
            })
            .await??;
        self.message(&message_id).await
    }

    /// OutgoingMessageEvidence 回填(T050,消费方式:dispatch_loop 直调):
    /// 按 request_key 匹配出站行,回填 platform_message_id;sending/uncertain
    /// 行按严格关联接纳证据补 sent。无匹配(交付类回执)返回 None,正常路径。
    pub async fn apply_evidence(
        &self,
        request_id: &str,
        platform_message_id: &str,
    ) -> Result<Option<(String, bool)>, ChatError> {
        let result = self
            .db
            .call({
                let rk = request_id.to_string();
                let pm = platform_message_id.to_string();
                move |conn| chat::backfill_by_request_key(conn, &rk, &pm)
            })
            .await??;
        if let Some((id, promoted)) = result.as_ref()
            && *promoted
        {
            tracing::info!(message = %id, "聊天发送回执:补记 sent(uncertain/sending → sent)");
        }
        Ok(result)
    }

    // ---------- 查询与轻量用例(transport 直用) ----------

    pub async fn message(&self, id: &str) -> Result<MessageRow, ChatError> {
        self.db
            .call({
                let id = id.to_string();
                move |conn| chat::get_message(conn, &id)
            })
            .await??
            .ok_or(ChatError::MessageNotFound)
    }

    pub async fn list_conversations(
        &self,
        account_id: &str,
        search: Option<&str>,
        unread_only: bool,
        cursor: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<ConversationRow>, Option<String>), ChatError> {
        let parsed = match cursor.filter(|c| !c.is_empty()) {
            Some(c) => {
                let mut parts = c.splitn(3, '|');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(a), Some(b), Some(i)) => (a.to_string(), b.to_string(), i.to_string()),
                    _ => return Err(ChatError::InvalidCursor),
                }
            }
            None => (String::new(), String::new(), String::new()),
        };
        let has_cursor = cursor.is_some_and(|c| !c.is_empty());
        self.db
            .call({
                let account = account_id.to_string();
                let search = search.map(|s| s.to_string());
                let (a, b, i) = parsed;
                move |conn| {
                    chat::list_conversations(
                        conn,
                        &account,
                        search.as_deref(),
                        unread_only,
                        has_cursor.then_some((a.as_str(), b.as_str(), i.as_str())),
                        limit,
                    )
                }
            })
            .await?
            .map_err(ChatError::from)
    }

    pub async fn list_messages(
        &self,
        conversation_id: &str,
        before_id: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, Option<String>), ChatError> {
        // 隐藏(已删除)会话视同不存在(404;删除=本机不可见)
        let visible = self
            .db
            .call({
                let cid = conversation_id.to_string();
                move |conn| {
                    Ok::<bool, rusqlite::Error>(
                        chat::find_conversation(conn, &cid)?
                            .is_some_and(|c| c.hidden_at.is_none()),
                    )
                }
            })
            .await??;
        if !visible {
            return Err(ChatError::ConversationNotFound);
        }
        self.db
            .call({
                let cid = conversation_id.to_string();
                let before = before_id.map(|s| s.to_string());
                move |conn| chat::list_messages(conn, &cid, before.as_deref(), limit)
            })
            .await?
            .map_err(ChatError::from)
    }

    /// 打开会话未读清零(FR-043)。
    pub async fn mark_read(&self, conversation_id: &str) -> Result<(), ChatError> {
        let updated = self
            .db
            .call({
                let cid = conversation_id.to_string();
                move |conn| chat::mark_conversation_read(conn, &cid)
            })
            .await??;
        if updated == 0 {
            return Err(ChatError::ConversationNotFound);
        }
        Ok(())
    }

    /// 删除会话:本机隐藏 + 物理清空该会话展示消息(FR-045;确认流由前端承担)。
    pub async fn delete_conversation(&self, conversation_id: &str) -> Result<(), ChatError> {
        let updated = self
            .db
            .call({
                let cid = conversation_id.to_string();
                move |conn| -> rusqlite::Result<i64> {
                    let existed = chat::find_conversation(conn, &cid)?.is_some();
                    chat::hide_conversation(conn, &cid)?;
                    Ok(existed as i64)
                }
            })
            .await??;
        if updated == 0 {
            return Err(ChatError::ConversationNotFound);
        }
        Ok(())
    }

    pub async fn unread_summary(&self) -> Result<(i64, Vec<(String, i64)>), ChatError> {
        self.db
            .call(move |conn| chat::unread_summary(conn))
            .await?
            .map_err(ChatError::from)
    }

    // ---------- 快捷回复 / 买家备注 ----------

    pub async fn quick_replies(&self) -> Result<Vec<QuickReplyRow>, ChatError> {
        self.db
            .call(move |conn| chat::list_quick_replies(conn))
            .await?
            .map_err(ChatError::from)
    }

    pub async fn add_quick_reply(&self, body: &str) -> Result<QuickReplyRow, ChatError> {
        chat_domain::check_quick_reply_body(body).map_err(ChatError::Invalid)?;
        // 上限检查与插入分两步:DbThread 单连接串行,同服务内无竞态窗口
        let count = self
            .db
            .call(move |conn| chat::count_quick_replies(conn))
            .await??;
        chat_domain::check_quick_reply_cap(count as usize)
            .map_err(|_| ChatError::ReplyLimitReached)?;
        let row_id = ids::new_id("qr");
        self.db
            .call({
                let id = row_id.clone();
                let body = body.to_string();
                move |conn| chat::insert_quick_reply(conn, &id, &body)
            })
            .await??;
        Ok(QuickReplyRow {
            id: row_id,
            body: body.to_string(),
            created_at: crate::domain::time_util::format_rfc3339(utc_now_ms()),
        })
    }

    pub async fn delete_quick_reply(&self, id: &str) -> Result<bool, ChatError> {
        self.db
            .call({
                let id = id.to_string();
                move |conn| chat::delete_quick_reply(conn, &id)
            })
            .await?
            .map_err(ChatError::from)
    }

    pub async fn buyer_note(
        &self,
        account_id: &str,
        buyer_id: &str,
    ) -> Result<Option<String>, ChatError> {
        self.db
            .call({
                let a = account_id.to_string();
                let b = buyer_id.to_string();
                move |conn| -> rusqlite::Result<Option<String>> { Ok(chat::get_buyer_note(conn, &a, &b)) }
            })
            .await?
            .map_err(ChatError::from)
    }

    pub async fn save_buyer_note(
        &self,
        account_id: &str,
        buyer_id: &str,
        note: &str,
    ) -> Result<(), ChatError> {
        chat_domain::check_buyer_note(note).map_err(ChatError::Invalid)?;
        self.db
            .call({
                let a = account_id.to_string();
                let b = buyer_id.to_string();
                let n = note.to_string();
                move |conn| chat::upsert_buyer_note(conn, &a, &b, &n)
            })
            .await?
            .map_err(ChatError::from)
    }

    /// 图片能力门禁(transport 判 403 用)。
    pub fn chat_send_image_supported(&self) -> bool {
        self.capabilities.chat_send_image
    }
}

/// 发送摘要口径:文本用原文;图片用路径/URL(replies::chat_content 同口径)。
fn content_text_of(kind: ChatMsgKind, body_text: Option<&str>, image_path: Option<&str>) -> String {
    match kind {
        ChatMsgKind::Image => image_path.unwrap_or_default().to_string(),
        _ => body_text.unwrap_or_default().to_string(),
    }
}

// ---------- 对象安全出口(transport AppState 注入,与 ManualOps 同法) ----------

/// ChatService 的对象安全视图:transport 经 AppState 持有(Arc<dyn ChatOps>)。
#[async_trait::async_trait]
pub trait ChatOps: Send + Sync {
    async fn ingest(&self, msg: IncomingChatMessage) -> Result<IngestOutcome, ChatError>;
    async fn apply_evidence(
        &self,
        request_id: &str,
        platform_message_id: &str,
    ) -> Result<Option<(String, bool)>, ChatError>;
    async fn send_text(&self, conversation_id: &str, text: &str) -> Result<MessageRow, ChatError>;
    async fn send_image(
        &self,
        conversation_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<MessageRow, ChatError>;
    async fn retry(&self, message_id: &str) -> Result<MessageRow, ChatError>;
    async fn list_conversations(
        &self,
        account_id: &str,
        search: Option<&str>,
        unread_only: bool,
        cursor: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<ConversationRow>, Option<String>), ChatError>;
    async fn list_messages(
        &self,
        conversation_id: &str,
        before_id: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, Option<String>), ChatError>;
    async fn mark_read(&self, conversation_id: &str) -> Result<(), ChatError>;
    async fn delete_conversation(&self, conversation_id: &str) -> Result<(), ChatError>;
    async fn unread_summary(&self) -> Result<(i64, Vec<(String, i64)>), ChatError>;
    async fn quick_replies(&self) -> Result<Vec<QuickReplyRow>, ChatError>;
    async fn add_quick_reply(&self, body: &str) -> Result<QuickReplyRow, ChatError>;
    async fn delete_quick_reply(&self, id: &str) -> Result<bool, ChatError>;
    async fn buyer_note(
        &self,
        account_id: &str,
        buyer_id: &str,
    ) -> Result<Option<String>, ChatError>;
    async fn save_buyer_note(
        &self,
        account_id: &str,
        buyer_id: &str,
        note: &str,
    ) -> Result<(), ChatError>;
    fn chat_send_image_supported(&self) -> bool;
}

#[async_trait::async_trait]
impl<A: PlatformAdapter + Clone + 'static> ChatOps for ChatService<A> {
    async fn ingest(&self, msg: IncomingChatMessage) -> Result<IngestOutcome, ChatError> {
        ChatService::ingest(self, msg).await
    }
    async fn apply_evidence(
        &self,
        request_id: &str,
        platform_message_id: &str,
    ) -> Result<Option<(String, bool)>, ChatError> {
        ChatService::apply_evidence(self, request_id, platform_message_id).await
    }
    async fn send_text(&self, conversation_id: &str, text: &str) -> Result<MessageRow, ChatError> {
        ChatService::send_text(self, conversation_id, text).await
    }
    async fn send_image(
        &self,
        conversation_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<MessageRow, ChatError> {
        ChatService::send_image(self, conversation_id, filename, bytes).await
    }
    async fn retry(&self, message_id: &str) -> Result<MessageRow, ChatError> {
        ChatService::retry(self, message_id).await
    }
    async fn list_conversations(
        &self,
        account_id: &str,
        search: Option<&str>,
        unread_only: bool,
        cursor: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<ConversationRow>, Option<String>), ChatError> {
        ChatService::list_conversations(self, account_id, search, unread_only, cursor, limit).await
    }
    async fn list_messages(
        &self,
        conversation_id: &str,
        before_id: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<MessageRow>, Option<String>), ChatError> {
        ChatService::list_messages(self, conversation_id, before_id, limit).await
    }
    async fn mark_read(&self, conversation_id: &str) -> Result<(), ChatError> {
        ChatService::mark_read(self, conversation_id).await
    }
    async fn delete_conversation(&self, conversation_id: &str) -> Result<(), ChatError> {
        ChatService::delete_conversation(self, conversation_id).await
    }
    async fn unread_summary(&self) -> Result<(i64, Vec<(String, i64)>), ChatError> {
        ChatService::unread_summary(self).await
    }
    async fn quick_replies(&self) -> Result<Vec<QuickReplyRow>, ChatError> {
        ChatService::quick_replies(self).await
    }
    async fn add_quick_reply(&self, body: &str) -> Result<QuickReplyRow, ChatError> {
        ChatService::add_quick_reply(self, body).await
    }
    async fn delete_quick_reply(&self, id: &str) -> Result<bool, ChatError> {
        ChatService::delete_quick_reply(self, id).await
    }
    async fn buyer_note(
        &self,
        account_id: &str,
        buyer_id: &str,
    ) -> Result<Option<String>, ChatError> {
        ChatService::buyer_note(self, account_id, buyer_id).await
    }
    async fn save_buyer_note(
        &self,
        account_id: &str,
        buyer_id: &str,
        note: &str,
    ) -> Result<(), ChatError> {
        ChatService::save_buyer_note(self, account_id, buyer_id, note).await
    }
    fn chat_send_image_supported(&self) -> bool {
        ChatService::chat_send_image_supported(self)
    }
}
