//! 漏通知补偿(T064):普通待发货首次被可靠观察后至少等 120 秒,
//! 再为缺事件订单补建交付(给实时消息先处理的机会);有任务走既有路径不建第二初始任务。
//! 迟到付款通知由 handle 的唯一 initial 约束天然去重(FR-018)。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::application::delivery::service::{DeliveryService, HandleOutcome};
use crate::application::ports::platform::PlatformAdapter;

/// 补偿等待窗口(spec 默认值,不是实现自由项)。
pub const COMPENSATION_DELAY_MS: i64 = 120_000;

#[derive(Debug, thiserror::Error)]
pub enum CompensateError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Delivery(#[from] crate::application::delivery::service::DeliveryError),
}

pub struct CompensationReport {
    pub candidates_total: usize,
    /// 尚在等待窗口内的(120 秒未满,本轮跳过)
    pub waiting: usize,
    pub handled: usize,
}

pub struct CompensationService<A: PlatformAdapter> {
    db: DbThread,
    delivery: DeliveryService<A>,
}

pub struct Candidate {
    pub order_db_id: String,
    pub account_id: String,
    pub external_order_id: String,
    pub observed_at_ms: Option<i64>,
}

impl<A: PlatformAdapter> CompensationService<A> {
    pub fn new(db: DbThread, delivery: DeliveryService<A>) -> Self {
        Self { db, delivery }
    }

    /// 待补偿候选:平台状态 pending_ship 且**尚无 initial 交付**的订单
    /// (有任务者属任务恢复范畴,绝不建第二个初始任务,data-model)。
    pub async fn candidates(&self) -> Result<Vec<Candidate>, CompensateError> {
        let rows = self
            .db
            .call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT o.id, o.account_id, o.external_order_id, o.observed_at
                     FROM orders o
                     WHERE o.platform_status = 'pending_ship'
                       AND o.paid_at IS NOT NULL
                       AND NOT EXISTS (
                           SELECT 1 FROM deliveries d WHERE d.order_id = o.id AND d.kind = 'initial'
                       )",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(Candidate {
                            order_db_id: r.get(0)?,
                            account_id: r.get(1)?,
                            external_order_id: r.get(2)?,
                            observed_at_ms: r.get(3)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok::<_, rusqlite::Error>(rows)
            })
            .await??;
        Ok(rows)
    }

    /// 执行一轮补偿;返回各计数。candidates 内 observed_at 缺失视为"刚发现",本轮等待。
    pub async fn run_once(&self) -> Result<CompensationReport, CompensateError> {
        let candidates = self.candidates().await?;
        let now = crate::domain::time_util::utc_now_ms();
        let mut report = CompensationReport {
            candidates_total: candidates.len(),
            waiting: 0,
            handled: 0,
        };
        for c in candidates {
            let age = c.observed_at_ms.map(|t| now - t).unwrap_or(0);
            if age < COMPENSATION_DELAY_MS {
                report.waiting += 1;
                continue;
            }
            // handle 全程重新核验资格;唯一 initial 约束兜底并发/迟到事件
            let outcome = self
                .delivery
                .handle_payment(&c.account_id, &c.external_order_id)
                .await?;
            match outcome {
                HandleOutcome::Delivered { .. }
                | HandleOutcome::Ineligible { .. }
                | HandleOutcome::NotSent { .. }
                | HandleOutcome::Unknown { .. } => report.handled += 1,
                HandleOutcome::AlreadyHandled | HandleOutcome::Busy => {}
            }
        }
        Ok(report)
    }
}
