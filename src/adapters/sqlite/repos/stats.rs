//! 运营统计只读查询(005,T005):SQL 仅做区间过滤与计数,口径在 domain::stats
//! (研究 R3:SQL 不分桶不求和,原料交由领域层计算,宪章 II)。

use rusqlite::Connection;

use crate::domain::stats::PaidOrderRaw;
use crate::domain::stats::StatsRange;

/// 区间内已付款订单原料行:`paid_at ∈ [from, to)`。
/// 不过滤币种/金额/平台状态——退款单保留,由领域口径决定用途(FR-010/澄清 Q1)。
pub fn paid_orders_in(
    conn: &Connection,
    range: &StatsRange,
) -> rusqlite::Result<Vec<PaidOrderRaw>> {
    let mut stmt = conn.prepare(
        "SELECT paid_at, amount_minor, currency FROM orders
         WHERE paid_at IS NOT NULL AND paid_at >= ?1 AND paid_at < ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![range.from_ms, range.to_ms], |r| {
            Ok(PaidOrderRaw {
                paid_at: r.get(0)?,
                amount_minor: r.get(1)?,
                currency: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 账号活跃快照:在线(观测 `status='online'`,与账号页徽标唯一同源值)/ 总数。
pub fn account_activity(conn: &Connection) -> rusqlite::Result<(i64, i64)> {
    let online: i64 = conn.query_row(
        "SELECT COUNT(*) FROM accounts WHERE status = 'online'",
        [],
        |r| r.get(0),
    )?;
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
    Ok((online, total))
}

/// 开放人工处理事项数(与 /api/v1/dashboard `open_issue_count` 同口径)。
pub fn open_issue_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM issues WHERE state = 'open'",
        [],
        |r| r.get(0),
    )
}

/// 概览第 5 卡(007 T014):启用批量(data)组的可用余量合计。
/// 只读计数;查询失败由应用层降级为 None(不影响其余卡片)。
pub fn card_stock_available(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM card_entries e
         JOIN card_pools p ON p.id = e.pool_id
         WHERE e.state = 'available' AND p.enabled = 1 AND p.kind = 'data'",
        [],
        |r| r.get(0),
    )
}

/// 恢复隔离快照(restore_epoch=0 表示未隔离);横幅展示用,与 dashboard 同口径。
pub fn restore_snapshot(conn: &Connection) -> rusqlite::Result<Option<(i64, Option<i64>, i64)>> {
    let restore_epoch: i64 = conn.query_row(
        "SELECT restore_epoch FROM installation WHERE id='singleton'",
        [],
        |r| r.get(0),
    )?;
    if restore_epoch <= 0 {
        return Ok(None);
    }
    let row = conn.query_row(
        "SELECT quarantine_started_at,
                (SELECT COUNT(*) FROM restore_reviews WHERE status='unresolved')
         FROM installation WHERE id='singleton'",
        [],
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)),
    );
    match row {
        Ok((started, unresolved)) => Ok(Some((restore_epoch, started, unresolved))),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("内存库");
        let dir = tempfile::tempdir().expect("临时目录");
        crate::adapters::sqlite::migrations::apply(&mut conn, dir.path()).expect("迁移应用");
        conn
    }

    fn seed_account(conn: &Connection, id: &str, status: &str) {
        conn.execute(
            "INSERT INTO accounts (id, platform, external_user_id, display_name, status, created_at, updated_at)
             VALUES (?1, 'xianyu', ?1, '账号', ?2, 0, 0)",
            rusqlite::params![id, status],
        )
        .unwrap();
    }

    fn seed_order(
        conn: &Connection,
        id: &str,
        paid_at: i64,
        amount: Option<i64>,
        currency: Option<&str>,
        status: &str,
    ) {
        conn.execute(
            "INSERT INTO orders (id, platform, account_id, external_order_id, paid_at, amount_minor, currency, platform_status, created_at, updated_at)
             VALUES (?1, 'xianyu', 'a1', ?1, ?2, ?3, ?4, ?5, 0, 0)",
            rusqlite::params![id, paid_at, amount, currency, status],
        )
        .unwrap();
    }

    #[test]
    fn 区间过滤含端正确_状态不过滤() {
        let conn = test_conn();
        seed_account(&conn, "a1", "online");
        // 半开 [100, 200):100 入、200 不入、99 不入
        seed_order(&conn, "o1", 100, Some(100), Some("CNY"), "paid");
        seed_order(&conn, "o2", 199, Some(200), Some("CNY"), "refund_success");
        seed_order(&conn, "o3", 200, Some(300), Some("CNY"), "paid");
        seed_order(&conn, "o4", 99, Some(400), Some("CNY"), "paid");
        seed_order(&conn, "o5", 150, None, None, "paid");
        let range = StatsRange::new(100, 200).unwrap();
        let rows = paid_orders_in(&conn, &range).unwrap();
        let ids: Vec<&str> = vec![];
        let _ = ids;
        assert_eq!(rows.len(), 3, "o1/o2(退款保留)/o5 入区间,o3/o4 不入");
        assert!(rows.iter().all(|r| r.paid_at < 200));
    }

    #[test]
    fn 账号快照只计online() {
        let conn = test_conn();
        seed_account(&conn, "a1", "online");
        seed_account(&conn, "a2", "offline");
        seed_account(&conn, "a3", "disabled");
        assert_eq!(account_activity(&conn).unwrap(), (1, 3));
    }

    #[test]
    fn 事项计数只计open() {
        let conn = test_conn();
        seed_account(&conn, "a1", "online");
        for (id, state) in [("i1", "open"), ("i2", "resolved")] {
            conn.execute(
                "INSERT INTO issues (id, account_id, kind, reason_code, state, created_at)
                 VALUES (?1, 'a1', 'delivery_unknown', 'x', ?2, 0)",
                rusqlite::params![id, state],
            )
            .unwrap();
        }
        assert_eq!(open_issue_count(&conn).unwrap(), 1);
    }
}
