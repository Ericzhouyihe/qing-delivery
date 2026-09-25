//! 运营统计用例(005,T006):只读装配区间营收+对比+趋势+账号快照+事项数(FR-011)。
//! 应用层组合领域口径(domain::stats)与仓储查询(repos::stats),不执行 SQL(宪章 II);
//! 全程只读,不触碰交付/值守/写路径(FR-012)。

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::stats as stats_repo;
use crate::domain::stats::{self, StatsRange, TrendPoint};

/// 恢复隔离信息(横幅展示);未隔离为 None。
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreInfo {
    pub restore_epoch: i64,
    pub quarantine_started_ms: Option<i64>,
    pub unresolved_count: i64,
}

/// 概览统计投影(契约 §GET /api/v1/stats/overview 的载荷形状)。
#[derive(Debug, Clone, PartialEq)]
pub struct StatsOverview {
    pub range: StatsRange,
    pub revenue_minor_units: i64,
    pub revenue_order_count: i64,
    pub previous_minor_units: i64,
    pub change_percent: Option<i32>,
    pub accounts_online: i64,
    pub accounts_total: i64,
    pub pending_issues: i64,
    pub trend: Vec<TrendPoint>,
    pub restore: Option<RestoreInfo>,
}

#[derive(Clone)]
pub struct StatsService {
    db: DbThread,
}

