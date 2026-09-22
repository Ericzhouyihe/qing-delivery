//! 订单追溯扫描(T063):每 60 秒限量拉取已售列表、分页上限 100 页;
//! 输出覆盖报告(不可追溯范围明示缺口);候选订单逐笔走 handle 再核验,
//! 扫描本身不是发货指令(FR-018)。间隔由调用方调度;源时钟 UTC。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::application::delivery::service::{DeliveryService, HandleOutcome};
use crate::application::ports::platform::{PlatformAdapter, RequestContext, TraceReport};

/// 扫描间隔与分页上限(spec 默认值)。
pub const SCAN_INTERVAL_MS: i64 = 60_000;
pub const MAX_TRACE_PAGES: u32 = 100;

#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Delivery(#[from] crate::application::delivery::service::DeliveryError),
    #[error(transparent)]
    Platform(#[from] crate::application::ports::platform::PlatformError),
}

pub struct TraceScanReport {
    pub coverage: TraceReport,
    pub candidates: usize,
    pub handled: usize,
}

pub struct TraceScanService<A: PlatformAdapter> {
    db: DbThread,
    delivery: DeliveryService<A>,
}

impl<A: PlatformAdapter> TraceScanService<A> {
    pub fn new(db: DbThread, delivery: DeliveryService<A>) -> Self {
        Self { db, delivery }
    }

    /// 一轮扫描:追溯 → 逐候选核验补建。窗口默认最近 24 小时。
    pub async fn run_once(
        &self,
        account_id: &str,
        ctx: &RequestContext,
    ) -> Result<TraceScanReport, TraceError> {
        let to = crate::domain::time_util::utc_now_ms();
        let from = to - 24 * 60 * 60 * 1000;
        let (candidate_ids, coverage) = self
            .delivery_adapter_trace(account_id, ctx, from, to)
            .await?;
        let mut handled = 0;
        for external in &candidate_ids {
            // handle 重新核验并走唯一 initial;已处理订单返回 AlreadyHandled
            let outcome = self.delivery.handle_payment(account_id, external).await?;
            match outcome {
                HandleOutcome::Delivered { .. }
                | HandleOutcome::Ineligible { .. }
                | HandleOutcome::NotSent { .. }
                | HandleOutcome::Unknown { .. } => handled += 1,
                HandleOutcome::AlreadyHandled | HandleOutcome::Busy => {}
            }
        }
        // 覆盖缺口持久化到 sync_jobs(可审计,不宣称无漏单)
        let account = account_id.to_string();
        let cov = coverage.clone();
        self.db
            .call(move |conn| -> rusqlite::Result<()> {
                conn.execute(
                    "INSERT INTO sync_jobs(id, account_id, kind, status, complete, gap_reason,
                             coverage_from, coverage_to, created_at, updated_at)
                     VALUES (?1, ?2, 'orders', ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                    rusqlite::params![
                        crate::domain::ids::new_id("syn"),
                        account,
                        if cov.coverage_complete {
                            "complete"
                        } else {
                            "incomplete"
                        },
                        cov.coverage_complete as i64,
                        if cov.coverage_complete {
                            None
                        } else {
                            cov.stop_reason
                                .clone()
                                .or_else(|| Some("追溯范围不完整".into()))
                        },
                        cov.observed_from_ms.unwrap_or(from),
                        cov.observed_to_ms.unwrap_or(to),
                        crate::domain::time_util::utc_now_ms()
                    ],
                )?;
                Ok(())
            })
            .await??;
        Ok(TraceScanReport {
            coverage,
            candidates: candidate_ids.len(),
            handled,
        })
    }

    async fn delivery_adapter_trace(
        &self,
        _account_id: &str,
        ctx: &RequestContext,
        from: i64,
        to: i64,
    ) -> Result<(Vec<String>, TraceReport), TraceError> {
        // 追溯经 DeliveryService 内的适配器完成;此处直接复用其 adapter 句柄
        self.delivery
            .trace(ctx, from, to, MAX_TRACE_PAGES)
            .await
            .map_err(TraceError::from)
    }
}
