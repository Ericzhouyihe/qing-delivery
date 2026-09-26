//! 007 US3 自动回复分流(T036,research D7):买家文本消息 → 关键词
//! (商品级优先→账号级,包含匹配忽略大小写)→ AI 占位 hook → 默认回复
//! (reply_once 经 default_reply_log 判定)。任一环节 NotSubmitted/Rejected
//! 顺延下一环节;Unknown 保守返回 uncertain 不顺延;全失败 → None。
//! 全部经 send_chat_message 发送,**绝不写 orders/deliveries 表**(宪章红线)。
//! 事件管道接线在 US4(ChatService::ingest 触发);AI 实装在 US7(T077)。

pub mod ai;
pub mod review_reminder;

pub use ai::HttpAiProvider;
pub use review_reminder::ReviewReminderService;

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{accounts, rules_ext};
use crate::application::ports::platform::{
    ChatPeer, ChatSendKind, ContentForSend, PlatformAdapter, RequestContext, SendOutcome,
};
use crate::domain::ids;
use crate::domain::time_util::utc_now_ms;
use sha2::{Digest, Sha256};

/// AI 回复输出截断上限(research D7:输出截断至 2000 字)
pub const MAX_AI_REPLY_CHARS: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum RepliesError {
    #[error("账号不存在")]
    AccountNotFound,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// AI 自动回复提供方(US7/T077 注入实装):输入会话上下文,输出建议回复;
/// None = 无建议(顺延默认回复环节);Err = 上游不可用(静默降级,不阻塞分流)。
#[async_trait::async_trait]
pub trait AiReplyProvider: Send + Sync {
    async fn reply(
        &self,
        account_id: &str,
        item_id: Option<&str>,
        buyer_id: &str,
        text: &str,
    ) -> Result<Option<String>, String>;
}

/// 占位实现(US7 前):恒 Ok(None),AI 环节等效直通默认回复。
pub struct NoAiProvider;

#[async_trait::async_trait]
impl AiReplyProvider for NoAiProvider {
    async fn reply(
        &self,
        _account_id: &str,
        _item_id: Option<&str>,
        _buyer_id: &str,
        _text: &str,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// 分流环节(留痕/测试断言用)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyStage {
    Keyword,
    Ai,
    Default,
}

/// 分流结果:命中即止;Unknown 保守返回 Uncertain(禁止自动重发/顺延)。
/// 007 T050:Sent/Uncertain 携带发送正文(payload)与是否图片,
/// 供 ChatService 落 chat_messages 出站行(零订单/交付写入不变)。
#[derive(Clone, Debug)]
pub enum ReplyDispatch {
    /// 发送且收到严格关联接纳证明
    Sent {
        stage: ReplyStage,
        proof: crate::application::ports::platform::SendProof,
        payload: String,
        is_image: bool,
    },
    /// 发出但结果未知(uncertain):留痕待人工,不自动重发
    Uncertain {
        stage: ReplyStage,
        hint: Option<String>,
        payload: String,
        is_image: bool,
    },
    /// 无回复(全部环节未命中或最终失败)
    None,
}

fn digest_of(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// 可发送内容:文字用原文;图片以 URL 为正文(摘要仍覆盖全文,幂等口径一致)。
fn chat_content(snapshot_id: &str, payload: &str) -> ContentForSend {
    ContentForSend {
        snapshot_id: snapshot_id.to_string(),
        text: payload.to_string(),
        text_digest: digest_of(payload),
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub struct ReplyService<A: PlatformAdapter> {
    db: DbThread,
    adapter: A,
    ai: std::sync::Arc<dyn AiReplyProvider>,
}

impl<A: PlatformAdapter> ReplyService<A> {
    pub fn new(db: DbThread, adapter: A) -> Self {
        Self::with_ai(db, adapter, std::sync::Arc::new(NoAiProvider))
    }

    /// 注入 AI 提供方(US7 实装;测试用记录型假实现)。
    pub fn with_ai(db: DbThread, adapter: A, ai: std::sync::Arc<dyn AiReplyProvider>) -> Self {
        Self { db, adapter, ai }
    }

    /// 一次买家文本消息的分流编排(独立用例;US4 由事件管道触发)。
    pub async fn resolve_and_send(
        &self,
        account_id: &str,
        item_id: Option<&str>,
        buyer_id: &str,
        chat_id: Option<&str>,
        text: &str,
    ) -> Result<ReplyDispatch, RepliesError> {
        let account = self
            .db
            .call({
                let account_id = account_id.to_string();
                move |conn| accounts::get(conn, &account_id)
            })
            .await??
            .ok_or(RepliesError::AccountNotFound)?;
        let ctx = RequestContext {
            operation_id: ids::new_id("op"),
            account_id: account.id.clone(),
            credential_generation: account.credential_epoch,
            control_generation: account.control_epoch,
            deadline_ms: utc_now_ms() + 15_000,
        };

        // ① 关键词(商品级优先→账号级,包含匹配忽略大小写);取最高优先一条,命中即止
        let keyword_hit = self
            .db
            .call({
                let account_id = account_id.to_string();
                let item_id = item_id.map(|s| s.to_string());
                let text = text.to_string();
                move |conn| {
                    rules_ext::match_reply_rules(conn, &account_id, item_id.as_deref(), &text)
                }
            })
            .await??
            .into_iter()
            .next();
        if let Some(rule) = keyword_hit {
            let is_image = rule.reply_kind == "image";
            let payload = if is_image {
                rule.reply_image_url.clone().unwrap_or_default()
            } else {
                rule.reply_text.clone().unwrap_or_default()
            };
            if !payload.is_empty() {
                let kind = if is_image {
                    ChatSendKind::Image { url: payload.clone() }
                } else {
                    ChatSendKind::Text
                };
                let content = chat_content(&format!("reply_rule:{}", rule.id), &payload);
                match self.send_once(&ctx, buyer_id, chat_id, kind, &content).await {
                    SendOutcome::Accepted(proof) => {
                        return Ok(ReplyDispatch::Sent {
                            stage: ReplyStage::Keyword,
                            proof,
                            payload: payload.clone(),
                            is_image,
                        })
                    }
                    SendOutcome::Unknown { hint } => {
                        // 保守:结果未知不顺延、不自动重发
                        tracing::warn!(account = account_id, rule = %rule.id, "关键词回复发送结果未知(uncertain)");
                        return Ok(ReplyDispatch::Uncertain {
                            stage: ReplyStage::Keyword,
                            hint,
                            payload: payload.clone(),
                            is_image,
                        });
                    }
                    // NotSubmitted/Rejected:顺延 AI 环节
                    SendOutcome::NotSubmitted { retryable } => {
                        tracing::warn!(account = account_id, rule = %rule.id, retryable, "关键词回复未提交,顺延 AI 环节");
                    }
                    SendOutcome::Rejected { safe_code } => {
                        tracing::warn!(account = account_id, rule = %rule.id, code = %safe_code, "关键词回复被拒,顺延 AI 环节");
                    }
                }
            }
        }

        // ② AI hook(US7 实装;NoAiProvider 恒 None 直通)。
        // 账号级 ai_reply_enabled 门禁随 0006 迁移(US5/T058)与 US7 实装一并接入。
        match self
            .ai
            .reply(account_id, item_id, buyer_id, text)
            .await
        {
            Ok(Some(raw)) => {
                let payload = truncate_chars(raw.trim(), MAX_AI_REPLY_CHARS);
                if !payload.is_empty() {
                    let content = chat_content(
                        &format!("ai_reply:{}", &digest_of(&payload)[..16]),
                        &payload,
                    );
                    match self
                        .send_once(&ctx, buyer_id, chat_id, ChatSendKind::Text, &content)
                        .await
                    {
                        SendOutcome::Accepted(proof) => {
                            return Ok(ReplyDispatch::Sent {
                                stage: ReplyStage::Ai,
                                proof,
                                payload: payload.clone(),
                                is_image: false,
                            })
                        }
                        SendOutcome::Unknown { hint } => {
                            tracing::warn!(account = account_id, "AI 回复发送结果未知(uncertain)");
                            return Ok(ReplyDispatch::Uncertain {
                                stage: ReplyStage::Ai,
                                hint,
                                payload: payload.clone(),
                                is_image: false,
                            });
                        }
                        SendOutcome::NotSubmitted { retryable } => {
                            tracing::warn!(account = account_id, retryable, "AI 回复未提交,顺延默认回复环节");
                        }
                        SendOutcome::Rejected { safe_code } => {
                            tracing::warn!(account = account_id, code = %safe_code, "AI 回复被拒,顺延默认回复环节");
                        }
                    }
                }
            }
            Ok(None) => {}
            // AI 不可用静默降级:不阻塞,顺延默认回复(research D7)
            Err(e) => {
                tracing::warn!(account = account_id, error = %e, "AI 回复不可用,静默降级");
            }
        }

        // ③ 默认回复:enabled + reply_once 时同 (account,buyer) 无 accepted 记录
        let default_cfg = self
            .db
            .call({
                let account_id = account_id.to_string();
                move |conn| rules_ext::get_default_reply(conn, &account_id)
            })
            .await??;
        if let Some(cfg) = default_cfg.filter(|c| c.enabled) {
            let already = if cfg.reply_once {
                self.db
                    .call({
                        let account_id = account_id.to_string();
                        let buyer_id = buyer_id.to_string();
                        move |conn| {
                            rules_ext::has_accepted_default_reply(conn, &account_id, &buyer_id)
                        }
                    })
                    .await??
            } else {
                false
            };
            if !already {
                let is_image = cfg.reply_text.is_none() && cfg.reply_image_url.is_some();
                let payload = if is_image {
                    cfg.reply_image_url.clone().unwrap_or_default()
                } else {
                    cfg.reply_text.clone().unwrap_or_default()
                };
                if !payload.is_empty() {
                    let kind = if is_image {
                        ChatSendKind::Image { url: payload.clone() }
                    } else {
                        ChatSendKind::Text
                    };
                    let content =
                        chat_content(&format!("default_reply:{}", account_id), &payload);
                    match self
                        .send_once(&ctx, buyer_id, chat_id, kind, &content)
                        .await
                    {
                        SendOutcome::Accepted(proof) => {
                            self.append_default_log(account_id, buyer_id, "accepted").await?;
                            return Ok(ReplyDispatch::Sent {
                                stage: ReplyStage::Default,
                                proof,
                                payload: payload.clone(),
                                is_image,
                            });
                        }
                        SendOutcome::Unknown { hint } => {
                            // 留痕 unknown;uncertain 不顺延、不自动重发
                            self.append_default_log(account_id, buyer_id, "unknown").await?;
                            tracing::warn!(account = account_id, "默认回复发送结果未知(uncertain)");
                            return Ok(ReplyDispatch::Uncertain {
                                stage: ReplyStage::Default,
                                hint,
                                payload: payload.clone(),
                                is_image,
                            });
                        }
                        // 末位环节:留痕 not_sent 后全失败 → None
                        SendOutcome::NotSubmitted { retryable } => {
                            tracing::warn!(account = account_id, retryable, "默认回复未提交,本轮无回复");
                        }
                        SendOutcome::Rejected { safe_code } => {
                            tracing::warn!(account = account_id, code = %safe_code, "默认回复被拒,本轮无回复");
                        }
                    }
                    self.append_default_log(account_id, buyer_id, "not_sent").await?;
                }
            }
        }

        Ok(ReplyDispatch::None)
    }

    /// 单次聊天发送(网络不进事务;发送在 db.call 之外)。
    async fn send_once(
        &self,
        ctx: &RequestContext,
        buyer_id: &str,
        chat_id: Option<&str>,
        kind: ChatSendKind,
        content: &ContentForSend,
    ) -> SendOutcome {
        let peer = ChatPeer {
            buyer_id: buyer_id.to_string(),
            chat_id: chat_id.map(|s| s.to_string()),
        };
        match self
            .adapter
            .send_chat_message(ctx, &peer, kind, content)
            .await
        {
            Ok(outcome) => outcome,
            // Err(PersistenceUnavailable/CancelledBeforeSubmit 等)语义 = 未提交:
            // 按顺延处理(与 NotSubmitted 同类),不视为已发送
            Err(e) => {
                tracing::warn!(account = %ctx.account_id, error = %e, "聊天发送调用失败(未提交)");
                SendOutcome::NotSubmitted { retryable: true }
            }
        }
    }

    async fn append_default_log(
        &self,
        account_id: &str,
        buyer_id: &str,
        state: &str,
    ) -> Result<(), RepliesError> {
        self.db
            .call({
                let id = ids::new_id("drl");
                let account_id = account_id.to_string();
                let buyer_id = buyer_id.to_string();
                let state = state.to_string();
                move |conn| rules_ext::append_default_reply_log(conn, &id, &account_id, &buyer_id, &state)
            })
            .await??;
        Ok(())
    }
}