impl StatsService {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    /// 一次请求装配整屏统计:四卡与趋势同源同一时刻快照(研究 R7)。
    pub async fn overview(&self, range: StatsRange) -> Result<StatsOverview, DbError> {
        match self
            .db
            .call(move |conn| -> rusqlite::Result<StatsOverview> {
                let rows = stats_repo::paid_orders_in(conn, &range)?;
                let prev_rows = stats_repo::paid_orders_in(conn, &range.previous())?;
                let totals = stats::revenue_totals(&rows);
                let previous = stats::revenue_totals(&prev_rows);
                let trend = stats::trend_points(&rows, &range);
                let (accounts_online, accounts_total) = stats_repo::account_activity(conn)?;
                let pending_issues = stats_repo::open_issue_count(conn)?;
                let restore =
                    stats_repo::restore_snapshot(conn)?.map(|(epoch, started, unresolved)| {
                        RestoreInfo {
                            restore_epoch: epoch,
                            quarantine_started_ms: started,
                            unresolved_count: unresolved,
                        }
                    });
                Ok(StatsOverview {
                    range,
                    revenue_minor_units: totals.minor_units,
                    revenue_order_count: totals.order_count,
                    previous_minor_units: previous.minor_units,
                    change_percent: stats::change_percent(totals.minor_units, previous.minor_units),
                    accounts_online,
                    accounts_total,
                    pending_issues,
                    trend,
                    restore,
                })
            })
            .await
        {
            Ok(Ok(ov)) => Ok(ov),
            Ok(Err(e)) => Err(e.into()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite::migrations;

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    async fn seeded_service() -> (StatsService, i64) {
        let dir = tempfile::tempdir().expect("临时目录");
        let db = DbThread::spawn(&dir.path().join("t.db")).expect("DB 线程");
        let migration_dir = dir.path().to_path_buf();
        db.call(move |conn| migrations::apply(conn, &migration_dir))
            .await
            .expect("迁移提交")
            .expect("迁移应用");
        let now = crate::domain::time_util::utc_now_ms();
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO accounts (id, platform, external_user_id, display_name, status, created_at, updated_at)
                 VALUES ('a1', 'xianyu', 'u1', '账号', 'online', 0, 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO orders (id, platform, account_id, external_order_id, paid_at, amount_minor, currency, platform_status, created_at, updated_at)
                 VALUES ('o1', 'xianyu', 'a1', 'o1', ?1, 12_00, 'CNY', 'paid', 0, 0)",
                rusqlite::params![now - 60_000],
            )?;
            conn.execute(
                "INSERT INTO orders (id, platform, account_id, external_order_id, paid_at, amount_minor, currency, platform_status, created_at, updated_at)
                 VALUES ('o2', 'xianyu', 'a1', 'o2', ?1, 7_00, 'CNY', 'refund_success', 0, 0)",
                rusqlite::params![now - 30_000],
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .unwrap()
        .unwrap();
        (StatsService::new(db), now)
    }

    /// 让 tokio 测试运行时可用:应用层测试直接用 #[tokio::test]。
    #[tokio::test]
    async fn overview_装配四卡与趋势且趋势一致() {
        let (service, now) = seeded_service().await;
        let from = crate::domain::time_util::local_day_start_ms(now, 0);
        let to = crate::domain::time_util::local_step_day_start(from, 1);
        let range = StatsRange::new(from, to).unwrap();
        let ov = service.overview(range).await.unwrap();
        // 合计:o1(1200)+o2(700,退款不回冲)= 1900;订单数 2
        assert_eq!(ov.revenue_minor_units, 1_900);
        assert_eq!(ov.revenue_order_count, 2);
        assert_eq!(ov.accounts_online, 1);
        assert_eq!(ov.accounts_total, 1);
        assert_eq!(ov.pending_issues, 0);
        assert!(ov.change_percent.is_none(), "前区间无数据");
        assert_eq!(
            ov.trend.iter().map(|p| p.minor_units).sum::<i64>(),
            ov.revenue_minor_units
        );
        assert_eq!(
            ov.trend.iter().map(|p| p.order_count).sum::<i64>(),
            ov.revenue_order_count
        );
    }

    #[tokio::test]
    async fn overview_区间外订单不计入() {
        let (service, now) = seeded_service().await;
        let from = crate::domain::time_util::local_day_start_ms(now, 90);
        let range = StatsRange::new(from, from + DAY_MS).unwrap();
        let ov = service.overview(range).await.unwrap();
        assert_eq!(ov.revenue_minor_units, 0);
        assert_eq!(ov.revenue_order_count, 0);
    }

    /// SC-003:1 万订单、一个月内(30 自然日)窗口 <3s(T019,宪章 III 记录测量)。
    #[tokio::test]
    async fn overview_一万订单三十天窗口三秒内返回() {
        let dir = tempfile::tempdir().expect("临时目录");
        let db = DbThread::spawn(&dir.path().join("perf.db")).expect("DB 线程");
        let migration_dir = dir.path().to_path_buf();
        db.call(move |conn| migrations::apply(conn, &migration_dir))
            .await
            .expect("迁移提交")
            .expect("迁移应用");
        let now = crate::domain::time_util::utc_now_ms();
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO accounts (id, platform, external_user_id, display_name, status, created_at, updated_at)
                 VALUES ('a1', 'xianyu', 'u1', '账号', 'online', 0, 0)",
                [],
            )?;
            let tx = conn.transaction()?;
            for i in 0..10_000i64 {
                // 91 天内均匀分布;金额 1~500 元
                let paid_at = now - (i * 786_240); // ~786s 步进 ≈ 覆盖 91 天
                tx.execute(
                    "INSERT INTO orders (id, platform, account_id, external_order_id, paid_at,
                         amount_minor, currency, platform_status, created_at, updated_at)
                     VALUES (?1, 'xianyu', 'a1', ?1, ?2, ?3, 'CNY', 'paid', 0, 0)",
                    rusqlite::params![format!("perf-{i}"), paid_at, 100 + (i % 49_900)],
                )?;
            }
            tx.commit()
        })
        .await
        .unwrap()
        .unwrap();

        let service = StatsService::new(db);
        let from = crate::domain::time_util::local_day_start_ms(now, 29);
        let range = StatsRange::new(from, from + 30 * DAY_MS).unwrap();
        let started = std::time::Instant::now();
        let ov = service.overview(range).await.unwrap();
        let elapsed = started.elapsed();
        assert!(
            ov.revenue_order_count > 3_000,
            "30 天窗口应覆盖约 1/3 订单:实际 {}",
            ov.revenue_order_count
        );
        assert_eq!(
            ov.trend.iter().map(|p| p.order_count).sum::<i64>(),
            ov.revenue_order_count
        );
        assert!(
            elapsed.as_millis() < 3_000,
            "SC-003 违约:30 天窗口耗时 {:?}(目标 <3s)",
            elapsed
        );
        tracing::info!(
            elapsed_ms = elapsed.as_millis() as u64,
            orders = ov.revenue_order_count,
            "SC-003 性能抽查"
        );
    }
}
