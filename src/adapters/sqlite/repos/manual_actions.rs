//! manual_actions 表仓储:追加式审计;原因 1—500 字符由服务层校验。

use rusqlite::{Connection, params};

use crate::domain::time_util::utc_now_ms;

/// 插入一条人工动作审计(追加式,不更新历史)。
pub fn insert(
    conn: &Connection,
    id: &str,
    admin_id: &str,
    order_id: &str,
    delivery_id: Option<&str>,
    action: &str,
    reason: &str,
    risk_confirmed: bool,
    request_key: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO manual_actions(id, admin_id, order_id, delivery_id, action, reason,
             risk_confirmed, request_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            admin_id,
            order_id,
            delivery_id,
            action,
            reason,
            risk_confirmed as i64,
            request_key,
            utc_now_ms()
        ],
    )?;
    Ok(())
}

pub fn count_for_order(conn: &Connection, order_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM manual_actions WHERE order_id = ?1",
        params![order_id],
        |r| r.get(0),
    )
}
