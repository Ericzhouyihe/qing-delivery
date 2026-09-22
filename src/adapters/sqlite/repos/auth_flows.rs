//! auth_flows 表仓储:二维码/授权会话;终态不可被晚到结果覆盖。

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::time_util::utc_now_ms;

pub struct AuthFlowRow {
    pub id: String,
    pub account_id: Option<String>,
    pub generation: i64,
    pub status: String,
    pub expires_at: i64,
    pub bound_user_id: Option<String>,
    pub session_ref: Option<String>,
    pub created_at: i64,
}

pub fn insert(
    conn: &Connection,
    id: &str,
    account_id: Option<&str>,
    expires_at: i64,
    session_ref: &str,
) -> rusqlite::Result<AuthFlowRow> {
    let now = utc_now_ms();
    // generation 单调:同一账号新会话代次更高,晚到的旧代次结果将被拒绝
    let generation: i64 = conn.query_row(
        "SELECT COALESCE(MAX(generation), 0) + 1 FROM auth_flows",
        [],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO auth_flows(id, account_id, generation, status, expires_at, session_ref, created_at)
         VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6)",
        params![id, account_id, generation, expires_at, session_ref, now],
    )?;
    get(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<AuthFlowRow>> {
    conn.query_row(
        "SELECT id, account_id, generation, status, expires_at, bound_user_id, session_ref, created_at
         FROM auth_flows WHERE id = ?1",
        params![id],
        |r| {
            Ok(AuthFlowRow {
                id: r.get(0)?,
                account_id: r.get(1)?,
                generation: r.get(2)?,
                status: r.get(3)?,
                expires_at: r.get(4)?,
                bound_user_id: r.get(5)?,
                session_ref: r.get(6)?,
                created_at: r.get(7)?,
            })
        },
    )
    .optional()
}

/// 状态推进:仅当前状态与迁移表允许时更新;返回是否生效。
pub fn advance(
    conn: &Connection,
    id: &str,
    expected_status: &str,
    new_status: &str,
    bound_user_id: Option<&str>,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE auth_flows SET status = ?2, bound_user_id = COALESCE(?3, bound_user_id)
         WHERE id = ?1 AND status = ?4",
        params![id, new_status, bound_user_id, expected_status],
    )?;
    Ok(n == 1)
}

/// 到期未完成的流程标记 expired(observe 时惰性触发)。
pub fn expire_if_due(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE auth_flows SET status = 'expired'
         WHERE id = ?1 AND status IN ('pending','scanned') AND expires_at <= ?2",
        params![id, utc_now_ms()],
    )?;
    Ok(n == 1)
}

pub fn delete_expired_before(conn: &Connection, before_ms: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM auth_flows WHERE expires_at < ?1",
        params![before_ms],
    )
}
