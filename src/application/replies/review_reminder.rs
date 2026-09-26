//! 007 US3 求评计划(T037,FR-035):对 content_state='accepted' 的订单,
//! 按 review_config(wait_hours/interval_hours/max_count/text)定时发送求评消息。
//! 独立用例(不经事件管道);定时器装配在 supervisor(每小时一轮)。
//! 网络不进事务;Unknown 不自动重发(宪章红线)。

use crate::adapters::sqlite::db::DbThread;
use crate::adapters::sqlite::repos::rules_ext;
use crate::application::ports::platform::{
    ChatPeer, ChatSendKind, ContentForSend, PlatformAdapter, RequestContext, SendOutcome,
};
use crate::application::replies::RepliesError;
use crate::domain::ids;
use crate::domain::rules_ext::ReviewConfig;
use crate::domain::time_util::{format_rfc3339, parse_rfc3339_ms, utc_now_ms};
use sha2::{Digest, Sha256};

const HOUR_MS: i64 = 3_600_000;

pub struct ReviewReminderService<A: PlatformAdapter> {
    db: DbThread,
    adapter: A,
}

impl<A: PlatformAdapter> ReviewReminderService<A> {
    pub fn new(db: DbThread, adapter: A) -> Self {
        Self { db, adapter }
    }

    /// 一轮扫描:装载候选(只读)→ 逐条门禁/到期判定 → 发送 → 推进状态。
    /// 返回本轮成功送出(accepted)的求评条数;单条失败不中断整轮。
    pub async fn run_once(&self) -> Result<usize, RepliesError> {
        let now_ms = utc_now_ms();
        let candidates = self
            .db
            .call(|conn| rules_ext::list_review_candidates(conn))
            .await??;
        let mut sent = 0usize;
        for c in candidates {
            // 账号门禁(FR-035):停用/离线跳过并留痕,不排期、不发送
            if !c.runtime_enabled || c.status != "online" {
                tracing::info!(
                    account = %c.account_id,
                    order = %c.order_db_id,
                    status = %c.status,
                    runtime_enabled = c.runtime_enabled,
                    "账号停用或离线,跳过求评"
                );
                continue;
            }
            let Some(buyer_id) = c
                .buyer_id
                .clone()
                .filter(|b| !b.trim().is_empty())
            else {
                tracing::info!(order = %c.order_db_id, "订单缺少可信买家号,跳过求评");
                continue;
            };
            let Ok(review) = serde_json::from_str::<ReviewConfig>(&c.review_config) else {
                tracing::warn!(
                    rule = %c.rule_id,
                    "求评配置解析失败,跳过(需重新配置)"
                );
                continue;
            };
            // 到期判定:已有状态行按 next_due_at;缺失行按 内容接纳时间+wait_hours 排首轮
            let due_ms = match c.next_due_at.as_deref() {
                Some(due) => match parse_rfc3339_ms(due) {
                    Some(ms) => ms,
                    None => {
                        tracing::warn!(order = %c.order_db_id, "next_due_at 解析失败,跳过");
                        continue;
                    }
                },
                None => c.accepted_at_ms + review.wait_hours * HOUR_MS,
            };
            if now_ms < due_ms {
                continue; // 未到期
            }
            // 防御:达上限不再排(正常路径达成上限时 next_due_at 已置 NULL)
            if c.reminded_count >= review.max_count {
                tracing::info!(order = %c.order_db_id, rule = %c.rule_id, "求评已达上限,跳过");
                continue;
            }

            // 发送(网络不进事务;求评会话地址缺省按买家号兜底)
            let ctx = RequestContext {
                operation_id: ids::new_id("op"),
                account_id: c.account_id.clone(),
                credential_generation: c.credential_epoch,
                control_generation: c.control_epoch,
                deadline_ms: now_ms + 15_000,
            };
            let content = ContentForSend {
                snapshot_id: format!("review:{}:{}", c.order_db_id, c.rule_id),
                text: review.text.clone(),
                text_digest: hex::encode(Sha256::digest(review.text.as_bytes())),
            };
            let peer = ChatPeer {
                buyer_id,
                chat_id: None,
            };
            let outcome = self
                .adapter
                .send_chat_message(&ctx, &peer, ChatSendKind::Text, &content)
                .await;
            let outcome = match outcome {
                Ok(o) => o,
                // 调用失败 = 未提交(确定未送出),与 NotSubmitted 同处置
                Err(e) => {
                    tracing::warn!(order = %c.order_db_id, error = %e, "求评发送调用失败(未提交)");
                    SendOutcome::NotSubmitted { retryable: true }
                }
            };
            match outcome {
                SendOutcome::Accepted(_) => {
                    let new_count = c.reminded_count + 1;
                    let next_due = if new_count >= review.max_count {
                        // 达上限:不再排期(FR-035)
                        None
                    } else {
                        Some(format_rfc3339(now_ms + review.interval_hours * HOUR_MS))
                    };
                    self.advance(&c.order_db_id, &c.rule_id, new_count, Some(format_rfc3339(now_ms)), next_due)
                        .await?;
                    sent += 1;
                }
                SendOutcome::Unknown { hint } => {
                    // 简化(偏离交付路径的 issue 化,特此注明):不开待处理事项;
                    // 状态行留痕(最近一次尝试时间)后不再自动排期——
                    // unknown 不自动重发(宪章 I),人工可在规则页重排。
                    tracing::warn!(
                        order = %c.order_db_id,
                        hint = ?hint,
                        "求评发送结果未知:留痕并停止自动排期"
                    );
                    self.advance(&c.order_db_id, &c.rule_id, c.reminded_count, Some(format_rfc3339(now_ms)), None)
                        .await?;
                }
                // 确定未送出:不计数,顺延一个间隔(下轮再试;失败不丢失计划)
                SendOutcome::NotSubmitted { retryable } => {
                    tracing::warn!(order = %c.order_db_id, retryable, "求评未提交,顺延一个间隔");
                    let next_due = format_rfc3339(now_ms + review.interval_hours * HOUR_MS);
                    self.advance(&c.order_db_id, &c.rule_id, c.reminded_count, c.last_reminded_at.clone(), Some(next_due))
                        .await?;
                }
                SendOutcome::Rejected { safe_code } => {
                    tracing::warn!(order = %c.order_db_id, code = %safe_code, "求评被平台拒绝,顺延一个间隔");
                    let next_due = format_rfc3339(now_ms + review.interval_hours * HOUR_MS);
                    self.advance(&c.order_db_id, &c.rule_id, c.reminded_count, c.last_reminded_at.clone(), Some(next_due))
                        .await?;
                }
            }
        }
        Ok(sent)
    }

    async fn advance(
        &self,
        order_db_id: &str,
        rule_id: &str,
        reminded_count: i64,
        last_reminded_at: Option<String>,
        next_due_at: Option<String>,
    ) -> Result<(), RepliesError> {
        self.db
            .call({
                let order_db_id = order_db_id.to_string();
                let rule_id = rule_id.to_string();
                move |conn| {
                    rules_ext::upsert_reminder(
                        conn,
                        &order_db_id,
                        &rule_id,
                        reminded_count,
                        last_reminded_at.as_deref(),
                        next_due_at.as_deref(),
                    )
                }
            })
            .await??;
        Ok(())
    }
}
