//! issues 表仓储:同一未解决原因不重复创建(部分唯一索引,FR-024)。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub struct IssueRow {
    pub id: String,
    pub order_id: Option<String>,
    pub account_id: String,
    pub delivery_id: Option<String>,
    pub kind: String,
    pub reason_code: String,
    pub allowed_actions: String,
    pub state: String,
    pub version: i64,
}

/// 打开待处理事项;相同 (order, kind, reason) 未解决时返回已存在行。
pub fn open(
    conn: &Connection,
    id: &str,
    order_id: Option<&str>,
    account_id: &str,
    delivery_id: Option<&str>,
    kind: &str,
    reason_code: &str,
    allowed_actions: &str,
) -> rusqlite::Result<IssueRow> {
    if let Some(order_id) = order_id {
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM issues WHERE order_id = ?1 AND kind = ?2 AND reason_code = ?3
                 AND state = 'open'",
                params![order_id, kind, reason_code],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing_id) = existing {
            return get(conn, &existing_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows);
        }
    }
    conn.execute(
        "INSERT INTO issues(id, order_id, account_id, delivery_id, kind, reason_code,
             allowed_actions, state, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'open', ?8)",
        params![
            id,
            order_id,
            account_id,
            delivery_id,
            kind,
            reason_code,
            allowed_actions,
            utc_now_ms()
        ],
    )?;
    get(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<IssueRow>> {
    conn.query_row(
        "SELECT id, order_id, account_id, delivery_id, kind, reason_code, allowed_actions,
                state, version FROM issues WHERE id = ?1",
        params![id],
        |r| {
            Ok(IssueRow {
                id: r.get(0)?,
                order_id: r.get(1)?,
                account_id: r.get(2)?,
                delivery_id: r.get(3)?,
                kind: r.get(4)?,
                reason_code: r.get(5)?,
                allowed_actions: r.get(6)?,
                state: r.get(7)?,
                version: r.get(8)?,
            })
        },
    )
    .optional()
}

pub fn resolve(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE issues SET state = 'resolved', resolved_at = ?1 WHERE id = ?2 AND state = 'open'",
        params![utc_now_ms(), id],
    )?;
    Ok(())
}
